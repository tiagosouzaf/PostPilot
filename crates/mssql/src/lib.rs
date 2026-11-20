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
mod pool;
mod query;
pub mod script;
mod tree;
pub mod value;

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, SslMode,
};
use pgstudio_i18n::{tr, trf};
use pgstudio_pg_client::DbRuntime;
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, SqlBrowser};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub use explorer::MssqlExplorer;
pub use query::{MssqlQueryService, MssqlQuerySession};

/// Tempo máximo para abrir a conexão.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Quanto a árvore e os outros metadados esperam o servidor mandar o
/// próximo pedaço de uma resposta. As janelas de consulta não têm limite,
/// como no SSMS: quem decide parar é o usuário, no Cancelar.
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(60);

/// Por que uma conexão não serve mais.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    /// O servidor demorou demais: a resposta ficou pela metade.
    TimedOut,
    /// A conexão caiu ou saiu de sincronia.
    Lost,
}

/// Nome que o servidor mostra em `sys.dm_exec_sessions.program_name`.
pub const APPLICATION_NAME: &str = "PostPilot";

/// O cliente do tiberius sobre TCP.
pub type TdsClient = Client<Compat<TcpStream>>;

/// Uma conexão aberta com o servidor.
pub struct Session {
    client: TdsClient,
    settings: ConnectionSettings,
    server_version: String,
    /// Limite de espera por pedaço de resposta (`None`: sem limite).
    command_timeout: Option<Duration>,
    /// Depois de uma falha do driver (tempo esgotado, queda) a conexão não
    /// pode ser reutilizada: quem a guarda abre outra.
    broken: Option<Broken>,
    /// O túnel SSH por onde a conexão passa: fica aberto enquanto ela
    /// existir.
    _tunnel: Option<Arc<pgstudio_ssh::Tunnel>>,
}

