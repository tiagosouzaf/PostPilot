//! Conexão do PostPilot com o Redis, por um cliente RESP próprio.
//!
//! - [`Connection::connect`] abre a conexão (TLS pelo [`SslMode`], `HELLO 3`
//!   com `AUTH` quando há senha, `SELECT` do banco) e traduz as falhas para
//!   [`ConnectError`]. Em servidores antigos (antes do 6) cai para RESP2.
//! - [`RedisQueryService`] abre uma sessão por janela de consulta: um
//!   comando por linha ([`command`]), cada resposta numa grade.
//! - [`RedisConnector`] é o [`Connector`] da interface para o Redis.
//!
//! O cliente é próprio (sem crate de terceiros) porque o protocolo é
//! pequeno e a janela de consulta precisa devolver qualquer resposta de
//! qualquer comando, sem tipos no meio.

pub mod command;
mod explorer;
mod query;
pub mod render;
pub mod resp;

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, SslMode, redis_database_index,
};
use pgstudio_i18n::trf;
use pgstudio_pg_client::DbRuntime;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub use explorer::RedisExplorer;
pub use query::{RedisQueryService, RedisQuerySession};
use resp::Value;

/// Tempo máximo para abrir a conexão.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Nome que o servidor mostra no `CLIENT LIST`.
pub const APPLICATION_NAME: &str = "PostPilot";

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Uma conexão aberta com o servidor.
pub struct Connection {
    stream: BufReader<Box<dyn Io>>,
    server_version: String,
    resp3: bool,
}

