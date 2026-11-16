//! Histórico de consultas executadas: cada F5 vira uma entrada com
//! servidor, banco, duração e resultado; o arquivo sobrevive a fechar o
//! app; a janela pesquisa e reabre a consulta.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::history::{HistoryStore, Status};
use pgstudio_ui_shell::panels::Panel;
use pgstudio_ui_shell::session::SessionStore;

struct FakeQueries;

impl QueryService for FakeQueries {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession {
            database: database.to_string(),
            cancel: Arc::default(),
        })
    }
}

/// `sleep` espera o cancelamento; `bad` vira erro; o resto devolve três
/// linhas.
struct FakeSession {
    database: String,
    cancel: Arc<AtomicBool>,
}

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let cancel = self.cancel.clone();
        cancel.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            if sql.contains("sleep") {
                while !cancel.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                sink(QueryEvent::Finished { canceled: true });
                return;
            }
            if sql.contains("bad") {
                sink(QueryEvent::Error(ServerMessage {
                    severity: "ERROR".into(),
                    code: "42703".into(),
                    message: "column \"bad\" does not exist".into(),
                    ..Default::default()
                }));
            } else {
                sink(QueryEvent::Columns(vec!["n".into()]));
                for i in 0..3 {
                    sink(QueryEvent::Row(vec![Some(i.to_string())]));
                }
                sink(QueryEvent::CommandComplete { rows: 3 });
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn database(&self) -> String {
        self.database.clone()
    }
}

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries),
        })
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "postpilot-history-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn settings() -> ConnectionSettings {
    ConnectionSettings {
        host: "db.cliente-a.com.br".into(),
        user: "ana".into(),
        database: "loja".into(),
        ..Default::default()
    }
}

fn harness(app: ShellApp, connect: bool) -> Harness<'static, ShellApp> {
    let mut app = app.with_connector(Arc::new(FakeConnector));
    if connect {
        app.connect_to(settings(), "muito-secreta".into());
    }
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while connect && harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    harness
}

fn query_with(harness: &mut Harness<'static, ShellApp>, text: &str) -> u32 {
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    harness.event(Event::Text(text.to_string()));
    harness.run();
    harness.state().active_document().unwrap()
}

fn wait(harness: &mut Harness<'static, ShellApp>, id: u32) {
    let start = Instant::now();
    loop {
        harness.step();
        let running = harness
            .state()
            .documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| e.is_running());
        if !running && !harness.state().is_loading_catalog() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "query never finished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
}

fn run(harness: &mut Harness<'static, ShellApp>, text: &str) -> u32 {
    let id = query_with(harness, text);
    harness.key_press(Key::F5);
    wait(harness, id);
    id
}

#[test]
fn every_execution_is_recorded_with_server_database_and_outcome() {
    let mut h = harness(ShellApp::default(), true);
    run(&mut h, "select * from clientes");
    run(&mut h, "select bad from clientes");
    let id = query_with(&mut h, "select sleep");
    h.key_press(Key::F5);
    h.run_steps(3);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::CancelQuery);
    wait(&mut h, id);

    let entries = h.state().history().store.entries();
    assert_eq!(entries.len(), 3);
    let ok = &entries[0];
    assert_eq!(ok.sql, "select * from clientes");
    assert_eq!(ok.server, "ana@db.cliente-a.com.br:5432");
    assert!(ok.server_name.contains("db.cliente-a.com.br"), "{ok:?}");
    assert_eq!(ok.database, "loja");
    assert_eq!(ok.status, Status::Succeeded);
    assert_eq!(ok.rows, 3);
    assert!(ok.when > 1_700_000_000);
    assert_eq!(ok.window, "SQLQuery1.sql");
    assert_eq!(entries[1].status, Status::Failed);
    assert_eq!(
        entries[1].error.as_deref(),
        Some("column \"bad\" does not exist")
    );
    assert_eq!(entries[2].status, Status::Canceled);
}

#[test]
fn estimated_plans_and_object_information_stay_out() {
    let mut h = harness(ShellApp::default(), true);
    let id = query_with(&mut h, "select 1 from clientes");
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::EstimatedPlan);
    wait(&mut h, id);
    h.state_mut().execute(&ctx, CommandId::ObjectInfo);
    wait(&mut h, id);
    assert!(h.state().history().store.is_empty());
    // Só a seleção executada entra.
    h.key_press(Key::F5);
    wait(&mut h, id);
    assert_eq!(h.state().history().store.len(), 1);
}

#[test]
fn the_history_survives_closing_the_app_without_the_password() {
    let dir = temp_dir("restart");
    let file = dir.join("history.jsonl");
    {
        let app = ShellApp::default().with_history(HistoryStore::open(&file));
        let mut h = harness(app, true);
        run(&mut h, "alter role ana password 'outra-senha'");
        run(&mut h, "select * from pedidos");
    }
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains("muito-secreta"), "{text}");
    assert!(!text.contains("outra-senha"), "{text}");

    // O app abre de novo, ainda sem conectar.
    let app = ShellApp::default().with_history(HistoryStore::open(&file));
    let mut h = harness(app, false);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::QueryHistory);
    h.run();
    h.get_by_label("2 executions");
    h.get_by_label("select * from pedidos");
    h.get_by_label("alter role ana password '***'");
}

#[test]
fn the_window_searches_and_reopens_a_query() {
    let mut h = harness(ShellApp::default(), true);
    run(&mut h, "select * from clientes\nwhere cidade = 'São Paulo'");
    run(&mut h, "update pedidos set status = 'faturado'");
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::QueryHistory);
    h.run();
    h.get_by_label("select * from clientes where cidade = 'São Paulo'");
    h.get_by_label("update pedidos set status = 'faturado'");

    // A pesquisa olha dentro do texto, sem ligar para acento.
    h.state_mut().history_mut().filter = "sao paulo".into();
    h.run();
    h.get_by_label("1 of 2 executions");
    assert!(
        h.query_by_label("update pedidos set status = 'faturado'")
            .is_none()
    );

    // Um clique seleciona e mostra o detalhe; o botão reabre a consulta.
    h.get_by_label("select * from clientes where cidade = 'São Paulo'")
        .click();
    h.run();
    let before = h.state().documents().ids().len();
    h.get(by().label_contains("Open in New Query Window"))
        .click();
    h.run();
    let ids = h.state().documents().ids();
    assert_eq!(ids.len(), before + 1);
    let id = h.state().active_document().unwrap();
    let doc = h.state().documents().get(id).unwrap();
    assert_eq!(
        doc.text,
        "select * from clientes\nwhere cidade = 'São Paulo'"
    );
    assert_eq!(doc.database.as_deref(), Some("loja"));
    assert!(!ids.is_empty());
}

#[test]
fn the_window_and_its_search_come_back_with_the_session() {
    let dir = temp_dir("session");
    {
        let app = ShellApp::default().with_session(SessionStore::open(dir.join("session.json")));
        let mut h = harness(app, false);
        let ctx = h.ctx.clone();
        h.state_mut().execute(&ctx, CommandId::QueryHistory);
        h.state_mut().history_mut().filter = "pedidos".into();
        h.run();
        h.state_mut().save_session();
    }
    let app = ShellApp::default().with_session(SessionStore::open(dir.join("session.json")));
    let h = harness(app, false);
    assert_eq!(h.state().history().filter, "pedidos");
    assert!(h.state().is_panel_open(Panel::QueryHistory));
}
