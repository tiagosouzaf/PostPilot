//! O widget do editor sozinho: digitação, seleção, desfazer, indentação,
//! mouse e rolagem.

use eframe::egui::{self, Event, Id, Key, Modifiers, PointerButton, Pos2, Rect, Vec2};
use egui_kittest::Harness;
use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_sql_editor::{EditorState, SqlEditor, selection, set_selection};

fn catalog() -> Catalog {
    let rel = |name: &str, cols: &[&str]| Relation {
        name: name.into(),
        kind: RelationKind::Table,
        columns: cols
            .iter()
            .map(|c| Column {
                name: (*c).into(),
                ty: "integer".into(),
            })
            .collect(),
    };
    Catalog {
        engine: Default::default(),
        database: "loja".into(),
        search_path: vec!["public".into()],
        schemas: vec![Schema {
            name: "public".into(),
            relations: vec![
                rel("cliente", &["id", "nome"]),
                rel("pedido", &["id", "cliente_id", "total"]),
            ],
            functions: Vec::new(),
        }],
    }
}

struct Doc {
    catalog: Option<Catalog>,
    text: String,
    state: EditorState,
    rect: Rect,
    cursor: (usize, usize),
}

fn id() -> Id {
    Id::new("editor")
}

fn harness(text: &str) -> Harness<'static, Doc> {
    let mut state = EditorState::default();
    state.focus_editor();
    let doc = Doc {
        catalog: Some(catalog()),
        text: text.to_string(),
        state,
        rect: Rect::NOTHING,
        cursor: (1, 1),
    };
    let mut h = Harness::builder()
        .with_size([800.0, 400.0])
        // Frames de 60 Hz: com o padrão (0,25 s) dois cliques seguidos
        // passam do tempo de um duplo clique.
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(120)
        .build_ui_state(
            |ui, doc: &mut Doc| {
                let out = SqlEditor::new(&mut doc.text, &mut doc.state, id())
                    .catalog(doc.catalog.as_ref())
                    .show(ui);
                doc.rect = out.response.rect;
                doc.cursor = out.cursor.unwrap();
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

fn sel(h: &Harness<'static, Doc>) -> (usize, usize) {
    selection(&h.ctx, id()).unwrap()
}

fn place(h: &mut Harness<'static, Doc>, at: (usize, usize)) {
    set_selection(&h.ctx, id(), at);
    h.run();
}

fn click(h: &mut Harness<'static, Doc>, pos: Pos2, count: usize) {
    // Espera o bastante para não emendar com o clique anterior.
    h.run_steps(40);
    h.event(Event::PointerMoved(pos));
    h.step();
    for _ in 0..count {
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
    h.run();
}

/// Centro do caractere `col` da linha `line` (a partir de 0), com a fonte
/// monoespaçada de 13 pt do editor.
fn char_pos(h: &mut Harness<'static, Doc>, line: usize, col: usize) -> Pos2 {
    let font = egui::FontId::monospace(13.0);
    let (w, row) = h
        .ctx
        .fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let digits = h.state().text.lines().count().to_string().len().max(3);
    let left = h.state().rect.left() + w * digits as f32 + 16.0 + 4.0;
    Pos2::new(
        left + w * (col as f32 + 0.25),
        h.state().rect.top() + row * (line as f32 + 0.5),
    )
}

#[test]
fn typing_enter_keeps_the_indent_and_backspace_deletes() {
    let mut h = harness("");
    typed(&mut h, "select");
    key(&mut h, Modifiers::NONE, Key::Enter);
    typed(&mut h, "  a,");
    key(&mut h, Modifiers::NONE, Key::Enter);
    typed(&mut h, "b");
    assert_eq!(h.state().text, "select\n  a,\n  b");
    assert_eq!(h.state().cursor, (3, 4));
    key(&mut h, Modifiers::NONE, Key::Backspace);
    key(&mut h, Modifiers::CTRL, Key::Backspace);
    assert_eq!(h.state().text, "select\n  a,\n");
}

#[test]
fn undo_takes_back_a_word_at_a_time_and_redo_brings_it_back() {
    let mut h = harness("");
    typed(&mut h, "select 1 from t");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "select 1 from");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "select 1");
    key(&mut h, Modifiers::CTRL, Key::Y);
    key(&mut h, Modifiers::CTRL | Modifiers::SHIFT, Key::Z);
    assert_eq!(h.state().text, "select 1 from t");
    assert_eq!(sel(&h), (15, 15));
}

#[test]
fn arrows_home_end_and_shift_select() {
    let mut h = harness("    select a\nfrom t");
    place(&mut h, (8, 8));
    key(&mut h, Modifiers::NONE, Key::Home);
    assert_eq!(sel(&h), (4, 4), "Home vai ao primeiro caractere");
    key(&mut h, Modifiers::NONE, Key::Home);
    assert_eq!(sel(&h), (0, 0), "o segundo Home vai à coluna 0");
    key(&mut h, Modifiers::SHIFT, Key::End);
    assert_eq!(sel(&h), (0, 12));
    key(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(h.state().cursor, (2, 7), "a linha de baixo é mais curta");
    key(&mut h, Modifiers::CTRL | Modifiers::SHIFT, Key::ArrowLeft);
    assert_eq!(sel(&h), (19, 18));
    key(&mut h, Modifiers::CTRL, Key::A);
    assert_eq!(sel(&h), (0, 19));
    key(&mut h, Modifiers::CTRL, Key::Home);
    key(&mut h, Modifiers::CTRL, Key::ArrowRight);
    assert_eq!(sel(&h), (4, 4), "Ctrl+→ pula os espaços");
}

#[test]
fn cut_and_paste() {
    let mut h = harness("select a, b");
    place(&mut h, (7, 10));
    h.event(Event::Cut);
    h.run();
    assert_eq!(h.state().text, "select b");
    place(&mut h, (8, 8));
    h.event(Event::Paste(", c\r\n".into()));
    h.run();
    assert_eq!(h.state().text, "select b, c\n");
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "select b");
}

#[test]
fn tab_indents_lines_and_shift_tab_takes_it_back() {
    let mut h = harness("a\nb\nc");
    place(&mut h, (1, 1));
    key(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(
        h.state().text,
        "a   \nb\nc",
        "Tab vai até a próxima coluna de 4"
    );
    key(&mut h, Modifiers::CTRL, Key::Z);
    place(&mut h, (0, 3));
    key(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(h.state().text, "    a\n    b\nc");
    key(&mut h, Modifiers::SHIFT, Key::Tab);
    assert_eq!(h.state().text, "a\nb\nc");
}

#[test]
fn clicks_place_the_cursor_and_double_click_selects_a_word() {
    let mut h = harness("select total from orders\nwhere x");
    let p = char_pos(&mut h, 1, 3);
    click(&mut h, p, 1);
    assert_eq!(h.state().cursor, (2, 4));
    let p = char_pos(&mut h, 0, 9);
    click(&mut h, p, 2);
    assert_eq!(sel(&h), (7, 12));
    let p = char_pos(&mut h, 0, 2);
    click(&mut h, p, 3);
    assert_eq!(sel(&h), (0, 25), "o triplo clique pega a linha com o \\n");
}

#[test]
fn moving_to_the_end_of_a_long_script_scrolls_to_it() {
    let text = "select 1;\n".repeat(5000);
    let mut h = harness(&text);
    key(&mut h, Modifiers::CTRL, Key::End);
    assert_eq!(h.state().cursor, (5001, 1));
    // A última linha está visível: um clique perto do fundo cai nela.
    let rect = h.state().rect;
    click(&mut h, Pos2::new(rect.center().x, rect.bottom() - 20.0), 1);
    assert!(h.state().cursor.0 > 4990, "{:?}", h.state().cursor);
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, 100_000.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    // A rolagem da roda é suave: leva alguns frames.
    h.run_steps(120);
    click(&mut h, Pos2::new(rect.center().x, rect.top() + 5.0), 1);
    assert_eq!(h.state().cursor.0, 1, "a roda volta ao começo");
}

fn labels(h: &Harness<'static, Doc>) -> Vec<String> {
    h.state().state.completion_labels()
}

#[test]
fn typing_after_from_lists_tables_and_tab_accepts() {
    let mut h = harness("");
    typed(&mut h, "select * from p");
    assert_eq!(labels(&h), ["pedido", "public"]);
    typed(&mut h, "e");
    assert_eq!(labels(&h), ["pedido"]);
    key(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(h.state().text, "select * from pedido");
    assert!(!h.state().state.is_completing());
    typed(&mut h, " p where p.");
    assert_eq!(labels(&h), ["cliente_id", "id", "total"]);
    key(&mut h, Modifiers::NONE, Key::ArrowDown);
    key(&mut h, Modifiers::NONE, Key::ArrowDown);
    key(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(h.state().text, "select * from pedido p where p.total");
    // Uma linha nova não veio junto com o Enter que escolheu.
    assert_eq!(h.state().cursor, (1, 37));
}

#[test]
fn escape_and_space_close_the_list() {
    let mut h = harness("select * from cliente c where c.");
    place(&mut h, (33, 33));
    key(&mut h, Modifiers::CTRL, Key::J);
    assert_eq!(labels(&h), ["id", "nome"]);
    key(&mut h, Modifiers::NONE, Key::Escape);
    assert!(!h.state().state.is_completing());
    typed(&mut h, "n");
    assert_eq!(labels(&h), ["nome"]);
    typed(&mut h, " ");
    assert!(!h.state().state.is_completing());
    assert_eq!(h.state().text, "select * from cliente c where c.n ");
}

#[test]
fn ctrl_space_completes_a_single_match_directly() {
    let mut h = harness("select c.no from cliente c");
    place(&mut h, (11, 11));
    key(&mut h, Modifiers::CTRL, Key::Space);
    assert_eq!(h.state().text, "select c.nome from cliente c");
    assert!(!h.state().state.is_completing());
    key(&mut h, Modifiers::CTRL, Key::Z);
    assert_eq!(h.state().text, "select c.no from cliente c");
}

#[test]
fn backspace_updates_the_list() {
    let mut h = harness("");
    typed(&mut h, "select * from pedido where tot");
    assert_eq!(labels(&h), ["total"]);
    key(&mut h, Modifiers::NONE, Key::Backspace);
    key(&mut h, Modifiers::NONE, Key::Backspace);
    assert!(labels(&h).contains(&"total".to_string()));
    assert!(labels(&h).contains(&"TO".to_string()));
}

#[test]
fn clicking_a_suggestion_inserts_it() {
    let mut h = harness("");
    typed(&mut h, "select * from c");
    assert_eq!(labels(&h), ["cliente"]);
    // A lista fica embaixo da linha 1, começando na palavra.
    let p = char_pos(&mut h, 1, 16);
    click(&mut h, Pos2::new(p.x, p.y + 3.0), 1);
    assert_eq!(h.state().text, "select * from cliente");
}

#[test]
fn the_selected_suggestion_is_explained_under_the_list() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness("");
    typed(&mut h, "sel");
    assert_eq!(labels(&h), ["SELECT"]);
    h.get_by_label("Reads rows: chooses the columns and expressions of the result.");
    // Uma tabela diz o schema e as colunas dela.
    let mut h = harness("");
    typed(&mut h, "select * from cli");
    h.get_by_label_contains("Table of the schema public.");
    // A explicação acompanha a seta.
    let mut h = harness("");
    typed(&mut h, "select * from pedido where t");
    h.get_by_label("Column of pedido, of type integer.");
    key(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert!(
        h.query_by_label("Column of pedido, of type integer.")
            .is_none()
    );
}

#[test]
fn syntax_errors_are_underlined_while_they_last() {
    let mut h = harness("");
    typed(&mut h, "select f(1");
    assert_eq!(h.state().state.errors().len(), 1);
    typed(&mut h, ")");
    assert!(h.state().state.errors().is_empty());
    typed(&mut h, " 'abc");
    assert_eq!(
        h.state().state.errors()[0].1,
        "Unclosed quotation mark after the character string."
    );
    // O mouse em cima mostra a mensagem.
    let p = char_pos(&mut h, 0, 14);
    h.event(Event::PointerMoved(p));
    h.run_steps(30);
    use egui_kittest::kittest::Queryable;
    h.get_by_label("Unclosed quotation mark after the character string.");
}