impl Connection {
    /// Conecta com as configurações e a senha dadas.
    pub async fn connect(
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connection, ConnectError> {
        match tokio::time::timeout(CONNECT_TIMEOUT, Self::connect_inner(settings, password)).await {
            Ok(r) => r,
            Err(_) => Err(ConnectError::Network(trf!(
                "Timed out connecting to {server}.",
                server = settings.server_name()
            ))),
        }
    }

    async fn connect_inner(
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connection, ConnectError> {
        let network = |e: &dyn std::fmt::Display| {
            ConnectError::Network(trf!(
                "Could not connect to {server}: {e}",
                server = settings.server_name(),
                e = e
            ))
        };
        let Some(database) = redis_database_index(&settings.database) else {
            return Err(ConnectError::Database(trf!(
                "\"{database}\" is not a Redis database. Use db0, db1...",
                database = settings.database
            )));
        };
        let tcp = TcpStream::connect((settings.host.as_str(), settings.port))
            .await
            .map_err(|e| network(&e))?;
        tcp.set_nodelay(true).map_err(|e| network(&e))?;
        let stream: Box<dyn Io> = if settings.ssl == SslMode::Disable {
            Box::new(tcp)
        } else {
            let tls = |m: String| {
                ConnectError::Tls(trf!(
                    "Could not set up encryption with {server}: {m}",
                    server = settings.server_name(),
                    m = m
                ))
            };
            let config = pgstudio_pg_client::tls::client_config(settings.ssl).map_err(tls)?;
            let name = rustls::pki_types::ServerName::try_from(settings.host.clone())
                .map_err(|e| tls(e.to_string()))?;
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            Box::new(
                connector
                    .connect(name, tcp)
                    .await
                    .map_err(|e| tls(e.to_string()))?,
            )
        };
        let mut conn = Connection {
            stream: BufReader::new(stream),
            server_version: String::new(),
            resp3: false,
        };
        conn.handshake(settings, password)
            .await
            .map_err(|e| match e {
                Failure::Io(e) => network(&e),
                Failure::Server(m) if is_auth_error(&m) => ConnectError::Authentication(m),
                Failure::Server(m) => ConnectError::Other(m),
            })?;
        if database != 0 {
            match conn.call_text(&["SELECT", &database.to_string()]).await {
                Ok(Value::Error(m)) => return Err(ConnectError::Database(m)),
                Ok(_) => {}
                Err(e) => return Err(network(&e)),
            }
        }
        Ok(conn)
    }

    async fn handshake(
        &mut self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<(), Failure> {
        let user = if settings.user.trim().is_empty() {
            "default"
        } else {
            settings.user.trim()
        };
        let mut hello = vec!["HELLO", "3"];
        if !password.is_empty() {
            hello.extend(["AUTH", user, password]);
        }
        hello.extend(["SETNAME", APPLICATION_NAME]);
        match self.call_text(&hello).await? {
            Value::Error(m) if m.starts_with("ERR unknown command") || m.contains("NOPROTO") => {
                // Antes do Redis 6: RESP2, com o AUTH de então.
                if !password.is_empty() {
                    let auth = if settings.user.trim().is_empty() {
                        self.call_text(&["AUTH", password]).await?
                    } else {
                        self.call_text(&["AUTH", user, password]).await?
                    };
                    if let Value::Error(m) = auth {
                        return Err(Failure::Server(m));
                    }
                }
                let _ = self
                    .call_text(&["CLIENT", "SETNAME", APPLICATION_NAME])
                    .await?;
                // Sem senha num servidor que pede: o primeiro comando diz.
                if let Value::Error(m) = self.call_text(&["PING"]).await? {
                    return Err(Failure::Server(m));
                }
            }
            Value::Error(m) => return Err(Failure::Server(m)),
            hello => {
                self.resp3 = true;
                for (k, v) in hello.into_pairs() {
                    if k.as_text().as_deref() == Some("version") {
                        self.server_version = v.as_text().unwrap_or_default();
                    }
                }
            }
        }
        if self.server_version.is_empty()
            && let Value::Bulk(info) = self.call_text(&["INFO", "server"]).await?
        {
            self.server_version = resp::bytes_to_text(&info)
                .lines()
                .find_map(|l| l.strip_prefix("redis_version:"))
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        Ok(())
    }

    /// Versão do servidor, como `7.2.4`.
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// Se a conexão fala RESP3 (mapas e conjuntos nas respostas).
    pub fn resp3(&self) -> bool {
        self.resp3
    }

    /// Manda um comando sem esperar a resposta.
    pub async fn send(&mut self, args: &[Vec<u8>]) -> std::io::Result<()> {
        let stream = self.stream.get_mut();
        stream.write_all(&resp::encode(args)).await?;
        stream.flush().await
    }

    /// Lê a próxima resposta.
    pub async fn read(&mut self) -> std::io::Result<Value> {
        resp::read_value(&mut self.stream).await
    }

    /// Manda um comando e lê a resposta (um erro do servidor vem como
    /// [`Value::Error`]).
    pub async fn call(&mut self, args: &[Vec<u8>]) -> std::io::Result<Value> {
        self.send(args).await?;
        self.read().await
    }

    /// [`Connection::call`] com os argumentos em texto.
    pub async fn call_text(&mut self, args: &[&str]) -> std::io::Result<Value> {
        let args: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        self.call(&args).await
    }

    /// [`Connection::call_text`], com o erro do servidor ou da rede como
    /// texto.
    pub async fn query(&mut self, args: &[&str]) -> Result<Value, String> {
        match self.call_text(args).await {
            Ok(Value::Error(m)) => Err(m),
            Ok(v) => Ok(v),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Quantos bancos o servidor tem (`databases` da configuração). Onde o
    /// `CONFIG` é proibido (serviços gerenciados), os 16 de costume.
    pub async fn database_count(&mut self) -> Result<u32, String> {
        match self.call_text(&["CONFIG", "GET", "databases"]).await {
            Ok(Value::Error(_)) => Ok(16),
            Ok(v) => Ok(v
                .into_pairs()
                .into_iter()
                .find_map(|(_, v)| v.as_int())
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(16)),
            Err(e) => Err(e.to_string()),
        }
    }
}

enum Failure {
    Io(std::io::Error),
    Server(String),
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::Io(e)
    }
}

/// Se o erro do servidor é de usuário ou senha.
fn is_auth_error(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.starts_with("wrongpass")
        || m.starts_with("noauth")
        || m.contains("invalid password")
        || m.contains("invalid username-password")
        || m.contains("without any password configured")
        || m.contains("authentication required")
}

/// O [`Connector`] do Redis.
#[derive(Clone)]
pub struct RedisConnector {
    runtime: DbRuntime,
}

impl RedisConnector {
    pub fn new(runtime: DbRuntime) -> Self {
        RedisConnector { runtime }
    }
}

impl Connector for RedisConnector {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError> {
        let mut settings = settings.clone();
        settings.engine = Engine::Redis;
        // O nome do banco sempre como `dbN`.
        if let Some(n) = redis_database_index(&settings.database) {
            settings.database = format!("db{n}");
        }
        let connection = self
            .runtime
            .block_on(Connection::connect(&settings, password))?;
        let server_version = connection.server_version().to_string();
        let explorer = RedisExplorer::new(connection, &settings, self.runtime.clone());
        Ok(Connected {
            settings: settings.clone(),
            server_version,
            metadata: Arc::new(explorer),
            queries: Arc::new(RedisQueryService::new(
                settings,
                password.to_string(),
                self.runtime.clone(),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_are_recognized() {
        assert!(is_auth_error(
            "WRONGPASS invalid username-password pair or user is disabled."
        ));
        assert!(is_auth_error(
            "NOAUTH HELLO must be called with the client already authenticated"
        ));
        assert!(is_auth_error(
            "ERR AUTH <password> called without any password configured for the default user."
        ));
        assert!(!is_auth_error("ERR unknown command"));
    }
}
