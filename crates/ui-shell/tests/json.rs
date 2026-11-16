//! JSON Viewer e JSON Query Guide.
//!
//! Com uma sessão falsa: o duplo clique numa célula JSON abre o viewer, o
//! clique num nó monta o SQL e o Insert põe o SQL na janela de consulta.
//! Com `PGSTUDIO_TEST_HOST`: todos os exemplos do guia e todas as
//! expressões que o viewer monta rodam num PostgreSQL de verdade, e os
//! filtros acham a linha de onde o valor veio.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_result_grid::Cell;
use pgstudio_result_grid::json::{self, Node, Step};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::json_guide::{SETUP, TOPICS};
use pgstudio_ui_shell::json_viewer::{JsonKind, expressions};

const DOC: &str = r#"{"customer": {"name": "Ana", "city": "São Paulo"}, "items": [{"sku": "A-1", "qty": 2}], "tags": ["vip"]}"#;

struct FakeQueries;

impl QueryService for FakeQueries {
    fn open(&self, _: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession)
    }
}

/// Toda consulta devolve `id`, `data` (JSON) e `tags` (array do
/// PostgreSQL, que não é JSON).
struct FakeSession;

impl QuerySession for FakeSession {
    fn execute(&self, _: String, mut sink: QuerySink) {
        std::thread::spawn(move || {
            sink(QueryEvent::Columns(vec![
                "id".into(),
                "data".into(),
                "tags".into(),
            ]));
            for i in 0..3 {
                sink(QueryEvent::Row(vec![
                    Some(i.to_string()),
                    Some(DOC.to_string()),
                    Some("{a,b}".into()),
                ]));
            }
            sink(QueryEvent::CommandComplete { rows: 3 });
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        "loja".into()
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

fn harness() -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector));
    app.connect_to(
        ConnectionSettings {
            database: "loja".into(),
            ..Default::default()
        },
        "secret".into(),
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        // Frames de 60 Hz: com o padrão (0,25 s) dois cliques seguidos
        // passam do tempo de um duplo clique.
        .with_step_dt(1.0 / 60.0)
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

/// Abre uma janela, executa e espera os resultados.
fn run_query(harness: &mut Harness<'static, ShellApp>) -> u32 {
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    harness.event(Event::Text("select * from orders".into()));
    harness.run();
    let id = harness.state().active_document().unwrap();
    harness.key_press(Key::F5);
    let start = Instant::now();
    loop {
        harness.step();
        let done = harness
            .state()
            .documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running() && e.results.first().is_some_and(|r| r.len() == 3));
        if done && !harness.state().is_loading_catalog() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "query never finished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    id
}

fn cell_pos(harness: &Harness<'static, ShellApp>, doc: u32, cell: Cell) -> eframe::egui::Pos2 {
    harness
        .state()
        .documents()
        .get(doc)
        .unwrap()
        .execution
        .as_ref()
        .unwrap()
        .results[0]
        .grid
        .cell_center(cell)
        .expect("visible cell")
}

/// `count` cliques de `button` em `pos`, no mesmo frame (como o teste
/// do editor), depois de esperar o bastante para não emendar com o
/// clique anterior.
fn click(
    harness: &mut Harness<'static, ShellApp>,
    pos: eframe::egui::Pos2,
    button: eframe::egui::PointerButton,
    count: usize,
) {
    harness.run_steps(40);
    harness.event(Event::PointerMoved(pos));
    harness.step();
    for _ in 0..count {
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
    harness.run();
}

fn double_click(harness: &mut Harness<'static, ShellApp>, pos: eframe::egui::Pos2) {
    click(harness, pos, eframe::egui::PointerButton::Primary, 2);
}

#[test]
fn json_columns_are_recognized_but_postgres_arrays_are_not() {
    let mut harness = harness();
    let doc = run_query(&mut harness);
    let exec = harness
        .state()
        .documents()
        .get(doc)
        .unwrap()
        .execution
        .as_ref()
        .unwrap();
    let g = &exec.results[0].grid;
    assert_eq!(
        [g.is_json(0), g.is_json(1), g.is_json(2)],
        [false, true, false]
    );
}

#[test]
fn double_click_opens_the_viewer_and_a_node_builds_its_query() {
    let mut harness = harness();
    let doc = run_query(&mut harness);
    let pos = cell_pos(&harness, doc, Cell { row: 1, col: 1 });
    double_click(&mut harness, pos);
    let viewer = harness.state().json_viewer().expect("viewer open");
    assert_eq!(viewer.title(), "JSON Viewer - data (row 2)");
    harness.get(by().label_contains("Query this value"));

    // Clique no nó "city" da árvore.
    harness
        .get(by().label_contains("\"city\": \"São Paulo\""))
        .click();
    harness.run();
    let viewer = harness.state().json_viewer().unwrap();
    assert_eq!(
        viewer.selected(),
        [Step::Key("customer".into()), Step::Key("city".into())]
    );
    let first = viewer.expressions()[0].sql.clone();
    assert_eq!(first, "data -> 'customer' ->> 'city'");

    // Duplo clique fora das colunas JSON não abre nada.
    harness.get_by_label("Close window").click();
    harness.run();
    let pos = cell_pos(&harness, doc, Cell { row: 1, col: 2 });
    double_click(&mut harness, pos);
    assert!(harness.state().json_viewer().is_none());
    let pos = cell_pos(&harness, doc, Cell { row: 1, col: 1 });
    double_click(&mut harness, pos);
    assert!(harness.state().json_viewer().is_some());
    harness
        .get(by().label_contains("\"city\": \"São Paulo\""))
        .click();
    harness.run();

    // Insert põe o SQL na janela de consulta.
    harness.get_all_by_label("Insert").next().unwrap().click();
    harness.run();
    let text = &harness.state().documents().get(doc).unwrap().text;
    assert_eq!(text, &format!("select * from orders\n{first}"));
}

#[test]
fn enter_and_context_menu_open_the_viewer_too() {
    let mut harness = harness();
    let doc = run_query(&mut harness);
    let pos = cell_pos(&harness, doc, Cell { row: 0, col: 1 });
    click(&mut harness, pos, eframe::egui::PointerButton::Primary, 1);
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    assert!(harness.state().json_viewer().is_some());
    harness.get(by().label_contains("Tree")).click();
    harness.get_by_label("Close window").click();
    harness.run();
    assert!(harness.state().json_viewer().is_none());

    let pos = cell_pos(&harness, doc, Cell { row: 2, col: 1 });
    click(&mut harness, pos, eframe::egui::PointerButton::Secondary, 1);
    harness.run();
    harness.get_by_label("View JSON...").click();
    harness.run();
    assert_eq!(
        harness.state().json_viewer().unwrap().title(),
        "JSON Viewer - data (row 3)"
    );
}

#[test]
fn the_guide_opens_from_the_help_menu_and_inserts_examples() {
    let mut harness = harness();
    let doc = run_query(&mut harness);
    harness.get_by_label("Help").click();
    harness.run();
    harness.get_by_label("JSON Query Guide").click();
    harness.run();
    harness
        .get_all_by_label("Start here")
        .next()
        .unwrap()
        .click();
    harness.run();
    harness
        .get(
            by().role(eframe::egui::accesskit::Role::Button)
                .label_contains("Insert"),
        )
        .click();
    harness.run();
    let text = &harness.state().documents().get(doc).unwrap().text;
    assert!(text.ends_with(SETUP), "{text}");
}

// --- PostgreSQL de verdade ---

fn session() -> Option<(Arc<dyn QuerySession>, u32)> {
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
        ssh: None,
    };
    let service = pgstudio_pg_client::PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    );
    let session = service.open("postgres");
    let version = rows(session.as_ref(), "SHOW server_version_num")[0][0]
        .clone()
        .unwrap()
        .parse::<u32>()
        .unwrap()
        / 10000;
    Some((session, version))
}

/// Roda `sql`; falha no erro. Devolve as linhas do último resultado.
fn rows(session: &dyn QuerySession, sql: &str) -> Vec<Vec<Option<String>>> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.to_string(), Box::new(move |e| drop(tx.send(e))));
    let mut out = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).expect("Finished") {
            QueryEvent::Columns(_) => out.clear(),
            QueryEvent::Row(r) => out.push(r),
            QueryEvent::Error(e) => panic!("{sql}\n-> {}", e.message),
            QueryEvent::Finished { .. } => return out,
            _ => {}
        }
    }
}

