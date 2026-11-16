//! Capturas de tela das sugestões da IA no editor, em cada banco. As
//! respostas em `tests/assist/*.json` são do Claude Code de verdade, para
//! a pergunta que o PostPilot monta para cada comando. Só com
//! `--features screenshots`; os PNGs vão para `PGSTUDIO_SHOTS` (ou a
//! pasta temporária).
#![cfg(feature = "screenshots")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Id, Key, Modifiers};
use egui_kittest::Harness;
use pgstudio_automation::assist;
use pgstudio_automation::llm::{Provider, ProviderKind};
use pgstudio_sql_editor::hints::{Hint, HintAction, HintKind, HintStatus, Hints};
use pgstudio_sql_editor::lexer::Dialect;
use pgstudio_sql_editor::{Colors, EditorState, SqlEditor};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::ai::{Ai, AiSettings, AssistMode};

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    dir.join(name)
}

fn hints(answer: &str, statement: &str) -> Vec<Hint> {
    assist::parse(answer, statement)
        .expect("answer")
        .into_iter()
        .map(|s| Hint {
            kind: HintKind::parse(&s.kind),
            title: s.title,
            detail: s.detail,
            action: match s.action {
                assist::Action::Replace => HintAction::Replace(s.code),
                assist::Action::Insert => HintAction::Insert(s.code),
                assist::Action::None => HintAction::None,
            },
        })
        .collect()
}

struct Doc {
    text: String,
    state: EditorState,
}

/// O editor sozinho no dialeto, com a lista aberta na sugestão `down`.
fn shot(dialect: Dialect, statement: &str, answer: &str, down: usize, name: &str) {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut state = EditorState::default();
    state.set_dialect(dialect);
    state.set_hint_shortcut("Ctrl+.");
    state.set_hints(Some(Hints {
        items: hints(answer, statement),
        range: 0..statement.len(),
        base: statement.to_string(),
        source: "Claude Code".into(),
    }));
    state.focus_editor();
    let doc = Doc {
        text: statement.to_string(),
        state,
    };
    let mut h = Harness::builder()
        .with_size([820.0, 520.0])
        .with_pixels_per_point(2.0)
        .build_ui_state(
            |ui, doc: &mut Doc| {
                egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                    SqlEditor::new(&mut doc.text, &mut doc.state, Id::new("editor"))
                        .colors(Colors::DARK)
                        .show(ui);
                });
            },
            doc,
        );
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::End);
    h.run();
    h.state_mut().state.open_hints();
    h.run();
    for _ in 0..down {
        h.key_press(Key::ArrowDown);
        h.run();
    }
    assert!(h.state().state.hints_open(), "{name}: lista fechada");
    h.run_steps(3);
    h.render()
        .expect("render")
        .save(out(name))
        .expect("save png");
}

#[test]
fn each_engine() {
    shot(
        Dialect::Postgres,
        "select * from pedidos p where p.total > 100",
        include_str!("assist/pg.json"),
        1,
        "postgresql.png",
    );
    shot(
        Dialect::TSql,
        "SELECT * FROM Pedidos WHERE YEAR(CriadoEm) = 2026",
        include_str!("assist/mssql.json"),
        0,
        "sqlserver.png",
    );
    shot(
        Dialect::MongoDb,
        "db.pedidos.find({ status: \"pago\" })",
        include_str!("assist/mongo.json"),
        2,
        "mongodb.png",
    );
    shot(
        Dialect::Redis,
        "KEYS sessao:*",
        include_str!("assist/redis.json"),
        0,
        "redis.png",
    );
    shot(
        Dialect::Elasticsearch,
        "GET /pedidos/_search\n{\n  \"query\": { \"match\": { \"status\": \"pago\" } }\n}",
        include_str!("assist/es.json"),
        0,
        "elasticsearch.png",
    );
}

