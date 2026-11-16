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
            if sql.contains("aclexplode") {
                // Permissões de uma tabela: app tem SELECT.
                let results: [Vec<Vec<&str>>; 3] = [
                    vec![vec!["public.table_00003", "postgres", "r"]],
                    vec![vec!["app", "SELECT", "f"]],
                    vec![vec!["app"], vec!["leitor"], vec!["postgres"]],
                ];
                for rows in results {
                    sink(QueryEvent::Columns(vec!["c".into()]));
                    for r in &rows {
                        sink(QueryEvent::Row(
                            r.iter().map(|v| Some(v.to_string())).collect(),
                        ));
                    }
                    sink(QueryEvent::CommandComplete {
                        rows: rows.len() as u64,
                    });
                }
                sink(QueryEvent::Finished { canceled: false });
                return;
            }
            // Edit Top 200 Rows: colunas (id é a chave), duas linhas e o
            // UPDATE devolvendo a linha gravada.
            let edit: Option<Vec<Vec<&str>>> = if sql.contains("attidentity") {
                let t = "public.table_00003";
                Some(vec![
                    vec!["id", "integer", "t", "f", t, t],
                    vec!["name", "text", "f", "f", t, t],
                ])
            } else if sql.starts_with("SELECT id, name FROM") {
                Some(vec![vec!["1", "a"], vec!["2", "b"]])
            } else if sql.starts_with("UPDATE ") {
                Some(vec![vec!["1", "novo"]])
            } else {
                None
            };
            if let Some(rows) = edit {
                sink(QueryEvent::Columns(vec!["c".into()]));
                for r in &rows {
                    sink(QueryEvent::Row(
                        r.iter().map(|v| Some(v.to_string())).collect(),
                    ));
                }
                sink(QueryEvent::CommandComplete {
                    rows: rows.len() as u64,
                });
                sink(QueryEvent::Finished { canceled: false });
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
            if sql.contains("EXPLAIN") && !sql.contains("bad") {
                // Um plano por comando, com o custo crescendo; com ANALYZE,
                // 5.000 linhas reais contra 10 estimadas.
                let mut i = 0;
                for part in sql.split(";\n") {
                    if !part.starts_with("EXPLAIN") {
                        sink(QueryEvent::CommandComplete { rows: 0 });
                        continue;
                    }
                    let actual = if part.contains("ANALYZE") {
                        r#", "Actual Rows": 5000, "Actual Loops": 1, "Actual Total Time": 2.5"#
                    } else {
                        ""
                    };
                    let timing = if part.contains("ANALYZE") {
                        r#", "Planning Time": 0.1, "Execution Time": 3.0"#
                    } else {
                        ""
                    };
                    let json = format!(
                        r#"[{{"Plan": {{"Node Type": "Seq Scan", "Relation Name": "t{i}", "Total Cost": {}.0, "Plan Rows": 10{actual}}}{timing}}}]"#,
                        i + 1
                    );
                    i += 1;
                    sink(QueryEvent::Columns(vec!["QUERY PLAN".into()]));
                    sink(QueryEvent::Row(vec![Some(json)]));
                    sink(QueryEvent::CommandComplete { rows: 1 });
                }
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
fn ctrl_l_shows_the_estimated_plan_of_each_statement() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "select 1;\n-- comentário\nselect 2;");
    harness.key_press_modifiers(Modifiers::CTRL, Key::L);
    wait(&mut harness, id);

    assert_eq!(
        *log.lock().unwrap(),
        vec!["EXPLAIN (FORMAT JSON) select 1;\nEXPLAIN (FORMAT JSON) select 2".to_string()]
    );
    let exec = execution(&harness, id);
    assert!(exec.is_plan());
    assert_eq!(exec.outcome(), Outcome::Succeeded);
    assert!(exec.results.is_empty());
    assert_eq!(exec.plans.len(), 2);
    assert_eq!(exec.plans[1].0.statement, "select 2");
    assert_eq!(exec.tab, Tab::Plan);
    harness.get_by_label("Query 1: Query cost (relative to the batch): 33%");
    harness.get_by_label("Query 2: Query cost (relative to the batch): 67%");
    harness.get(by().label_contains("Execution plan"));
    harness.get_by_label("Seq Scan, t0, Cost: 100 %");
}

