//! Conexões com o servidor: o que o usuário digita no Connect to Server, os
//! erros de conexão e o trait que a interface usa para conectar.
//!
//! A interface não conhece o driver do PostgreSQL: ela recebe um
//! [`Connector`] e chama [`Connector::connect`] fora da thread da interface.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::metadata::MetadataProvider;

/// Porta padrão do PostgreSQL.
pub const DEFAULT_PORT: u16 = 5432;

/// Banco usado quando o usuário não escolhe outro.
pub const DEFAULT_DATABASE: &str = "postgres";

/// Uso de TLS na conexão, com os mesmos nomes do `sslmode` do libpq.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    /// Sem TLS.
    Disable,
    /// Tenta TLS e, se o servidor não aceitar, conecta sem. Não confere o
    /// certificado.
    #[default]
    Prefer,
    /// Exige TLS, sem conferir o certificado.
    Require,
    /// Exige TLS e confere o certificado e o nome do servidor.
    VerifyFull,
}

impl SslMode {
    /// Todos os modos, na ordem do diálogo.
    pub const ALL: [SslMode; 4] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyFull,
    ];

    /// Nome exibido no diálogo.
    pub fn label(self) -> &'static str {
        match self {
            SslMode::Disable => "Disable",
            SslMode::Prefer => "Prefer",
            SslMode::Require => "Require",
            SslMode::VerifyFull => "Verify full",
        }
    }
}

/// Dados de uma conexão, sem a senha. É o que fica salvo em "conexões
/// recentes".
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionSettings {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    #[serde(default)]
    pub ssl: SslMode,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        ConnectionSettings {
            host: "localhost".into(),
            port: DEFAULT_PORT,
            database: DEFAULT_DATABASE.into(),
            user: "postgres".into(),
            ssl: SslMode::default(),
        }
    }
}

impl ConnectionSettings {
    /// "Server name" como no SSMS: o host, mais a porta quando não é a
    /// padrão (`db.exemplo.com,5433`).
    pub fn server_name(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == DEFAULT_PORT {
            host
        } else {
            format!("{host},{}", self.port)
        }
    }

    /// Preenche host e porta a partir do "Server name" digitado.
    pub fn set_server_name(&mut self, text: &str) -> Result<(), String> {
        let (host, port) = parse_server_name(text)?;
        self.host = host;
        self.port = port;
        Ok(())
    }
}

/// Lê um "Server name": `host`, `host,porta` (como no SSMS), `host:porta`,
/// `[::1]` ou `[::1]:porta`.
pub fn parse_server_name(text: &str) -> Result<(String, u16), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Enter a server name.".into());
    }
    let port = |p: &str| -> Result<u16, String> {
        match p.trim().parse::<u16>() {
            Ok(0) | Err(_) => Err(format!("\"{}\" is not a valid port.", p.trim())),
            Ok(n) => Ok(n),
        }
    };
    if let Some(rest) = text.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return Err("Missing \"]\" after the IPv6 address.".into());
        };
        let after = after.trim();
        let p = match after.strip_prefix([':', ',']) {
            Some(p) => port(p)?,
            None if after.is_empty() => DEFAULT_PORT,
            None => return Err(format!("Unexpected \"{after}\" after the address.")),
        };
        return Ok((host.to_string(), p));
    }
    if let Some((host, p)) = text.rsplit_once(',') {
        return Ok((host.trim().to_string(), port(p)?));
    }
    // Mais de um ":" sem colchetes é um IPv6 sem porta.
    if text.matches(':').count() == 1 {
        let (host, p) = text.split_once(':').expect("one colon");
        return Ok((host.trim().to_string(), port(p)?));
    }
    Ok((text.to_string(), DEFAULT_PORT))
}

/// Por que uma conexão falhou, já com a mensagem para o usuário.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    /// Servidor fora do ar, nome que não resolve, porta fechada, tempo
    /// esgotado.
    Network(String),
    /// Usuário ou senha recusados.
    Authentication(String),
    /// Falha ao negociar TLS ou certificado recusado.
    Tls(String),
    /// O banco pedido não existe ou não aceita conexões.
    Database(String),
    /// Qualquer outro erro do servidor ou do driver.
    Other(String),
}

impl ConnectError {
    /// Mensagem para o usuário.
    pub fn message(&self) -> &str {
        match self {
            ConnectError::Network(m)
            | ConnectError::Authentication(m)
            | ConnectError::Tls(m)
            | ConnectError::Database(m)
            | ConnectError::Other(m) => m,
        }
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ConnectError {}

/// Uma conexão aberta, do jeito que a interface usa.
#[derive(Clone)]
pub struct Connected {
    /// Com o que se conectou.
    pub settings: ConnectionSettings,
    /// Versão do servidor, como `16.4`.
    pub server_version: String,
    /// Metadados para o Object Explorer. Soltar o último `Arc` fecha a
    /// conexão.
    pub metadata: Arc<dyn MetadataProvider>,
}

impl fmt::Debug for Connected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connected")
            .field("settings", &self.settings)
            .field("server_version", &self.server_version)
            .finish_non_exhaustive()
    }
}

/// Abre conexões. A chamada bloqueia até conectar ou falhar; a interface
/// chama numa thread separada.
pub trait Connector: Send + Sync {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_names() {
        let ok = |t: &str| parse_server_name(t).unwrap();
        assert_eq!(ok("localhost"), ("localhost".into(), 5432));
        assert_eq!(
            ok(" db.example.com , 5433 "),
            ("db.example.com".into(), 5433)
        );
        assert_eq!(ok("db:6432"), ("db".into(), 6432));
        assert_eq!(ok("[::1]"), ("::1".into(), 5432));
        assert_eq!(ok("[::1]:5433"), ("::1".into(), 5433));
        assert_eq!(ok("[fe80::1],5434"), ("fe80::1".into(), 5434));
        assert_eq!(ok("fe80::1"), ("fe80::1".into(), 5432));
        for bad in [
            "", "  ", "db,", "db,abc", "db:0", "db,70000", "[::1", "[::1]x",
        ] {
            assert!(parse_server_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn server_name_round_trips() {
        let mut s = ConnectionSettings::default();
        assert_eq!(s.server_name(), "localhost");
        for name in ["db,5433", "[::1],5433", "[::1]", "10.0.0.1"] {
            s.set_server_name(name).unwrap();
            assert_eq!(s.server_name(), name);
        }
    }

    #[test]
    fn settings_serialize_without_password() {
        let s = ConnectionSettings {
            ssl: SslMode::VerifyFull,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"verify-full\""), "{json}");
        assert!(!json.contains("password"));
        let back: ConnectionSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
