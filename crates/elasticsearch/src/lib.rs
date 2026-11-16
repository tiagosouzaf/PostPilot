//! Elasticsearch no PostPilot: conexão pela API REST ([`http`]),
//! requisições no estilo do console do Kibana nas janelas de consulta
//! ([`console`], [`query`], [`render`]) e os índices no Object Explorer
//! ([`explorer`]).

use std::sync::Arc;

use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, ELASTICSEARCH_DATABASE, Engine, SslMode,
};
use pgstudio_i18n::tr;

pub mod console;
pub mod explorer;
pub mod http;
pub mod json;
pub mod query;
pub mod render;

use explorer::EsExplorer;
use http::{EsClient, HttpError};
use json::Json;
use query::EsQueryService;

/// O [`Connector`] do Elasticsearch.
#[derive(Clone, Default)]
pub struct EsConnector;

impl EsConnector {
    pub fn new() -> Self {
        EsConnector
    }
}

impl Connector for EsConnector {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError> {
        let mut settings = settings.clone();
        settings.engine = Engine::Elasticsearch;
        settings.database = ELASTICSEARCH_DATABASE.into();
        let client = EsClient::new(&settings, password);
        // A raiz confere rede, TLS e credencial, e traz a versão.
        let reply = client.request("GET", "/", None).map_err(|e| match e {
            HttpError::Tls(m) => ConnectError::Tls(m),
            // Falar HTTP com uma porta HTTPS (o padrão do Elasticsearch
            // 8) derruba a conexão sem explicar.
            HttpError::Network(m) if settings.ssl == SslMode::Disable => {
                ConnectError::Network(format!(
                    "{m}\n{}",
                    tr("If the server uses HTTPS (the default since Elasticsearch 8), choose an encryption option.")
                ))
            }
            HttpError::Network(m) => ConnectError::Network(m),
        })?;
        match reply.status {
            401 => {
                return Err(ConnectError::Authentication(
                    tr("Authentication failed: check the login and the password (or the API key).")
                        .to_string(),
                ));
            }
            403 => return Err(ConnectError::Authentication(reply.error_text())),
            _ if !reply.ok() => return Err(ConnectError::Other(reply.error_text())),
            _ => {}
        }
        let info = reply.json().ok_or_else(|| {
            ConnectError::Other(
                tr("The server answered, but not like an Elasticsearch.").to_string(),
            )
        })?;
        let server_version = info
            .at("version.number")
            .and_then(Json::as_str)
            .ok_or_else(|| {
                ConnectError::Other(
                    tr("The server answered, but not like an Elasticsearch.").to_string(),
                )
            })?
            .to_string();
        let explorer = EsExplorer::new(client.clone(), &settings, &server_version);
        Ok(Connected {
            settings,
            server_version,
            metadata: Arc::new(explorer),
            queries: Arc::new(EsQueryService::new(client)),
        })
    }
}
