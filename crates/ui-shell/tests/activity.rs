//! Activity Monitor: com uma sessão falsa (500 sessões no servidor, Cancel
//! e Kill registrados) e, com `PGSTUDIO_TEST_HOST`, um Kill de verdade num
//! PostgreSQL.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::activity::{Action, ActivityMonitor};

type Log = Arc<Mutex<Vec<String>>>;

/// Sessões no servidor falso.
const SESSIONS: usize = 500;

struct FakeQueries(Log);

impl QueryService for FakeQueries {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession(self.0.clone(), database.to_string()))
    }
}

struct FakeSession(Log, String);

fn row(values: &[&str]) -> Vec<Option<String>> {
    values
        .iter()
        .map(|v| (!v.is_empty()).then(|| v.to_string()))
        .collect()
}

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        self.0.lock().unwrap().push(sql.clone());
        std::thread::spawn(move || {
            if sql.contains("pg_stat_activity") {
                sink(QueryEvent::Columns(vec!["pid".into()]));
                for i in 0..SESSIONS {
                    let pid = (1000 + i).to_string();
                    let (state, wait, event) = if i % 10 == 0 {
                        ("active", "Lock", "transactionid")
                    } else if i % 2 == 0 {
                        ("active", "", "")
                    } else {
                        ("idle", "Client", "ClientRead")
                    };
                    let app = if i == 7 { "payroll" } else { "psql" };
                    // Quem espera lock está bloqueado pela 1001.
                    let blocked = if i % 10 == 0 { "1001" } else { "" };
                    sink(QueryEvent::Row(row(&[
                        &pid,
                        "app",
                        "loja",
                        app,
                        "127.0.0.1",
                        state,
                        wait,
                        event,
                        blocked,
                        "3",
                        "2",
                        "client backend",
                        "select * from pedido",
                    ])));
                }
                sink(QueryEvent::CommandComplete {
                    rows: SESSIONS as u64,
                });
                sink(QueryEvent::Columns(vec!["datname".into()]));
                sink(QueryEvent::Row(row(&["loja", "100", "20", "80", "0"])));
                sink(QueryEvent::CommandComplete { rows: 1 });
                sink(QueryEvent::Columns(vec!["pid".into()]));
                sink(QueryEvent::Row(row(&[
                    "1010",
                    "transactionid",
                    "ShareLock",
                    "f",
                    "",
                    "740",
                    "loja",
                ])));
                sink(QueryEvent::CommandComplete { rows: 1 });
                sink(QueryEvent::Columns(vec!["count".into()]));
                sink(QueryEvent::Row(row(&["0"])));
                sink(QueryEvent::CommandComplete { rows: 1 });
            } else {
                sink(QueryEvent::Columns(vec!["ok".into()]));
                sink(QueryEvent::Row(row(&["t"])));
                sink(QueryEvent::CommandComplete { rows: 1 });
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.1.clone()
    }
}

struct FakeConnector(Log);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries(self.0.clone())),
        })
    }
}

/// Conectado e com o Activity Monitor aberto pelo Ctrl+Alt+A, depois da
/// primeira leitura.
fn monitor() -> (Harness<'static, ShellApp>, Log) {
    let log = Log::default();
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(log.clone())));
    app.connect_to(ConnectionSettings::default(), "secret".into());
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run_steps(2);
    harness.key_press_modifiers(
        Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        },
        Key::A,
    );
    wait_refresh(&mut harness);
    (harness, log)
}

fn wait_refresh(harness: &mut Harness<'static, ShellApp>) {
    let start = Instant::now();
    loop {
        harness.step();
        if harness
            .state()
            .activity_monitor()
            .is_some_and(|m| m.snapshot().is_some() && !m.is_refreshing())
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "no refresh");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run_steps(3);
}

#[test]
fn ctrl_alt_a_opens_the_monitor_with_charts_and_processes() {
    let (harness, log) = monitor();
    let m = harness.state().activity_monitor().unwrap();
    assert_eq!(m.snapshot().unwrap().processes.len(), SESSIONS);
    assert!(log.lock().unwrap()[0].contains("pg_stat_activity"));
    // 250 rodando, 50 delas esperando lock.
    harness.get_by_label("Active Sessions: 250");
    harness.get_by_label("Waiting Sessions: 50");
    harness.get_by_label_contains("Transactions/sec");
    harness.get_by_label("500 of 500 sessions");
    harness.get_by_label("1000");
    // Só as linhas visíveis são desenhadas.
    assert!(m.drawn_rows() < 40, "{}", m.drawn_rows());
    assert!(m.drawn_rows() > 5, "{}", m.drawn_rows());
}

