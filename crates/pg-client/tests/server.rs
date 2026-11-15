//! Testes contra um PostgreSQL de verdade.
//!
//! Os testes que precisam de servidor leem `PGSTUDIO_TEST_HOST`,
//! `PGSTUDIO_TEST_PORT`, `PGSTUDIO_TEST_USER` e `PGSTUDIO_TEST_PASSWORD`. Sem
//! `PGSTUDIO_TEST_HOST` eles avisam e passam; o CI roda com PostgreSQL 13 e 17
//! em container (job "PostgreSQL").

use std::time::{Duration, Instant};

use pgstudio_core::connection::{ConnectError, ConnectionSettings, SslMode};
use pgstudio_pg_client::{DbRuntime, Session};
use tokio_postgres::SimpleQueryMessage;
use tokio_postgres::error::SqlState;

struct Server {
    settings: ConnectionSettings,
    password: String,
}

fn server() -> Option<Server> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping the server part of this test");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    Some(Server {
        settings: ConnectionSettings {
            engine: Default::default(),
            host,
            port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
            database: "postgres".into(),
            user: var("PGSTUDIO_TEST_USER", "postgres"),
            ssl: SslMode::Disable,
        },
        password: var("PGSTUDIO_TEST_PASSWORD", ""),
    })
}

fn runtime() -> DbRuntime {
    DbRuntime::new().unwrap()
}

fn connect(
    rt: &DbRuntime,
    s: &ConnectionSettings,
    password: &str,
) -> Result<Session, ConnectError> {
    rt.block_on(Session::connect(s, password))
}

fn first_value(rt: &DbRuntime, session: &Session, sql: &str) -> String {
    let rows = rt.block_on(session.simple_query(sql)).unwrap();
    rows.iter()
        .find_map(|m| match m {
            SimpleQueryMessage::Row(r) => r.get(0).map(str::to_string),
            _ => None,
        })
        .unwrap()
}

#[test]
fn closed_port_is_a_network_error() {
    let rt = runtime();
    let s = ConnectionSettings {
        host: "127.0.0.1".into(),
        port: 1,
        ssl: SslMode::Disable,
        ..Default::default()
    };
    let err = connect(&rt, &s, "x").err().unwrap();
    assert!(matches!(err, ConnectError::Network(_)), "{err:?}");
    assert!(err.message().contains("127.0.0.1,1"), "{err}");
}

#[test]
fn connects_and_reads_the_version() {
    let Some(srv) = server() else { return };
    let rt = runtime();
    for ssl in [SslMode::Disable, SslMode::Prefer] {
        let s = ConnectionSettings {
            ssl,
            ..srv.settings.clone()
        };
        let session = connect(&rt, &s, &srv.password).unwrap();
        let version = session.server_version();
        let major: u32 = version.split('.').next().unwrap().parse().unwrap();
        assert!(major >= 13, "{version}");
        assert_eq!(
            first_value(&rt, &session, "SELECT current_setting('application_name')"),
            "PostPilot"
        );
    }
}

#[test]
fn wrong_password_and_missing_database_are_explained() {
    let Some(srv) = server() else { return };
    let rt = runtime();
    let err = connect(&rt, &srv.settings, "definitely-not-the-password")
        .err()
        .unwrap();
    // Servidor em "trust" aceita qualquer senha; aí não há o que testar.
    if !matches!(err, ConnectError::Authentication(_)) {
        panic!("expected an authentication error, got {err:?}");
    }
    assert!(err.message().contains(&srv.settings.user), "{err}");

    let s = ConnectionSettings {
        database: "pgstudio_no_such_database".into(),
        ..srv.settings.clone()
    };
    let err = connect(&rt, &s, &srv.password).err().unwrap();
    assert!(matches!(err, ConnectError::Database(_)), "{err:?}");
    assert!(err.message().contains("pgstudio_no_such_database"), "{err}");
}

#[test]
fn require_and_verify_full_follow_the_server_tls() {
    let Some(srv) = server() else { return };
    let rt = runtime();
    let plain = connect(&rt, &srv.settings, &srv.password).unwrap();
    let server_ssl = first_value(&rt, &plain, "SHOW ssl") == "on";

    let require = ConnectionSettings {
        ssl: SslMode::Require,
        ..srv.settings.clone()
    };
    match connect(&rt, &require, &srv.password) {
        Ok(session) => {
            assert!(server_ssl);
            let encrypted = first_value(
                &rt,
                &session,
                "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            );
            assert_eq!(encrypted, "t");
        }
        Err(err) => {
            assert!(!server_ssl, "{err:?}");
            assert!(matches!(err, ConnectError::Tls(_)), "{err:?}");
        }
    }

    if server_ssl {
        // O servidor de teste usa certificado próprio, que o sistema não
        // conhece: Verify full tem de recusar.
        let verify = ConnectionSettings {
            ssl: SslMode::VerifyFull,
            ..srv.settings.clone()
        };
        let err = connect(&rt, &verify, &srv.password).err().unwrap();
        assert!(matches!(err, ConnectError::Tls(_)), "{err:?}");
    }
}

#[test]
fn cancel_stops_a_running_command() {
    let Some(srv) = server() else { return };
    let rt = runtime();
    let session = std::sync::Arc::new(connect(&rt, &srv.settings, &srv.password).unwrap());
    let running = session.clone();
    let started = Instant::now();
    let query = rt
        .handle()
        .spawn(async move { running.simple_query("SELECT pg_sleep(30)").await });
    std::thread::sleep(Duration::from_millis(300));
    rt.block_on(session.cancel()).unwrap();
    let result = rt.block_on(query).unwrap();
    let Err(err) = result else {
        panic!("the sleep must be cancelled");
    };
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // A sessão continua usável depois do cancelamento.
    assert_eq!(first_value(&rt, &session, "SELECT 1"), "1");
}
