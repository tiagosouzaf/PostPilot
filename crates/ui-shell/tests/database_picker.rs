//! Seletor de banco da janela de consulta ("Available Databases" do SSMS),
//! com uma sessão falsa: Ctrl+U abre a lista, escolher um banco troca a
//! conexão da janela, uma transação aberta pede confirmação, e Ctrl+N abre
//! no banco do que está selecionado no Object Explorer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::databases::{IN_TRANSACTION_SQL, LIST_SQL};

/// SQL recebido, com o banco da sessão: `(banco, sql)`.
type Log = Arc<Mutex<Vec<(String, String)>>>;

#[derive(Clone, Default)]
struct Fake {
    log: Log,
    /// `begin` abre uma transação; `commit` fecha.
    in_transaction: Arc<AtomicBool>,
}

impl QueryService for Fake {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(Session(self.clone(), database.to_string()))
    }
}

struct Session(Fake, String);

impl QuerySession for Session {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let fake = self.0.clone();
        fake.log.lock().unwrap().push((self.1.clone(), sql.clone()));
        std::thread::spawn(move || {
            let rows: Vec<&str> = if sql == LIST_SQL {
                vec!["loja", "postgres", "vendas"]
            } else if sql == IN_TRANSACTION_SQL {
                vec![if fake.in_transaction.load(Ordering::SeqCst) {
                    "t"
                } else {
                    "f"
                }]
            } else {
                match sql.as_str() {
                    "begin" => fake.in_transaction.store(true, Ordering::SeqCst),
                    "commit" => fake.in_transaction.store(false, Ordering::SeqCst),
                    _ => {}
                }
                vec!["1"]
            };
            sink(QueryEvent::Columns(vec!["c".into()]));
            for r in &rows {
                sink(QueryEvent::Row(vec![Some(r.to_string())]));
            }
            sink(QueryEvent::CommandComplete {
                rows: rows.len() as u64,
            });
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.1.clone()
    }
}

struct FakeConnector(Fake);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(self.0.clone()),
        })
    }
}

fn harness(fake: &Fake, expanded: &[&str]) -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(fake.clone())));
    app.connect_to(
        ConnectionSettings {
            database: "loja".into(),
            ..Default::default()
        },
        "secret".into(),
    );
    let app = app.with_expanded(expanded.iter().map(|s| s.to_string()));
    let mut harness = Harness::builder()
        .with_size([1280.0, 900.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    harness
}

/// Roda frames até `done` valer.
fn until(harness: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    while !done(harness.state()) {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
}

fn run(harness: &mut Harness<'static, ShellApp>, id: u32, sql: &str) {
    harness.key_press_modifiers(Modifiers::CTRL, Key::A);
    harness.event(Event::Text(sql.to_string()));
    harness.run();
    harness.key_press(Key::F5);
    until(harness, "query never finished", |app| {
        app.documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running())
    });
}

fn session_db(app: &ShellApp, id: u32) -> Option<String> {
    let doc = app.documents().get(id)?;
    doc.session.as_ref().map(|s| s.0.database())
}

/// Ctrl+U e clica no banco quando a lista chegar.
fn pick(harness: &mut Harness<'static, ShellApp>, database: &str) {
    harness.key_press_modifiers(Modifiers::CTRL, Key::U);
    harness.run();
    let start = Instant::now();
    while harness.query_by_label(database).is_none() {
        harness.step();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no {database} in the list"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.get_by_label(database).click();
    harness.run();
}

#[test]
fn ctrl_u_lists_the_databases_and_moves_the_window() {
    let fake = Fake::default();
    let mut harness = harness(&fake, &[]);
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    let id = harness.state().active_document().unwrap();
    // A toolbar já mostra o banco antes de executar.
    assert!(harness.query_by_label("loja").is_some());
    run(&mut harness, id, "select 1");
    assert_eq!(session_db(harness.state(), id).as_deref(), Some("loja"));

    pick(&mut harness, "vendas");
    until(&mut harness, "database never changed", |app| {
        app.documents().get(id).unwrap().database.as_deref() == Some("vendas")
    });
    let log = fake.log.lock().unwrap().clone();
    assert!(log.iter().any(|(_, sql)| sql == LIST_SQL));
    assert!(harness.state().status().contains("vendas"));

    run(&mut harness, id, "select 2");
    assert_eq!(session_db(harness.state(), id).as_deref(), Some("vendas"));
    let (db, sql) = fake.log.lock().unwrap().last().cloned().unwrap();
    assert_eq!((db.as_str(), sql.as_str()), ("vendas", "select 2"));
}

#[test]
fn an_open_transaction_asks_before_changing() {
    let fake = Fake::default();
    let mut harness = harness(&fake, &[]);
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    let id = harness.state().active_document().unwrap();
    run(&mut harness, id, "begin");

    pick(&mut harness, "vendas");
    let start = Instant::now();
    while harness.query_by_label("Roll Back and Change").is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5), "no question");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.get_by_label("Cancel").click();
    harness.run();
    assert_eq!(session_db(harness.state(), id).as_deref(), Some("loja"));
    assert_eq!(harness.state().documents().get(id).unwrap().database, None);

    pick(&mut harness, "vendas");
    let start = Instant::now();
    while harness.query_by_label("Roll Back and Change").is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5), "no question");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.get_by_label("Roll Back and Change").click();
    harness.run();
    let doc = harness.state().documents().get(id).unwrap();
    assert!(doc.session.is_none());
    assert_eq!(doc.database.as_deref(), Some("vendas"));
}