#[test]
fn the_filter_narrows_the_processes() {
    let (mut harness, _) = monitor();
    harness.state_mut().activity_monitor_mut().unwrap().filter = "payroll".into();
    harness.run_steps(2);
    harness.get_by_label("1 of 500 sessions");
    harness.get_by_label("1007");
}

#[test]
fn kill_asks_first_and_then_terminates() {
    let (mut harness, log) = monitor();
    harness.get_by_label("1002").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().activity_monitor().unwrap().selected,
        Some(1002)
    );
    let sent = log.lock().unwrap().len();
    harness.get_by_label("Kill Process").click();
    harness.run_steps(2);
    harness.get_by_label_contains("Are you sure you want to kill process 1002?");
    assert_eq!(log.lock().unwrap().len(), sent, "ran before asking");
    harness.get_by_label("Yes").click();
    let start = Instant::now();
    while harness
        .state()
        .activity_monitor()
        .unwrap()
        .message()
        .is_none()
    {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run_steps(2);
    assert!(
        log.lock()
            .unwrap()
            .contains(&"SELECT pg_terminate_backend(1002)".to_string())
    );
    assert_eq!(
        harness.state().activity_monitor().unwrap().message(),
        Some("Process 1002 was killed.")
    );
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l == "Activity Monitor: Process 1002 was killed.")
    );
}

#[test]
fn answering_no_runs_nothing() {
    let (mut harness, log) = monitor();
    harness.get_by_label("1004").click();
    harness.run_steps(2);
    let sent = log.lock().unwrap().len();
    harness.get_by_label("Cancel Query").click();
    harness.run_steps(2);
    harness.get_by_label_contains("cancel the query running in process 1004");
    harness.get_by_label("No").click();
    harness.run_steps(3);
    assert!(!harness.state().activity_monitor().unwrap().is_confirming());
    assert_eq!(log.lock().unwrap().len(), sent);
    assert!(harness.query_by_label("Yes").is_none());
}

#[test]
fn without_a_connection_it_only_warns() {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), ShellApp::default());
    harness.run();
    harness.state_mut().execute(
        &egui::Context::default(),
        pgstudio_core::CommandId::ActivityMonitor,
    );
    assert!(harness.state().activity_monitor().is_none());
}

// --- PostgreSQL de verdade ---

