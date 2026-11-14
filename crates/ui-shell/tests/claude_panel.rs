//! O painel do Claude (marco 7.4) com um `claude` falso: a conversa, a
//! linha de contexto, o botão Insert e o canal local do painel. Só em
//! unix (o falso é um script sh).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::claude_panel::Item;
use serde_json::json;

/// Um `claude` que guarda argumentos e mensagens em `dir` e responde com
/// um bloco de SQL.
fn fake_claude(dir: &Path) -> PathBuf {
    let script = dir.join("claude");
    let log = dir.join("log");
    let body = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" > '{args}'
echo '{{"type":"system","subtype":"init","session_id":"sess-1","model":"fake-model"}}'
while read -r line; do
  printf '%s\n' "$line" >> '{log}'
  echo '{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"Aqui "}}}}}}'
  printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Aqui está:\n```sql\nselect 42 as resposta;\n```"}}]}}}}'
  echo '{{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sess-1"}}'
done
"#,
        args = dir.join("args").display(),
        log = log.display(),
    );
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn wait(h: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    while !done(h.state()) {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn conversation_insert_and_panel_channel() {
    let dir = std::env::temp_dir().join(format!("postpilot-panel-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = fake_claude(&dir);
    let app = ShellApp::default().with_claude_panel(
        Some(program),
        Some(PathBuf::from("/usr/bin/postpilot-test")),
        Some(dir.clone()),
    );
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.run();

    // Uma janela de consulta com texto selecionado.
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    let doc = h.state().active_document().unwrap();
    h.event(Event::Text("select 1".into()));
    h.run();
    pgstudio_sql_editor::set_selection(&h.ctx, pgstudio_ui_shell::panels::editor_id(doc), (0, 8));
    h.run();

    // View > Claude abre o painel com o foco na caixa de mensagem.
    h.get_by_label("View").click();
    h.run();
    h.get_by_label("Claude").click();
    h.run();
    h.event(Event::Text("quanto é 6 x 7?".into()));
    h.key_press(Key::Enter);
    h.run();
    wait(&mut h, "no answer", |a| {
        !a.claude_panel().is_busy() && a.claude_panel().items().len() >= 2
    });

    let items = h.state().claude_panel().items().to_vec();
    assert_eq!(items[0], Item::User("quanto é 6 x 7?".into()));
    let Item::Assistant(answer) = &items[1] else {
        panic!("{items:?}")
    };
    assert!(answer.contains("select 42"), "{answer}");

    // O `claude` recebeu a linha de contexto e o MCP do painel.
    let log = std::fs::read_to_string(dir.join("log")).unwrap();
    let message: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    let content = message["message"]["content"].as_str().unwrap();
    assert!(
        content.starts_with("[PostPilot context] Active query window"),
        "{content}"
    );
    assert!(
        content.contains("Selected text (chars 0-8): select 1"),
        "{content}"
    );
    assert!(content.ends_with("quanto é 6 x 7?"), "{content}");
    let args = std::fs::read_to_string(dir.join("args")).unwrap();
    assert!(args.contains("--strict-mcp-config"), "{args}");
    assert!(args.contains("/usr/bin/postpilot-test"), "{args}");
    assert!(args.contains("panel-endpoint-"), "{args}");

    // O canal do painel atende as ferramentas, como o do MCP.
    let endpoint = dir.join(format!("panel-endpoint-{}.json", std::process::id()));
    assert!(endpoint.is_file());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reply = pgstudio_automation::endpoint::call(&endpoint, "list_query_windows", json!({}));
        let _ = tx.send(reply);
    });
    let start = Instant::now();
    let reply = loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            break r;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "channel");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(!reply.is_error, "{}", reply.text);
    let windows: serde_json::Value = serde_json::from_str(&reply.text).unwrap();
    assert_eq!(windows["windows"][0]["window"], doc, "{windows}");

    // Insert troca a seleção pelo SQL da resposta.
    h.get_by_label(&format!(
        "{} Insert",
        egui_phosphor::regular::ARROW_SQUARE_IN
    ))
    .click();
    h.run();
    let text = h.state().documents().get(doc).unwrap().text.clone();
    assert_eq!(text, "select 42 as resposta;");

    // New chat limpa a conversa e para o processo.
    h.get_by_label(&format!("{} New chat", egui_phosphor::regular::PLUS))
        .click();
    h.run();
    assert!(h.state().claude_panel().items().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_claude_explains_how_to_install() {
    let dir = std::env::temp_dir().join(format!("postpilot-panel-missing-{}", std::process::id()));
    let app = ShellApp::default().with_claude_panel(
        Some(dir.join("does-not-exist")),
        None,
        Some(dir.clone()),
    );
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.run();
    let ctx = h.ctx.clone();
    h.state_mut().claude_panel_send(&ctx, "oi");
    h.run();
    let items = h.state().claude_panel().items().to_vec();
    let Some(Item::Note(note)) = items.last() else {
        panic!("{items:?}")
    };
    assert!(note.contains("Claude Code was not found"), "{note}");
    assert!(!h.state().claude_panel().is_busy());
    let _ = std::fs::remove_dir_all(&dir);
}

/// O painel com o `claude` falso numa pasta só deste teste.
fn panel_app(name: &str, demo: bool) -> (Harness<'static, ShellApp>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("postpilot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let program = fake_claude(&dir);
    let mut app = ShellApp::default().with_claude_panel(
        Some(program),
        Some(PathBuf::from("/usr/bin/postpilot-test")),
        Some(dir.clone()),
    );
    if demo {
        app.connect_demo();
    }
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.run();
    (h, dir)
}

/// A última mensagem que o `claude` falso recebeu.
fn last_message(dir: &Path) -> String {
    let log = std::fs::read_to_string(dir.join("log")).unwrap();
    let message: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    message["message"]["content"].as_str().unwrap().to_string()
}

fn run_command(h: &mut Harness<'static, ShellApp>, cmd: pgstudio_core::CommandId) {
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, cmd);
}

#[test]
fn quick_actions_from_the_editor_context_menu() {
    let (mut h, dir) = panel_app("quick", false);
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    let doc = h.state().active_document().unwrap();
    h.event(Event::Text("select * from pedido".into()));
    h.run();

    // Sem seleção: a consulta inteira, e a versão nova troca tudo.
    let editor = pgstudio_ui_shell::panels::editor_id(doc);
    run_command(&mut h, pgstudio_core::CommandId::ClaudeOptimize);
    h.run();
    wait(&mut h, "no answer", |a| !a.claude_panel().is_busy());
    let items = h.state().claude_panel().items().to_vec();
    assert_eq!(items[0], Item::User("Optimize this query.".into()));
    let sent = last_message(&dir);
    assert!(
        sent.contains(&format!("the query in query window {doc}")),
        "{sent}"
    );
    assert!(sent.contains("mode replace_all"), "{sent}");

    // Com seleção: só ela.
    pgstudio_sql_editor::set_selection(&h.ctx, editor, (0, 6));
    h.run();
    run_command(&mut h, pgstudio_core::CommandId::ClaudeExplain);
    h.run();
    wait(&mut h, "no answer", |a| !a.claude_panel().is_busy());
    let sent = last_message(&dir);
    assert!(sent.contains("Explain the selected SQL."), "{sent}");
    assert!(
        sent.contains(&format!("the selected text of query window {doc}")),
        "{sent}"
    );
    assert!(sent.contains("Selected text (chars 0-6): select"), "{sent}");

    // Corrigir leva o erro da última execução (aqui, nenhuma).
    run_command(&mut h, pgstudio_core::CommandId::ClaudeFix);
    h.run();
    wait(&mut h, "no answer", |a| !a.claude_panel().is_busy());
    let sent = last_message(&dir);
    assert!(sent.contains("Fix the error in this query."), "{sent}");
    assert!(sent.contains("reported no error"), "{sent}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ask_claude_about_a_table_from_object_explorer() {
    use pgstudio_core::metadata::NodeId;
    use pgstudio_object_explorer::Load;
    let (mut h, dir) = panel_app("ask", true);
    let first_child = |h: &mut Harness<'static, ShellApp>, id: &str| -> String {
        let ex = h.state_mut().explorer_mut();
        assert!(ex.wait_idle(Duration::from_secs(10)));
        ex.expand(&NodeId(id.into()));
        assert!(ex.wait_idle(Duration::from_secs(10)));
        match ex.children(&NodeId(id.into())) {
            Some(Load::Loaded(ids)) => ids[0].0.clone(),
            other => panic!("{id}: {other:?}"),
        }
    };
    let databases = first_child(&mut h, "srv");
    let db = first_child(&mut h, &databases);
    first_child(&mut h, &db);
    let schema = first_child(&mut h, &format!("{db}/Schemas"));
    let tables = first_child(&mut h, &schema);
    let table = first_child(&mut h, &tables);
    h.state_mut().explorer_mut().select(&NodeId(table.clone()));
    h.run_steps(3);
    let name = h
        .state()
        .explorer()
        .node(&NodeId(table))
        .unwrap()
        .name
        .clone();
    h.get_all_by_label_contains(&name)
        .next()
        .expect("no table row")
        .click_secondary();
    h.run_steps(3);
    h.get_by_label(&format!("{} Ask Claude", egui_phosphor::regular::SPARKLE))
        .click();
    h.run_steps(3);
    wait(&mut h, "no answer", |a| !a.claude_panel().is_busy());
    let items = h.state().claude_panel().items().to_vec();
    assert_eq!(items[0], Item::User(format!("Tell me about {name}.")));
    let sent = last_message(&dir);
    assert!(
        sent.contains(&format!("{name} is a table in database")),
        "{sent}"
    );
    assert!(sent.contains("describe_table"), "{sent}");

    // Inserir sem janela de consulta aberta abre uma.
    assert!(h.state().active_document().is_none());
    h.get_by_label(&format!("{} Insert", egui_phosphor::regular::ARROW_SQUARE_IN))
        .click();
    h.run_steps(3);
    let doc = h.state().active_document().expect("a new query window");
    let text = h.state().documents().get(doc).unwrap().text.clone();
    assert_eq!(text, "select 42 as resposta;");
    let _ = std::fs::remove_dir_all(&dir);
}
