//! O PostgreSQL por um túnel SSH de verdade. Sem `SSH_TEST_HOST` os testes
//! só avisam; as variáveis são as de `crates/ssh/tests/tunnel.rs`, mais
//! `SSH_TEST_PG_PASSWORD` (a senha do `postgres` no banco).

use std::time::{Duration, Instant};

use pgstudio_core::connection::{ConnectError, ConnectionSettings, SshAuth, SshSettings, SslMode};
use pgstudio_pg_client::{DbRuntime, Session};
use tokio_postgres::SimpleQueryMessage;
use tokio_postgres::error::SqlState;

fn server() -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("SSH_TEST_HOST") else {
        eprintln!("SSH_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let target = var("SSH_TEST_PG", "pgdb:5432");
    let (db_host, db_port) = target.rsplit_once(':').expect("host:port");
    let settings = ConnectionSettings {
        host: db_host.into(),
        port: db_port.parse().unwrap(),
        ssl: SslMode::Disable,
        ssh: Some(SshSettings {
            host,
            port: var("SSH_TEST_PORT", "22").parse().unwrap(),
            user: var("SSH_TEST_USER", "tunnel"),
            auth: SshAuth::KeyFile,
            key_file: format!("{}/id_plain", var("SSH_TEST_KEYS", ".")),
        }),
        ..Default::default()
    };
    Some((settings, var("SSH_TEST_PG_PASSWORD", "")))
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
fn connects_queries_and_cancels_through_the_tunnel() {
    let Some((settings, password)) = server() else {
        return;
    };
    let rt = DbRuntime::new().unwrap();
    let session = rt
        .block_on(Session::connect(&settings, &password))
        .expect("connect through the tunnel");
    // A sessão guarda o que o usuário digitou, não a porta local.
    assert_eq!(session.settings(), &settings);
    assert_eq!(first_value(&rt, &session, "SELECT 40 + 2"), "42");
    assert!(pgstudio_ssh::local_addr(&settings).is_some());

    // Um resultado grande atravessa inteiro.
    let big = first_value(&rt, &session, "SELECT repeat('x', 5000000)");
    assert_eq!(big.len(), 5_000_000);

    // O cancelamento abre a segunda conexão pelo mesmo túnel.
    let canceller = session.canceller();
    let started = Instant::now();
    let handle = rt.handle().clone();
    let cancel = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        handle.block_on(canceller.cancel())
    });
    let err = rt
        .block_on(session.simple_query("SELECT pg_sleep(30)"))
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED));
    assert!(started.elapsed() < Duration::from_secs(10));
    cancel.join().unwrap().unwrap();

    // Duas sessões no mesmo banco dividem o túnel.
    let addr = pgstudio_ssh::local_addr(&settings);
    let second = rt
        .block_on(Session::connect(&settings, &password))
        .expect("second session");
    assert_eq!(pgstudio_ssh::local_addr(&settings), addr);
    drop(session);
    assert_eq!(first_value(&rt, &second, "SELECT 1"), "1");
    drop(second);
}

#[test]
fn database_errors_still_say_what_happened() {
    let Some((settings, password)) = server() else {
        return;
    };
    let rt = DbRuntime::new().unwrap();
    match rt.block_on(Session::connect(&settings, "errada")) {
        Err(ConnectError::Authentication(_)) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
    // Um banco que o bastion não alcança: erro de rede, não travamento.
    let mut nowhere = settings.clone();
    nowhere.port = 1;
    let started = Instant::now();
    match rt.block_on(Session::connect(&nowhere, &password)) {
        Err(ConnectError::Network(m)) => assert!(m.contains("could not reach"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(started.elapsed() < Duration::from_secs(20));
}
