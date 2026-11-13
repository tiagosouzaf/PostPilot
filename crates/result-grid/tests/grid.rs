//! A grade com o mouse e o teclado, e o desempenho com 1 milhão de linhas.

use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use pgstudio_result_grid::{Cell, GridState, Mode, ResultStore, grid};

struct App {
    store: ResultStore,
    state: GridState,
    copied: Option<String>,
}

fn harness(store: ResultStore) -> Harness<'static, App> {
    Harness::builder().with_size([900.0, 500.0]).build_ui_state(
        |ui, app: &mut App| {
            let out = grid::show(ui, ui.id().with("grid"), &mut app.store, &mut app.state);
            if out.copied.is_some() {
                app.copied = out.copied;
            }
        },
        App {
            store,
            state: GridState::default(),
            copied: None,
        },
    )
}

fn small() -> ResultStore {
    let mut s = ResultStore::new(vec!["id".into(), "nome".into(), "email".into()]);
    for (id, nome, email) in [
        ("1", "Ana", Some("ana@x")),
        ("2", "Bruno", None),
        ("3", "Carla", Some("carla@x")),
    ] {
        s.push(&[Some(id.into()), Some(nome.into()), email.map(String::from)]);
    }
    s
}

fn click(h: &mut Harness<'static, App>, pos: eframe::egui::Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.run();
    h.event_modifiers(
        eframe::egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
        modifiers,
    );
    h.event_modifiers(
        eframe::egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
        modifiers,
    );
    h.run();
}

#[test]
fn clicks_select_cells_rows_and_columns_and_copy_works() {
    let mut h = harness(small());
    h.run();
    let at = |h: &Harness<'static, App>, row, col| {
        h.state().state.cell_center(Cell { row, col }).unwrap()
    };

    let p = at(&h, 0, 1);
    click(&mut h, p, Modifiers::NONE);
    let sel = h.state().state.selection.unwrap();
    assert_eq!(
        (sel.anchor, sel.cursor),
        (Cell { row: 0, col: 1 }, Cell { row: 0, col: 1 })
    );

    // Shift+clique estende o retângulo.
    let p = at(&h, 1, 2);
    click(&mut h, p, Modifiers::SHIFT);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    assert_eq!(
        h.state().copied.as_deref(),
        Some("Ana\tana@x\nBruno\tNULL\n")
    );
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::C);
    h.run();
    assert_eq!(
        h.state().copied.as_deref(),
        Some("nome\temail\nAna\tana@x\nBruno\tNULL\n")
    );

    // Número da linha seleciona a linha toda; cabeçalho, a coluna toda.
    let p = h.state().state.row_header_center(2).unwrap();
    click(&mut h, p, Modifiers::NONE);
    let sel = h.state().state.selection.unwrap();
    assert_eq!(sel.mode, Mode::Rows);
    let mut app_state = h.state().state.clone();
    assert_eq!(
        app_state.copy(&mut h.state_mut().store, false).as_deref(),
        Some("3\tCarla\tcarla@x\n")
    );
    let p = h.state().state.header_center(0).unwrap();
    click(&mut h, p, Modifiers::NONE);
    app_state = h.state().state.clone();
    assert_eq!(
        app_state.copy(&mut h.state_mut().store, false).as_deref(),
        Some("1\n2\n3\n")
    );

    // Ctrl+A seleciona tudo.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    let sel = h.state().state.selection.unwrap();
    assert_eq!(sel.rows(3), 0..3);
    assert_eq!(sel.cols(3), 0..3);
}

#[test]
fn arrows_move_the_cursor_and_shift_extends() {
    let mut h = harness(small());
    h.run();
    let p = h
        .state()
        .state
        .cell_center(Cell { row: 0, col: 0 })
        .unwrap();
    click(&mut h, p, Modifiers::NONE);
    h.key_press(Key::ArrowDown);
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(
        h.state().state.selection.unwrap().cursor,
        Cell { row: 1, col: 1 }
    );
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    h.run();
    let sel = h.state().state.selection.unwrap();
    assert_eq!(sel.anchor, Cell { row: 1, col: 1 });
    assert_eq!(sel.cursor, Cell { row: 2, col: 1 });
    h.key_press_modifiers(Modifiers::COMMAND, Key::Home);
    h.run();
    assert_eq!(
        h.state().state.selection.unwrap().cursor,
        Cell { row: 0, col: 0 }
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::End);
    h.run();
    assert_eq!(
        h.state().state.selection.unwrap().cursor,
        Cell { row: 2, col: 2 }
    );
}

#[test]
fn dragging_a_header_border_resizes_the_column() {
    let mut h = harness(small());
    h.run();
    let before = h.state().state.widths[1];
    let from = h.state().state.column_border(1).unwrap();
    h.drag_at(from);
    h.run();
    h.hover_at(from + eframe::egui::vec2(60.0, 0.0));
    h.run();
    h.drop_at(from + eframe::egui::vec2(60.0, 0.0));
    h.run();
    let after = h.state().state.widths[1];
    assert!((after - before - 60.0).abs() < 2.0, "{before} -> {after}");
    // Arrastar a borda não muda a seleção.
    assert_eq!(h.state().state.selection, None);
}

#[test]
fn a_million_rows_scroll_within_a_frame() {
    let start = Instant::now();
    let mut store = ResultStore::with_budget(
        ["id", "cliente", "email", "total", "status", "criado_em"]
            .map(String::from)
            .to_vec(),
        32 * 1024 * 1024,
    );
    for i in 0..1_000_000u64 {
        store.push(&[
            Some(i.to_string()),
            Some(format!("Cliente número {}", i % 9973)),
            (i % 17 != 0).then(|| format!("cliente{i}@exemplo.com.br")),
            Some(format!("{}.{:02}", i % 5000, i % 100)),
            Some(["aberto", "pago", "enviado"][(i % 3) as usize].into()),
            Some("2026-11-13 22:59:10.123-03".into()),
        ]);
    }
    let filled = start.elapsed();
    assert_eq!(store.len(), 1_000_000);
    assert!(store.is_spilled());
    let resident = store.resident_bytes();
    assert!(resident < 40 * 1024 * 1024, "{resident}");

    let mut h = harness(store);
    h.run();
    let mut worst = Duration::ZERO;
    // Saltos para longe (páginas no disco) e rolagem contínua.
    for top in [0.0, 500_000.0, 999_990.0, 123_456.0, 10.0, 750_000.0] {
        h.state_mut().state.top = top;
        let t = Instant::now();
        h.step();
        worst = worst.max(t.elapsed());
    }
    for _ in 0..30 {
        h.state_mut().state.top += 3.5;
        let t = Instant::now();
        h.step();
        worst = worst.max(t.elapsed());
    }
    let first = h.state().state.top.floor() as u64;
    let id = h.state_mut().store.cell(first, 0).map(str::to_string);
    assert_eq!(id, Some(first.to_string()));
    eprintln!(
        "1 milhão de linhas: preenchido em {filled:?}, {} MB em memória, pior frame {worst:?}",
        resident / (1024 * 1024)
    );
    // Folgado para build de teste; em release fica abaixo de 16 ms.
    assert!(worst < Duration::from_millis(250), "{worst:?}");
}
