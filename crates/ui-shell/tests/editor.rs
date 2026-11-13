//! O editor de SQL dentro da casca: atalhos de comentar, busca, Go To Line
//! e a posição do cursor na status bar.

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::panels::editor_id;

fn harness() -> Harness<'static, ShellApp> {
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), ShellApp::default())
}

fn press(harness: &mut Harness<'static, ShellApp>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run();
}

/// Abre uma consulta com `text` e devolve o número dela.
fn query_with(harness: &mut Harness<'static, ShellApp>, text: &str) -> u32 {
    press(harness, Modifiers::CTRL, Key::N);
    harness.event(Event::Text(text.to_string()));
    harness.run();
    harness.state().active_document().unwrap()
}

fn text_of(harness: &Harness<'static, ShellApp>, id: u32) -> String {
    harness.state().documents().get(id).unwrap().text.clone()
}

fn select(harness: &mut Harness<'static, ShellApp>, id: u32, range: (usize, usize)) {
    pgstudio_sql_editor::set_selection(&harness.ctx, editor_id(id), range);
    harness.run();
}

#[test]
fn ctrl_k_ctrl_c_comments_and_ctrl_k_ctrl_u_uncomments() {
    let mut harness = harness();
    let id = query_with(&mut harness, "select 1\nfrom t\nwhere x");
    select(&mut harness, id, (0, 12));
    press(&mut harness, Modifiers::CTRL, Key::K);
    press(&mut harness, Modifiers::CTRL, Key::C);
    assert_eq!(text_of(&harness, id), "-- select 1\n-- from t\nwhere x");
    assert!(harness.state().documents().get(id).unwrap().is_dirty());

    press(&mut harness, Modifiers::CTRL, Key::K);
    press(&mut harness, Modifiers::CTRL, Key::U);
    assert_eq!(text_of(&harness, id), "select 1\nfrom t\nwhere x");
}

#[test]
fn edit_menu_comment_works_on_the_cursor_line() {
    let mut harness = harness();
    let id = query_with(&mut harness, "a\nb\nc");
    select(&mut harness, id, (3, 3));
    harness.get_by_label("Edit").click();
    harness.run();
    harness.get(by().label_contains("Advanced")).hover();
    harness.run();
    harness
        .get(by().label_contains("Comment Selection"))
        .click();
    harness.run();
    assert_eq!(text_of(&harness, id), "a\n-- b\nc");
}

#[test]
fn ctrl_f_finds_the_next_match_and_says_when_there_is_none() {
    let mut harness = harness();
    let id = query_with(&mut harness, "select a from t;\nselect b from u;");
    select(&mut harness, id, (0, 0));
    press(&mut harness, Modifiers::CTRL, Key::F);
    let doc = harness.state().documents().get(id).unwrap();
    assert!(doc.editor.is_find_open());
    harness.get_by_label("Find:");

    harness.event(Event::Text("from".into()));
    harness.run();
    harness.get_by_label("Find Next").click();
    harness.run();
    let editor = editor_id(id);
    assert_eq!(
        pgstudio_sql_editor::selection(&harness.ctx, editor),
        Some((9, 13))
    );
    harness.get_by_label("Find Next").click();
    harness.run();
    assert_eq!(
        pgstudio_sql_editor::selection(&harness.ctx, editor),
        Some((26, 30))
    );

    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .editor
        .set_find_text("nothing here");
    harness.run();
    harness.get_by_label("Find Next").click();
    harness.run();
    assert!(
        harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .editor
            .find_failed()
    );
    harness.get_by_label("The text was not found.");

    harness.get(by().label_contains("✕")).click();
    harness.run();
    assert!(
        !harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .editor
            .is_find_open()
    );
}

#[test]
fn ctrl_f_starts_with_the_selected_word() {
    let mut harness = harness();
    let id = query_with(&mut harness, "select total from orders");
    select(&mut harness, id, (7, 12));
    press(&mut harness, Modifiers::CTRL, Key::F);
    let doc = harness.state().documents().get(id).unwrap();
    assert_eq!(doc.editor.find_text(), Some("total"));
}

#[test]
fn ctrl_g_goes_to_a_line_and_the_status_bar_follows_the_cursor() {
    let mut harness = harness();
    let id = query_with(&mut harness, "one\ntwo\nthree\nfour");
    harness.get(by().label("Ln 4   Col 5"));

    press(&mut harness, Modifiers::CTRL, Key::G);
    assert!(
        harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .editor
            .is_goto_open()
    );
    harness.get_by_label("Line number (1 - 4):");
    harness.event(Event::Text("9".into()));
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();
    harness.get(by().label_contains("between 1 and 4"));

    // Corrige para a linha 3.
    let state = &mut harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .editor;
    assert!(state.is_goto_open());
    harness.key_press(Key::Backspace);
    harness.event(Event::Text("3".into()));
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    harness.run();
    assert!(
        !harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .editor
            .is_goto_open()
    );
    let editor = editor_id(id);
    assert_eq!(
        pgstudio_sql_editor::selection(&harness.ctx, editor),
        Some((8, 8))
    );
    harness.get(by().label("Ln 3   Col 1"));
}

#[test]
fn editor_commands_without_a_query_explain_why() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::F);
    assert_eq!(harness.state().status(), "No query window is open.");
}

#[test]
fn typing_in_a_five_thousand_line_script_stays_responsive() {
    let mut harness = harness();
    let line = "SELECT o.id, o.total FROM loja.orders o WHERE o.total > 100; -- fim\n";
    let id = query_with(&mut harness, &line.repeat(5000));
    select(&mut harness, id, (0, 0));
    harness.run();
    // Cada tecla é um frame: o tempo dele é o que a pessoa sente.
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    for _ in 0..20 {
        harness.event(Event::Text("x".into()));
        let start = std::time::Instant::now();
        harness.step();
        let elapsed = start.elapsed();
        worst = worst.max(elapsed);
        total += elapsed;
    }
    harness.run();
    eprintln!(
        "tecla num script de 5000 linhas: média {:?}, pior {worst:?}",
        total / 20
    );
    assert!(text_of(&harness, id).starts_with(&"x".repeat(20)));
    // O editor monta só as linhas visíveis; o custo que cresce com o
    // arquivo é refazer os tokens e o índice de linhas. Em release a tecla
    // fica bem abaixo de um frame (16 ms); o limite é folgado para o build
    // de teste.
    assert!(worst < std::time::Duration::from_millis(500), "{worst:?}");
}
