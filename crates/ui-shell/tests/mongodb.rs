//! O app contra um MongoDB de verdade (com `MONGODB_TEST_HOST`; sem ele,
//! os testes só avisam; veja `docs/mongodb.md`): conectar pelo Connect to
//! Server, rodar consultas, abrir uma coleção pela árvore e os comandos só
//! relacionais avisando que não existem para o MongoDB.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::Engine;
use pgstudio_core::metadata::{NodeId, NodeKind, ScriptAction};
use pgstudio_object_explorer::{Event, Load};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::execution::MessageKind;

const WAIT: Duration = Duration::from_secs(15);

fn var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn connected(database: &str) -> Option<Harness<'static, ShellApp>> {
    let Ok(host) = std::env::var("MONGODB_TEST_HOST") else {
        eprintln!("MONGODB_TEST_HOST not set; skipping");
        return None;
    };
    let runtime = pgstudio_pg_client::DbRuntime::new().unwrap();
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(pgstudio_mongodb::MongoConnector::new(runtime)));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    {
        let dialog = harness.state_mut().connect_dialog_mut().unwrap();
        dialog.set_engine(Engine::MongoDb);
        // Login vazio e banco `test` são os padrões do MongoDB.
        assert_eq!(
            (dialog.user.as_str(), dialog.database.as_str()),
            ("", "test")
        );
        dialog.server = format!("{host},{}", var("MONGODB_TEST_PORT", "27017"));
        dialog.user = var("MONGODB_TEST_USER", "");
        dialog.password = var("MONGODB_TEST_PASSWORD", "");
        dialog.database = database.into();
    }
    // O diálogo muda com o motor: desenha de novo antes de clicar.
    harness.run();
    harness.get_by_label("Connect").click();
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < WAIT, "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    Some(harness)
}

fn run_text(harness: &mut Harness<'static, ShellApp>, text: &str) {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
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
        assert!(start.elapsed() < WAIT, "never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn connects_runs_queries_and_shows_grids() {
    let Some(mut harness) = connected("pptest_app") else {
        return;
    };
    let c = harness.state().connection().unwrap();
    assert_eq!(c.settings.engine, Engine::MongoDb);
    assert_eq!(c.settings.database, "pptest_app");
    run_text(
        &mut harness,
        "db.dropDatabase()\n\
         db.clientes.insertMany([{ _id: 1, nome: 'Ana', cidade: { nome: 'Recife' } }, { _id: 2, nome: 'Bia' }])\n\
         db.clientes.find({}).sort({ _id: 1 })\n\
         db.clientes.voar()",
    );
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    // O erro de leitura (método que não existe) vem antes de rodar
    // qualquer coisa: nenhuma grade.
    let exec = doc.execution.as_ref().expect("execution");
    assert_eq!(exec.results.len(), 0);
    let errors: Vec<&str> = exec
        .messages
        .iter()
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("voar"), "{errors:?}");
    assert!(errors[0].contains("Line 4"), "{errors:?}");

    run_text(
        &mut harness,
        "db.dropDatabase()\n\
         db.clientes.insertMany([{ _id: 1, nome: 'Ana', cidade: { nome: 'Recife' } }, { _id: 2, nome: 'Bia' }])\n\
         db.clientes.find({}).sort({ _id: 1 })",
    );
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    let exec = doc.execution.as_ref().expect("execution");
    assert_eq!(exec.results.len(), 1);
    let grid = &exec.results[0];
    assert_eq!(grid.columns(), ["_id", "nome", "cidade"]);
    assert_eq!(
        grid.row(0),
        [
            Some("1".into()),
            Some("Ana".into()),
            Some(r#"{"nome":"Recife"}"#.into())
        ]
    );
    assert_eq!(grid.row(1)[2], None);
    assert!(
        exec.messages
            .iter()
            .any(|m| m.text == "2 document(s) inserted."),
        "the insert count is a message: {:?}",
        exec.messages.iter().map(|m| &m.text).collect::<Vec<_>>()
    );

    // A árvore: banco, coleção, índice; "Open First 1000 Documents".
    let expand = |harness: &mut Harness<'static, ShellApp>, id: &str| -> Vec<(String, NodeKind)> {
        let id = NodeId(id.into());
        let explorer = harness.state_mut().explorer_mut();
        explorer.refresh(&id);
        assert!(explorer.wait_idle(WAIT), "tree never loaded");
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
    harness.state_mut().explorer_mut().wait_idle(WAIT);
    let dbs = expand(&mut harness, "server");
    assert!(
        dbs.contains(&("pptest_app".into(), NodeKind::Database)),
        "{dbs:?}"
    );
    let db = format!("db/{}", hex("pptest_app"));
    assert_eq!(
        expand(&mut harness, &db),
        [("clientes".to_string(), NodeKind::Table)]
    );
    let collection = NodeId(format!("{db}/c/{}", hex("clientes")));
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .explorer_event(&ctx, Event::SelectTop(collection.clone()));
    let start = Instant::now();
    loop {
        harness.step();
        let app = harness.state();
        let doc = app.active_document().and_then(|d| app.documents().get(d));
        if doc.is_some_and(|d| d.text.contains(".limit(1000)") && d.execution.is_some())
            && !app.is_executing()
        {
            break;
        }
        assert!(start.elapsed() < WAIT, "the collection never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    assert_eq!(doc.text.trim(), "db.clientes.find({}).limit(1000)");
    assert_eq!(doc.execution.as_ref().unwrap().results[0].len(), 2);

    // "Drop Collection...": só escreve o comando.
    harness
        .state_mut()
        .explorer_event(&ctx, Event::Script(collection, ScriptAction::Drop));
    let start = Instant::now();
    loop {
        harness.step();
        let app = harness.state();
        let doc = app.active_document().and_then(|d| app.documents().get(d));
        if doc.is_some_and(|d| d.text.contains(".drop()")) {
            break;
        }
        assert!(start.elapsed() < WAIT, "the drop never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    assert_eq!(doc.text.trim(), "db.clientes.drop()");
    assert!(doc.execution.is_none());

    run_text(&mut harness, "db.dropDatabase()");
}

#[test]
fn relational_tools_say_they_are_not_available() {
    let Some(mut harness) = connected("pptest_app2") else {
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
            status.contains("not available for MongoDB"),
            "{cmd:?}: {status}"
        );
    }
}
