//! Conexão do PGStudio com o PostgreSQL, sobre o `tokio-postgres`.
//!
//! - [`Session::connect`] abre a conexão com o TLS pedido em
//!   [`SslMode`] e traduz as falhas para [`ConnectError`], com mensagens que
//!   dizem ao usuário o que fazer.
//! - [`Session::cancel`] cancela o comando em andamento por uma segunda
//!   conexão, como o `pg_cancel_backend`.
//! - [`DbRuntime`] é a runtime Tokio compartilhada pelas conexões; quem usa
//!   de fora de uma runtime chama [`DbRuntime::block_on`].
//!
//! O Object Explorer e o `Connector` da interface ficam em `pg-metadata`.

mod error;
mod query;
mod tls;

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use pgstudio_core::connection::{ConnectError, ConnectionSettings, SslMode};
use tokio::runtime::{Handle, Runtime};
use tokio_postgres::error::DbError;
use tokio_postgres::{AsyncMessage, CancelToken, Client, SimpleQueryMessage};

pub use error::{connect_error, describe};
pub use query::{PgQueryService, PgQuerySession};

/// Tempo máximo para abrir a conexão.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Nome que o servidor mostra em `pg_stat_activity.application_name`.
pub const APPLICATION_NAME: &str = "PostPilot";

/// Uma conexão aberta com o servidor.
pub struct Session {
    client: Client,
    cancel: CancelToken,
    ssl: SslMode,
    settings: ConnectionSettings,
    server_version: String,
    /// Avisos do servidor (`NOTICE`, `WARNING`), na ordem em que chegaram.
    notices: Mutex<mpsc::Receiver<DbError>>,
    /// O túnel SSH por onde a conexão passa: fica aberto enquanto ela
    /// existir.
    _tunnel: Option<Arc<pgstudio_ssh::Tunnel>>,
}

/// Cancela o comando em andamento de uma sessão, mesmo com a sessão
/// ocupada.
#[derive(Clone)]
pub struct Canceller {
    token: CancelToken,
    ssl: SslMode,
}

impl Canceller {
    /// Pede o cancelamento por uma segunda conexão.
    pub async fn cancel(&self) -> Result<(), String> {
        let tls = tls::connector(self.ssl)?;
        self.token
            .cancel_query(tls)
            .await
            .map_err(|e| error::describe(&e))
    }
}

impl Session {
    /// Conecta com `settings` e `password`. A tarefa que mantém a conexão
    /// roda na runtime atual e termina quando a sessão é solta.
    pub async fn connect(
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Session, ConnectError> {
        // Com túnel SSH, o socket vai para a porta local dele; o nome do
        // servidor continua valendo para o certificado.
        let tunnel = pgstudio_ssh::open(settings).await?;
        let mut config = tokio_postgres::Config::new();
        match &tunnel {
            Some(tunnel) => {
                let local = tunnel.local_addr();
                config.hostaddr(local.ip()).port(local.port())
            }
            None => config.port(settings.port),
        };
        config
            .host(&settings.host)
            .dbname(&settings.database)
            .user(&settings.user)
            .application_name(APPLICATION_NAME)
            .connect_timeout(CONNECT_TIMEOUT)
            .keepalives(true)
            .ssl_mode(match settings.ssl {
                SslMode::Disable => tokio_postgres::config::SslMode::Disable,
                SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
                SslMode::Require | SslMode::VerifyFull => tokio_postgres::config::SslMode::Require,
            });
        if !password.is_empty() {
            config.password(password);
        }
        let tls = tls::connector(settings.ssl).map_err(ConnectError::Tls)?;
        let (client, connection) = config
            .connect(tls)
            .await
            .map_err(|e| connect_error(&e, settings))?;
        let (notice_tx, notices) = mpsc::channel();
        tokio::spawn(async move {
            let mut connection = connection;
            // Erros aqui aparecem na próxima consulta como conexão fechada.
            while let Some(Ok(message)) =
                std::future::poll_fn(|cx| connection.poll_message(cx)).await
            {
                if let AsyncMessage::Notice(notice) = message {
                    let _ = notice_tx.send(notice);
                }
            }
        });
        let server_version = server_version(&client)
            .await
            .map_err(|e| connect_error(&e, settings))?;
        Ok(Session {
            cancel: client.cancel_token(),
            client,
            ssl: settings.ssl,
            settings: settings.clone(),
            server_version,
            notices: Mutex::new(notices),
            _tunnel: tunnel,
        })
    }

    /// Com o que se conectou.
    pub fn settings(&self) -> &ConnectionSettings {
        &self.settings
    }

    /// Versão do servidor, como `16.4`.
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// O cliente do `tokio-postgres`, para consultas.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Roda um ou mais comandos pelo protocolo simples e devolve as linhas
    /// como texto.
    pub async fn simple_query(
        &self,
        sql: &str,
    ) -> Result<Vec<SimpleQueryMessage>, tokio_postgres::Error> {
        self.client.simple_query(sql).await
    }

    /// Pede ao servidor para cancelar o comando em andamento nesta sessão.
    pub async fn cancel(&self) -> Result<(), String> {
        self.canceller().cancel().await
    }

    /// Quem cancela o comando desta sessão, para guardar fora dela.
    pub fn canceller(&self) -> Canceller {
        Canceller {
            token: self.cancel.clone(),
            ssl: self.ssl,
        }
    }

    /// Avisos recebidos desde a última chamada.
    pub fn take_notices(&self) -> Vec<DbError> {
        match self.notices.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Se a conexão caiu.
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }
}

async fn server_version(client: &Client) -> Result<String, tokio_postgres::Error> {
    let rows = client.simple_query("SHOW server_version").await?;
    let full = rows
        .iter()
        .find_map(|m| match m {
            SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
            _ => None,
        })
        .unwrap_or_default();
    // "16.4 (Ubuntu 16.4-1)" vira "16.4".
    Ok(full
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string())
}

/// Runtime Tokio compartilhada pelas conexões do aplicativo.
#[derive(Clone)]
pub struct DbRuntime(Arc<Runtime>);

impl DbRuntime {
    /// Cria a runtime com duas threads de trabalho.
    pub fn new() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("pgstudio-db")
            .enable_all()
            .build()?;
        Ok(DbRuntime(Arc::new(rt)))
    }

    /// Handle para rodar tarefas.
    pub fn handle(&self) -> &Handle {
        self.0.handle()
    }

    /// Roda `f` até o fim, bloqueando a thread atual. Não chame de dentro da
    /// runtime.
    pub fn block_on<F: Future>(&self, f: F) -> F::Output {
        self.0.block_on(f)
    }
}
