//! Conexões com o servidor: o que o usuário digita no Connect to Server, os
//! erros de conexão e o trait que a interface usa para conectar.
//!
//! A interface não conhece o driver do banco: ela recebe um [`Connector`] e
//! chama [`Connector::connect`] fora da thread da interface. O motor do
//! servidor (PostgreSQL ou SQL Server) vem em [`ConnectionSettings::engine`].

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::metadata::MetadataProvider;
use crate::query::QueryService;

/// Porta padrão do PostgreSQL.
pub const DEFAULT_PORT: u16 = 5432;

/// Banco usado quando o usuário não escolhe outro.
pub const DEFAULT_DATABASE: &str = "postgres";

/// Porta padrão do SQL Server.
pub const SQL_SERVER_PORT: u16 = 1433;

/// O motor do servidor. Configurações salvas antes do SQL Server não têm
/// o campo e abrem como PostgreSQL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    #[default]
    #[serde(rename = "postgresql")]
    PostgreSql,
    SqlServer,
}

impl Engine {
    /// Os motores, na ordem do Connect to Server.
    pub const ALL: [Engine; 2] = [Engine::PostgreSql, Engine::SqlServer];

    /// Nome do produto (não se traduz).
    pub fn label(self) -> &'static str {
        match self {
            Engine::PostgreSql => "PostgreSQL",
            Engine::SqlServer => "SQL Server",
        }
    }

    /// Porta em que o servidor escuta se ninguém mudou.
    pub fn default_port(self) -> u16 {
        match self {
            Engine::PostgreSql => DEFAULT_PORT,
            Engine::SqlServer => SQL_SERVER_PORT,
        }
    }

    /// Banco de quando o usuário não escolhe outro.
    pub fn default_database(self) -> &'static str {
        match self {
            Engine::PostgreSql => DEFAULT_DATABASE,
            Engine::SqlServer => "master",
        }
    }

    /// Usuário sugerido num servidor novo.
    pub fn default_user(self) -> &'static str {
        match self {
            Engine::PostgreSql => "postgres",
            Engine::SqlServer => "sa",
        }
    }

    /// O motor tem o recurso. O PostgreSQL tem todos; o SQL Server ganha
    /// os seus marco a marco (Fase S do plano).
    pub fn supports(self, feature: Feature) -> bool {
        match self {
            Engine::PostgreSql => true,
            Engine::SqlServer => matches!(feature, Feature::Query),
        }
    }

    /// Para o `serde(skip_serializing_if)`: o PostgreSQL não é gravado,
    /// e os arquivos ficam como eram.
    pub fn is_postgres(&self) -> bool {
        *self == Engine::PostgreSql
    }
}

/// O que o app sabe fazer num servidor, recurso a recurso. Serve para a
/// interface avisar, em vez de mandar SQL de um motor para o outro.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Rodar consultas numa janela de consulta.
    Query,
    /// Plano de execução estimado e real (Ctrl+L, Ctrl+M).
    ExecutionPlan,
    /// Alt+F1 (informações do objeto sob o cursor).
    ObjectInfo,
    /// Editar a grade do resultado e o Edit Top 200 Rows.
    EditRows,
    /// Activity Monitor e bloqueios.
    ActivityMonitor,
    /// Logins, roles e permissões.
    Security,
    /// Backup e restore.
    Backup,
    /// Schema Compare.
    SchemaCompare,
    /// Table Designer e diagramas.
    Designers,
    /// Query Designer.
    QueryDesigner,
    /// Ferramentas do Claude sobre o servidor.
    Claude,
}

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
        pgstudio_i18n::tr(match self {
            SslMode::Disable => "Disable",
            SslMode::Prefer => "Prefer",
            SslMode::Require => "Require",
            SslMode::VerifyFull => "Verify full",
        })
    }
}

/// Dados de uma conexão, sem a senha. É o que fica salvo em "conexões
/// recentes".
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionSettings {
    /// Motor do servidor; o PostgreSQL não é gravado.
    #[serde(default, skip_serializing_if = "Engine::is_postgres")]
    pub engine: Engine,
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
            engine: Engine::PostgreSql,
            host: "localhost".into(),
            port: DEFAULT_PORT,
            database: DEFAULT_DATABASE.into(),
            user: "postgres".into(),
            ssl: SslMode::default(),
        }
    }
}

impl ConnectionSettings {
    /// Configurações padrão de um servidor de `engine` em `localhost`.
    pub fn for_engine(engine: Engine) -> Self {
        ConnectionSettings {
            engine,
            port: engine.default_port(),
            database: engine.default_database().into(),
            user: engine.default_user().into(),
            ..Default::default()
        }
    }