fn service() -> Option<pgstudio_pg_client::PgQueryService> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
    };
    Some(pgstudio_pg_client::PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

/// Roda o monitor até `done` valer, sem janela.
fn pump(m: &mut ActivityMonitor, ctx: &egui::Context, done: impl Fn(&ActivityMonitor) -> bool) {
    let start = Instant::now();
    while !done(m) {
        m.step(ctx);
        assert!(start.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn kill_terminates_a_real_session() {
    let Some(service) = service() else { return };
    // Uma sessão "vítima" presa num pg_sleep, com nome para achar.
    let victim = service.open("postgres");
    let (tx, rx) = mpsc::channel();
    victim.execute(
        "SET application_name = 'pgstudio_victim'; SELECT pg_sleep(60)".into(),
        Box::new(move |e| drop(tx.send(e))),
    );
    let ctx = egui::Context::default();
    let mut m = ActivityMonitor::new(service.open("postgres"), "test".into());
    m.interval = 1;
    let pid = {
        let start = Instant::now();
        loop {
            m.refresh(&ctx);
            pump(&mut m, &ctx, |m| !m.is_refreshing());
            let found = m.snapshot().and_then(|s| {
                s.processes
                    .iter()
                    .find(|p| p.application == "pgstudio_victim" && p.state == "active")
                    .map(|p| p.pid)
            });
            if let Some(pid) = found {
                break pid;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "victim not seen");
        }
    };
    let p = m
        .snapshot()
        .unwrap()
        .processes
        .iter()
        .find(|p| p.pid == pid)
        .unwrap()
        .clone();
    assert_eq!(p.wait_event, "PgSleep");
    assert!(p.query.contains("pg_sleep"));
    m.request(Action::Kill(pid));
    m.answer(true, &ctx);
    pump(&mut m, &ctx, |m| m.message().is_some());
    assert_eq!(
        m.message(),
        Some(format!("Process {pid} was killed.").as_str())
    );
    // A vítima recebe o erro e a sessão some do servidor.
    let mut error = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QueryEvent::Error(e) => error = Some(e),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    let error = error.expect("the victim should fail");
    assert!(
        error.code == "57P01" || !error.message.is_empty(),
        "{error:?}"
    );
    m.refresh(&ctx);
    pump(&mut m, &ctx, |m| !m.is_refreshing());
    assert!(m.snapshot().unwrap().processes.iter().all(|p| p.pid != pid));
}

#[test]
fn the_locks_section_shows_who_blocks_whom() {
    let (mut harness, _) = monitor();
    // Fecha Overview e Processes para a seção Locks caber na tela.
    harness.get_by_label("Overview").click();
    harness.get_by_label("Processes").click();
    harness.run_steps(10);
    // Já começa aberta: há sessões bloqueadas.
    harness.get_by_label_contains("Head blocker 1001");
    harness.get_by_label_contains("1010 waits");
    harness.get_by_label("Waiting");
    harness.get_by_label("ShareLock");
}

/// Roda `sql` e espera o fim; devolve o erro, se houver.
fn run(session: &Arc<dyn QuerySession>, sql: &str) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut error = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QueryEvent::Error(e) => error = Some(e.message),
            QueryEvent::Finished { .. } => return error,
            _ => {}
        }
    }
}

#[test]
fn a_real_block_shows_up_with_its_chain() {
    let Some(service) = service() else { return };
    let table = format!("pgstudio_locks_{}", std::process::id());
    let holder = service.open("postgres");
    assert_eq!(
        run(
            &holder,
            &format!("SET application_name = 'pgstudio_holder'; CREATE TABLE {table} (id int)")
        ),
        None
    );
    // Num lote só, o BEGIN levaria o CREATE junto para a transação aberta.
    assert_eq!(run(&holder, &format!("BEGIN; LOCK TABLE {table}")), None);
    let waiter = service.open("postgres");
    let (tx, rx) = mpsc::channel();
    waiter.execute(
        format!("SELECT * FROM {table}"),
        Box::new(move |e| drop(tx.send(e))),
    );
    let ctx = egui::Context::default();
    let mut m = ActivityMonitor::new(service.open("postgres"), "test".into());
    let start = Instant::now();
    let (head, blocked) = loop {
        m.refresh(&ctx);
        pump(&mut m, &ctx, |m| !m.is_refreshing());
        let s = m.snapshot().unwrap();
        let pid = |app: &str| {
            s.processes
                .iter()
                .find(|p| p.application == app)
                .map(|p| p.pid)
        };
        let waiter_pid = s
            .processes
            .iter()
            .find(|p| p.query == format!("SELECT * FROM {table}"))
            .map(|p| p.pid);
        if let (Some(h), Some(w)) = (pid("pgstudio_holder"), waiter_pid) {
            let chains = pgstudio_ui_shell::activity::blocking_chains(&s.processes);
            if chains.contains(&(1, w)) {
                assert!(chains.contains(&(0, h)), "{chains:?}");
                break (h, w);
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "block not seen: {:?} {:?}",
            m.error(),
            s.processes
                .iter()
                .filter(|p| p.database == "postgres")
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let s = m.snapshot().unwrap();
    let waiting = s
        .locks
        .iter()
        .find(|l| l.pid == blocked && !l.granted)
        .expect("the waiter's lock");
    assert_eq!(waiting.relation, table);
    assert!(
        s.locks
            .iter()
            .any(|l| l.pid == head && l.granted && l.mode == "AccessExclusiveLock")
    );
    let p = s.processes.iter().find(|p| p.pid == blocked).unwrap();
    assert!(p.is_waiting());
    assert_eq!(p.blocked_by, head.to_string());
    // Solta: o waiter termina sem erro.
    assert_eq!(run(&holder, &format!("ROLLBACK; DROP TABLE {table}")), None);
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QueryEvent::Error(e) => panic!("{}", e.message),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
}