#[test]
fn every_guide_example_runs_on_postgres() {
    let Some((session, version)) = session() else {
        return;
    };
    let mut ran = 0;
    for topic in TOPICS {
        for e in topic.examples {
            if e.since.is_some_and(|v| version < v) {
                continue;
            }
            if e.sql != SETUP {
                rows(session.as_ref(), &format!("BEGIN; {}; ROLLBACK", e.sql));
            } else {
                rows(session.as_ref(), "DROP TABLE IF EXISTS orders_demo");
                rows(session.as_ref(), SETUP);
            }
            ran += 1;
        }
    }
    assert!(ran > 30, "{ran}");
    // Os resultados que o guia promete.
    let ids = |sql: &str| -> Vec<String> {
        rows(session.as_ref(), sql)
            .into_iter()
            .map(|r| r[0].clone().unwrap())
            .collect()
    };
    let find = |title: &str| {
        TOPICS
            .iter()
            .flat_map(|t| t.examples)
            .find(|e| e.title.starts_with(title))
            .unwrap()
            .sql
    };
    assert_eq!(ids(find("Orders that are paid")), ["1", "3"]);
    assert_eq!(ids(find("Any item with sku A-1")), ["1", "3"]);
    assert_eq!(ids(find("Has a phone key")), ["3"]);
    assert_eq!(ids(find("Orders with an item of qty 4")), ["3"]);
    assert_eq!(ids(find("Variables instead of literals")), ["1"]);
}

