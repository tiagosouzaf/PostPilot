//! Editores por tipo na grade: o calendário das colunas de data e hora e
//! a lista de valores das booleanas, com o mouse e o teclado.

use eframe::egui::{self, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_result_grid::{Cell, ColumnKind, GridState, ResultStore, grid};

struct App {
    store: ResultStore,
    state: GridState,
    commits: Vec<(Cell, Option<String>)>,
}

/// `id`, `criado` (timestamptz que aceita NULL), `ativo` (boolean que
/// aceita NULL), `dia` (date NOT NULL) e `certo` (boolean NOT NULL).
fn harness(size: [f32; 2]) -> Harness<'static, App> {
    let mut store = ResultStore::new(
        ["id", "criado", "ativo", "dia", "certo"]
            .map(String::from)
            .to_vec(),
    );
    let row = |v: [Option<&str>; 5]| v.map(|c| c.map(String::from)).to_vec();
    store.push(&row([
        Some("1"),
        Some("2026-11-12 10:46:35.187386+00"),
        Some("t"),
        Some("2026-11-12"),
        Some("f"),
    ]));
    store.push(&row([Some("2"), None, None, Some("2024-02-29"), Some("t")]));
    let pg = |name: &str, nullable| ColumnKind::from_type(false, name, nullable, &[]);
    let state = GridState {
        editable: vec![false, true, true, true, true],
        kinds: vec![
            ColumnKind::default(),
            pg("timestamp with time zone", true),
            pg("boolean", true),
            pg("date", false),
            pg("boolean", false),
        ],
        ..GridState::default()
    };
    Harness::builder().with_size(size).build_ui_state(
        |ui, app: &mut App| {
            let out = grid::show(ui, ui.id().with("grid"), &mut app.store, &mut app.state);
            if let Some(c) = out.commit {
                app.commits.push(c);
            }
        },
        App {
            store,
            state,
            commits: Vec::new(),
        },
    )
}

fn open(h: &mut Harness<'static, App>, row: u64, col: usize) {
    h.run();
    let pos = h.state().state.cell_center(Cell { row, col }).unwrap();
    // O harness entrega cada clique num frame, com o relógio andando
    // 0,25 s: os dois cliques contam como um duplo clique, e os frames de
    // antes separam este de um clique anterior do teste.
    h.ctx
        .options_mut(|o| o.input_options.max_double_click_delay = 0.6);
    h.hover_at(pos);
    for _ in 0..4 {
        h.step();
    }
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
    h.run();
    h.run();
    assert!(h.state().state.editing.is_some(), "not editing");
}

fn text(h: &Harness<'static, App>) -> String {
    h.state()
        .state
        .editing
        .as_ref()
        .expect("editing")
        .text
        .clone()
}

#[test]
fn a_day_from_the_calendar_keeps_the_time_the_microseconds_and_the_zone() {
    let mut h = harness([900.0, 600.0]);
    open(&mut h, 0, 1);
    // O calendário abre no mês do valor, com o dia dele marcado.
    assert!(h.query_by_label("November 2026").is_some());
    h.get_by_label("2026-11-30").click();
    h.run();
    assert_eq!(text(&h), "2026-11-30 10:46:35.187386+00");
    assert!(h.state().commits.is_empty(), "a click does not save");
    // O teclado voltou para a célula: Enter grava.
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    assert_eq!(
        h.state().commits,
        vec![(
            Cell { row: 0, col: 1 },
            Some("2026-11-30 10:46:35.187386+00".into())
        )]
    );
}

#[test]
fn the_arrows_change_month_and_year_without_changing_the_value() {
    let mut h = harness([900.0, 600.0]);
    open(&mut h, 0, 3);
    assert!(h.query_by_label("November 2026").is_some());
    h.get_by_label(egui_phosphor::regular::CARET_RIGHT).click();
    h.run();
    assert!(h.query_by_label("December 2026").is_some());
    h.get_by_label(egui_phosphor::regular::CARET_RIGHT).click();
    h.run();
    assert!(h.query_by_label("January 2027").is_some());
    h.get_by_label(egui_phosphor::regular::CARET_DOUBLE_LEFT)
        .click();
    h.run();
    assert!(h.query_by_label("January 2026").is_some());
    assert_eq!(text(&h), "2026-11-12");
    h.get_by_label("2026-01-15").click();
    h.run();
    assert_eq!(text(&h), "2026-01-15");
    // OK grava o que está na célula.
    h.get_by_label("OK").click();
    h.run();
    h.run();
    assert_eq!(
        h.state().commits,
        vec![(Cell { row: 0, col: 3 }, Some("2026-01-15".into()))]
    );
}

