//! A IA por provedores (Tools > AI Providers): a mensagem de commit, o
//! nome de arquivo sugerido ao salvar e o painel com um provedor por chave
//! de API. O provedor é um servidor HTTP de mentira em 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pgstudio_automation::llm::{Provider, ProviderKind};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::ai::{Ai, AiSettings, CLAUDE_CODE};
use pgstudio_ui_shell::claude_panel::Item;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::repo::QUERIES;
use serde_json::{Value, json};

/// Um provedor de mentira: responde cada pedido com o próximo corpo SSE e
/// guarda os pedidos recebidos.
fn fake_provider(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for body in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let request = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + length {
                        break text;
                    }
                }
                if n == 0 {
                    break text;
                }
            };
            log.lock().unwrap().push(request);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
    });
    (url, seen)
}

fn sse(chunks: &[Value]) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str(&format!("data: {c}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    out
}

fn text_reply(text: &str) -> String {
    sse(&[json!({"choices":[{"delta":{"content":text}}]})])
}

/// O corpo JSON do pedido `n`.
fn body(seen: &Arc<Mutex<Vec<String>>>, n: usize) -> Value {
    let seen = seen.lock().unwrap();
    serde_json::from_str(seen[n].split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

/// Uma IA com um provedor GLM apontado para `url`, com chave.
fn ai_at(url: &str) -> Ai {
    let mut provider = Provider::new(ProviderKind::Glm, "glm-1");
    provider.base_url = url.to_string();
    provider.model = "glm-test".into();
    Ai::with(
        AiSettings {
            active: "glm-1".into(),
            providers: vec![provider],
        },
        &[("glm-1", "sk-test")],
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-ai-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

fn workspace_with_git(root: &Path, app: &mut ShellApp) {
    assert!(app.create_workspace(root));
    std::fs::write(
        root.join(QUERIES).join("clientes.sql"),
        "select * from clientes where ativo;\n",
    )
    .unwrap();
    std::fs::write(root.join("secrets.json"), "{\"segredo\":\"NAO-ENVIAR\"}\n").unwrap();
    let sc = app.workspace_panel_mut().source_control_mut().unwrap();
    sc.wait();
    sc.init();
    sc.wait();
    sc.refresh();
    sc.wait();
}

#[test]
fn commit_message_written_by_the_ai() {
    let (url, seen) = fake_provider(vec![text_reply(
        "```\nAdiciona consulta de clientes ativos\n```",
    )]);
    let root = temp_dir("commit").join("vendas");
    let mut app = ShellApp::default().with_ai(ai_at(&url));
    workspace_with_git(&root, &mut app);
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
        let sc = a.workspace_panel().source_control().unwrap();
        !sc.is_generating()
    });
    let sc = h.state().workspace_panel().source_control().unwrap();
    assert_eq!(sc.commit_error(), None);
    assert_eq!(
        sc.commit_message(),
        Some("Adiciona consulta de clientes ativos")
    );

    // O que foi para o provedor: a lista, o conteúdo do arquivo novo e
    // nada do cofre de senhas.
    let sent = body(&seen, 0);
    assert_eq!(sent["model"], "glm-test");
    let user = sent["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("Added queries/clientes.sql"), "{user}");
    assert!(user.contains("select * from clientes where ativo;"), "{user}");
    assert!(user.contains("Added secrets.json"), "{user}");
    assert!(!user.contains("NAO-ENVIAR"), "{user}");
    assert!(seen.lock().unwrap()[0].contains("Bearer sk-test"));
}

#[test]
fn without_an_ai_the_button_is_off() {
    let root = temp_dir("no-ai").join("vendas");
    let mut app = ShellApp::default();
    workspace_with_git(&root, &mut app);
    let mut h = harness(app);
    h.run();
    h.state_mut()
        .workspace_panel_mut()
        .source_control_mut()
        .unwrap()
        .open_commit();
    h.run();
    let button = h.get_by_label_contains("Generate with AI");
    assert!(button.accesskit_node().is_disabled());
}

/// Diálogo de salvar que guarda o nome sugerido e responde com ele na
/// pasta `dir`.
struct Recorder {
    dir: PathBuf,
    names: Arc<Mutex<Vec<String>>>,
}

impl FileDialogs for Recorder {
    fn save_path(&mut self, name: &str, _current: Option<&Path>) -> Option<PathBuf> {
        self.names.lock().unwrap().push(name.to_string());
        Some(self.dir.join(name))
    }

    fn open_path(&mut self) -> Option<PathBuf> {
        None
    }
}

fn type_and_save(h: &mut Harness<'static, ShellApp>, sql: &str) {
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    h.event(Event::Text(sql.into()));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::S);
    h.run_steps(2);
}

#[test]
fn saving_a_new_query_suggests_a_name() {
    let (url, seen) = fake_provider(vec![text_reply("Clientes Ativos por Mês.sql")]);
    let dir = temp_dir("save");
    let names = Arc::new(Mutex::new(Vec::new()));
    let app = ShellApp::default()
        .with_ai(ai_at(&url))
        .with_file_dialogs(Recorder {
            dir: dir.clone(),
            names: names.clone(),
        });
    let mut h = harness(app);
    h.run();
    type_and_save(&mut h, "select mes, count(*) from clientes group by 1");
    wait(&mut h, "no name", |a| !a.is_suggesting_name());
    assert_eq!(*names.lock().unwrap(), ["clientes_ativos_por_mes.sql"]);
    assert!(dir.join("clientes_ativos_por_mes.sql").is_file());
    let sent = body(&seen, 0);
    assert_eq!(
        sent["messages"][1]["content"],
        "select mes, count(*) from clientes group by 1"
    );

    // Sem IA conectada, o nome padrão, na hora.
    let names = Arc::new(Mutex::new(Vec::new()));
    let app = ShellApp::default().with_file_dialogs(Recorder {
        dir,
        names: names.clone(),
    });
    let mut h = harness(app);
    h.run();
    type_and_save(&mut h, "select 1");
    assert!(!h.state().is_suggesting_name());
    assert_eq!(*names.lock().unwrap(), ["SQLQuery1.sql"]);
}

#[test]
fn panel_talks_to_the_provider_and_runs_its_tools() {
    let first = sse(&[json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1",
        "function":{"name":"list_query_windows","arguments":"{}"}}]}}]})]);
    let (url, seen) = fake_provider(vec![first, text_reply("Há uma janela aberta.")]);
    let mut h = harness(ShellApp::default().with_ai(ai_at(&url)));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    h.get_by_label("View").click();
    h.run();
    h.get_by_label("Claude").click();
    h.run();
    h.event(Event::Text("quantas janelas?".into()));
    h.key_press(Key::Enter);
    h.run_steps(2);
    wait(&mut h, "no answer", |a| {
        !a.claude_panel().is_busy() && a.claude_panel().items().len() >= 3
    });
    let items = h.state().claude_panel().items().to_vec();
    assert_eq!(items[0], Item::User("quantas janelas?".into()));
    let Item::Tool { name, result, .. } = &items[1] else {
        panic!("{items:?}")
    };
    assert_eq!(name, "list_query_windows");
    let (is_error, text) = result.clone().expect("tool answered");
    assert!(!is_error, "{text}");
    assert!(text.contains("SQLQuery1.sql"), "{text}");
    assert_eq!(items[2], Item::Assistant("Há uma janela aberta.".into()));

    // O provedor recebeu as ferramentas, a linha de contexto e, na segunda
    // chamada, o resultado da ferramenta.
    let sent = body(&seen, 0);
    assert!(sent["tools"].as_array().unwrap().len() >= 10);
    let user = sent["messages"][1]["content"].as_str().unwrap();
    assert!(user.starts_with("[PostPilot context]"), "{user}");
    let sent = body(&seen, 1);
    assert_eq!(sent["messages"][3]["role"], "tool");
    // O painel mostra o nome do provedor.
    assert!(h.query_all_by_label_contains("GLM (Z.ai)").count() > 0);
}

#[test]
fn options_page_adds_a_provider() {
    let mut h = harness(ShellApp::default());
    h.run();
    h.get_by_label("Tools").click();
    h.run();
    h.get_by_label("AI Providers…").click();
    h.run();
    assert_eq!(h.state().ai().settings().active, CLAUDE_CODE);
    {
        let dialog = h.state_mut().options_mut().expect("options open");
        dialog.ai.add(ProviderKind::OpenRouter);
        let id = dialog.ai.settings.providers[0].id.clone();
        dialog.ai.new_keys.insert(id, "sk-or-123".into());
    }
    h.run();
    // Sem Claude Code nesta "máquina", o provedor novo já fica escolhido.
    h.get_by_label("Base URL:");
    h.get_by_label("OK").click();
    h.run();
    let ai = h.state().ai();
    assert_eq!(ai.settings().active, "openrouter-1");
    let Some(pgstudio_automation::llm::Backend::Api { name, conn }) = ai.backend() else {
        panic!("no backend")
    };
    assert_eq!(name, "OpenRouter");
    assert_eq!(conn.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(conn.api_key, "sk-or-123");
}
