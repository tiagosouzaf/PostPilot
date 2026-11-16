//! Vários cursores e seleção em coluna no editor.

use eframe::egui::{self, Event, Id, Key, Modifiers, PointerButton, Pos2, Rect};
use egui_kittest::Harness;
use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_sql_editor::{
    EditorState, SqlEditor, selected_text, selection, selections, set_selection,
};

const ALT_SHIFT: Modifiers = Modifiers {
    alt: true,
    shift: true,
    ..Modifiers::NONE
};

struct Doc {
    catalog: Catalog,
    text: String,
    state: EditorState,
    rect: Rect,
}

fn id() -> Id {
    Id::new("editor")
}

fn harness(text: &str) -> Harness<'static, Doc> {
    let mut state = EditorState::default();
    state.focus_editor();
    let catalog = Catalog {
        engine: Default::default(),
        database: "loja".into(),
        search_path: vec!["public".into()],
        schemas: vec![Schema {
            name: "public".into(),
            relations: vec![Relation {
                name: "cliente".into(),
                kind: RelationKind::Table,
                columns: vec![Column {
                    name: "id".into(),
                    ty: "integer".into(),
                }],
            }],
            functions: Vec::new(),
        }],
    };
    let doc = Doc {
        catalog,
        text: text.to_string(),
        state,
        rect: Rect::NOTHING,
    };
    let mut h = Harness::builder()
        .with_size([800.0, 400.0])
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(120)
        .build_ui_state(
            |ui, doc: &mut Doc| {
                let out = SqlEditor::new(&mut doc.text, &mut doc.state, id())
                    .catalog(Some(&doc.catalog))
                    .show(ui);
                doc.rect = out.response.rect;
            },
            doc,
        );
    h.run();
    h
}

fn key(h: &mut Harness<'static, Doc>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.run();
}

fn typed(h: &mut Harness<'static, Doc>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.run();
    }
}

fn place(h: &mut Harness<'static, Doc>, at: (usize, usize)) {
    set_selection(&h.ctx, id(), at);
    h.run();
}

fn sels(h: &Harness<'static, Doc>) -> Vec<(usize, usize)> {
    selections(&h.ctx, id())
}

fn char_pos(h: &mut Harness<'static, Doc>, line: usize, col: usize) -> Pos2 {
    let font = egui::FontId::monospace(13.0);
    let (w, row) = h
        .ctx
        .fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let left = h.state().rect.left() + w * 3.0 + 16.0 + 4.0;
    Pos2::new(
        left + w * (col as f32 + 0.25),
        h.state().rect.top() + row * (line as f32 + 0.5),
    )
}

fn button(h: &mut Harness<'static, Doc>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    });
    h.run();
}

/// Clica (ou arrasta até `to`) segurando `modifiers`.
fn drag(h: &mut Harness<'static, Doc>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.run_steps(40);
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(from));
    h.run();
    button(h, from, true, modifiers);
    if to != from {
        h.event(Event::PointerMoved(to));
        h.run();
    }
    button(h, to, false, modifiers);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run();
}

/// Três cursores, um no fim de cada linha de "a\nb\nc".
fn three_cursors() -> Harness<'static, Doc> {
    let mut h = harness("a\nb\nc");
    place(&mut h, (1, 1));
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    assert_eq!(sels(&h), [(1, 1), (3, 3), (5, 5)]);
    h
}