#[test]
fn ctrl_l_errors_point_into_the_document() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "select 1;\nselect bad from t;");
    harness.key_press_modifiers(Modifiers::CTRL, Key::L);
    wait(&mut harness, id);
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Failed);
    assert_eq!(exec.tab, Tab::Messages);
    let error = exec
        .messages
        .iter()
        .find(|m| m.kind == MessageKind::Error)
        .unwrap();
    assert_eq!(error.position, Some(17));
    assert!(
        error
            .text
            .starts_with("Msg 42703, Level ERROR, Line 2, Column 8"),
        "{}",
        error.text
    );
}

#[test]
fn ctrl_l_without_statements_only_warns() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "-- nada aqui");
    harness.key_press_modifiers(Modifiers::CTRL, Key::L);
    harness.run();
    assert!(log.lock().unwrap().is_empty());
    assert!(
        harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .execution
            .is_none()
    );
    assert_eq!(
        harness.state().status(),
        "There is no statement to show a plan for."
    );
}

#[test]
fn ctrl_m_makes_f5_show_the_actual_plan() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "select 1");
    harness.key_press_modifiers(Modifiers::CTRL, Key::M);
    harness.run();
    assert!(harness.state().documents().get(id).unwrap().actual_plan);
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    assert_eq!(
        *log.lock().unwrap(),
        vec!["EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) select 1".to_string()]
    );
    let exec = execution(&harness, id);
    assert_eq!(exec.tab, Tab::Plan);
    let plan = &exec.plans[0].0;
    assert_eq!(plan.root().actual_total_rows(), Some(5000.0));
    assert_eq!(plan.misestimate(0), Some(500.0));
    harness.get(by().label_contains("Execution time: 3.000 ms"));
    harness.get_by_label("Seq Scan, t0, Cost: 100 %, Warning: estimated rows differ from actual");
    // Ctrl+M de novo desliga: o F5 volta a mostrar resultados.
    harness.key_press_modifiers(Modifiers::CTRL, Key::M);
    harness.run();
    assert!(!harness.state().documents().get(id).unwrap().actual_plan);
}

fn dml_with_actual_plan(text: &str) -> (Harness<'static, ShellApp>, Log, u32) {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, text);
    harness.key_press_modifiers(Modifiers::CTRL, Key::M);
    harness.run();
    harness.key_press(Key::F5);
    harness.run();
    assert!(log.lock().unwrap().is_empty(), "ran before asking");
    harness.get_by_label("Include Actual Execution Plan");
    (harness, log, id)
}

#[test]
fn actual_plan_of_dml_asks_and_rolls_back_by_default() {
    let (mut harness, log, id) = dml_with_actual_plan("update t set a = 1");
    harness.get_by_label("Run and Roll Back").click();
    harness.run_steps(4);
    wait(&mut harness, id);
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "BEGIN;\nEXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) update t set a = 1;\nROLLBACK"
                .to_string()
        ]
    );
    let exec = execution(&harness, id);
    assert_eq!(exec.plans.len(), 1);
    assert!(
        texts(&harness, id)
            .contains(&"The changes made by the statements were rolled back.".to_string())
    );
}

#[test]
fn actual_plan_of_dml_can_keep_the_changes_or_cancel() {
    let (mut harness, log, id) = dml_with_actual_plan("delete from t");
    harness.get_by_label("Run and Keep Changes").click();
    harness.run_steps(4);
    wait(&mut harness, id);
    assert_eq!(
        *log.lock().unwrap(),
        vec!["EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) delete from t".to_string()]
    );

    let (mut harness, log, _) = dml_with_actual_plan("delete from t");
    harness.get_by_label("Cancel").click();
    harness.run_steps(4);
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(harness.state().status(), "The query was not executed.");
}