/// Todos os caminhos de `node`, com o nó de cada um.
fn walk<'a>(node: &'a Node, path: &mut Vec<Step>, out: &mut Vec<(Vec<Step>, &'a Node)>) {
    out.push((path.clone(), node));
    match node {
        Node::Object(pairs) => {
            for (k, v) in pairs {
                path.push(Step::Key(k.clone()));
                walk(v, path, out);
                path.pop();
            }
        }
        Node::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                path.push(Step::Index(i));
                walk(v, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

#[test]
fn every_viewer_expression_runs_and_finds_its_row() {
    let Some((session, _)) = session() else {
        return;
    };
    let s = session.as_ref();
    rows(s, "DROP TABLE IF EXISTS orders_demo, \"Orders Json\"");
    rows(s, SETUP);
    // Uma coluna json com nome que precisa de aspas.
    rows(
        s,
        "CREATE TEMP TABLE \"Orders Json\" AS SELECT id, data::json AS \"Order Data\" FROM orders_demo",
    );
    let mut checked = 0;
    for (table, column, kind) in [
        ("orders_demo", "data", JsonKind::Jsonb),
        ("\"Orders Json\"", "Order Data", JsonKind::Json),
    ] {
        let docs = rows(
            s,
            &format!(
                "SELECT id, {} FROM {table} ORDER BY id",
                pgstudio_core::catalog::quote_ident(column)
            ),
        );
        for doc in docs {
            let id = doc[0].clone().unwrap();
            let root = json::parse(doc[1].as_deref().unwrap()).unwrap();
            let mut nodes = Vec::new();
            walk(&root, &mut Vec::new(), &mut nodes);
            for (path, node) in nodes {
                for e in expressions(column, &path, node, kind) {
                    let sql = e.sql.replace("table_name", table);
                    if sql.starts_with("SELECT") {
                        let found = rows(s, &sql);
                        if sql.contains("\nWHERE ") {
                            // Os filtros acham a linha de onde o valor veio.
                            let with_id = sql.replacen("SELECT *", "SELECT id", 1);
                            let ids: Vec<_> = rows(s, &with_id)
                                .into_iter()
                                .map(|r| r[0].clone().unwrap())
                                .collect();
                            assert!(
                                ids.contains(&id),
                                "{} did not find row {id}:\n{sql}",
                                e.label
                            );
                        } else {
                            assert!(!found.is_empty() || node_is_empty(node), "{sql}");
                        }
                    } else {
                        rows(
                            s,
                            &format!("SELECT {sql} FROM {table} AS t WHERE id = {id}"),
                        );
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100, "{checked}");
}

fn node_is_empty(node: &Node) -> bool {
    matches!(node, Node::Array(a) if a.is_empty())
        || matches!(node, Node::Object(o) if o.is_empty())
}
