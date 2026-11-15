//! Conexão do PostPilot com o Microsoft SQL Server, pelo tiberius (TDS).
//!
//! - [`Session::connect`] abre a conexão (autenticação SQL, TLS pelo
//!   [`SslMode`], instância nomeada `servidor\INSTANCIA`, redirecionamento do
//!   Azure) e traduz as falhas para [`ConnectError`].
//! - [`MssqlQueryService`] abre uma sessão por janela de consulta: divide o
//!   script no `GO`, devolve todos os resultados, contagens, mensagens e
//!   erros, e cancela pelo sinal de atenção do TDS (`CancelHandle`).
//! - [`MssqlConnector`] é o [`Connector`] da interface para o SQL Server.
//!
//! Decisões em `docs/adr/0004-sql-server.md`.

mod explorer;
mod model;
mod query;
pub mod script;
pub mod value;

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, SslMode,
};
use pgstudio_i18n::trf;
use pgstudio_pg_client::DbRuntime;
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, SqlBrowser};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub use explorer::MssqlExplorer;
pub use query::{MssqlQueryService, MssqlQuerySession};

/// Tempo máximo para abrir a conexão.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Nome que o servidor mostra em `sys.dm_exec_sessions.program_name`.
pub const APPLICATION_NAME: &str = "PostPilot";

/// O cliente do tiberius sobre TCP.
pub type TdsClient = Client<Compat<TcpStream>>;

/// Uma conexão aberta com o servidor.
pub struct Session {
    client: TdsClient,
    settings: ConnectionSettings,
    server_version: String,
}

impl Session {
    /// Conecta com as configurações e a senha dadas.
    pub async fn connect(
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Session, ConnectError> {
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
    ) -> Result<Session, ConnectError> {
        let mut config = config(settings, password);
        // O Azure SQL pode mandar para outro servidor (gateway).
        for _ in 0..2 {
            let stream = open_socket(&config, settings).await?;
            match Client::connect(config.clone(), stream.compat_write()).await {
                Ok(mut client) => {
                    let server_version = server_version(&mut client).await.unwrap_or_default();
                    return Ok(Session {
                        client,
                        settings: settings.clone(),
                        server_version,
                    });
                }
                Err(tiberius::error::Error::Routing { host, port }) => {
                    config.host(host);
                    config.port(port);
                }
                Err(e) => return Err(connect_error(&e, settings)),
            }
        }
        Err(ConnectError::Other(trf!(
            "{server} redirected the connection too many times.",
            server = settings.server_name()
        )))
    }

    pub fn settings(&self) -> &ConnectionSettings {
        &self.settings
    }

    /// Versão do servidor, como `16.0.4135.4`.
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    pub fn client(&mut self) -> &mut TdsClient {
        &mut self.client
    }

    /// Para cancelar de fora o lote em andamento.
    pub fn cancel_handle(&self) -> tiberius::CancelHandle {
        self.client.cancel_handle()
    }

    /// Roda `sql` e devolve as linhas do primeiro resultado como texto.
    pub async fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        self.table(sql).await.map(|(_, rows)| rows)
    }

    /// Como [`Session::rows`], com os nomes das colunas do primeiro
    /// resultado.
    pub async fn table(
        &mut self,
        sql: &str,
    ) -> Result<(Vec<String>, Vec<Vec<Option<String>>>), String> {
        use futures_util::TryStreamExt;
        let mut stream = self.client.batch(sql).await.map_err(|e| describe(&e))?;
        let mut names = Vec::new();
        let mut rows = Vec::new();
        let mut results = 0;
        let mut error = None;
        while let Some(item) = stream.try_next().await.map_err(|e| describe(&e))? {
            match item {
                tiberius::BatchItem::Metadata(columns) => {
                    results += 1;
                    if results == 1 {
                        names = columns.iter().map(|c| c.name().to_string()).collect();
                    }
                }
                tiberius::BatchItem::Row(row) if results == 1 => rows.push(
                    row.cells()
                        .map(|(c, d)| value::to_text(d, c.column_type()))
                        .collect(),
                ),
                tiberius::BatchItem::Message(m) if m.is_error && error.is_none() => {
                    error = Some(m.message)
                }
                _ => {}
            }
        }
        match error {
            Some(e) => Err(e),
            None => Ok((names, rows)),
        }
    }
}

