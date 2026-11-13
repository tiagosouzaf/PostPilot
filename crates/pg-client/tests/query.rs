//! Execução de consultas das janelas de consulta contra um PostgreSQL de
//! verdade (mesmas variáveis de `server.rs`; sem `PGSTUDIO_TEST_HOST` os
//! testes avisam e passam).

use std::sync::mpsc;
use std::time::{Duration, Instant};

use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_pg_client::{DbRuntime, PgQueryService};

fn service() -> Option<PgQueryService> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
    };
    Some(PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        DbRuntime::new().unwrap(),
    ))
}

/// Roda `sql` e junta os eventos até o `Finished`.
fn run(session: &dyn QuerySession, sql: &str) -> Vec<QueryEvent> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.to_string(), Box::new(move |e| drop(tx.send(e))));
    collect(&rx)
}

fn collect(rx: &mpsc::Receiver<QueryEvent>) -> Vec<QueryEvent> {
    let mut events = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("Finished");
        let done = matches!(e, QueryEvent::Finished { .. });
        events.push(e);
        if done {
            return events;
        }
    }
}

#[test]
fn several_statements_give_several_results() {
    let Some(service) = service() else { return };
    let session = service.open("postgres");
    let events = run(
        session.as_ref(),
        "SELECT 1 AS a, NULL::text AS b; SELECT g FROM generate_series(1, 3) g",
    );
    assert_eq!(
        events,
        vec![
            QueryEvent::Columns(vec!["a".into(), "b".into()]),
            QueryEvent::Row(vec![Some("1".into()), None]),
            QueryEvent::CommandComplete { rows: 1 },
            QueryEvent::Columns(vec!["g".into()]),
            QueryEvent::Row(vec![Some("1".into())]),
            QueryEvent::Row(vec![Some("2".into())]),
            QueryEvent::Row(vec![Some("3".into())]),
            QueryEvent::CommandComplete { rows: 3 },
            QueryEvent::Finished { canceled: false },
        ]
    );
    assert_eq!(session.database(), "postgres");
}

#[test]
fn notices_and_errors_carry_the_server_fields() {
    let Some(service) = service() else { return };
    let session = service.open("postgres");
    let events = run(
        session.as_ref(),
        "DO $$ BEGIN RAISE NOTICE 'olá %', 42; END $$;\nSELECT nope FROM pg_class",
    );
    let notice = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Notice(n) => Some(n),
            _ => None,
        })
        .expect("notice");
    assert_eq!(notice.message, "olá 42");
    assert_eq!(notice.severity, "NOTICE");
    let error = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Error(m) => Some(m),
            _ => None,
        })
        .expect("error");
    assert_eq!(error.code, "42703");
    // A posição conta caracteres (o "á" ocupa dois bytes) a partir de 1.
    let sql = "DO $$ BEGIN RAISE NOTICE 'olá %', 42; END $$;\nSELECT nope FROM pg_class";
    let nope = sql[..sql.find("nope").unwrap()].chars().count() + 1;
    assert_eq!(error.position, Some(nope));
    assert_eq!(
        events.last(),
        Some(&QueryEvent::Finished { canceled: false })
    );
}

#[test]
fn a_long_query_cancels_quickly() {
    let Some(service) = service() else { return };
    let session = service.open("postgres");
    // Abre a conexão antes, para o cancelamento ter a quem pedir.
    run(session.as_ref(), "SELECT 1");
    let (tx, rx) = mpsc::channel();
    session.execute(
        "SELECT pg_sleep(30)".into(),
        Box::new(move |e| drop(tx.send(e))),
    );
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    session.cancel();
    let events = collect(&rx);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(
        events.last(),
        Some(&QueryEvent::Finished { canceled: true })
    );
    assert!(
        !events.iter().any(|e| matches!(e, QueryEvent::Error(_))),
        "{events:?}"
    );

    // A sessão continua usável depois do cancelamento.
    let events = run(session.as_ref(), "SELECT 2");
    assert!(events.contains(&QueryEvent::Row(vec![Some("2".into())])));
}

#[test]
fn connection_errors_end_the_execution() {
    let Some(service) = service() else { return };
    let session = service.open("database_that_does_not_exist");
    let events = run(session.as_ref(), "SELECT 1");
    assert!(
        matches!(&events[0], QueryEvent::Error(m) if m.message.contains("database_that_does_not_exist")),
        "{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&QueryEvent::Finished { canceled: false })
    );
}
