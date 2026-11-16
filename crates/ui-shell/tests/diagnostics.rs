//! Consultas de diagnóstico prontas: um clique abre uma janela de consulta
//! e já traz o resultado. Com um servidor de verdade (`PGSTUDIO_TEST_HOST`
//! para o PostgreSQL, `MSSQL_TEST_HOST` para o SQL Server), roda todas e
//! confere a árvore de quem bloqueia quem; sem eles, esses testes avisam e
//! passam.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::panels::Panel;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::secrets::MemoryStore;
use pgstudio_ui_shell::templates::builtin::{self, DIAGNOSTICS};

fn settings(engine: Engine, database: &str) -> Option<(ConnectionSettings, String)> {
    let prefix = match engine {
        Engine::PostgreSql => "PGSTUDIO_TEST",
        _ => "MSSQL_TEST",
    };
    let Ok(host) = std::env::var(format!("{prefix}_HOST")) else {
        eprintln!("{prefix}_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| {
        std::env::var(format!("{prefix}_{name}")).unwrap_or_else(|_| default.into())
    };
    let mut s = ConnectionSettings::for_engine(engine);
    s.host = host;
    s.database = database.into();
    if engine == Engine::PostgreSql {
        s.port = var("PORT", "5432").parse().unwrap();
        s.user = var("USER", "postgres");
        s.ssl = SslMode::Disable;
    } else {
        s.port = var("PORT", "1433").parse().unwrap();
        s.user = var("USER", "sa");
        s.ssl = SslMode::Require;
    }
    Some((s, var("PASSWORD", "")))
}

fn connector(engine: Engine) -> Arc<dyn Connector> {
    let runtime = pgstudio_pg_client::DbRuntime::new().unwrap();
    match engine {
        Engine::PostgreSql => Arc::new(pgstudio_pg_metadata::PgConnector::new(runtime)),
        _ => Arc::new(pgstudio_mssql::MssqlConnector::new(runtime)),
    }
}

fn start(session: &Arc<dyn QuerySession>, sql: &str) -> std::sync::mpsc::Receiver<QueryEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    rx
}

fn run(session: &Arc<dyn QuerySession>, sql: &str) {
    let rx = start(session, sql);
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

fn app() -> ShellApp {
    ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
}

fn connected(settings: ConnectionSettings, password: String) -> Harness<'static, ShellApp> {
    let mut app = app().with_connector(connector(settings.engine));
    app.connect_to(settings, password);
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(20), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
    harness
}

fn index(engine: Engine, name: &str) -> usize {
    builtin::ALL
        .iter()
        .position(|t| t.engine == engine && t.name == name)
        .unwrap_or_else(|| panic!("{name}"))
}