/// O `Config` do tiberius para as configurações dadas.
fn config(settings: &ConnectionSettings, password: &str) -> Config {
    let mut config = Config::new();
    let (host, instance) = split_instance(&settings.host);
    config.host(host);
    match instance {
        Some(instance) => config.instance_name(instance),
        None => config.port(settings.port),
    }
    if !settings.database.is_empty() {
        config.database(&settings.database);
    }
    config.application_name(APPLICATION_NAME);
    config.authentication(AuthMethod::sql_server(&settings.user, password));
    match settings.ssl {
        // Só o login vai cifrado (o "Optional" do SSMS); o certificado do
        // login não é conferido, como no SSMS.
        SslMode::Disable => {
            config.encryption(EncryptionLevel::Off);
            config.trust_cert();
        }
        SslMode::Prefer => {
            config.encryption(EncryptionLevel::On);
            config.trust_cert();
        }
        SslMode::Require => {
            config.encryption(EncryptionLevel::Required);
            config.trust_cert();
        }
        SslMode::VerifyFull => config.encryption(EncryptionLevel::Required),
    }
    config
}

/// `servidor\INSTANCIA` vira o servidor e o nome da instância.
fn split_instance(host: &str) -> (&str, Option<&str>) {
    match host.split_once('\\') {
        Some((h, i)) if !i.is_empty() => (h, Some(i)),
        _ => (host, None),
    }
}

async fn open_socket(
    config: &Config,
    settings: &ConnectionSettings,
) -> Result<TcpStream, ConnectError> {
    let network = |e: &dyn std::fmt::Display| {
        ConnectError::Network(trf!(
            "Could not connect to {server}: {e}",
            server = settings.server_name(),
            e = e
        ))
    };
    let stream = if split_instance(&settings.host).1.is_some() {
        TcpStream::connect_named(config)
            .await
            .map_err(|e| network(&e))?
    } else {
        TcpStream::connect(config.get_addr())
            .await
            .map_err(|e| network(&e))?
    };
    stream.set_nodelay(true).map_err(|e| network(&e))?;
    Ok(stream)
}

async fn server_version(client: &mut TdsClient) -> Option<String> {
    let row = client
        .simple_query("SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128))")
        .await
        .ok()?
        .into_row()
        .await
        .ok()??;
    row.get::<&str, _>(0).map(str::to_string)
}

/// Erro do driver como texto.
pub fn describe(e: &tiberius::error::Error) -> String {
    match e {
        tiberius::error::Error::Server(t) => t.message().to_string(),
        tiberius::error::Error::Io { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// Falha de conexão com a mensagem para o usuário.
pub fn connect_error(e: &tiberius::error::Error, settings: &ConnectionSettings) -> ConnectError {
    use tiberius::error::Error;
    match e {
        // 18456: login falhou; 18470: login desativado; 18487/18488: senha
        // expirada ou que precisa trocar.
        Error::Server(t) if matches!(t.code(), 18456 | 18470 | 18487 | 18488) => {
            ConnectError::Authentication(t.message().to_string())
        }
        // 4060: o banco não existe ou o login não tem acesso.
        Error::Server(t) if t.code() == 4060 => ConnectError::Database(t.message().to_string()),
        Error::Server(t) => ConnectError::Other(t.message().to_string()),
        Error::Tls(m) => ConnectError::Tls(trf!(
            "Could not set up encryption with {server}: {m}",
            server = settings.server_name(),
            m = m
        )),
        Error::Io { message, .. } => ConnectError::Network(trf!(
            "Could not connect to {server}: {e}",
            server = settings.server_name(),
            e = message
        )),
        other => ConnectError::Other(other.to_string()),
    }
}

/// O [`Connector`] do SQL Server.
#[derive(Clone)]
pub struct MssqlConnector {
    runtime: DbRuntime,
}

impl MssqlConnector {
    pub fn new(runtime: DbRuntime) -> Self {
        MssqlConnector { runtime }
    }
}

impl Connector for MssqlConnector {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError> {
        let mut settings = settings.clone();
        settings.engine = Engine::SqlServer;
        let session = self
            .runtime
            .block_on(Session::connect(&settings, password))?;
        let server_version = session.server_version().to_string();
        let explorer = MssqlExplorer::new(session, password.to_string(), self.runtime.clone());
        Ok(Connected {
            settings: settings.clone(),
            server_version,
            metadata: Arc::new(explorer),
            queries: Arc::new(MssqlQueryService::new(
                settings,
                password.to_string(),
                self.runtime.clone(),
            )),
        })
    }
}

/// Escolhe o conector pelo motor das configurações.
pub struct EngineConnector {
    pub postgres: Arc<dyn Connector>,
    pub sql_server: Arc<dyn Connector>,
}

impl Connector for EngineConnector {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError> {
        match settings.engine {
            Engine::PostgreSql => self.postgres.connect(settings, password),
            Engine::SqlServer => self.sql_server.connect(settings, password),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_instances() {
        assert_eq!(split_instance("db\\SQLEXPRESS"), ("db", Some("SQLEXPRESS")));
        assert_eq!(split_instance("db"), ("db", None));
        assert_eq!(split_instance("db\\"), ("db\\", None));
    }
}
