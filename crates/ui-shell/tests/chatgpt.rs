//! O ChatGPT pela conta (Tools > AI Providers > ChatGPT (your account)):
//! o login pelo Codex, o painel, a mensagem de commit. O `codex` é um
//! script de mentira que grava o que recebeu.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_automation::codex::Login;
use pgstudio_automation::llm::Backend;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::ai::{Ai, AiSettings, CHATGPT};
use pgstudio_ui_shell::claude_panel::Item;
use pgstudio_ui_shell::repo::QUERIES;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-chatgpt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Um `codex` de mentira. `login status` diz se o arquivo `signed-in`
/// existe; `login` cria; `exec` grava os argumentos e a pergunta em
/// `calls.log` e responde (JSONL com `--json`, texto sem).
fn fake_codex(dir: &Path) -> PathBuf {
    let script = dir.join("fake-codex");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
DIR='{dir}'
if [ "$1" = login ] && [ "$2" = status ]; then
  if [ -f "$DIR/signed-in" ]; then printf 'Logged in using ChatGPT\n'; exit 0; fi
  printf 'Not logged in\n' >&2; exit 1
fi
if [ "$1" = login ]; then
  printf 'If your browser did not open, use https://auth.example/login?x=1\n' >&2
  : > "$DIR/signed-in"
  exit 0
fi
printf 'ARGS %s\n' "$*" >> "$DIR/calls.log"
PROMPT=$(cat)
printf 'PROMPT %s\n' "$PROMPT" >> "$DIR/calls.log"
case "$*" in
  *--json*)
    printf '%s\n' '{{"type":"thread.started","thread_id":"th-9"}}'
    printf '%s\n' '{{"type":"turn.started"}}'
    printf '%s\n' '{{"type":"item.started","item":{{"id":"item_0","type":"mcp_tool_call","server":"postpilot","tool":"list_servers","arguments":{{}},"status":"in_progress"}}}}'
    printf '%s\n' '{{"type":"item.completed","item":{{"id":"item_0","type":"mcp_tool_call","server":"postpilot","tool":"list_servers","arguments":{{}},"result":{{"content":[{{"type":"text","text":"[]"}}]}},"error":null,"status":"completed"}}}}'
    printf '%s\n' '{{"type":"item.completed","item":{{"id":"item_1","type":"agent_message","text":"Nenhum servidor conectado."}}}}'
    printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":1}}}}'
    ;;
  *) printf 'Adiciona consulta de clientes ativos\n' ;;
