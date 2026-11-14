//! Execução de consultas na janela de consulta, com uma sessão falsa (sem
//! banco): F5 com e sem seleção, erros que levam o cursor à posição,
//! cancelamento e o painel Results/Messages.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::execution::{MessageKind, Outcome, Tab};
use pgstudio_ui_shell::panels::editor_id;

/// O SQL recebido por cada execução, para conferir o que foi enviado.
type Log = Arc<Mutex<Vec<String>>>;

struct FakeQueries(Log);

impl QueryService for FakeQueries {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession {
            log: self.0.clone(),
            database: database.to_string(),
            cancel: Arc::default(),
        })
    }
}

/// `sleep` espera o cancelamento; `bad` vira erro na posição da palavra;
/// `notice` manda um aviso; o resto devolve uma linha com o próprio SQL.
struct FakeSession {
    log: Log,
    database: String,
    cancel: Arc<AtomicBool>,
}

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        self.log.lock().unwrap().push(sql.clone());
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
            if sql.contains("notice") {
                sink(QueryEvent::Notice(ServerMessage {
                    severity: "NOTICE".into(),
                    message: "hello".into(),
                    ..Default::default()
                }));
            }
            if let Some(n) = sql
                .strip_prefix("rows ")
                .and_then(|n| n.trim().parse::<u64>().ok())
            {
                sink(QueryEvent::Columns(vec!["n".into(), "label".into()]));
                for i in 0..n {
                    sink(QueryEvent::Row(vec![
                        Some(i.to_string()),
                        Some(format!("row {i}")),
                    ]));
                }
                sink(QueryEvent::CommandComplete { rows: n });
                sink(QueryEvent::Finished { canceled: false });
                return;
            }
            if let Some(b) = sql.find("bad") {
                sink(QueryEvent::Error(ServerMessage {
                    severity: "ERROR".into(),
                    code: "42703".into(),
                    message: "column \"bad\" does not exist".into(),
                    position: Some(sql[..b].chars().count() + 1),
                    ..Default::default()
                }));
            } else {
                sink(QueryEvent::Columns(vec!["sql".into(), "nothing".into()]));
                sink(QueryEvent::Row(vec![Some(sql.clone()), None]));
                sink(QueryEvent::CommandComplete { rows: 1 });
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

fn harness_with(connect: bool) -> (Harness<'static, ShellApp>, Log) {
    let log = Log::default();
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(log.clone())));
    if connect {
        app.connect_to(
            ConnectionSettings {
                database: "loja".into(),
                ..Default::default()
            },
            "secret".into(),
        );
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
    (harness, log)
}

fn query_with(harness: &mut Harness<'static, ShellApp>, text: &str) -> u32 {
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    harness.event(Event::Text(text.to_string()));
    harness.run();
    harness.state().active_document().unwrap()
}

/// Roda frames até a execução da janela `id` terminar.
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
        // O catálogo do IntelliSense chega por outra thread e pede um frame
        // novo; espera ele também para o `run` abaixo não pegar o meio.
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

fn execution<'a>(
    harness: &'a Harness<'static, ShellApp>,
    id: u32,
) -> &'a pgstudio_ui_shell::execution::Execution {
    harness
        .state()
        .documents()
        .get(id)
        .unwrap()
        .execution
        .as_ref()
        .unwrap()
}

fn texts(harness: &Harness<'static, ShellApp>, id: u32) -> Vec<String> {
    execution(harness, id)
        .messages
        .iter()
        .map(|m| m.text.clone())
        .collect()
}

#[test]
fn f5_runs_the_whole_query_and_shows_results() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "select 1;\nselect 2;");
    harness.key_press(Key::F5);
    wait(&mut harness, id);

    assert_eq!(
        *log.lock().unwrap(),
        vec!["select 1;\nselect 2;".to_string()]
    );
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Succeeded);
    assert_eq!(exec.database, "loja");
    assert_eq!(exec.results.len(), 1);
    assert_eq!(exec.results[0].columns(), ["sql", "nothing"]);
    assert_eq!(exec.results[0].len(), 1);
    assert_eq!(exec.tab, Tab::Results);
    assert_eq!(harness.state().status(), "Query executed successfully.");
    harness.get(by().label_contains("Results"));
    // Na status bar e na faixa da janela de consulta.
    assert_eq!(
        harness
            .get_all_by_label("Query executed successfully.")
            .count(),
        2
    );
    harness.get_by_label("1 row");
    harness.get_by_label("loja");

    harness.get(by().label_contains("Messages")).click();
    harness.run();
    let t = texts(&harness, id);
    assert_eq!(t[0], "(1 row affected)");
    assert!(
        t.last()
            .unwrap()
            .starts_with("Total execution time: 00:00:00."),
        "{t:?}"
    );
    harness.get_by_label("(1 row affected)");
}