/// O aviso no canto, sem a lista: lendo, pronto e sem resposta.
#[test]
fn corner_notice() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let statement = "select * from pedidos p where p.total > 100";
    for (name, status, ready) in [
        ("aviso-lendo.png", HintStatus::Thinking, false),
        ("aviso-pronto.png", HintStatus::Idle, true),
    ] {
        let mut state = EditorState::default();
        state.set_hint_shortcut("Ctrl+.");
        state.set_hint_status(status);
        if ready {
            state.set_hints(Some(Hints {
                items: hints(include_str!("assist/pg.json"), statement),
                range: 0..statement.len(),
                base: statement.to_string(),
                source: "Claude Code".into(),
            }));
        }
        state.focus_editor();
        let doc = Doc {
            text: statement.to_string(),
            state,
        };
        let mut h = Harness::builder()
            .with_size([820.0, 200.0])
            .with_pixels_per_point(2.0)
            .build_ui_state(
                |ui, doc: &mut Doc| {
                    egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                        SqlEditor::new(&mut doc.text, &mut doc.state, Id::new("editor"))
                            .colors(Colors::DARK)
                            .show(ui);
                    });
                },
                doc,
            );
        pgstudio_ui_shell::install_fonts(&h.ctx);
        pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
        h.run();
        h.key_press_modifiers(Modifiers::CTRL, Key::End);
        h.run_steps(3);
        h.render()
            .expect("render")
            .save(out(name))
            .expect("save png");
    }
}

/// Um provedor de mentira que responde uma vez com `reply`.
fn fake_provider(reply: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
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
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let body = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices":[{"delta":{"content":reply}}]})
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body.as_bytes());
    });
    url
}

/// A janela inteira: a consulta digitada, a resposta chegando pelo
/// provedor e a lista aberta com Ctrl+.
#[test]
fn whole_window() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let url = fake_provider(include_str!("assist/pg.json").to_string());
    let mut provider = Provider::new(ProviderKind::OpenRouter, "openrouter-1");
    provider.base_url = url;
    let ai = Ai::with(
        AiSettings {
            active: "openrouter-1".into(),
            providers: vec![provider],
            assist: AssistMode::Auto,
        },
        &[("openrouter-1", "sk-test")],
    );
    let app = ShellApp::default()
        .with_ai(ai)
        .with_assist_pause(Duration::ZERO);
    let mut h = Harness::builder()
        .with_size([1280.0, 760.0])
        .with_pixels_per_point(1.5)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    h.event(Event::Text(
        "-- pedidos grandes\nselect * from pedidos p where p.total > 100".into(),
    ));
    h.run();
    let start = Instant::now();
    loop {
        h.step();
        let app = h.state();
        if app
            .ai_hints(app.active_document().unwrap())
            .is_some_and(|hints| !hints.items.is_empty())
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "no suggestions");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h.render()
        .expect("render")
        .save(out("janela-aviso.png"))
        .expect("save png");
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run_steps(3);
    h.render()
        .expect("render")
        .save(out("janela-lista.png"))
        .expect("save png");
}

/// A janela com o Claude Code de verdade, quando `PGSTUDIO_CLAUDE` aponta
/// para ele: a pergunta sai da janela e a resposta dele vira a lista.
#[test]
fn real_claude_code() {
    let Some(claude) = std::env::var_os("PGSTUDIO_CLAUDE").map(PathBuf::from) else {
        return;
    };
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut ai = Ai::default();
    ai.set_program(Some(claude));
    let app = ShellApp::default()
        .with_ai(ai)
        .with_assist_pause(Duration::ZERO);
    let mut h = Harness::builder()
        .with_size([1280.0, 760.0])
        .with_pixels_per_point(1.5)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    h.event(Event::Text(
        "select c.nome, count(*) from clientes c, pedidos p\nwhere c.id = p.cliente_id and extract(year from p.criado_em) = 2026\ngroup by c.nome".into(),
    ));
    h.run();
    let start = Instant::now();
    loop {
        h.step();
        let app = h.state();
        let id = app.active_document().unwrap();
        if app.ai_hints(id).is_some() {
            break;
        }
        let status = app
            .documents()
            .get(id)
            .unwrap()
            .editor
            .hint_status()
            .clone();
        assert!(
            !matches!(status, HintStatus::Failed(_)),
            "{status:?} {:?}",
            app.output()
        );
        assert!(start.elapsed() < Duration::from_secs(120), "no answer");
        std::thread::sleep(Duration::from_millis(20));
    }
    let app = h.state();
    let found = app.ai_hints(app.active_document().unwrap()).unwrap();
    println!("answer after {:?}", start.elapsed());
    for hint in &found.items {
        println!("- [{}] {} :: {}", hint.kind.id(), hint.title, hint.detail);
    }
    assert!(!found.items.is_empty());
    h.key_press_modifiers(Modifiers::CTRL, Key::Period);
    h.run_steps(3);
    h.render()
        .expect("render")
        .save(out("claude-code-de-verdade.png"))
        .expect("save png");
}
