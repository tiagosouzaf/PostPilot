//! Connection strings do PostgreSQL, como a libpq aceita: a URI
//! (`postgresql://usuario:senha@host:5432/banco?sslmode=require`) e a forma
//! chave=valor (`host=db port=5432 dbname=vendas user=app password=...`).
//!
//! O resultado separa a senha das demais configurações: quem chama decide
//! onde ela vai (um cofre), e a connection string nunca é guardada inteira.

use crate::connection::{ConnectionSettings, DEFAULT_DATABASE, DEFAULT_PORT, SslMode};
use pgstudio_i18n::{tr, trf};

/// Uma connection string lida.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    /// Servidor, porta, banco, usuário e SSL.
    pub settings: ConnectionSettings,
    /// A senha, se a connection string tinha uma.
    pub password: Option<String>,
}

/// Se o texto parece uma connection string (e não um nome de servidor).
pub fn looks_like(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("postgres://")
        || t.starts_with("postgresql://")
        || t.starts_with("redis://")
        || t.starts_with("rediss://")
        || t.starts_with("mongodb://")
        || t.starts_with("mongodb+srv://")
        || t.starts_with("http://")
        || t.starts_with("https://")
        || t.contains('=')
}

/// Lê uma connection string nas duas formas.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(tr("Paste a connection string.").into());
    }
    if let Some((rest, tls)) = text
        .strip_prefix("redis://")
        .map(|r| (r, false))
        .or_else(|| text.strip_prefix("rediss://").map(|r| (r, true)))
    {
        return parse_redis(rest, tls);
    }
    if let Some((rest, srv)) = text
        .strip_prefix("mongodb://")
        .map(|r| (r, false))
        .or_else(|| text.strip_prefix("mongodb+srv://").map(|r| (r, true)))
    {
        return parse_mongodb(rest, srv);
    }
    if let Some((rest, tls)) = text
        .strip_prefix("http://")
        .map(|r| (r, false))
        .or_else(|| text.strip_prefix("https://").map(|r| (r, true)))
    {
        return parse_elasticsearch(rest, tls);
    }
    let mut parsed = Parsed {
        settings: ConnectionSettings {
            engine: Default::default(),
            host: "localhost".into(),
            port: DEFAULT_PORT,
            database: String::new(),
            user: String::new(),
            ssl: SslMode::default(),
            ssh: None,
        },
        password: None,
    };
    let pairs = if let Some(rest) = text
        .strip_prefix("postgresql://")
        .or_else(|| text.strip_prefix("postgres://"))
    {
        parse_uri(rest, &mut parsed)?
    } else if text.contains('=') {
        parse_pairs(text)?
    } else {
        return Err(tr(
            "Not a connection string. Use postgresql://user:password@host:5432/database or host=... dbname=... user=...",
        )
        .into());
    };
    for (key, value) in pairs {
        apply(&mut parsed, &key, value)?;
    }
    if parsed.settings.database.is_empty() {
        parsed.settings.database = if parsed.settings.user.is_empty() {
            DEFAULT_DATABASE.into()
        } else {
            // Como a libpq: sem banco, o do nome do usuário.
            parsed.settings.user.clone()
        };
    }
    if parsed.settings.user.is_empty() {
        parsed.settings.user = "postgres".into();
    }
    Ok(parsed)
}

/// `redis://[usuario][:senha]@host[:porta][/banco]`; `rediss://` é com
/// TLS.
fn parse_redis(rest: &str, tls: bool) -> Result<Parsed, String> {
    use crate::connection::{Engine, redis_database_index};
    let mut parsed = Parsed {
        settings: ConnectionSettings::for_engine(Engine::Redis),
        password: None,
    };
    if tls {
        parsed.settings.ssl = SslMode::Require;
    }
    for (key, value) in parse_uri(rest, &mut parsed)? {
        match key.as_str() {
            "user" => parsed.settings.user = value,
            "password" => parsed.password = Some(value),
            "host" => parsed.settings.host = value,
            "dbname" => {
                let n = redis_database_index(&value).ok_or_else(|| {
                    trf!(
                        "\"{database}\" is not a Redis database. Use db0, db1...",
                        database = value
                    )
                })?;
                parsed.settings.database = format!("db{n}");
            }
            // Parâmetros de outros clientes: não mudam a conexão.
            _ => {}
        }
    }
    Ok(parsed)
}