#[test]
fn ctrl_n_opens_in_the_database_selected_in_object_explorer() {
    let fake = Fake::default();
    let schema = "srv/Databases/app_01/Schemas/public";
    let tables = format!("{schema}/Tables");
    let mut harness = harness(
        &fake,
        &[
            "srv",
            "srv/Databases",
            "srv/Databases/app_01",
            "srv/Databases/app_01/Schemas",
            schema,
            &tables,
        ],
    );
    let start = Instant::now();
    while harness
        .query_by_label_contains("public.table_00003")
        .is_none()
    {
        harness.step();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "tree never loaded"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.get_by_label_contains("public.table_00003").click();
    harness.run();
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    let id = harness.state().active_document().unwrap();
    let doc = harness.state().documents().get(id).unwrap();
    assert_eq!(doc.database.as_deref(), Some("app_01"));
}

/// Num PostgreSQL de verdade (com `PGSTUDIO_TEST_HOST`; sem ele, avisa e
/// passa): a lista traz os bancos sem os modelos, e a checagem vê a
/// transação aberta, a abortada e a fechada.
#[test]
fn the_list_and_the_transaction_check_work_on_postgresql() {
    use pgstudio_core::connection::SslMode;
    use pgstudio_ui_shell::databases::{DatabaseLists, TransactionProbe};

    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return;
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
    let connector =
        pgstudio_pg_metadata::PgConnector::new(pgstudio_pg_client::DbRuntime::new().unwrap());
    let connected = connector
        .connect(&settings, &var("PGSTUDIO_TEST_PASSWORD", ""))
        .unwrap();
    let ctx = eframe::egui::Context::default();

    let mut lists = DatabaseLists::default();
    lists.refresh("srv", &connected.queries, "postgres", &ctx);
    let start = Instant::now();
    while lists.is_loading("srv") {
        lists.poll();
        assert!(start.elapsed() < Duration::from_secs(30), "no list");
        std::thread::sleep(Duration::from_millis(5));
    }
    let list = lists.get("srv");
    assert!(list.iter().any(|d| d == "postgres"), "{list:?}");
    assert!(!list.iter().any(|d| d.starts_with("template")), "{list:?}");

    let session = connected.queries.open("postgres");
    let probe = |session: &Arc<dyn QuerySession>| {
        let p = TransactionProbe::start(session, &ctx);
        let start = Instant::now();
        loop {
            if let Some(open) = p.poll() {
                return open;
            }
            assert!(start.elapsed() < Duration::from_secs(30), "no answer");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let exec = |session: &Arc<dyn QuerySession>, sql: &str| {
        let (tx, rx) = std::sync::mpsc::channel();
        session.execute(
            sql.into(),
            Box::new(move |e| {
                if matches!(e, QueryEvent::Finished { .. }) {
                    let _ = tx.send(());
                }
            }),
        );
        rx.recv_timeout(Duration::from_secs(30)).unwrap();
    };
    assert!(!probe(&session));
    exec(&session, "BEGIN; SELECT 1");
    assert!(probe(&session));
    exec(&session, "SELECT 1/0");
    assert!(probe(&session), "an aborted transaction is still open");
    exec(&session, "ROLLBACK");
    assert!(!probe(&session));
}