#[test]
fn typing_in_the_cell_moves_the_calendar() {
    let mut h = harness([900.0, 600.0]);
    open(&mut h, 0, 3);
    for _ in 0..10 {
        h.key_press(Key::Backspace);
    }
    h.event(egui::Event::Text("2024-02-29".into()));
    h.run();
    h.run();
    assert!(h.query_by_label("February 2024").is_some());
    // Texto que não é data continua sendo só texto: o calendário espera.
    h.event(egui::Event::Text("x".into()));
    h.run();
    assert_eq!(text(&h), "2024-02-29x");
    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    assert!(h.state().commits.is_empty());
}

#[test]
fn null_is_offered_only_where_the_column_accepts_it() {
    let mut h = harness([900.0, 600.0]);
    open(&mut h, 0, 3);
    assert!(h.query_by_label("NULL").is_none(), "date is NOT NULL");
    h.get_by_label("Cancel").click();
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());

    open(&mut h, 0, 1);
    h.get_by_label("NULL").click();
    h.run();
    h.run();
    assert_eq!(h.state().commits, vec![(Cell { row: 0, col: 1 }, None)]);
}

#[test]
fn an_empty_cell_starts_at_today_and_now_fills_it() {
    let mut h = harness([900.0, 600.0]);
    open(&mut h, 1, 1);
    assert_eq!(text(&h), "");
    h.get_by_label("Now").click();
    h.run();
    let value = text(&h);
    let kind = h.state().state.kind(1).kind;
    let stamp = pgstudio_result_grid::typed::Stamp::parse(&kind, &value).expect("a timestamp");
    assert!(!stamp.offset.is_empty(), "{value}");
}

#[test]
fn a_boolean_is_picked_from_a_list() {
    let mut h = harness([900.0, 600.0]);
    open(&mut h, 0, 2);
    for label in ["true", "false", "NULL"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    h.get_by_label("false").click();
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    assert_eq!(
        h.state().commits,
        vec![(Cell { row: 0, col: 2 }, Some("f".into()))]
    );

    // NOT NULL: sem o NULL na lista.
    open(&mut h, 0, 4);
    assert!(h.query_by_label("NULL").is_none());
    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    assert_eq!(h.state().commits.len(), 1);
}

#[test]
fn the_keyboard_picks_from_the_list() {
    let mut h = harness([900.0, 600.0]);
    // A célula é NULL: a lista abre no NULL; seta para cima vai a false.
    open(&mut h, 1, 2);
    h.key_press(Key::ArrowUp);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(
        h.state().commits,
        vec![(Cell { row: 1, col: 2 }, Some("f".into()))]
    );
    // A seleção da grade não andou com a seta.
    assert_eq!(
        h.state().state.selection.unwrap().cursor,
        Cell { row: 1, col: 2 }
    );

    // Uma letra vai para o valor que começa com ela; o mesmo valor não
    // grava nada.
    open(&mut h, 0, 4);
    h.event(egui::Event::Text("t".into()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().commits.len(), 2);
    assert_eq!(
        h.state().commits[1],
        (Cell { row: 0, col: 4 }, Some("t".into()))
    );
    open(&mut h, 0, 2);
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    assert_eq!(h.state().commits.len(), 2, "t stays t");
}

#[test]
fn the_editors_stay_inside_a_small_window() {
    for size in [[320.0, 260.0], [240.0, 200.0]] {
        let mut h = harness(size);
        open(&mut h, 0, 1);
        let screen = h.ctx.content_rect();
        let day = h.get_by_label("2026-11-12").rect();
        assert!(screen.contains_rect(day), "{size:?}: {day:?}");
        h.key_press(Key::Escape);
        h.run();
        h.run();
        // A coluna booleana fica fora da janela: rola até ela.
        let state = &mut h.state_mut().state;
        state.left = state.widths[..2].iter().sum();
        open(&mut h, 0, 2);
        let item = h.get_by_label("false").rect();
        assert!(screen.contains_rect(item), "{size:?}: {item:?}");
    }
}
