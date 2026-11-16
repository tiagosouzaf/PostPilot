//! Parar o mouse no `*` de um `SELECT` mostra as colunas que ele
//! representa; a lista cabe na janela, por menor que ela seja.

use eframe::egui::{self, Id, Pos2, Rect};
use egui_kittest::Harness;
use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_sql_editor::{EditorState, SqlEditor};

/// `lotes` com `n` colunas de nomes compridos, e `fazendas` com duas.
fn catalog(n: usize) -> Catalog {
    let col = |name: String, ty: &str| Column {
        name,
        ty: ty.into(),
    };
    Catalog {
        engine: Default::default(),
        database: "loja".into(),
        search_path: vec!["public".into()],
        schemas: vec![Schema {
            name: "public".into(),
            relations: vec![
                Relation {
                    name: "lotes".into(),
                    kind: RelationKind::Table,
                    columns: (0..n)
                        .map(|i| {
                            col(
                                format!("quantidade_de_animais_na_entrada_{i:02}"),
                                "numeric(12,2)",
                            )
                        })
                        .collect(),
                },
                Relation {
                    name: "fazendas".into(),
                    kind: RelationKind::Table,
                    columns: vec![col("id".into(), "integer"), col("nome".into(), "text")],
                },
            ],
            functions: Vec::new(),
        }],
    }
}

struct Doc {
    catalog: Catalog,
    text: String,
    state: EditorState,
    rect: Rect,
}

fn harness(text: &str, columns: usize, size: [f32; 2]) -> Harness<'static, Doc> {
    let doc = Doc {
        catalog: catalog(columns),
        text: text.to_string(),
        state: EditorState::default(),
        rect: Rect::NOTHING,
    };
    let mut h = Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, doc: &mut Doc| {
                let out = SqlEditor::new(&mut doc.text, &mut doc.state, Id::new("editor"))
                    .catalog(Some(&doc.catalog))
                    .show(ui);
                doc.rect = out.response.rect;
            },
            doc,
        );
    h.run();
    h
}

/// Onde está o caractere `col` da primeira linha: o editor usa uma fonte
/// de largura fixa, e a margem dos números vem antes do texto.
fn char_pos(h: &Harness<'static, Doc>, col: usize) -> Pos2 {
    let font = egui::TextStyle::Monospace.resolve(&h.ctx.global_style());
    let (char_w, line_h) = h
        .ctx
        .fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let rect = h.state().rect;
    // Três dígitos de número de linha + 16 de margem + 4 até o texto.
    let left = rect.left() + char_w * 3.0 + 16.0 + 4.0;
    Pos2::new(
        left + (col as f32 + 0.5) * char_w,
        rect.top() + line_h * 0.5,
    )
}

/// Para o ponteiro no caractere `col` e espera a lista aparecer.
fn hover(h: &mut Harness<'static, Doc>, col: usize) {
    let pos = char_pos(h, col);
    h.hover_at(pos);
    h.run_steps(40);
}

fn names(h: &Harness<'static, Doc>) -> Vec<String> {
    let (star, _) = h.state().state.star_tip().expect("lista das colunas");
    star.tables.iter().map(|t| t.name.clone()).collect()
}

#[test]
fn hovering_the_star_lists_the_columns() {
    let mut h = harness("select * from lotes l where l.", 5, [800.0, 400.0]);
    assert!(h.state().state.star_tip().is_none());
    hover(&mut h, 7);
    assert_eq!(names(&h), ["lotes"]);
    let (star, rect) = h.state().state.star_tip().unwrap();
    assert_eq!(star.column_count(), 5);
    // Embaixo do asterisco.
    assert!(rect.top() > char_pos(&h, 7).y);
    // Fora do asterisco, some.
    hover(&mut h, 3);
    assert!(h.state().state.star_tip().is_none());
}

#[test]
fn the_list_waits_for_the_pointer_to_rest() {
    let mut h = harness("select * from lotes", 5, [800.0, 400.0]);
    let pos = char_pos(&h, 7);
    h.hover_at(pos);
    h.run_steps(3);
    assert!(h.state().state.star_tip().is_none(), "apareceu de passagem");
    h.run_steps(40);
    assert!(h.state().state.star_tip().is_some());
}

#[test]
fn a_qualified_star_lists_one_table() {
    let sql = "select f.*, l.* from lotes l join fazendas f on true";
    let mut h = harness(sql, 3, [800.0, 400.0]);
    hover(&mut h, 9);
    assert_eq!(names(&h), ["fazendas"]);
    hover(&mut h, 14);
    assert_eq!(names(&h), ["lotes"]);
}

#[test]
fn a_multiplication_shows_nothing() {
    let mut h = harness("select 2 * 3, count(*) from lotes", 3, [800.0, 400.0]);
    hover(&mut h, 9);
    assert!(h.state().state.star_tip().is_none());
    hover(&mut h, 20);
    assert!(h.state().state.star_tip().is_none());
}

#[test]
fn typing_or_clicking_hides_the_list() {
    let mut h = harness("select * from lotes", 5, [800.0, 400.0]);
    hover(&mut h, 7);
    assert!(h.state().state.star_tip().is_some());
    // Com o botão apertado (uma seleção começando), a lista sai.
    let pos = char_pos(&h, 7);
    h.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.run_steps(2);
    assert!(h.state().state.star_tip().is_none());
}

#[test]
fn dozens_of_long_columns_fit_in_the_window() {
    // Do tamanho de um monitor ao de um ladrilho estreito.
    for size in [
        [1400.0, 900.0],
        [800.0, 400.0],
        [640.0, 300.0],
        [360.0, 240.0],
        [220.0, 160.0],
    ] {
        let mut h = harness("select * from lotes l join fazendas f on true", 80, size);
        hover(&mut h, 7);
        let (star, rect) = h
            .state()
            .state
            .star_tip()
            .unwrap_or_else(|| panic!("{size:?}: sem a lista"));
        assert_eq!(star.column_count(), 82);
        let screen = h.ctx.content_rect();
        assert!(
            screen.expand(0.5).contains_rect(rect),
            "{size:?}: a lista {rect:?} sai da janela {screen:?}"
        );
    }
}
