//! Tradução dos erros do `tokio-postgres` para mensagens ao usuário.

use std::error::Error as _;

use pgstudio_core::connection::{ConnectError, ConnectionSettings};
use tokio_postgres::error::SqlState;

/// Texto do erro com as causas, sem repetir o prefixo genérico do driver
/// quando há uma causa mais útil.
pub fn describe(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        return db.message().to_string();
    }
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        source = s.source();
    }
    parts.join(": ")
}

/// Classifica a falha ao conectar e escreve a mensagem para o diálogo.
pub fn connect_error(e: &tokio_postgres::Error, settings: &ConnectionSettings) -> ConnectError {
    let server = settings.server_name();
    let detail = describe(e);
    if let Some(code) = e.code() {
        let c = code.clone();
        if c == SqlState::INVALID_PASSWORD || c == SqlState::INVALID_AUTHORIZATION_SPECIFICATION {
            return ConnectError::Authentication(format!(
                "Login failed for user \"{}\": {detail}",
                settings.user
            ));
        }
        if c == SqlState::INVALID_CATALOG_NAME || c == SqlState::OBJECT_NOT_IN_PREREQUISITE_STATE {
            return ConnectError::Database(format!(
                "Cannot open database \"{}\": {detail}",
                settings.database
            ));
        }
        return ConnectError::Other(format!("The server refused the connection: {detail}"));
    }
    let text = e.to_string();
    if text.starts_with("error performing TLS handshake") {
        let detail = strip(&detail, "error performing TLS handshake");
        if detail.contains("invalid peer certificate") {
            return ConnectError::Tls(format!(
                "The certificate of {server} is not trusted by this computer. Choose \
                 \"Require\" to encrypt without verifying the certificate. ({detail})"
            ));
        }
        return ConnectError::Tls(format!(
            "Could not establish an encrypted connection to {server}: {detail}"
        ));
    }
    if text.starts_with("invalid configuration") && detail.contains("password missing") {
        return ConnectError::Authentication(format!(
            "The server asked for a password for user \"{}\". Enter the password and try again.",
            settings.user
        ));
    }
    if text.starts_with("error connecting to server")
        || text.starts_with("timeout waiting for server")
        || text.starts_with("error communicating with the server")
        || e.is_closed()
    {
        let detail = strip(&detail, "error connecting to server");
        return ConnectError::Network(format!(
            "Could not connect to {server}. Check that the server is running and accepts \
             connections on port {}. ({detail})",
            settings.port
        ));
    }
    ConnectError::Other(detail)
}

fn strip<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.strip_prefix(prefix)
        .map(|t| t.trim_start_matches(':').trim_start())
        .filter(|t| !t.is_empty())
        .unwrap_or(text)
}