    /// Troca o motor. Porta, banco e usuário que estavam no padrão do motor
    /// anterior passam para o padrão do novo.
    pub fn set_engine(&mut self, engine: Engine) {
        let old = self.engine;
        if old == engine {
            return;
        }
        if self.port == old.default_port() {
            self.port = engine.default_port();
        }
        if self.database.is_empty() || self.database == old.default_database() {
            self.database = engine.default_database().into();
        }
        if self.user.is_empty() || self.user == old.default_user() {
            self.user = engine.default_user().into();
        }
        self.engine = engine;
    }

    /// "Server name" como no SSMS: o host, mais a porta quando não é a
    /// padrão (`db.exemplo.com,5433`).
    pub fn server_name(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == self.engine.default_port() {
            host
        } else {
            format!("{host},{}", self.port)
        }
    }

    /// Preenche host e porta a partir do "Server name" digitado.
    pub fn set_server_name(&mut self, text: &str) -> Result<(), String> {
        let (host, port) = parse_server_name_for(text, self.engine.default_port())?;
        self.host = host;
        self.port = port;
        Ok(())
    }
}

/// Lê um "Server name": `host`, `host,porta` (como no SSMS), `host:porta`,
/// `[::1]` ou `[::1]:porta`. Sem porta, a do PostgreSQL.
pub fn parse_server_name(text: &str) -> Result<(String, u16), String> {
    parse_server_name_for(text, DEFAULT_PORT)
}

/// Como [`parse_server_name`], com `default_port` quando o texto não tem
/// porta.
pub fn parse_server_name_for(text: &str, default_port: u16) -> Result<(String, u16), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(pgstudio_i18n::tr("Enter a server name.").into());
    }
    let port = |p: &str| -> Result<u16, String> {
        match p.trim().parse::<u16>() {
            Ok(0) | Err(_) => Err(pgstudio_i18n::trf!(
                "\"{port}\" is not a valid port.",
                port = p.trim()
            )),
            Ok(n) => Ok(n),
        }
    };
    if let Some(rest) = text.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return Err(pgstudio_i18n::tr("Missing \"]\" after the IPv6 address.").into());
        };
        let after = after.trim();
        let p = match after.strip_prefix([':', ',']) {
            Some(p) => port(p)?,
            None if after.is_empty() => default_port,
            None => {
                return Err(pgstudio_i18n::trf!(
                    "Unexpected \"{after}\" after the address.",
                    after = after
                ));
            }
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
    Ok((text.to_string(), default_port))
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
    /// Abre as conexões das janelas de consulta.
    pub queries: Arc<dyn QueryService>,
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
    fn ssl_labels_have_translations() {
        use pgstudio_i18n::{Language, has_translation, language};
        assert_eq!(language(), Language::English);
        for mode in [
            SslMode::Disable,
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyFull,
        ] {
            assert!(has_translation(
                Language::PortugueseBrazil,
                None,
                mode.label()
            ));
        }
    }

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
    fn engine_defaults_follow_the_server() {
        let mut s = ConnectionSettings::default();
        assert_eq!(s.engine, Engine::PostgreSql);
        s.set_engine(Engine::SqlServer);
        assert_eq!(
            (s.port, s.database.as_str(), s.user.as_str()),
            (1433, "master", "sa")
        );
        assert_eq!(s.server_name(), "localhost");
        s.set_server_name("db,5433").unwrap();
        assert_eq!(s.port, 5433);
        s.set_server_name("sql.example.com").unwrap();
        assert_eq!(s.port, 1433);
        // O que o usuário mudou fica.
        s.database = "vendas".into();
        s.set_engine(Engine::PostgreSql);
        assert_eq!(
            (s.port, s.database.as_str(), s.user.as_str()),
            (5432, "vendas", "postgres")
        );
        assert_eq!(ConnectionSettings::for_engine(Engine::SqlServer).port, 1433);
    }

    #[test]
    fn postgres_is_not_written_and_old_files_open() {
        let json = serde_json::to_string(&ConnectionSettings::default()).unwrap();
        assert!(!json.contains("engine"), "{json}");
        let old = r#"{"host":"h","port":5432,"database":"d","user":"u"}"#;
        let s: ConnectionSettings = serde_json::from_str(old).unwrap();
        assert_eq!(s.engine, Engine::PostgreSql);
        let mssql = ConnectionSettings::for_engine(Engine::SqlServer);
        let json = serde_json::to_string(&mssql).unwrap();
        assert!(json.contains(r#""engine":"sql-server""#), "{json}");
        assert_eq!(
            serde_json::from_str::<ConnectionSettings>(&json).unwrap(),
            mssql
        );
    }

    #[test]
    fn sql_server_supports_only_what_is_done() {
        assert!(Engine::PostgreSql.supports(Feature::ActivityMonitor));
        assert!(Engine::SqlServer.supports(Feature::Query));
        assert!(!Engine::SqlServer.supports(Feature::ActivityMonitor));
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