#[test]
fn a_failed_rolled_back_batch_closes_the_transaction() {
    let (mut harness, log, id) = dml_with_actual_plan("update t set bad = 1");
    harness.get_by_label("Run and Roll Back").click();
    harness.run_steps(4);
    wait(&mut harness, id);
    let start = Instant::now();
    while log.lock().unwrap().len() < 2 {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(log.lock().unwrap()[1], "ROLLBACK");
    let error = execution(&harness, id)
        .messages
        .iter()
        .find(|m| m.kind == MessageKind::Error)
        .cloned()
        .unwrap();
    assert_eq!(error.position, Some(13));
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
    harness.run_steps(4);
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
    harness_with_folder("Tables", "public.table_00003")
}

/// Conectado, com a árvore aberta até a pasta `folder` de `app_01.public`,
/// esperando `node` aparecer.
fn harness_with_folder(folder: &str, node: &str) -> (Harness<'static, ShellApp>, Log) {
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
        .chain([format!("{schema}/{folder}")]),
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 900.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.query_by_label_contains(node).is_none() {
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
fn edit_top_on_a_view_shows_the_rows_read_only() {
    let (mut harness, log) = harness_with_folder("Views", "public.view_1");
    context_menu(&mut harness, "public.view_1", "Edit Top 200 Rows");
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

/// Roda frames até a janela Edit Rows aberta terminar de ler ou gravar.
fn wait_edit(harness: &mut Harness<'static, ShellApp>) -> u32 {
    let start = Instant::now();
    loop {
        harness.step();
        if let Some(&id) = harness.state().edit_rows_ids().last()
            && let Some(e) = harness.state().edit_rows(id)
            && !e.is_loading()
            && !e.is_saving()
        {
            harness.run();
            return id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "edit rows never loaded"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn edit_top_on_a_table_edits_the_rows_by_their_key() {
    let (mut harness, log) = harness_with_tables();
    context_menu(&mut harness, "public.table_00003", "Edit Top 200 Rows");
    let id = wait_edit(&mut harness);
    {
        let e = harness.state().edit_rows(id).unwrap();
        assert_eq!(e.title(), "app_01 - public.table_00003");
        assert_eq!(e.row_count(), 2);
        assert_eq!(e.read_only_reason(), None);
    }
    assert!(
        log.lock().unwrap()[1].starts_with("SELECT id, name FROM public.table_00003 LIMIT 200")
    );
    // Clica na célula, digita e Enter: a célula fecha, a seleção desce e
    // a linha alterada vai ao banco pela chave.
    harness.get_by_label("a").click();
    harness.run();
    harness.event(Event::Text("novo".into()));
    harness.run();
    harness.key_press(Key::Enter);
    let id = wait_edit(&mut harness);
    let update = log.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        update,
        "UPDATE public.table_00003 SET name = 'novo' WHERE id = '1' RETURNING id, name"
    );
    let e = harness.state().edit_rows(id).unwrap();
    assert_eq!(e.value(0, 1), Some("novo"));
    assert_eq!(e.selected(), Some((1, 1)));
    assert_eq!(e.message(), Some("1 row updated."));
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
    // O catálogo do IntelliSense chega por outra thread e pede repaint;
    // `wait_document` espera por ele.
    harness.step();
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

#[test]
fn alt_f1_shows_the_object_under_the_cursor() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "select * from public.pedido p");
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), (17, 17));
    harness.run();
    harness.key_press_modifiers(Modifiers::ALT, Key::F1);
    wait(&mut harness, id);
    let sent = log.lock().unwrap()[0].clone();
    assert!(sent.contains("'public.pedido'::regclass"), "{sent}");
    // O texto do documento fica como estava.
    assert_eq!(
        harness.state().documents().get(id).unwrap().text,
        "select * from public.pedido p"
    );
}

#[test]
fn alt_f1_errors_do_not_point_into_the_document() {
    let (mut harness, _) = harness_with(true);
    let id = query_with(&mut harness, "select * from bad");
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), (15, 15));
    harness.run();
    harness.key_press_modifiers(Modifiers::ALT, Key::F1);
    wait(&mut harness, id);
    let exec = execution(&harness, id);
    let error = exec
        .messages
        .iter()
        .find(|m| m.kind == MessageKind::Error)
        .unwrap();
    assert_eq!(error.position, None);
    assert!(
        error.text.starts_with("Msg 42703, Level ERROR\n"),
        "{}",
        error.text
    );
}

#[test]
fn alt_f1_without_a_name_only_warns() {
    let (mut harness, log) = harness_with(true);
    let id = query_with(&mut harness, "1 + 2");
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), (2, 2));
    harness.run();
    harness.key_press_modifiers(Modifiers::ALT, Key::F1);
    harness.run();
    assert!(log.lock().unwrap().is_empty());
    assert!(
        harness
            .state()
            .status()
            .starts_with("Select an object name")
    );
}