#[test]
fn typing_and_deleting_happen_at_every_cursor() {
    let mut h = three_cursors();
    typed(&mut h, "xy");
    assert_eq!(h.state().text, "axy\nbxy\ncxy");
    assert_eq!(sels(&h), [(3, 3), (7, 7), (11, 11)]);
    key(&mut h, Modifiers::NONE, Key::Backspace);
    assert_eq!(h.state().text, "ax\nbx\ncx");
    key(&mut h, Modifiers::NONE, Key::Home);
    key(&mut h, Modifiers::NONE, Key::Delete);
    assert_eq!(h.state().text, "x\nx\nx");
    key(&mut h, Modifiers::NONE, Key::End);
    key(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(h.state().text, "x\n\nx\n\nx\n");
}

#[test]
fn undo_takes_back_all_cursors_in_one_step() {
    let mut h = three_cursors();
    typed(&mut h, "xy z");
    assert_eq!(h.state().text, "axy z\nbxy z\ncxy z");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "axy\nbxy\ncxy", "uma palavra por vez");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "a\nb\nc");
    assert_eq!(sels(&h), [(1, 1), (3, 3), (5, 5)], "os cursores voltam");
    key(&mut h, Modifiers::CTRL, Key::Y);
    assert_eq!(h.state().text, "axy\nbxy\ncxy");
    assert_eq!(sels(&h), [(3, 3), (7, 7), (11, 11)]);
    // Continua editando nos três depois de refazer.
    typed(&mut h, "!");
    assert_eq!(h.state().text, "axy!\nbxy!\ncxy!");
    key(&mut h, Modifiers::CTRL, Key::Z);
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "a\nb\nc");
}

#[test]
fn alt_shift_arrows_select_a_column_block() {
    let mut h = harness("abcdef\nab\nabcdef\nabcdef");
    place(&mut h, (1, 1));
    for k in [
        Key::ArrowRight,
        Key::ArrowRight,
        Key::ArrowRight,
        Key::ArrowDown,
        Key::ArrowDown,
    ] {
        key(&mut h, ALT_SHIFT, k);
    }
    // A linha curta fica com o que tem; o bloco continua na coluna depois.
    assert_eq!(sels(&h), [(1, 4), (8, 9), (11, 14)]);
    assert_eq!(selection(&h.ctx, id()), Some((11, 14)), "a principal");
    assert_eq!(
        selected_text(&h.ctx, id(), &h.state().text).as_deref(),
        Some("bcd\nb\nbcd")
    );
    h.event(Event::Copy);
    h.step();
    let copied = h
        .output()
        .platform_output
        .commands
        .iter()
        .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "bcd\nb\nbcd"));
    assert!(copied, "copia uma linha por seleção");
    // Encolher o bloco de volta deixa um cursor só.
    key(&mut h, ALT_SHIFT, Key::ArrowUp);
    key(&mut h, ALT_SHIFT, Key::ArrowUp);
    assert_eq!(sels(&h), [(1, 4)]);
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    typed(&mut h, "-");
    assert_eq!(h.state().text, "a-ef\na-\na-ef\nabcdef");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "abcdef\nab\nabcdef\nabcdef");
}

#[test]
fn a_block_skips_lines_that_do_not_reach_it() {
    let mut h = harness("abcdef\n\nabcdef");
    place(&mut h, (3, 3));
    key(&mut h, ALT_SHIFT, Key::ArrowRight);
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    assert_eq!(sels(&h), [(3, 4), (11, 12)]);
}

#[test]
fn paste_gives_one_line_to_each_cursor() {
    let mut h = three_cursors();
    h.event(Event::Paste("1\n2\n3".into()));
    h.run();
    assert_eq!(h.state().text, "a1\nb2\nc3");
    h.event(Event::Paste("-".into()));
    h.run();
    assert_eq!(h.state().text, "a1-\nb2-\nc3-");
    key(&mut h, Modifiers::CTRL, Key::Z);
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "a\nb\nc");
}

#[test]
fn cut_removes_every_selection() {
    let mut h = three_cursors();
    key(&mut h, Modifiers::SHIFT, Key::Home);
    assert_eq!(sels(&h), [(1, 0), (3, 2), (5, 4)]);
    h.event(Event::Cut);
    h.run();
    assert_eq!(h.state().text, "\n\n");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "a\nb\nc");
}