#[test]
fn ctrl_e_runs_only_the_selection() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "select 1;\nselect 2;");
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), (10, 19));
    harness.run();
    harness.key_press_modifiers(Modifiers::CTRL, Key::E);
    wait(&mut harness, id);
    assert_eq!(*log.lock().unwrap(), vec!["select 2;".to_string()]);
}

#[test]
fn an_error_moves_the_cursor_to_its_position() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "select 1;\nselect bad from t;");
    harness.key_press(Key::F5);
    wait(&mut harness, id);

    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Failed);
    assert_eq!(exec.tab, Tab::Messages);
    let error = exec
        .messages
        .iter()
        .find(|m| m.kind == MessageKind::Error)
        .unwrap();
    assert_eq!(
        error.text,
        "Msg 42703, Level ERROR, Line 2, Column 8\ncolumn \"bad\" does not exist"
    );
    assert_eq!(error.position, Some(17));
    assert_eq!(harness.state().status(), "Query completed with errors.");
    assert_eq!(
        pgstudio_sql_editor::selection(&harness.ctx, editor_id(id)),
        Some((17, 17))
    );
    harness.get(by().label("Ln 2   Col 8"));
    // O trecho do erro fica sublinhado até o texto mudar.
    let doc = harness.state().documents().get(id).unwrap();
    let text = doc.text.clone();
    let errors = doc.editor.errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(&text[errors[0].0.clone()], "bad");
    assert_eq!(errors[0].1, "column \"bad\" does not exist");
    harness.event(Event::Text("x".into()));
    harness.run();
    let doc = harness.state().documents().get(id).unwrap();
    assert!(doc.editor.errors().is_empty());
}

#[test]
fn errors_in_a_selection_point_into_the_document() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "select 1;\nselect bad from t;");
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), (10, 28));
    harness.run();
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    // "bad" está na coluna 8 da seleção, que começa na linha 2 do documento.
    let error = &execution(&harness, id).messages[0];
    assert!(
        error
            .text
            .starts_with("Msg 42703, Level ERROR, Line 1, Column 8"),
        "{}",
        error.text
    );
    assert_eq!(error.position, Some(17));

    // Clicar no erro leva o cursor de novo até ele.
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), (0, 0));
    harness.run();
    harness.get(by().label_contains("does not exist")).click();
    harness.run();
    harness.run();
    assert_eq!(
        pgstudio_sql_editor::selection(&harness.ctx, editor_id(id)),
        Some((17, 17))
    );
}

#[test]
fn cancel_stops_a_running_query() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "select pg_sleep(60)");
    // Enquanto roda, o relógio pede frames: `step` em vez de `run`.
    harness.key_press(Key::F5);
    harness.step();
    harness.step();
    assert!(execution(&harness, id).is_running());
    assert!(harness.get_all_by_label("Executing query...").count() >= 1);

    // F5 de novo enquanto roda só avisa.
    harness.key_press(Key::F5);
    harness.step();
    assert_eq!(
        harness.state().status(),
        "A query is already executing in this window."
    );

    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::CancelQuery);
    wait(&mut harness, id);
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Canceled);
    assert!(texts(&harness, id).contains(&"Query was cancelled by user.".to_string()));
    assert_eq!(harness.state().status(), "Query was canceled.");

    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::CancelQuery);
    assert_eq!(harness.state().status(), "No query is executing.");
}

#[test]
fn notices_and_commands_without_rows_go_to_messages() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "notice bad");
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    let t = texts(&harness, id);
    assert_eq!(t[0], "NOTICE: hello");
    assert_eq!(
        execution(&harness, id).messages[0].kind,
        MessageKind::Notice
    );
}

#[test]
fn executing_without_a_connection_opens_the_connect_dialog() {
    let (mut harness, log) = harness_with(false);
    harness.state_mut().connect_dialog_mut();
    // Fecha o diálogo de abertura para ver que o F5 o abre de novo.
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.state().connect_dialog().is_none());
    let id = query_with(&mut harness, "select 1");
    harness.key_press(Key::F5);
    harness.run();
    assert_eq!(
        harness.state().status(),
        "Connect to a server to run queries."
    );
    assert!(harness.state().connect_dialog().is_some());
    assert!(
        harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .execution
            .is_none()
    );
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn ctrl_t_shows_the_next_results_as_text_and_ctrl_d_goes_back() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "select 1");
    harness.key_press_modifiers(Modifiers::CTRL, Key::T);
    harness.run();
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    let exec = execution(&harness, id);
    assert_eq!(exec.mode, pgstudio_ui_shell::execution::ResultsMode::Text);
    harness.get_by_label("sql      nothing");
    harness.get_by_label("-------- -------");
    harness.get_by_label("select 1 NULL");
    harness.get_by_label("(1 row affected)");

    harness.key_press_modifiers(Modifiers::CTRL, Key::D);
    harness.run();
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    assert_eq!(
        execution(&harness, id).mode,
        pgstudio_ui_shell::execution::ResultsMode::Grid
    );
}

