//! O app contra um Elasticsearch de verdade (com
//! `ELASTICSEARCH_TEST_HOST`; sem ele, os testes só avisam; veja
//! `docs/elasticsearch.md`): conectar pelo Connect to Server, rodar
//! requisições, abrir um índice pela árvore e os comandos só relacionais
//! avisando que não existem para o Elasticsearch.

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

const WAIT: Duration = Duration::from_secs(30);

fn connected() -> Option<Harness<'static, ShellApp>> {
    let Ok(host) = std::env::var("ELASTICSEARCH_TEST_HOST") else {
        eprintln!("ELASTICSEARCH_TEST_HOST not set; skipping");
        return None;
    };
    let port = std::env::var("ELASTICSEARCH_TEST_PORT").unwrap_or_else(|_| "9200".into());
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(pgstudio_elasticsearch::EsConnector::new()));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    {
        let dialog = harness.state_mut().connect_dialog_mut().unwrap();
        dialog.set_engine(Engine::Elasticsearch);
        // Login vazio e a porta 9200 são os padrões do Elasticsearch.
        assert_eq!(dialog.user.as_str(), "");
        dialog.server = format!("{host},{port}");
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
fn connects_runs_requests_and_shows_grids() {
    let Some(mut harness) = connected() else {
        return;
    };
    let c = harness.state().connection().unwrap();
    assert_eq!(c.settings.engine, Engine::Elasticsearch);
    run_text(&mut harness, "DELETE /pptest-app");
    run_text(
        &mut harness,
        "POST /pptest-app/_bulk?refresh=true\n\
         { \"index\": { \"_id\": \"1\" } }\n\
         { \"nome\": \"Ana\", \"cidade\": { \"nome\": \"Recife\" } }\n\
         { \"index\": { \"_id\": \"2\" } }\n\
         { \"nome\": \"Bia\" }\n\
         \n\
         GET /pptest-app/_search\n\
         { \"sort\": [\"_doc\"], \"_source\": [\"nome\", \"cidade\"] }\n\
         GET /pptest-nao-existe/_count",
    );
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    let exec = doc.execution.as_ref().expect("execution");
    // A grade do _bulk e a da busca; a terceira requisição é o erro.
    assert_eq!(exec.results.len(), 2);
    let grid = &exec.results[1];
    assert_eq!(grid.columns()[..2], ["_index", "_id"]);
    assert!(grid.columns().contains(&"nome".to_string()));
    assert_eq!(grid.len(), 2);
    let errors: Vec<&str> = exec
        .messages
        .iter()
        .filter(|m| m.kind == MessageKind::Error)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("index_not_found_exception"),
        "{errors:?}"
    );
    assert!(errors[0].contains("Line 9"), "{errors:?}");
    assert!(
        exec.messages
            .iter()
            .any(|m| m.text.starts_with("2 document(s) found")),
        "{:?}",
        exec.messages.iter().map(|m| &m.text).collect::<Vec<_>>()
    );

    // A árvore: os grupos, o índice e os campos; "Open First 1000
    // Documents".
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
    let groups = expand(&mut harness, "server");
    assert_eq!(groups.len(), 4, "{groups:?}");
    assert_eq!(groups[0], ("Indices".to_string(), NodeKind::KeyGroup));
    let indices = expand(&mut harness, "indices");
    assert!(
        indices.contains(&("pptest-app".into(), NodeKind::Table)),
        "{indices:?}"
    );
    let index = NodeId(format!("i/{}", hex("pptest-app")));
    let fields = expand(&mut harness, &index.0);
    assert!(
        fields.contains(&("nome.keyword (keyword)".into(), NodeKind::Column)),
        "{fields:?}"
    );
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .explorer_event(&ctx, Event::SelectTop(index.clone()));
    let start = Instant::now();
    loop {
        harness.step();
        let app = harness.state();
        let doc = app.active_document().and_then(|d| app.documents().get(d));
        if doc.is_some_and(|d| d.text.contains("\"size\": 1000") && d.execution.is_some())
            && !app.is_executing()
        {
            break;
        }
        assert!(start.elapsed() < WAIT, "the index never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    assert!(
        doc.text.starts_with("GET /pptest-app/_search\n"),
        "{}",
        doc.text
    );
    assert_eq!(doc.execution.as_ref().unwrap().results[0].len(), 2);

    // "Delete Index...": só escreve a requisição.
    harness
        .state_mut()
        .explorer_event(&ctx, Event::Script(index, ScriptAction::Drop));
    let start = Instant::now();
    loop {
        harness.step();
        let app = harness.state();
        let doc = app.active_document().and_then(|d| app.documents().get(d));
        if doc.is_some_and(|d| d.text.contains("DELETE /pptest-app")) {
            break;
        }
        assert!(start.elapsed() < WAIT, "the delete never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let app = harness.state();
    let doc = app.documents().get(app.active_document().unwrap()).unwrap();
    assert_eq!(doc.text.trim(), "DELETE /pptest-app");
    assert!(doc.execution.is_none());

    run_text(&mut harness, "DELETE /pptest-app");
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
            status.contains("not available for Elasticsearch"),
            "{cmd:?}: {status}"
        );
    }
}