impl Session {
    /// Conecta com as configurações e a senha dadas, sem limite de tempo
    /// para os comandos (janelas de consulta).
    pub async fn connect(
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Session, ConnectError> {
        Self::connect_with(settings, password, None).await
    }

    /// Como [`Session::connect`], desistindo de um comando quando o
    /// servidor fica `command_timeout` sem mandar nada.
    pub async fn connect_with(
        settings: &ConnectionSettings,
        password: &str,
        command_timeout: Option<Duration>,
    ) -> Result<Session, ConnectError> {
        let connecting = Self::connect_inner(settings, password, command_timeout);
        match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
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
        command_timeout: Option<Duration>,
    ) -> Result<Session, ConnectError> {
        let mut config = config(settings, password);
        config.command_timeout(command_timeout);
        // Com túnel SSH, o socket vai para a porta local dele (o `config`
        // continua com o nome do servidor, para o certificado). Pelo túnel
        // a instância nomeada não é procurada: vale a porta.
        let mut tunnel = pgstudio_ssh::open(settings).await?;
        // O Azure SQL pode mandar para outro servidor (gateway).
        for _ in 0..2 {
            let stream = open_socket(&config, settings, tunnel.as_deref()).await?;
            match Client::connect(config.clone(), stream.compat_write()).await {
                Ok(mut client) => {
                    let server_version = server_version(&mut client).await.unwrap_or_default();
                    return Ok(Session {
                        client,
                        settings: settings.clone(),
                        server_version,
                        command_timeout,
                        broken: None,
                        _tunnel: tunnel,
                    });
                }
                Err(tiberius::error::Error::Routing { host, port }) => {
                    if let Some(ssh) = &settings.ssh {
                        tunnel = Some(pgstudio_ssh::open_to(ssh, &host, port).await?);
                    }
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

    /// O limite de espera por pedaço de resposta desta conexão.
    pub fn command_timeout(&self) -> Option<Duration> {
        self.command_timeout
    }

    pub fn client(&mut self) -> &mut TdsClient {
        &mut self.client
    }

    /// Se a conexão não serve mais, e por quê.
    pub fn broken(&self) -> Option<Broken> {
        self.broken
    }

    pub fn is_broken(&self) -> bool {
        self.broken.is_some()
    }

    /// Registra uma falha do driver: a conexão não é mais reutilizada.
    /// Devolve o texto para o usuário.
    pub fn fail(&mut self, e: &tiberius::error::Error) -> String {
        self.broken = Some(if timed_out(e) {
            Broken::TimedOut
        } else {
            Broken::Lost
        });
        describe(e)
    }

    /// Troca esta conexão por uma nova, com as mesmas configurações.
    pub async fn reopen(&mut self, password: &str) -> Result<(), String> {
        let settings = self.settings.clone();
        *self = Session::connect_with(&settings, password, self.command_timeout)
            .await
            .map_err(|e| e.message().to_string())?;
        Ok(())
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
        let mut names = Vec::new();
        let mut rows = Vec::new();
        let mut results = 0;
        let mut error = None;
        // Qualquer falha do driver (não as mensagens de erro do servidor,
        // que chegam como itens) deixa a conexão sem uso.
        let failed = 'read: {
            let mut stream = match self.client.batch(sql).await {
                Ok(s) => s,
                Err(e) => break 'read Some(e),
            };
            loop {
                let item = match stream.try_next().await {
                    Ok(Some(item)) => item,
                    Ok(None) => break 'read None,
                    Err(e) => break 'read Some(e),
                };
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
        };
        if let Some(e) = failed {
            return Err(self.fail(&e));
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
        Some(instance) if settings.ssh.is_none() => config.instance_name(instance),
        _ => config.port(settings.port),
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
    tunnel: Option<&pgstudio_ssh::Tunnel>,
) -> Result<TcpStream, ConnectError> {
    let network = |e: &dyn std::fmt::Display| {
        ConnectError::Network(trf!(
            "Could not connect to {server}: {e}",
            server = settings.server_name(),
            e = e
        ))
    };
    let stream = if let Some(tunnel) = tunnel {
        TcpStream::connect(tunnel.local_addr())
            .await
            .map_err(|e| network(&e))?
    } else if split_instance(&settings.host).1.is_some() {
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

/// Se o erro é o servidor não ter respondido a tempo.
fn timed_out(e: &tiberius::error::Error) -> bool {
    matches!(
        e,
        tiberius::error::Error::Io {
            kind: std::io::ErrorKind::TimedOut,
            ..
        }
    )
}

/// Erro do driver como texto.
pub fn describe(e: &tiberius::error::Error) -> String {
    match e {
        tiberius::error::Error::Server(t) => t.message().to_string(),
        _ if timed_out(e) => tr(
            "The server took too long to answer and the connection was closed. Try again: PostPilot opens a new connection by itself.",
        )
        .to_string(),
        // A conexão ficou pela metade (tempo esgotado ou envio
        // interrompido) e o driver a recusa.
        tiberius::error::Error::Protocol(m)
            if m.contains("out of sync") || m.contains("inconsistent state") =>
        {
            tr(
                "The connection to the server was lost. Try again: PostPilot opens a new connection by itself.",
            )
            .to_string()
        }
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
    metadata_timeout: Duration,
    page: usize,
}

impl MssqlConnector {
    pub fn new(runtime: DbRuntime) -> Self {
        MssqlConnector {
            runtime,
            metadata_timeout: METADATA_TIMEOUT,
            page: explorer::PAGE,
        }
    }

    /// Outro tamanho para as partes em que as listas da árvore chegam (os
    /// testes usam um pequeno).
    pub fn with_page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Outro limite de espera para a árvore e os metadados (os testes
    /// usam um curto).
    pub fn with_metadata_timeout(mut self, timeout: Duration) -> Self {
        self.metadata_timeout = timeout;
        self
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
        let session = self.runtime.block_on(Session::connect_with(
            &settings,
            password,
            Some(self.metadata_timeout),
        ))?;
        let server_version = session.server_version().to_string();
        let explorer = MssqlExplorer::new(session, password.to_string(), self.runtime.clone())
            .with_page(self.page);
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