/// `mongodb://[usuario:senha@]host[:porta][/banco][?tls=true]`;
/// `mongodb+srv://` é um cluster achado pelo DNS (Atlas), sempre com TLS.
fn parse_mongodb(rest: &str, srv: bool) -> Result<Parsed, String> {
    use crate::connection::{Engine, MONGODB_SRV};
    let mut parsed = Parsed {
        settings: ConnectionSettings::for_engine(Engine::MongoDb),
        password: None,
    };
    if srv {
        parsed.settings.ssl = SslMode::VerifyFull;
    }
    for (key, value) in parse_uri(rest, &mut parsed)? {
        match key.as_str() {
            "user" => parsed.settings.user = value,
            "password" => parsed.password = Some(value),
            "host" if srv => parsed.settings.host = format!("{MONGODB_SRV}{value}"),
            "host" => parsed.settings.host = value,
            "dbname" => parsed.settings.database = value,
            "tls" | "ssl" => {
                parsed.settings.ssl = match value.as_str() {
                    "true" => SslMode::VerifyFull,
                    _ => SslMode::Disable,
                };
            }
            "tlsAllowInvalidCertificates" | "tlsInsecure" if value == "true" => {
                parsed.settings.ssl = SslMode::Require;
            }
            // `authSource`, `replicaSet`, `retryWrites`...: não mudam a
            // conexão daqui.
            _ => {}
        }
    }
    if srv {
        parsed.settings.port = Engine::MongoDb.default_port();
    }
    Ok(parsed)
}

/// `http[s]://[usuario:senha@]host[:porta]`: o endereço de um
/// Elasticsearch. Com `https` o certificado é aceito como vier (o de uma
/// instalação nova é autoassinado); quem quiser conferir escolhe no
/// diálogo.
fn parse_elasticsearch(rest: &str, tls: bool) -> Result<Parsed, String> {
    use crate::connection::Engine;
    let mut parsed = Parsed {
        settings: ConnectionSettings::for_engine(Engine::Elasticsearch),
        password: None,
    };
    if tls {
        parsed.settings.ssl = SslMode::Require;
    }
    let database = parsed.settings.database.clone();
    for (key, value) in parse_uri(rest, &mut parsed)? {
        match key.as_str() {
            "user" => parsed.settings.user = value,
            "password" => parsed.password = Some(value),
            "host" => parsed.settings.host = value,
            _ => {}
        }
    }
    // O caminho da URL não é um banco.
    parsed.settings.database = database;
    Ok(parsed)
}

fn apply(parsed: &mut Parsed, key: &str, value: String) -> Result<(), String> {
    let s = &mut parsed.settings;
    match key {
        "host" | "hostaddr" => {
            if value.contains(',') {
                return Err(tr("Connection strings with several hosts are not supported.").into());
            }
            if !value.is_empty() {
                s.host = value;
            }
        }
        "port" => s.port = port(&value)?,
        "dbname" => s.database = value,
        "user" => s.user = value,
        "password" => parsed.password = Some(value),
        "sslmode" => {
            s.ssl = match value.as_str() {
                "disable" => SslMode::Disable,
                "allow" | "prefer" => SslMode::Prefer,
                "require" | "verify-ca" => SslMode::Require,
                "verify-full" => SslMode::VerifyFull,
                other => {
                    return Err(trf!("Unknown sslmode \"{mode}\".", mode = other));
                }
            }
        }
        // Opções que não mudam a conexão no PostPilot.
        _ => {}
    }
    Ok(())
}

fn port(value: &str) -> Result<u16, String> {
    match value.trim().parse::<u16>() {
        Ok(0) | Err(_) => Err(trf!("\"{port}\" is not a valid port.", port = value.trim())),
        Ok(n) => Ok(n),
    }
}

