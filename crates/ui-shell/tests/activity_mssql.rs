//! Activity Monitor contra um SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, o teste só avisa; veja `docs/sqlserver.md`):
//! uma sessão segura um lock numa transação, outra fica bloqueada; o
//! monitor mostra a espera, a cadeia de bloqueio, os locks, a E/S e as
//! consultas mais caras, e o Kill na sessão que bloqueia libera a outra.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::activity::{Action, ActivityMonitor, blocking_chains};

fn service() -> Option<pgstudio_mssql::MssqlQueryService> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::SqlServer);
    settings.host = host;
    settings.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    settings.user = var("MSSQL_TEST_USER", "sa");
    settings.ssl = SslMode::Require;
    Some(pgstudio_mssql::MssqlQueryService::new(
        settings,
        var("MSSQL_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

/// Roda `sql` até o fim; devolve os erros.
fn run(session: &dyn QuerySession, sql: &str) -> Vec<String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut errors = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => errors.push(e.message),
            QueryEvent::Finished { .. } => return errors,
            _ => {}
        }
    }
}

fn pump(m: &mut ActivityMonitor, ctx: &egui::Context, done: impl Fn(&ActivityMonitor) -> bool) {
    let start = Instant::now();
    while !done(m) {
        m.step(ctx);
        assert!(start.elapsed() < Duration::from_secs(30), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn blocked_session_is_shown_and_killing_the_blocker_frees_it() {
    let Some(service) = service() else { return };
    let blocker = service.open("tempdb");
    assert_eq!(
        run(
            blocker.as_ref(),
            "IF OBJECT_ID('dbo.pp_activity') IS NOT NULL DROP TABLE dbo.pp_activity;\n\
             CREATE TABLE dbo.pp_activity (id int PRIMARY KEY, n int);\n\
             INSERT INTO dbo.pp_activity VALUES (1, 0);\n\
             BEGIN TRANSACTION;\n\
             UPDATE dbo.pp_activity SET n = 1 WHERE id = 1 /* pp_blocker */",
        ),
        Vec::<String>::new()
    );
    let victim = service.open("tempdb");
    let (tx, rx) = mpsc::channel();
    victim.execute(
        "UPDATE dbo.pp_activity SET n = 2 WHERE id = 1 /* pp_victim */".into(),
        Box::new(move |e| drop(tx.send(e))),
    );

    let ctx = egui::Context::default();
    let mut m = ActivityMonitor::new(service.open("tempdb"), "test".into());
    assert!(!m.can_cancel(), "SQL Server only has KILL");
    m.interval = 1;
    let start = Instant::now();
    let victim_row = loop {
        m.refresh(&ctx);
        pump(&mut m, &ctx, |m| !m.is_refreshing());
        assert_eq!(m.error(), None);
        let found = m.snapshot().and_then(|s| {
            s.processes
                .iter()
                .find(|p| p.query.contains("pp_activity") && !p.blocked_by.is_empty())
                .cloned()
        });
        if let Some(p) = found {
            break p;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "victim not seen: {:?}",
            m.snapshot().map(|s| s
                .processes
                .iter()
                .filter(|p| p.is_client())
                .collect::<Vec<_>>())
        );
    };
    assert_eq!(victim_row.state, "active");
    assert_eq!(victim_row.wait_type, "Lock");
    assert!(
        victim_row.wait_event.starts_with("LCK_M_"),
        "{victim_row:?}"
    );
    assert!(victim_row.is_waiting() && victim_row.is_client());
    assert_eq!(victim_row.database, "tempdb");
    let blocker_pid: i32 = victim_row.blocked_by.parse().unwrap();

    // As consultas mais caras vêm da segunda leitura em diante.
    m.refresh(&ctx);
    pump(&mut m, &ctx, |m| !m.is_refreshing());
    assert_eq!(m.error(), None);
    let snap = m.snapshot().unwrap();
    let blocker_row = snap
        .processes
        .iter()
        .find(|p| p.pid == blocker_pid)
        .expect("blocker listed");
    assert_eq!(blocker_row.state, "idle in transaction");
    assert!(blocker_row.query.contains("pp_blocker"));
    assert!(blocker_row.transaction_seconds.is_some());
    // A cadeia: quem bloqueia na cabeça, a vítima um nível abaixo.
    let chain = blocking_chains(&snap.processes);
    assert!(chain.contains(&(0, blocker_pid)), "{chain:?}");
    assert!(chain.contains(&(1, victim_row.pid)), "{chain:?}");
    // Locks: o da vítima espera, na frente dos concedidos.
    let waiting = snap
        .locks
        .iter()
        .find(|l| !l.granted)
        .expect("waiting lock");
    assert_eq!(waiting.pid, victim_row.pid);
    assert!(
        snap.locks
            .iter()
            .any(|l| l.granted && l.pid == blocker_pid && l.relation == "dbo.pp_activity"),
        "{:?}",
        snap.locks
    );
    assert!(!snap.locks[0].granted);
    // E/S por banco e consultas mais caras (na segunda leitura em diante).
    assert!(snap.databases.iter().any(|d| d.name == "tempdb"));
    assert!(snap.has_statements);
    assert!(snap.statements.as_ref().is_some_and(|s| !s.is_empty()));

    // O Kill em quem bloqueia desfaz a transação e libera a vítima.
    m.request(Action::Kill(blocker_pid));
    m.answer(true, &ctx);
    pump(&mut m, &ctx, |m| m.message().is_some());
    assert_eq!(
        m.message(),
        Some(format!("Process {blocker_pid} was killed.").as_str())
    );
    let mut errors = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QueryEvent::Error(e) => errors.push(e.message),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(errors, Vec::<String>::new(), "the victim finishes");
    let check = service.open("tempdb");
    assert_eq!(
        run(check.as_ref(), "DROP TABLE dbo.pp_activity"),
        Vec::<String>::new()
    );
}
