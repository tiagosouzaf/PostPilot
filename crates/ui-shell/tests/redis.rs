//! O app contra um Redis de verdade (com `REDIS_TEST_HOST`; sem ele, os
//! testes só avisam; veja `docs/redis.md`): conectar pelo Connect to
//! Server, rodar comandos numa janela de consulta e ver as grades, e os
//! comandos só relacionais avisando que não existem para o Redis.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::Engine;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::execution::MessageKind;

fn var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn connected() -> Option<Harness<'static, ShellApp>> {
    let Ok(host) = std::env::var("REDIS_TEST_HOST") else {
        eprintln!("REDIS_TEST_HOST not set; skipping");
        return None;
    };
    let runtime = pgstudio_pg_client::DbRuntime::new().unwrap();
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(pgstudio_redis::RedisConnector::new(runtime)));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    {
        let dialog = harness.state_mut().connect_dialog_mut().unwrap();
        dialog.set_engine(Engine::Redis);
        // Login vazio e banco db0 são os padrões do Redis.
        assert_eq!(
            (dialog.user.as_str(), dialog.database.as_str()),
            ("", "db0")
        );
        dialog.server = format!("{host},{}", var("REDIS_TEST_PORT", "6379"));
        dialog.database = "db15".into();
        dialog.password = var("REDIS_TEST_PASSWORD", "");
    }
    // O diálogo muda com o motor (o Redis não tem túnel SSH): desenha de
    // novo antes de clicar.
    harness.run();
    harness.get_by_label("Connect").click();
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(10), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    Some(harness)
}

fn run_text(harness: &mut Harness<'static, ShellApp>, text: &str) {
    let ctx = harness.ctx.clone();
    let doc = harness.state().active_document().unwrap();
    harness
        .state_mut()
        .documents_mut()
        .get_mut(doc)
        .unwrap()
        .text = text.into();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    let start = Instant::now();
    loop {
        harness.step();
        if !harness.state().is_executing() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
}

#[test]
fn connects_runs_commands_and_shows_grids() {
    let Some(mut harness) = connected() else {
        return;
    };
    let ctx = harness.ctx.clone();
    let c = harness.state().connection().unwrap();
    assert_eq!(c.settings.engine, Engine::Redis);
    assert_eq!(c.settings.database, "db15");
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    run_text(
        &mut harness,
        "DEL ppui:h\nHSET ppui:h nome Ana cidade \"São Paulo\"\nHGETALL ppui:h\nGET ppui:h\nDEL ppui:h",
    );
    let doc = harness.state().active_document().unwrap();
    let exec = harness
        .state()
        .documents()
        .get(doc)
        .unwrap()
        .execution
        .as_ref()
        .expect("execution");
    // Só o HGETALL tem grade: DEL e HSET respondem com um número, que
    // vai para Messages, e o GET num hash é erro.
    assert_eq!(exec.results.len(), 1);
    let hash = &exec.results[0];
    assert_eq!(hash.columns(), ["field", "value"]);
    assert_eq!(
        hash.row(1),
        [Some("cidade".into()), Some("São Paulo".into())]
    );
    assert!(
        exec.messages.iter().any(|m| m.text == "(integer) 2"),
        "the HSET count is a message"
    );
    let errors: Vec<&str> = exec
        .messages
        .iter()
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("WRONGTYPE"), "{errors:?}");
    // O erro aponta para a linha do GET.
    assert!(errors[0].contains("Line 4"), "{errors:?}");
}

#[test]
fn relational_tools_say_they_are_not_available() {
    let Some(mut harness) = connected() else {
        return;
    };
    let ctx = harness.ctx.clone();
    for cmd in [
        CommandId::ActivityMonitor,
        CommandId::SchemaCompare,
        CommandId::EstimatedPlan,
    ] {
        harness.state_mut().execute(&ctx, CommandId::NewQuery);
        harness.run();
        harness.state_mut().execute(&ctx, cmd);
        harness.run();
        let status = harness.state().status().to_string();
        assert!(
            status.contains("not available for Redis"),
            "{cmd:?}: {status}"
        );
    }
}

