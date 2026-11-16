//! MongoDB no PostPilot: conexão pelo driver oficial, consultas no estilo
//! do mongosh nas janelas de consulta ([`shell`], [`query`]) e os bancos e
//! coleções no Object Explorer ([`explorer`]).

use std::sync::Arc;
use std::time::Duration;

use mongodb::Client;
use mongodb::bson::doc;
use mongodb::error::ErrorKind;
use mongodb::options::{ClientOptions, Credential, ServerAddress, Tls, TlsOptions};
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, SslMode,
};
use pgstudio_i18n::tr;
use pgstudio_pg_client::DbRuntime;

pub mod edit;
pub mod explorer;
pub mod query;
pub mod render;
pub mod shell;

use explorer::MongoExplorer;
use query::MongoQueryService;

/// Quanto esperar por um servidor que não responde.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// As opções do driver para `settings`. O usuário, quando há, é conferido
/// no banco `admin` (onde ficam os usuários na maioria das instalações e
/// no Atlas).
pub async fn client_options(
    settings: &ConnectionSettings,
    password: &str,
) -> Result<ClientOptions, ConnectError> {
    let mut options = match settings.mongodb_srv() {
        // Cluster achado pelo DNS (Atlas): o driver resolve os nós.
        Some(cluster) => ClientOptions::parse(format!("mongodb+srv://{cluster}/"))
            .await
            .map_err(|e| ConnectError::Network(e.to_string()))?,
        None => {
            let mut options = ClientOptions::default();
            options.hosts = vec![ServerAddress::Tcp {
                host: settings.host.clone(),
                port: Some(settings.port),
            }];
            // Fala só com este servidor, mesmo que ele seja de um replica
            // set cujos outros nomes não resolvem daqui (contêineres).
            options.direct_connection = Some(true);
            options
        }
    };
    options.app_name = Some("PostPilot".into());
    options.connect_timeout = Some(CONNECT_TIMEOUT);
    options.server_selection_timeout = Some(CONNECT_TIMEOUT);
    let user = settings.user.trim();
    if !user.is_empty() {
        let mut credential = Credential::default();
        credential.username = Some(user.to_string());
        credential.password = Some(password.to_string());
        credential.source = Some("admin".into());
        options.credential = Some(credential);
    }
    let tls = |verify: bool| {
        let mut tls = TlsOptions::default();
        tls.allow_invalid_certificates = Some(!verify);
        Tls::Enabled(tls)
    };
    match settings.ssl {
        // Um cluster pelo DNS é sempre com TLS.
        SslMode::Disable if settings.mongodb_srv().is_some() => {}
        SslMode::Disable => options.tls = Some(Tls::Disabled),
        SslMode::Prefer | SslMode::Require => options.tls = Some(tls(false)),
        SslMode::VerifyFull => options.tls = Some(tls(true)),
    }
    Ok(options)
}

/// O erro do driver como o diálogo de conexão mostra.
fn connect_error(e: mongodb::error::Error) -> ConnectError {
    let text = e.to_string();
    match *e.kind {
        ErrorKind::Authentication { .. } => ConnectError::Authentication(
            tr("Authentication failed: check the login and the password.").to_string(),
        ),
        ErrorKind::Command(ref c) if c.code == 18 || c.code == 13 => {
            ConnectError::Authentication(c.message.clone())
        }
        ErrorKind::ServerSelection { .. } | ErrorKind::Io(_) | ErrorKind::DnsResolve { .. } => {
            let lower = text.to_lowercase();
            if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
                ConnectError::Tls(text)
            } else if lower.contains("authentication") || lower.contains("auth failed") {
                ConnectError::Authentication(
                    tr("Authentication failed: check the login and the password.").to_string(),
                )
            } else {
                ConnectError::Network(text)
            }
        }
        ErrorKind::Command(ref c) => ConnectError::Other(c.message.clone()),
        _ => ConnectError::Other(text),
    }
}

/// O [`Connector`] do MongoDB.
#[derive(Clone)]
pub struct MongoConnector {
    runtime: DbRuntime,
}

impl MongoConnector {
    pub fn new(runtime: DbRuntime) -> Self {
        MongoConnector { runtime }
    }
}

impl Connector for MongoConnector {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError> {
        let mut settings = settings.clone();
        settings.engine = Engine::MongoDb;
        if settings.database.trim().is_empty() {
            settings.database = Engine::MongoDb.default_database().into();
        }
        let (client, server_version) = self.runtime.block_on(async {
            let options = client_options(&settings, password).await?;
            let client = Client::with_options(options).map_err(connect_error)?;
            // O driver só abre a conexão no primeiro comando: este confere
            // rede, TLS e senha, e traz a versão.
            let info = client
                .database("admin")
                .run_command(doc! { "buildInfo": 1 })
                .await
                .map_err(connect_error)?;
            // Um servidor com autenticação aceita o `buildInfo` de quem
            // não entrou: este comando não.
            client
                .database(&settings.database)
                .run_command(doc! { "listCollections": 1, "nameOnly": true })
                .await
                .map_err(connect_error)?;
            let version = info.get_str("version").unwrap_or_default().to_string();
            Ok::<_, ConnectError>((client, version))
        })?;
        let explorer = MongoExplorer::new(
            client.clone(),
            &settings,
            &server_version,
            self.runtime.clone(),
        );
        Ok(Connected {
            settings: settings.clone(),
            server_version,
            metadata: Arc::new(explorer),
            queries: Arc::new(MongoQueryService::new(
                client,
                settings,
                self.runtime.clone(),
            )),
        })
    }
}