/// Roda o modelo pronto como o clique faz e espera o fim. Devolve a
/// janela de consulta.
fn run_ready(h: &mut Harness<'static, ShellApp>, index: usize) -> u32 {
    let before = h.state().active_document();
    let ctx = h.ctx.clone();
    h.state_mut().run_ready_template(&ctx, index);
    let start = Instant::now();
    loop {
        h.step();
        let app = h.state();
        let id = app.active_document().filter(|id| Some(*id) != before);
        let done = id
            .and_then(|id| app.documents().get(id))
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running());
        if done {
            h.run_steps(2);
            return id.unwrap();
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "{} never finished",
            builtin::ALL[index].name
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Todas as células de todos os resultados da janela, como texto.
fn cells(h: &Harness<'static, ShellApp>, id: u32) -> Vec<Vec<String>> {
    let doc = h.state().documents().get(id).unwrap();
    let execution = doc.execution.as_ref().unwrap();
    let mut out = Vec::new();
    for result in &execution.results {
        for row in 0..result.len() {
            out.push(
                result
                    .row(row)
                    .into_iter()
                    .map(Option::unwrap_or_default)
                    .collect(),
            );
        }
    }
    out
}

/// Roda todos os modelos prontos do motor e confere que nenhum dá erro.
fn every_ready_template_runs(engine: Engine, database: &str) {
    let Some((settings, password)) = settings(engine, database) else {
        return;
    };
    let mut h = connected(settings, password);
    let mut ran = 0;
    for (i, t) in builtin::ALL.iter().enumerate() {
        if t.engine != engine || !t.ready {
            continue;
        }
        let id = run_ready(&mut h, i);
        let doc = h.state().documents().get(id).unwrap();
        let execution = doc.execution.as_ref().unwrap();
        assert_eq!(execution.first_error(), None, "{}:\n{}", t.name, doc.text);
        assert!(!execution.results.is_empty(), "{}", t.name);
        if t.name == "Full Diagnosis" {
            assert_eq!(execution.results.len(), 4, "{}", doc.text);
            // A visão geral tem uma linha, com o banco.
            assert_eq!(execution.results[0].len(), 1);
        }
        ran += 1;
    }
    assert!(ran >= 8, "{ran}");
}

#[test]
fn every_ready_template_runs_on_postgresql() {
    every_ready_template_runs(Engine::PostgreSql, "postgres");
}

#[test]
fn every_ready_template_runs_on_sql_server() {
    every_ready_template_runs(Engine::SqlServer, "master");
}

/// Num banco sem `pg_stat_statements`, o histórico de consultas sai e o
/// diagnóstico explica como ligar, em vez de falhar.
#[test]
fn without_pg_stat_statements_the_diagnosis_explains_instead_of_failing() {
    let Some((admin, password)) = settings(Engine::PostgreSql, "postgres") else {
        return;
    };
    let c = connector(Engine::PostgreSql)
        .connect(&admin, &password)
        .unwrap();
    let s = c.queries.open("postgres");
    run(&s, "DROP DATABASE IF EXISTS pp_diag_plain WITH (FORCE)");
    run(&s, "CREATE DATABASE pp_diag_plain");
    let (settings, password) = settings(Engine::PostgreSql, "pp_diag_plain").unwrap();
    let mut h = connected(settings, password);
    for name in ["Full Diagnosis", "Most Expensive Queries (History)"] {
        let id = run_ready(&mut h, index(Engine::PostgreSql, name));
        let doc = h.state().documents().get(id).unwrap();
        let execution = doc.execution.as_ref().unwrap();
        assert_eq!(execution.first_error(), None, "{name}");
        assert!(doc.text.contains("create extension pg_stat_statements"));
        assert!(!doc.text.contains("from pg_stat_statements"));
        let all = cells(&h, id);
        assert!(
            all.iter()
                .any(|r| r[0] == "pg_stat_statements is not installed in this database"),
            "{all:?}"
        );
    }
}

/// Uma sessão segura uma linha, outra espera por ela e uma terceira espera
/// pela segunda: a árvore mostra a primeira na margem e as outras embaixo.
fn blocking_tree(engine: Engine, database: &str, setup: &str, hold: &str, wait: [&str; 2]) {
    let Some((settings, password)) = settings(engine, database) else {
        return;
    };
    let c = connector(engine).connect(&settings, &password).unwrap();
    let blocker = c.queries.open(database);
    run(&blocker, setup);
    run(&blocker, hold);
    let second = c.queries.open(database);
    let _second = start(&second, wait[0]);
    std::thread::sleep(Duration::from_millis(700));
    let third = c.queries.open(database);
    let _third = start(&third, wait[1]);
    std::thread::sleep(Duration::from_millis(700));

    let mut h = connected(settings, password);
    let id = run_ready(&mut h, index(engine, "Who Blocks Whom"));
    let rows = cells(&h, id);
    let doc = h.state().documents().get(id).unwrap();
    assert_eq!(doc.execution.as_ref().unwrap().first_error(), None);
    let ours: Vec<&Vec<String>> = rows
        .iter()
        .filter(|r| r.last().is_some_and(|q| q.contains("pp_diag")))
        .collect();
    assert_eq!(ours.len(), 3, "{rows:?}");
    // A que segura o lock: na margem, com duas atrás.
    assert!(!ours[0][0].starts_with(' '), "{ours:?}");
    assert_eq!(ours[0][1], "BLOCKING");
    assert_eq!(ours[0][3], "2");
    // As duas que esperam, cada uma um nível para dentro.
    assert!(ours[1][0].starts_with("      └─ "), "{ours:?}");
    assert_eq!(ours[1][1], "blocked");
    assert_eq!(ours[1][2], ours[0][0]);
    assert!(ours[2][0].starts_with("            └─ "), "{ours:?}");
    assert_eq!(ours[2][2], ours[1][0].trim().trim_start_matches("└─ "));

    // O diagnóstico completo conta as duas bloqueadas na visão geral.
    let id = run_ready(&mut h, index(engine, "Full Diagnosis"));
    let doc = h.state().documents().get(id).unwrap();
    let execution = doc.execution.as_ref().unwrap();
    assert_eq!(execution.first_error(), None);
    let overview = &execution.results[0];
    let blocked = overview
        .columns()
        .iter()
        .position(|c| c == "blocked")
        .unwrap();
    let n: u32 = overview.row(0)[blocked].clone().unwrap().parse().unwrap();
    assert!(n >= 2, "{n}");

    blocker.cancel();
    second.cancel();
    third.cancel();
    run(&c.queries.open(database), "select 1");
}

#[test]
fn who_blocks_whom_draws_the_tree_on_postgresql() {
    blocking_tree(
        Engine::PostgreSql,
        "postgres",
        "drop table if exists pp_diag; create table pp_diag (id int primary key, n int); \
         insert into pp_diag values (1, 0), (2, 0)",
        "begin; update pp_diag set n = 1 where id = 1",
        [
            "begin; update pp_diag set n = 2 where id = 2; update pp_diag set n = 2 where id = 1",
            "update pp_diag set n = 3 where id = 2",
        ],
    );
}

#[test]
fn who_blocks_whom_draws_the_tree_on_sql_server() {
    blocking_tree(
        Engine::SqlServer,
        "tempdb",
        "IF OBJECT_ID('dbo.pp_diag') IS NOT NULL DROP TABLE dbo.pp_diag;\n\
         CREATE TABLE dbo.pp_diag (id int PRIMARY KEY, n int);\n\
         INSERT INTO dbo.pp_diag VALUES (1, 0), (2, 0);",
        "BEGIN TRANSACTION; UPDATE dbo.pp_diag SET n = 1 WHERE id = 1",
        [
            "BEGIN TRANSACTION; UPDATE dbo.pp_diag SET n = 2 WHERE id = 2; \
             UPDATE dbo.pp_diag SET n = 2 WHERE id = 1",
            "UPDATE dbo.pp_diag SET n = 3 WHERE id = 2",
        ],
    );
}

// ---------------------------------------------------------------------
// Sem banco: o painel, o comando e o menu da toolbar.
// ---------------------------------------------------------------------

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    if let Some(cancel) = harness.query_by_label("Cancel") {
        cancel.click();
    }
    harness.run();
    harness
}

#[test]
fn the_command_opens_the_panel_on_the_diagnostics_folder() {
    assert!(ShellApp::is_available(CommandId::DiagnosticQueries));
    let mut h = harness(app());
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::DiagnosticQueries);
    h.run();
    assert!(h.state().is_panel_open(Panel::TemplateExplorer));
    // A pasta aberta, com a primeira consulta selecionada e o "Run".
    assert!(h.query_by_label_contains("Who Blocks Whom").is_some());
    assert!(
        h.query_by_label_contains("Most Expensive Queries (History)")
            .is_some()
    );
    let run = format!("{} Run", egui_phosphor::regular::PLAY);
    assert!(h.query_by_label(&run).is_some());
    // Um modelo de preencher não tem "Run".
    h.state_mut().templates_mut().search = "Create Table".into();
    h.run();
    h.query_all_by_label_contains("Create Table")
        .next()
        .unwrap()
        .click();
    h.run();
    assert!(h.query_by_label(&run).is_none());
}

#[test]
fn without_a_connection_run_opens_the_query_and_says_what_is_missing() {
    let mut h = harness(app());
    let ctx = h.ctx.clone();
    let i = index(Engine::SqlServer, "Who Blocks Whom");
    h.state_mut().run_ready_template(&ctx, i);
    h.run();
    let app = h.state();
    let id = app.active_document().expect("a query window");
    let doc = app.documents().get(id).unwrap();
    assert!(doc.text.contains("sys.dm_exec_requests"));
    assert!(doc.execution.is_none());
    assert!(app.status().contains("SQL Server"), "{}", app.status());
}

#[test]
fn the_search_finds_the_diagnostics_by_what_they_answer() {
    let mut h = harness(app());
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::TemplateExplorer);
    // Em português ou em inglês, pelo nome ou pelo que a consulta lê.
    for term in ["who blocks", "pg_stat_statements", "dm_exec_query_stats"] {
        h.state_mut().templates_mut().search = term.into();
        h.run();
        let found = h.state_mut().templates_mut().found();
        assert!(!found.is_empty(), "{term}");
    }
    assert_eq!(
        builtin::in_folder(Engine::PostgreSql, DIAGNOSTICS).count(),
        8
    );
}