#[test]
fn escape_and_plain_moves_go_back_to_one_cursor() {
    let mut h = three_cursors();
    key(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(sels(&h), [(5, 5)]);
    typed(&mut h, "!");
    assert_eq!(h.state().text, "a\nb\nc!", "o editor continua com o foco");

    let mut h = three_cursors();
    key(&mut h, Modifiers::CTRL, Key::Home);
    assert_eq!(sels(&h), [(0, 0)]);

    // Cursores que se encontram viram um.
    let mut h = three_cursors();
    key(&mut h, Modifiers::CTRL, Key::A);
    assert_eq!(sels(&h), [(0, 5)]);
    let mut h = harness("ab");
    place(&mut h, (0, 0));
    let (a, b) = (char_pos(&mut h, 0, 1), char_pos(&mut h, 0, 2));
    drag(&mut h, a, a, Modifiers::CTRL);
    drag(&mut h, b, b, Modifiers::CTRL);
    assert_eq!(sels(&h).len(), 3);
    key(&mut h, Modifiers::NONE, Key::Home);
    assert_eq!(sels(&h), [(0, 0)]);
}

#[test]
fn ctrl_click_adds_and_removes_cursors() {
    let mut h = harness("select a\nselect b\nselect c");
    let p = char_pos(&mut h, 0, 6);
    drag(&mut h, p, p, Modifiers::NONE);
    let p = char_pos(&mut h, 2, 6);
    drag(&mut h, p, p, Modifiers::CTRL);
    assert_eq!(sels(&h), [(6, 6), (24, 24)]);
    assert_eq!(
        selection(&h.ctx, id()),
        Some((24, 24)),
        "o último é o principal"
    );
    typed(&mut h, "!");
    assert_eq!(h.state().text, "select! a\nselect b\nselect! c");
    // Ctrl+clique num cursor que já existe tira esse cursor.
    let p = char_pos(&mut h, 0, 7);
    drag(&mut h, p, p, Modifiers::CTRL);
    assert_eq!(sels(&h), [(26, 26)]);
    // Um clique sem Ctrl volta a um cursor só.
    let p = char_pos(&mut h, 1, 0);
    drag(&mut h, p, p, Modifiers::CTRL);
    assert_eq!(sels(&h).len(), 2);
    let p = char_pos(&mut h, 1, 2);
    drag(&mut h, p, p, Modifiers::NONE);
    assert_eq!(sels(&h), [(12, 12)]);
}

#[test]
fn alt_drag_selects_a_column_block() {
    let mut h = harness("abcdef\nabcdef\nabcdef");
    let (from, to) = (char_pos(&mut h, 0, 1), char_pos(&mut h, 2, 4));
    drag(&mut h, from, to, Modifiers::ALT);
    assert_eq!(sels(&h), [(1, 4), (8, 11), (15, 18)]);
    key(&mut h, Modifiers::NONE, Key::Delete);
    assert_eq!(h.state().text, "aef\naef\naef");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "abcdef\nabcdef\nabcdef");
}

#[test]
fn shift_alt_period_adds_the_next_match() {
    let mut h = harness("total, x, total, total");
    place(&mut h, (2, 2));
    let chord = Modifiers {
        alt: true,
        shift: true,
        ..Modifiers::NONE
    };
    key(&mut h, chord, Key::Period);
    assert_eq!(sels(&h), [(0, 5)], "primeiro seleciona a palavra");
    key(&mut h, chord, Key::Period);
    key(&mut h, chord, Key::Period);
    assert_eq!(sels(&h), [(0, 5), (10, 15), (17, 22)]);
    key(&mut h, chord, Key::Period);
    assert_eq!(sels(&h).len(), 3, "não há mais ocorrências");
    typed(&mut h, "soma");
    assert_eq!(h.state().text, "soma, x, soma, soma");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "total, x, total, total");
}

#[test]
fn intellisense_completes_at_every_cursor() {
    let mut h = harness("select * from \nselect * from ");
    place(&mut h, (14, 14));
    key(&mut h, ALT_SHIFT, Key::ArrowDown);
    assert_eq!(sels(&h), [(14, 14), (29, 29)]);
    typed(&mut h, "cli");
    assert!(h.state().state.is_completing());
    key(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(
        h.state().text,
        "select * from cliente\nselect * from cliente"
    );
}

#[test]
fn a_change_from_outside_leaves_one_cursor() {
    let mut h = three_cursors();
    // Formatar, comentar e o Claude trocam o texto do documento por fora.
    h.state_mut().text = "select 1".into();
    h.run();
    assert_eq!(sels(&h).len(), 1);
    typed(&mut h, "!");
    assert_eq!(h.state().text.matches('!').count(), 1);
}