/// As chaves no Object Explorer: grupos por prefixo, "Open Value" numa
/// janela nova já rodando, o filtro (que a sessão guarda) e o "Delete
/// Key..." que só escreve o comando. Usa o banco 10, só dele.
#[test]
fn keys_show_in_the_tree_and_open_in_a_window() {
    use pgstudio_core::metadata::{NodeId, NodeKind, ScriptAction};
    use pgstudio_object_explorer::{Event, Load};

    let Some(mut harness) = connected() else {
        return;
    };
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    run_text(
        &mut harness,
        "SELECT 10\nFLUSHDB\nHSET loja:1:dados nome Ana\nSET loja:1:visitas 7\nSET loja:2:visitas 9\nRPUSH fila a b",
    );

    let wait = Duration::from_secs(10);
    let expand = |harness: &mut Harness<'static, ShellApp>, id: &str| -> Vec<(String, NodeKind)> {
        let id = NodeId(id.into());
        let explorer = harness.state_mut().explorer_mut();
        explorer.refresh(&id);
        assert!(explorer.wait_idle(wait), "tree never loaded");
        match explorer.children(&id) {
            Some(Load::Loaded(ids)) => ids
                .iter()
                .map(|id| {
                    let node = explorer.node(id).unwrap();
                    (node.name.clone(), node.kind)
                })
                .collect(),
            other => panic!("{other:?}"),
        }
    };
    harness.state_mut().explorer_mut().wait_idle(wait);
    // A contagem do banco vem do servidor: relê a lista de bancos.
    let dbs = expand(&mut harness, "server");
    assert!(
        dbs.contains(&("db10 (4)".into(), NodeKind::Database)),
        "{dbs:?}"
    );
    assert_eq!(
        expand(&mut harness, "db/10"),
        [
            ("loja:* (3)".to_string(), NodeKind::KeyGroup),
            ("fila (list)".to_string(), NodeKind::Key),
        ]
    );
    harness.run();
    assert!(harness.query_by_label_contains("loja:* (3)").is_some());

    // "Open Value" na lista: janela nova no db10, com o LRANGE rodado.
    let fila = NodeId(format!("db/10/k/{}", hex("fila")));
    harness
        .state_mut()
        .explorer_event(&ctx, Event::SelectTop(fila.clone()));
    let start = Instant::now();
    loop {
        harness.step();
        let app = harness.state();
        let doc = app.active_document().and_then(|d| app.documents().get(d));
        if doc.is_some_and(|d| d.text.starts_with("LRANGE") && d.execution.is_some())
            && !app.is_executing()
        {
            break;
        }
        assert!(start.elapsed() < wait, "the value never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    assert_eq!(doc.text.trim(), "LRANGE fila 0 999");
    let exec = doc.execution.as_ref().unwrap();
    assert_eq!(exec.results.len(), 1);
    assert_eq!(exec.results[0].row(1)[0], Some("b".into()));

    // "Delete Key...": só escreve o DEL, sem rodar.
    harness
        .state_mut()
        .explorer_event(&ctx, Event::Script(fila, ScriptAction::Drop));
    let start = Instant::now();
    loop {
        harness.step();
        let app = harness.state();
        let doc = app.active_document().and_then(|d| app.documents().get(d));
        if doc.is_some_and(|d| d.text.starts_with("DEL")) {
            break;
        }
        assert!(start.elapsed() < wait, "the DEL never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    assert_eq!(doc.text.trim(), "DEL fila");
    assert!(doc.execution.is_none());

    // O filtro: a janela abre pelo menu; o texto estreita a lista e vai
    // para a sessão.
    let db = NodeId("db/10".into());
    harness.state_mut().explorer_mut().edit_filter(&db);
    harness.run();
    assert!(harness.query_by_label("Filter Keys").is_some());
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.query_by_label("Filter Keys").is_none());
    harness
        .state_mut()
        .explorer_mut()
        .set_filter(&db, "visitas");
    assert!(harness.state_mut().explorer_mut().wait_idle(wait));
    harness.run();
    assert!(harness.query_by_label_contains("Filter: visitas").is_some());
    assert!(harness.query_by_label_contains("loja:* (2)").is_some());
    assert!(harness.query_by_label_contains("fila (list)").is_none());
    let state = harness.state().session_state();
    assert_eq!(
        state.servers[0].filters,
        [("db/10".to_string(), "visitas".to_string())]
    );
    assert!(state.servers[0].expanded.contains(&"db/10".to_string()));

    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    run_text(&mut harness, "SELECT 10\nFLUSHDB");
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}