#[test]
fn a_large_result_arrives_over_several_frames_without_blocking() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "rows 300000");
    harness.key_press(Key::F5);
    let start = Instant::now();
    let mut frames = 0;
    let mut worst = Duration::ZERO;
    loop {
        let t = Instant::now();
        harness.step();
        worst = worst.max(t.elapsed());
        frames += 1;
        let exec = execution(&harness, id);
        if !exec.is_running() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "never finished");
    }
    let exec = execution(&harness, id);
    assert_eq!(exec.results[0].len(), 300_000);
    assert_eq!(exec.total_rows(), 300_000);
    eprintln!("300 mil linhas em {frames} frames, pior frame {worst:?}");
    assert!(worst < Duration::from_millis(500), "{worst:?}");
    assert!(frames > 1, "rows should be spread over frames");
    let doc = harness.state_mut().documents_mut().get_mut(id).unwrap();
    let set = &mut doc.execution.as_mut().unwrap().results[0];
    assert_eq!(
        set.row(299_999),
        vec![Some("299999".into()), Some("row 299999".into())]
    );
}

/// Conectado, com a árvore aberta até as tabelas de `app_01.public`.
fn harness_with_tables() -> (Harness<'static, ShellApp>, Log) {
    let log = Log::default();
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(log.clone())));
    app.connect_to(
        ConnectionSettings {
            database: "loja".into(),
            ..Default::default()
        },
        "secret".into(),
    );
    let schema = "srv/Databases/app_01/Schemas/public";
    let app = app.with_expanded(
        [
            "srv",
            "srv/Databases",
            "srv/Databases/app_01",
            "srv/Databases/app_01/Schemas",
            schema,
        ]
        .into_iter()
        .map(String::from)
        .chain([format!("{schema}/Tables")]),
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 900.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
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
    harness.run();
    (harness, log)
}

/// Roda frames até uma janela de consulta nova abrir e o catálogo dela
/// chegar.
fn wait_document(harness: &mut Harness<'static, ShellApp>) -> u32 {
    let start = Instant::now();
    loop {
        harness.step();
        if let Some(id) = harness.state().active_document()
            && harness.state().catalog_for(id).is_some()
            && !harness.state().is_loading_catalog()
        {
            return id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no query window opened"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn context_menu(harness: &mut Harness<'static, ShellApp>, node: &str, item: &str) {
    harness.get_by_label_contains(node).click_secondary();
    harness.run();
    harness.get_by_label(item).click();
    // A janela nova pode já estar executando (e redesenhando).
    harness.step();
}

#[test]
fn select_top_runs_in_the_database_of_the_table() {
    let (mut harness, log) = harness_with_tables();
    context_menu(&mut harness, "public.table_00003", "Select Top 1000 Rows");
    let id = wait_document(&mut harness);
    wait(&mut harness, id);
    let doc = harness.state().documents().get(id).unwrap();
    assert!(
        doc.text.starts_with("SELECT id,\n       name,"),
        "{}",
        doc.text
    );
    assert!(
        doc.text
            .ends_with("FROM public.table_00003\n LIMIT 1000;\n")
    );
    assert_eq!(doc.session.as_ref().unwrap().0.database(), "app_01");
    assert_eq!(log.lock().unwrap().last(), Some(&doc.text));
    assert_eq!(execution(&harness, id).outcome(), Outcome::Succeeded);
}

#[test]
fn edit_top_shows_two_hundred_rows_read_only() {
    let (mut harness, log) = harness_with_tables();
    context_menu(&mut harness, "public.table_00003", "Edit Top 200 Rows");
    let id = wait_document(&mut harness);
    wait(&mut harness, id);
    assert!(log.lock().unwrap()[0].ends_with(" LIMIT 200;\n"));
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l.contains("read-only"))
    );
}

#[test]
fn script_as_create_opens_a_query_without_running_it() {
    let (mut harness, log) = harness_with_tables();
    harness
        .get_by_label_contains("public.table_00003")
        .click_secondary();
    harness.run();
    harness.get_by_label_contains("Script Table as").hover();
    harness.run();
    harness.get_by_label("CREATE To New Query Window").click();
    harness.run();
    let id = wait_document(&mut harness);
    harness.run();
    let doc = harness.state().documents().get(id).unwrap();
    assert!(
        doc.text.starts_with("CREATE TABLE public.table_00003 (\n"),
        "{}",
        doc.text
    );
    assert!(doc.execution.is_none());
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(doc.database.as_deref(), Some("app_01"));
}

#[test]
fn new_query_on_a_database_connects_to_that_database() {
    let (mut harness, _log) = harness_with_tables();
    context_menu(&mut harness, " app_01", "New Query");
    let id = wait_document(&mut harness);
    harness.run();
    harness.event(Event::Text("select 1".into()));
    harness.run();
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    let doc = harness.state().documents().get(id).unwrap();
    assert_eq!(doc.session.as_ref().unwrap().0.database(), "app_01");
}