/// `usuario:senha@host:porta/banco?opcoes`.
fn parse_uri(rest: &str, parsed: &mut Parsed) -> Result<Vec<(String, String)>, String> {
    let (main, query) = match rest.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match main.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (main, None),
    };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let mut pairs = Vec::new();
    if let Some(userinfo) = userinfo {
        let (user, password) = match userinfo.split_once(':') {
            Some((u, p)) => (u, Some(p)),
            None => (userinfo, None),
        };
        pairs.push(("user".into(), decode(user)?));
        if let Some(p) = password {
            pairs.push(("password".into(), decode(p)?));
        }
    }
    if hostport.contains(',') {
        return Err(tr("Connection strings with several hosts are not supported.").into());
    }
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or_else(|| tr("Missing \"]\" after the IPv6 address.").to_string())?;
        pairs.push(("host".into(), host.to_string()));
        if let Some(p) = after.strip_prefix(':') {
            parsed.settings.port = port(p)?;
        }
    } else if !hostport.is_empty() {
        let (host, p) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        };
        pairs.push(("host".into(), decode(host)?));
        if let Some(p) = p {
            parsed.settings.port = port(p)?;
        }
    }
    if let Some(db) = path.filter(|p| !p.is_empty()) {
        pairs.push(("dbname".into(), decode(db)?));
    }
    for item in query
        .unwrap_or_default()
        .split('&')
        .filter(|i| !i.is_empty())
    {
        let (k, v) = item.split_once('=').unwrap_or((item, ""));
        pairs.push((decode(k)?, decode(v)?));
    }
    Ok(pairs)
}

/// Decodifica `%XX` (e mantém o resto).
fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| tr("Invalid %-escape in the connection string.").to_string())?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| tr("Invalid %-escape in the connection string.").into())
}

