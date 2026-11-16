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
    t.starts_with("postgres://") || t.starts_with("postgresql://") || t.contains('=')
}

/// Lê uma connection string nas duas formas.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(tr("Paste a connection string.").into());
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