fn table_permissions(harness: &mut Harness<'static, ShellApp>) {
    harness
        .get_by_label_contains("public.table_00003")
        .click_secondary();
    harness.run();
    // A aba do painel Properties tem o mesmo nome: o item do menu é o
    // botão.
    harness
        .query_all_by_label("Properties")
        .next()
        .unwrap()
        .click();
    harness.step();
    let start = Instant::now();
    while harness
        .state()
        .security_dialog()
        .is_none_or(|d| d.is_loading())
    {
        harness.step();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{} {:?}",
            harness.state().status(),
            harness.state().security_dialog().map(|d| d.error())
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run_steps(3);
    harness.get_by_label("Table Properties - public.table_00003");
    harness.get_by_label("Permissions for app");
}

#[test]
fn table_permissions_are_scripted_before_they_are_applied() {
    let (mut harness, log) = harness_with_tables();
    table_permissions(&mut harness);
    harness.get_by_label("Grant INSERT").click();
    harness.get_by_label("With Grant SELECT").click();
    harness.run_steps(2);
    harness.get_by_label_contains("Script").click();
    let id = wait_document(&mut harness);
    harness.run_steps(2);
    let doc = harness.state().documents().get(id).unwrap();
    assert_eq!(
        doc.text,
        "GRANT INSERT ON TABLE public.table_00003 TO app;\n\
         GRANT SELECT ON TABLE public.table_00003 TO app WITH GRANT OPTION;\n"
    );
    assert_eq!(doc.database.as_deref(), Some("app_01"));
    // O Script não aplica nada; o OK aplica numa transação.
    assert!(!log.lock().unwrap().iter().any(|s| s.starts_with("BEGIN")));
    harness.get_by_label("OK").click();
    let start = Instant::now();
    while harness.state().security_dialog().is_some() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run_steps(2);
    assert!(
        log.lock().unwrap().contains(
            &"BEGIN;\nGRANT INSERT ON TABLE public.table_00003 TO app;\n\
          GRANT SELECT ON TABLE public.table_00003 TO app WITH GRANT OPTION;\nCOMMIT"
                .to_string()
        )
    );
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l == "  GRANT INSERT ON TABLE public.table_00003 TO app;")
    );
}

#[test]
fn permissions_without_changes_have_nothing_to_script() {
    let (mut harness, log) = harness_with_tables();
    table_permissions(&mut harness);
    harness.get_by_label_contains("Script").click();
    harness.run_steps(2);
    harness.get_by_label("There are no changes to script.");
    // O aviso acabou de aparecer e empurra os botões: espera assentar.
    harness.run_steps(2);
    harness.get_by_label("Cancel").click();
    harness.run_steps(2);
    assert!(harness.state().security_dialog().is_none());
    assert!(!log.lock().unwrap().iter().any(|s| s.starts_with("BEGIN")));
}

#[test]
fn tasks_back_up_opens_the_backup_window() {
    let (mut harness, _) = harness_with_tables();
    harness.get_by_label_contains(" app_01").click_secondary();
    harness.run();
    harness.get_by_label_contains("Tasks").hover();
    harness.run();
    harness.get_by_label("Back Up...").click();
    harness.run_steps(4);
    harness.get_by_label("Back Up Database - app_01");
    let d = harness.state().backup_dialog().unwrap();
    assert_eq!(d.options.database, "app_01");
    assert!(d.options.file.ends_with("app_01.backup"));
    // O servidor falso é 17.2: sem um pg_dump 17, a janela explica.
    match d.tool() {
        Ok(t) => assert!(t.version.0 >= 17),
        Err(e) => assert!(e.contains("pg_dump"), "{e}"),
    }
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert!(harness.state().backup_dialog().is_none());
}