/// `chave=valor chave='valor com espaço'`, com `\'` e `\\` escapados.
fn parse_pairs(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.next() != Some('=') {
            return Err(trf!("Missing \"=\" after \"{key}\".", key = key));
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            value.push(n);
                        }
                    }
                    '\'' => {
                        closed = true;
                        break;
                    }
                    c => value.push(c),
                }
            }
            if !closed {
                return Err(tr("Unterminated quoted value in the connection string.").into());
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        value.push(n);
                    }
                } else {
                    value.push(c);
                }
            }
        }
        pairs.push((key, value));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elasticsearch_urls() {
        use crate::connection::Engine;
        let p = parse("https://elastic:s%40nha@es.exemplo.com:9243/").unwrap();
        assert_eq!(p.settings.engine, Engine::Elasticsearch);
        assert_eq!(p.settings.host, "es.exemplo.com");
        assert_eq!(p.settings.port, 9243);
        assert_eq!(p.settings.user, "elastic");
        assert_eq!(p.password.as_deref(), Some("s@nha"));
        assert_eq!(p.settings.ssl, SslMode::Require);
        assert_eq!(p.settings.database, "cluster");
        let p = parse("http://localhost:9200").unwrap();
        assert_eq!((p.settings.port, p.settings.ssl), (9200, SslMode::Disable));
        assert_eq!(p.settings.user, "");
        let p = parse("http://localhost").unwrap();
        assert_eq!(p.settings.port, 9200);
        assert!(looks_like("https://localhost:9200"));
    }

    #[test]
    fn mongodb_urls() {
        use crate::connection::Engine;
        let p =
            parse("mongodb://app:s%40nha@mongo.exemplo.com:27018/loja?authSource=admin").unwrap();
        assert_eq!(p.settings.engine, Engine::MongoDb);
        assert_eq!(
            (
                p.settings.host.as_str(),
                p.settings.port,
                p.settings.database.as_str()
            ),
            ("mongo.exemplo.com", 27018, "loja")
        );
        assert_eq!(
            (p.settings.user.as_str(), p.password.as_deref()),
            ("app", Some("s@nha"))
        );
        assert_eq!(p.settings.ssl, SslMode::Disable);
        let p = parse("mongodb://localhost/?tls=true&tlsAllowInvalidCertificates=true").unwrap();
        assert_eq!(
            (p.settings.database.as_str(), p.settings.ssl),
            ("test", SslMode::Require)
        );
        // Atlas: o cluster pelo DNS, com TLS.
        let p = parse("mongodb+srv://app:x@cluster0.abc.mongodb.net/?retryWrites=true&w=majority")
            .unwrap();
        assert_eq!(p.settings.mongodb_srv(), Some("cluster0.abc.mongodb.net"));
        assert_eq!(
            p.settings.server_name(),
            "mongodb+srv://cluster0.abc.mongodb.net"
        );
        assert_eq!(p.settings.ssl, SslMode::VerifyFull);
        assert!(looks_like("mongodb+srv://cluster0.abc.mongodb.net"));
        assert!(parse("mongodb://a,b/loja").is_err());
    }

    #[test]
    fn redis_urls() {
        use crate::connection::Engine;
        let p = parse("redis://:s3nha@cache.exemplo.com:6380/2").unwrap();
        assert_eq!(p.settings.engine, Engine::Redis);
        assert_eq!(
            (
                p.settings.host.as_str(),
                p.settings.port,
                p.settings.database.as_str()
            ),
            ("cache.exemplo.com", 6380, "db2")
        );
        assert_eq!(
            (p.settings.user.as_str(), p.password.as_deref()),
            ("", Some("s3nha"))
        );
        assert_eq!(p.settings.ssl, SslMode::Disable);
        let p = parse("rediss://app:p%40ss@cache").unwrap();
        assert_eq!((p.settings.port, p.settings.ssl), (6379, SslMode::Require));
        assert_eq!(
            (p.settings.user.as_str(), p.password.as_deref()),
            ("app", Some("p@ss"))
        );
        assert_eq!(p.settings.database, "db0");
        assert!(parse("redis://cache/vendas").is_err());
        assert!(looks_like("rediss://cache"));
    }

    #[test]
    fn uri_with_everything() {
        // Senha de teste, %-escapada (`@` e `:`).
        let escaped = "p%40ss%3Aw0rd";
        let p = parse(&format!(
            "postgresql://app:{escaped}@db.vendas.com:6432/vendas?sslmode=require&application_name=x"
        ))
        .unwrap();
        assert_eq!(p.settings.host, "db.vendas.com");
        assert_eq!(p.settings.port, 6432);
        assert_eq!(p.settings.database, "vendas");
        assert_eq!(p.settings.user, "app");
        assert_eq!(p.settings.ssl, SslMode::Require);
        assert_eq!(p.password.as_deref(), Some("p@ss:w0rd"));
    }

    #[test]
    fn uri_minimal_and_ipv6() {
        let p = parse("postgres://localhost").unwrap();
        assert_eq!(p.settings.host, "localhost");
        assert_eq!(p.settings.port, 5432);
        assert_eq!(p.settings.user, "postgres");
        assert_eq!(p.settings.database, "postgres");
        assert_eq!(p.password, None);
        let p = parse("postgresql://alice@[::1]:5433").unwrap();
        assert_eq!((p.settings.host.as_str(), p.settings.port), ("::1", 5433));
        // Sem banco: o do usuário, como a libpq.
        assert_eq!(p.settings.database, "alice");
        let p = parse("postgresql:///vendas?host=/var/run/postgresql").unwrap();
        assert_eq!(p.settings.host, "/var/run/postgresql");
        assert_eq!(p.settings.database, "vendas");
    }

    #[test]
    fn key_value_form() {
        let p = parse("host=db port=5433 dbname='minha base' user=app password='it\\'s secret' sslmode=verify-full connect_timeout=5")
            .unwrap();
        assert_eq!(p.settings.host, "db");
        assert_eq!(p.settings.port, 5433);
        assert_eq!(p.settings.database, "minha base");
        assert_eq!(p.settings.ssl, SslMode::VerifyFull);
        assert_eq!(p.password.as_deref(), Some("it's secret"));
        let p = parse("host = db  user = x").unwrap();
        assert_eq!(
            (p.settings.host.as_str(), p.settings.user.as_str()),
            ("db", "x")
        );
    }

    #[test]
    fn errors() {
        assert!(parse("").is_err());
        assert!(parse("db.vendas.com").is_err());
        assert!(parse("postgresql://h:0/db").is_err());
        assert!(parse("postgresql://h:abc").is_err());
        assert!(parse("postgresql://a,b/db").is_err());
        assert!(parse("host=a,b").is_err());
        assert!(parse("sslmode=talvez").is_err());
        assert!(parse("password='aberta").is_err());
        assert!(parse("host").is_err());
        let bad_escape = "%zz";
        assert!(parse(&format!("postgresql://u:{bad_escape}@h")).is_err());
        assert!(looks_like("postgres://x"));
        assert!(looks_like("host=x"));
        assert!(!looks_like("db.vendas.com,5433"));
    }
}