esac
"#,
            dir = dir.display(),
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn chatgpt_ai(codex: &Path) -> Ai {
    let mut ai = Ai::with(
        AiSettings {
            active: CHATGPT.into(),
            providers: Vec::new(),
        },
        &[],
    );
    ai.set_codex(Some(codex.to_path_buf()));
    ai
}

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
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
fn panel_talks_to_chatgpt_through_codex() {
    let dir = temp_dir("panel");
    let codex = fake_codex(&dir);
    let app = ShellApp::default()
        .with_ai(chatgpt_ai(&codex))
        .with_claude_panel(
            None,
            Some(PathBuf::from("/usr/bin/postpilot-test")),
            Some(dir.clone()),
        );
    let mut h = harness(app);
    h.run();
    h.get_by_label("View").click();
    h.run();
    h.get_by_label("Claude").click();
    h.run();
    let ctx = h.ctx.clone();
    h.state_mut()
        .claude_panel_send(&ctx, "quais servidores eu tenho?");
    wait(&mut h, "no answer", |a| !a.claude_panel().is_busy());
    let items = h.state().claude_panel().items().to_vec();
    assert!(
        matches!(&items[1], Item::Tool { name, result: Some((false, text)), .. }
            if name == "list_servers" && text == "[]"),
        "{items:?}"
    );
    assert_eq!(
        items[2],
        Item::Assistant("Nenhum servidor conectado.".into())
    );

    h.state_mut().claude_panel_send(&ctx, "e os bancos?");
    wait(&mut h, "no second answer", |a| !a.claude_panel().is_busy());

    let log = std::fs::read_to_string(dir.join("calls.log")).unwrap();
    let args: Vec<&str> = log.lines().filter(|l| l.starts_with("ARGS")).collect();
    assert_eq!(args.len(), 2, "{log}");
    // Só de leitura, sem shell, com o MCP do PostPilot no canal do painel.
    assert!(args[0].contains("exec --skip-git-repo-check --sandbox read-only"));
    assert!(args[0].contains("features.shell_tool=false"));
    assert!(args[0].contains("mcp_servers.postpilot.command='/usr/bin/postpilot-test'"));
    assert!(args[0].contains("panel-endpoint-"), "{}", args[0]);
    assert!(!args[0].contains("resume"));
    // A segunda mensagem continua a mesma conversa.
    assert!(args[1].ends_with("resume th-9"), "{}", args[1]);
    assert!(log.contains("[PostPilot context]"), "{log}");
    assert!(log.contains("quais servidores eu tenho?"), "{log}");
    // O painel leva o nome do ChatGPT e diz para onde os dados vão.
    assert!(h.query_all_by_label_contains("ChatGPT").count() > 0);
    h.get_by_label_contains("Uses your ChatGPT account");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn commit_message_written_by_chatgpt() {
    let dir = temp_dir("commit");
    let codex = fake_codex(&dir);
    let root = dir.join("vendas");
    let mut app = ShellApp::default().with_ai(chatgpt_ai(&codex));
    assert!(app.create_workspace(&root));
    std::fs::write(
        root.join(QUERIES).join("clientes.sql"),
        "select * from clientes where ativo;\n",
    )
    .unwrap();
    let sc = app.workspace_panel_mut().source_control_mut().unwrap();
    sc.wait();
    sc.init();
    sc.wait();
    sc.refresh();
    sc.wait();
    let mut h = harness(app);
    h.run();
    h.state_mut()
        .workspace_panel_mut()
        .source_control_mut()
        .unwrap()
        .open_commit();
    h.run();
    h.get_by_label_contains("Generate with AI").click();
    h.run_steps(2);
    wait(&mut h, "no commit message", |a| {
        !a.workspace_panel()
            .source_control()
            .unwrap()
            .is_generating()
    });
    let sc = h.state().workspace_panel().source_control().unwrap();
    assert_eq!(sc.commit_error(), None);
    assert_eq!(
        sc.commit_message(),
        Some("Adiciona consulta de clientes ativos")
    );
    let log = std::fs::read_to_string(dir.join("calls.log")).unwrap();
    assert!(log.contains("--ephemeral"), "{log}");
    assert!(!log.contains("--json"), "{log}");
    assert!(log.contains("queries/clientes.sql"), "{log}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn options_sign_in_with_the_chatgpt_account() {
    let dir = temp_dir("options");
    let codex = fake_codex(&dir);
    let mut ai = Ai::default();
    ai.set_codex(Some(codex.clone()));
    let mut h = harness(ShellApp::default().with_ai(ai));
    h.run();
    h.get_by_label("Tools").click();
    h.run();
    h.get_by_label("AI Providers…").click();
    h.run();
    h.state_mut().options_mut().unwrap().ai.settings.active = CHATGPT.into();
    h.run();
    // Ainda sem login: o botão de entrar aparece.
    wait(&mut h, "no sign-in state", |a| {
        a.options().unwrap().ai.chatgpt == Some(Login::SignedOut)
    });
    h.get_by_label_contains("Sign in with ChatGPT").click();
    h.run_steps(2);
    wait(&mut h, "sign-in did not finish", |a| {
        let ai = &a.options().unwrap().ai;
        !ai.is_signing_in() && ai.chatgpt == Some(Login::ChatGpt)
    });
    h.get_by_label_contains("Signed in with your ChatGPT account");
    h.get_by_label("OK").click();
    h.run();
    let ai = h.state().ai();
    assert!(ai.settings().uses_chatgpt());
    assert_eq!(ai.backend(), Some(Backend::Codex { program: codex }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_codex_the_options_say_how_to_install() {
    let mut h = harness(ShellApp::default());
    h.run();
    h.get_by_label("Tools").click();
    h.run();
    h.get_by_label("AI Providers…").click();
    h.run();
    h.state_mut().options_mut().unwrap().ai.settings.active = CHATGPT.into();
    h.run();
    h.get_by_label("npm install -g @openai/codex");
    h.get_by_label("Check again");
    h.get_by_label("OK").click();
    h.run();
    // Escolhido sem o Codex: o painel explica em vez de falhar calado.
    let ctx = h.ctx.clone();
    h.state_mut().claude_panel_send(&ctx, "oi");
    h.run();
    let items = h.state().claude_panel().items().to_vec();
    assert!(
        matches!(items.last(), Some(Item::Note(n)) if n.contains("Codex")),
        "{items:?}"
    );
}