#[test]
fn tasks_restore_opens_the_restore_window() {
    use pgstudio_ui_shell::restore::Target;
    let (mut harness, _) = harness_with_tables();
    harness.get_by_label_contains(" app_01").click_secondary();
    harness.run();
    harness.get_by_label_contains("Tasks").hover();
    harness.run();
    harness.get_by_label("Restore...").click();
    harness.run_steps(4);
    harness.get_by_label("Restore Database");
    let d = harness.state().restore_dialog().unwrap();
    assert_eq!(d.options.target, Target::Existing("app_01".into()));
    match d.tool() {
        Ok(t) => assert!(t.version.0 >= 17),
        Err(e) => assert!(e.contains("pg_restore"), "{e}"),
    }
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert!(harness.state().restore_dialog().is_none());

    // Na pasta Databases, o destino é um banco novo.
    harness
        .query_all_by_label_contains("Databases")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label("Restore Database...").click();
    harness.run_steps(4);
    let d = harness.state().restore_dialog().unwrap();
    assert_eq!(d.options.target, Target::New(String::new()));
}

#[test]
fn tasks_compare_schema_opens_with_the_database_as_source() {
    let (mut harness, _) = harness_with_tables();
    harness.get_by_label_contains(" app_01").click_secondary();
    harness.run();
    harness.get_by_label_contains("Tasks").hover();
    harness.run();
    harness.get_by_label("Compare Schema...").click();
    harness.run_steps(4);
    harness.get_by_label("Schema Compare");
    let d = harness.state().schema_compare().unwrap();
    assert_eq!(d.source.database, "app_01");
    harness.get_by_label("Close").click();
    harness.run_steps(3);
    assert!(harness.state().schema_compare().is_none());
}

/// Responde todo "salvar resultados" com o mesmo arquivo.
struct SaveResultsTo(std::path::PathBuf);

impl pgstudio_ui_shell::documents::FileDialogs for SaveResultsTo {
    fn save_path(&mut self, _: &str, _: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        None
    }

    fn open_path(&mut self) -> Option<std::path::PathBuf> {
        None
    }

    fn save_results_path(&mut self, _: &str) -> Option<std::path::PathBuf> {
        Some(self.0.clone())
    }
}

/// Conectado, com os resultados salvos sempre em `path`.
fn harness_saving(path: &std::path::Path) -> Harness<'static, ShellApp> {
    let log = Log::default();
    let mut app = ShellApp::default()
        .with_connector(Arc::new(FakeConnector(log)))
        .with_file_dialogs(SaveResultsTo(path.to_path_buf()));
    app.connect_to(
        ConnectionSettings {
            database: "loja".into(),
            ..Default::default()
        },
        "secret".into(),
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
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

fn temp_file(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pgstudio-{}-{name}", std::process::id()))
}

#[test]
fn results_to_file_writes_the_rows_without_keeping_them() {
    let path = temp_file("results.csv");
    let mut harness = harness_saving(&path);
    let id = query_with(&mut harness, "rows 3");
    harness.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::F);
    harness.run();
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Succeeded);
    assert!(exec.results.is_empty());
    assert_eq!(exec.file(), Some(path.as_path()));
    assert!(
        exec.messages
            .iter()
            .any(|m| m.text == format!("The results were saved to {}.", path.display())),
        "{:?}",
        exec.messages
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "n,label\n0,row 0\n1,row 1\n2,row 2\n"
    );
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn save_results_as_exports_the_grid_in_the_format_of_the_extension() {
    let path = temp_file("results.json");
    let mut harness = harness_saving(&path);
    let id = query_with(&mut harness, "rows 2");
    harness.key_press(Key::F5);
    wait(&mut harness, id);
    harness.run();
    // Botão direito numa célula da grade (desenhada, sem nó de
    // acessibilidade): pela posição.
    let pos = execution(&harness, id).results[0]
        .grid
        .cell_center(pgstudio_result_grid::Cell { row: 1, col: 1 })
        .unwrap();
    for pressed in [true, false] {
        harness.event(Event::PointerMoved(pos));
        harness.event(Event::PointerButton {
            pos,
            button: eframe::egui::PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
    harness.get_by_label("Save Results As...").click();
    let start = Instant::now();
    loop {
        harness.step();
        if !harness.state().is_exporting() && path.exists() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "never saved");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[\n  {\"n\": \"0\", \"label\": \"row 0\"},\n  {\"n\": \"1\", \"label\": \"row 1\"}\n]\n"
    );
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l == &format!("Saved 2 rows to {}.", path.display()))
    );
    std::fs::remove_file(&path).unwrap();
}
