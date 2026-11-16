//! As sugestões da IA no editor: com um provedor conectado (um servidor
//! HTTP de mentira em 127.0.0.1), a IA lê o comando do cursor depois de
//! uma pausa na digitação, o editor mostra a lista e Enter aplica.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use pgstudio_automation::llm::{Provider, ProviderKind};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_sql_editor::hints::{HintAction, HintKind, HintStatus};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::ai::{Ai, AiSettings, AssistMode};
use serde_json::{Value, json};

/// Um provedor de mentira: responde cada pedido com o próximo texto e
/// guarda os pedidos recebidos.
fn fake_provider(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for reply in replies {
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
            let body = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":reply}}]})
            );
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

fn sent(seen: &Arc<Mutex<Vec<String>>>, n: usize) -> Value {
    let seen = seen.lock().unwrap();
    serde_json::from_str(seen[n].split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn ai_at(url: &str, assist: AssistMode) -> Ai {
    let mut provider = Provider::new(ProviderKind::Glm, "glm-1");
    provider.base_url = url.to_string();
    provider.model = "glm-test".into();
    Ai::with(
        AiSettings {
            active: "glm-1".into(),
            providers: vec![provider],
            assist,
        },
        &[("glm-1", "sk-test")],
    )
}

/// Sem pausa na digitação: a pergunta sai no frame seguinte. A pausa de
/// verdade tem o teste dela.
fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    paused(app, Duration::ZERO)
}

fn paused(app: ShellApp, pause: Duration) -> Harness<'static, ShellApp> {
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(
            |ui, app: &mut ShellApp| app.show(ui),
            app.with_assist_pause(pause),
        );
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    h
}

fn wait(h: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    while !done(h.state()) {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

/// Passa o tempo de uma pausa sem que nada aconteça.
fn idle(h: &mut Harness<'static, ShellApp>) {
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn doc<'a>(h: &'a Harness<'static, ShellApp>) -> &'a pgstudio_ui_shell::documents::Document {
    let id = h.state().active_document().unwrap();
    h.state().documents().get(id).unwrap()
}

fn count(h: &Harness<'static, ShellApp>) -> Option<usize> {
    doc(h).editor.hints().map(|hints| hints.items.len())
}

const JOIN: &str = "select * from pedidos p join clientes c on c.id = p.cliente_id";

fn answer() -> String {
    json!({"suggestions":[
        {"kind":"columns","title":"Escolher as colunas","detail":"SELECT * traz tudo.",
         "action":"replace","code":"select p.id, p.total from pedidos p"},
        {"kind":"join","title":"Juntar com clientes","detail":"Pela chave cliente_id.",
         "action":"replace","code":JOIN},
        {"kind":"index","title":"Criar índice em total","detail":"Para filtrar por total.",
         "action":"insert","code":"create index on pedidos (total)"},
        {"kind":"safety","title":"Sem LIMIT","detail":"A tabela pode ser grande.",
         "action":"none","code":""}
    ]})
    .to_string()
}

#[test]
fn the_ai_reads_the_statement_after_a_pause_and_enter_applies() {
    let (url, seen) = fake_provider(vec![answer(), "{\"suggestions\":[]}".into(), "nope".into()]);
    let mut app = ShellApp::default().with_ai(ai_at(&url, AssistMode::Auto));
    app.explorer_mut()
        .connect(Arc::new(FakeProvider::new(Duration::ZERO)));
    let mut h = harness(app);
    h.event(Event::Text("select 1;\nselect * from pedidos p".into()));
    h.run();
    wait(&mut h, "no suggestions", |a| {
        a.ai_hints(a.active_document().unwrap()).is_some()
    });
    assert_eq!(h.state().assist_requests(), 1);
    let hints = doc(&h).editor.hints().unwrap().clone();
    assert_eq!(hints.items.len(), 4);
    assert_eq!(hints.base, "select * from pedidos p");
    assert_eq!(hints.items[1].kind, HintKind::Join);
    assert_eq!(hints.source, "GLM (Z.ai)");

    // Foi o comando do cursor, com a marca dele, o motor e o catálogo.
    let body = sent(&seen, 0);
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("PostgreSQL"), "{system}");
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("Engine: PostgreSQL"), "{user}");
    assert!(user.contains("select * from pedidos p‸"), "{user}");
    assert!(!user.contains("select 1"), "{user}");
    assert!(user.contains("Other tables and views:"), "{user}");
    assert!(user.contains("table_0001"), "{user}");

    // Ctrl+. abre a lista; setas escolhem; Enter troca o comando todo.
    assert!(!doc(&h).editor.hints_open());
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run();
    assert!(doc(&h).editor.hints_open());
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(doc(&h).editor.hint_selected(), Some(1));
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(doc(&h).text, format!("select 1;\n{JOIN}"));
    assert!(!doc(&h).editor.hints_open());
    assert!(
        h.state().status().contains("Juntar com clientes"),
        "{}",
        h.state().status()
    );

    // O comando novo é lido de novo; a IA não tem o que dizer dele.
    wait(&mut h, "no second answer", |a| {
        a.assist_requests() == 2 && !a.is_assist_thinking()
    });
    wait(&mut h, "no empty list", |a| {
        a.ai_hints(a.active_document().unwrap()).is_some()
    });
    assert_eq!(count(&h), Some(0));

    // Desfazer volta ao comando já lido: as sugestões saem do que foi
    // guardado, sem outra pergunta.
    h.key_press_modifiers(Modifiers::CTRL, Key::Z);
    h.run();
    assert_eq!(doc(&h).text, "select 1;\nselect * from pedidos p");
    wait(&mut h, "no cached suggestions", |a| {
        a.ai_hints(a.active_document().unwrap())
            .is_some_and(|hints| hints.items.len() == 4)
    });
    assert_eq!(h.state().assist_requests(), 2);

    // "Acrescentar depois": o índice entra depois do comando e as outras
    // sugestões continuam valendo.
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run();
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(
        doc(&h).text,
        "select 1;\nselect * from pedidos p;\n\ncreate index on pedidos (total)"
    );

    // Uma resposta que não é o JSON pedido vira um aviso, sem insistir.
    wait(&mut h, "no third answer", |a| {
        a.assist_requests() == 3 && !a.is_assist_thinking()
    });
    idle(&mut h);
    assert!(
        matches!(doc(&h).editor.hint_status(), HintStatus::Failed(_)),
        "{:?}",
        doc(&h).editor.hint_status()
    );
    assert_eq!(count(&h), None);
    assert_eq!(h.state().assist_requests(), 3);
    assert!(
        h.state()
            .output()
            .iter()
            .any(|l| l.starts_with("AI suggestions: ")),
        "{:?}",
        h.state().output()
    );
}

#[test]
fn typing_drops_stale_suggestions_and_an_advice_stays_open() {
    let (url, _seen) = fake_provider(vec![answer()]);
    let app = ShellApp::default().with_ai(ai_at(&url, AssistMode::Auto));
    let mut h = harness(app);
    h.event(Event::Text("select * from pedidos p".into()));
    h.run();
    wait(&mut h, "no suggestions", |a| {
        a.ai_hints(a.active_document().unwrap()).is_some()
    });
    // Um conselho não tem o que aplicar: Enter deixa o texto e a lista.
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run();
    for _ in 0..3 {
        h.key_press(Key::ArrowDown);
    }
    h.run();
    let hints = doc(&h).editor.hints().unwrap();
    assert_eq!(hints.items[3].action, HintAction::None);
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(doc(&h).text, "select * from pedidos p");
    assert!(doc(&h).editor.hints_open());
    // Esc fecha; digitar tira as sugestões, que eram de outro texto.
    h.key_press(Key::Escape);
    h.run();
    assert!(!doc(&h).editor.hints_open());
    assert_eq!(count(&h), Some(4));
    h.event(Event::Text(" where".into()));
    h.run();
    assert_eq!(count(&h), None);
    assert_eq!(doc(&h).text, "select * from pedidos p where");
}

#[test]
fn on_demand_asks_only_with_the_shortcut_and_off_never_asks() {
    let (url, seen) = fake_provider(vec![answer()]);
    let app = ShellApp::default().with_ai(ai_at(&url, AssistMode::OnDemand));
    let mut h = harness(app);
    h.event(Event::Text("select * from pedidos p".into()));
    h.run();
    idle(&mut h);
    assert_eq!(h.state().assist_requests(), 0);
    assert_eq!(doc(&h).editor.hint_status(), &HintStatus::Ask);
    // O atalho pergunta e a lista abre quando a resposta chega.
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run();
    wait(&mut h, "the list did not open", |a| {
        let id = a.active_document().unwrap();
        a.documents().get(id).unwrap().editor.hints_open()
    });
    assert_eq!(h.state().assist_requests(), 1);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(count(&h), Some(4));

    let (url, seen) = fake_provider(vec![answer()]);
    let app = ShellApp::default().with_ai(ai_at(&url, AssistMode::Off));
    let mut h = harness(app);
    h.event(Event::Text("select * from pedidos p".into()));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run();
    idle(&mut h);
    assert_eq!(h.state().assist_requests(), 0);
    assert!(seen.lock().unwrap().is_empty());
    assert!(
        h.state().status().contains("turned off"),
        "{}",
        h.state().status()
    );

    // Sem IA conectada, nada acontece e o atalho diz o que falta.
    let mut h = harness(ShellApp::default());
    h.event(Event::Text("select * from pedidos p".into()));
    h.run();
    idle(&mut h);
    assert_eq!(doc(&h).editor.hint_status(), &HintStatus::Off);
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run_steps(3);
    assert!(
        h.state().status().contains("Connect an AI"),
        "{}",
        h.state().status()
    );
}

#[test]
fn nothing_is_asked_while_the_typing_goes_on() {
    let (url, seen) = fake_provider(vec![answer()]);
    let app = ShellApp::default().with_ai(ai_at(&url, AssistMode::Auto));
    let mut h = paused(app, Duration::from_millis(400));
    // Uma letra a cada 20 ms: a pausa nunca se completa.
    for c in "select * from pedidos p".chars() {
        h.event(Event::Text(c.to_string()));
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(h.state().assist_requests(), 0);
    assert!(seen.lock().unwrap().is_empty());
    // Parou de digitar: uma pergunta só, com o comando inteiro.
    wait(&mut h, "no suggestions", |a| {
        a.ai_hints(a.active_document().unwrap()).is_some()
    });
    assert_eq!(h.state().assist_requests(), 1);
    let user = sent(&seen, 0)["messages"][1]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(user.contains("select * from pedidos p‸"), "{user}");
}

/// O clique no aviso do canto, como o de um mouse de verdade (o botão
/// aperta num frame e solta em outro): a lista abre e fica aberta, o
/// cursor do texto não sai do comando e a IA não é perguntada de novo.
#[test]
fn clicking_the_notice_keeps_the_cursor_and_the_list_open() {
    use eframe::egui::{ImeEvent, PointerButton};
    use egui_kittest::kittest::Queryable;
    let (url, _seen) = fake_provider(vec![answer(), answer()]);
    let app = ShellApp::default().with_ai(ai_at(&url, AssistMode::Auto));
    let mut h = paused(app, Duration::from_millis(300));
    h.event(Event::Text(
        "select * from pedidos p;\n\nselect 1 from outra_tabela".into(),
    ));
    h.step();
    for _ in 0..30 {
        h.key_press(Key::ArrowLeft);
    }
    h.step();
    wait(&mut h, "no suggestions", |a| {
        a.ai_hints(a.active_document().unwrap()).is_some()
    });
    let cursor = doc(&h).cursor_byte;
    assert_eq!(
        doc(&h).editor.hints().unwrap().base,
        "select * from pedidos p"
    );
    let at = h.get_by_label_contains("AI suggestions").rect().center();
    h.event(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    assert!(doc(&h).editor.hints_open());
    assert_eq!(doc(&h).cursor_byte, cursor);
    // Um aviso sem texto do método de entrada (Wayland) não fecha a lista.
    h.event(Event::Ime(ImeEvent::Preedit {
        text: String::new(),
        active_range_chars: None,
    }));
    // Passa bem mais que a pausa: nada fecha a lista nem pergunta de novo.
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(100));
        h.step();
    }
    assert!(doc(&h).editor.hints_open());
    assert_eq!(count(&h), Some(4));
    assert_eq!(h.state().assist_requests(), 1);
}

/// Com a lista aberta, levar o cursor para outro comando (um clique no
/// texto fecha a lista) só então troca as sugestões.
#[test]
fn an_open_list_is_not_replaced_under_the_reader() {
    let (url, _seen) = fake_provider(vec![answer(), answer()]);
    let app = ShellApp::default().with_ai(ai_at(&url, AssistMode::Auto));
    let mut h = paused(app, Duration::from_millis(300));
    h.event(Event::Text("select * from pedidos p".into()));
    h.step();
    wait(&mut h, "no suggestions", |a| {
        a.ai_hints(a.active_document().unwrap()).is_some()
    });
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.step();
    h.step();
    assert!(doc(&h).editor.hints_open());
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(100));
        h.step();
    }
    assert!(doc(&h).editor.hints_open());
    assert_eq!(h.state().assist_requests(), 1);
}
