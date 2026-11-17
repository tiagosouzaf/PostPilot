//! A grade com o mouse e o teclado, e o desempenho com 1 milhão de linhas.

use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_result_grid::{Cell, GridState, Mode, ResultStore, grid};

struct App {
    store: ResultStore,
    state: GridState,
    copied: Option<String>,
    open_json: Option<Cell>,
}

fn harness(store: ResultStore) -> Harness<'static, App> {
    Harness::builder()
        .with_size([900.0, 500.0])
        // Frames de 60 Hz: com o padrão (0,25 s) dois cliques seguidos
        // passam do tempo de um duplo clique.
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut App| {
                let out = grid::show(ui, ui.id().with("grid"), &mut app.store, &mut app.state);
                if out.copied.is_some() {
                    app.copied = out.copied;
                }
                if out.open_json.is_some() {
                    app.open_json = out.open_json;
                }
            },
            App {
                store,
                state: GridState::default(),
                copied: None,
                open_json: None,
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

fn json_store() -> ResultStore {
    let mut s = ResultStore::new(vec!["id".into(), "data".into(), "tags".into()]);
    for i in 0..3 {
        s.push(&[
            Some(i.to_string()),
            Some(r#"{"a": [1, 2], "b": "x"}"#.into()),
            Some("{a,b}".into()),
        ]);
    }
    s
}

/// `count` cliques no mesmo frame, em `pos`.
fn clicks(h: &mut Harness<'static, App>, pos: eframe::egui::Pos2, count: usize) {
    h.run_steps(40);
    h.hover_at(pos);
    h.step();
    for _ in 0..count {
        for pressed in [true, false] {
            h.event(eframe::egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            h.step();
        }
    }
    h.run();
}

/// O que a grade pediu, frame a frame, numa grade editável.
#[derive(Default)]
struct Edits {
    commits: Vec<(Cell, Option<String>)>,
    deletes: Vec<std::ops::Range<u64>>,
    refused: usize,
}

struct EditApp {
    store: ResultStore,
    state: GridState,
    edits: Edits,
}

/// Grade de `small()` com `id` e `nome` editáveis e `email` como JSON.
fn edit_harness(editable: Vec<bool>) -> Harness<'static, EditApp> {
    let state = GridState {
        editable,
        json_columns: vec![false, false, true],
        ..GridState::default()
    };
    Harness::builder().with_size([900.0, 500.0]).build_ui_state(
        |ui, app: &mut EditApp| {
            let out = grid::show(ui, ui.id().with("grid"), &mut app.store, &mut app.state);
            if let Some(c) = out.commit {
                app.edits.commits.push(c);
            }
            if let Some(d) = out.delete_rows {
                app.edits.deletes.push(d);
            }
            app.edits.refused += usize::from(out.edit_refused);
        },
        EditApp {
            store: small(),
            state,
            edits: Edits::default(),
        },
    )
}

fn double_click(h: &mut Harness<'static, EditApp>, pos: eframe::egui::Pos2) {
    // O harness entrega cada clique num frame, com o relógio andando.
    h.ctx
        .options_mut(|o| o.input_options.max_double_click_delay = 10.0);
    h.hover_at(pos);
    h.run();
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(eframe::egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }
    h.run();
}

#[test]
fn json_cells_open_by_double_click_button_and_enter() {
    let mut h = harness(json_store());
    h.run();
    let s = &h.state().state;
    assert_eq!(
        [s.is_json(0), s.is_json(1), s.is_json(2)],
        [false, true, false]
    );
    // Fora das colunas JSON o duplo clique não pede nada.
    let p = h
        .state()
        .state
        .cell_center(Cell { row: 1, col: 2 })
        .unwrap();
    clicks(&mut h, p, 2);
    assert_eq!(h.state().open_json, None);
    let p = h
        .state()
        .state
        .cell_center(Cell { row: 1, col: 1 })
        .unwrap();
    clicks(&mut h, p, 2);
    assert_eq!(h.state().open_json, Some(Cell { row: 1, col: 1 }));

    // O botão {…} da célula selecionada abre com um clique só.
    h.state_mut().open_json = None;
    let b = h
        .state()
        .state
        .json_button(Cell { row: 1, col: 1 })
        .unwrap();
    clicks(&mut h, b.center(), 1);
    assert_eq!(h.state().open_json, Some(Cell { row: 1, col: 1 }));

    // Enter na célula com o cursor.
    h.state_mut().open_json = None;
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().open_json, Some(Cell { row: 2, col: 1 }));
}

fn cell_at(h: &Harness<'static, EditApp>, row: u64, col: usize) -> eframe::egui::Pos2 {
    h.state().state.cell_center(Cell { row, col }).unwrap()
}

#[test]
fn double_click_edits_in_place_and_enter_commits() {
    let mut h = edit_harness(vec![true, true, false]);
    h.run();
    let p = cell_at(&h, 1, 1);
    double_click(&mut h, p);
    let editing = h.state().state.editing.clone().expect("editing");
    assert_eq!(editing.cell, Cell { row: 1, col: 1 });
    assert_eq!(editing.text, "Bruno");
    // Apaga o texto e digita outro.
    for _ in 0..5 {
        h.key_press(Key::Backspace);
    }
    h.event(eframe::egui::Event::Text("Beto".into()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    assert_eq!(
        h.state().edits.commits,
        vec![(Cell { row: 1, col: 1 }, Some("Beto".into()))]
    );
}

#[test]
fn escape_cancels_and_unchanged_text_commits_nothing() {
    let mut h = edit_harness(vec![true, true, false]);
    h.run();
    let p = cell_at(&h, 0, 1);
    double_click(&mut h, p);
    h.event(eframe::egui::Event::Text("xyz".into()));
    h.run();
    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_none());
    // F2 edita a célula do cursor; Enter sem mudar não grava nada.
    h.key_press(Key::F2);
    h.run();
    h.run();
    assert!(h.state().state.editing.is_some());
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.state().edits.commits.is_empty());
}

#[test]
fn read_only_columns_and_json_do_not_edit() {
    // `id` só leitura: dois cliques avisam o motivo.
    let mut h = edit_harness(vec![false, true, false]);
    h.run();
    let p = cell_at(&h, 0, 0);
    double_click(&mut h, p);
    assert!(h.state().state.editing.is_none());
    assert_eq!(h.state().edits.refused, 1);
    // Coluna JSON: o duplo clique é do visualizador, sem edição nem aviso.
    let p = cell_at(&h, 0, 2);
    double_click(&mut h, p);
    assert!(h.state().state.editing.is_none());
    assert_eq!(h.state().edits.refused, 1);
}

#[test]
fn delete_asks_for_the_selected_rows_but_not_whole_columns() {
    let mut h = edit_harness(vec![true, true, false]);
    h.run();
    let p = cell_at(&h, 0, 1);
    click_edit(&mut h, p, Modifiers::NONE);
    let p = cell_at(&h, 2, 1);
    click_edit(&mut h, p, Modifiers::SHIFT);
    h.key_press(Key::Delete);
    h.run();
    assert_eq!(h.state().edits.deletes, vec![0..3]);
    // Coluna inteira selecionada: Delete não pede nada.
    let header = h.state().state.header_center(1).unwrap();
    click_edit(&mut h, header, Modifiers::NONE);
    h.key_press(Key::Delete);
    h.run();
    assert_eq!(h.state().edits.deletes.len(), 1);
}

#[test]
fn a_read_only_grid_refuses_delete() {
    let mut h = edit_harness(Vec::new());
    h.run();
    let p = cell_at(&h, 0, 1);
    click_edit(&mut h, p, Modifiers::NONE);
    h.key_press(Key::Delete);
    h.run();
    assert!(h.state().edits.deletes.is_empty());
    assert_eq!(h.state().edits.refused, 1);
}

fn click_edit(h: &mut Harness<'static, EditApp>, pos: eframe::egui::Pos2, modifiers: Modifiers) {
    h.hover_at(pos);
    h.run();
    for pressed in [true, false] {
        h.event_modifiers(
            eframe::egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            },
            modifiers,
        );
    }
    h.run();
}

/// Com a grade rolada até o meio de uma linha, a linha cortada no topo fica
/// atrás do cabeçalho fixo: nenhum texto de dado pode ser desenhado na faixa
/// do cabeçalho (antes, "2365426 / 342" aparecia sobre "id / cliente_id").
#[test]
fn a_half_scrolled_row_does_not_paint_over_the_header() {
    let mut store = ResultStore::new(vec!["id".into(), "cliente_id".into()]);
    for i in 0..60u64 {
        store.push(&[Some(format!("v{i}")), Some(format!("w{i}"))]);
    }
    let mut h = harness(store);
    h.run();
    for top in [3.1, 3.5, 3.9, 0.25] {
        h.state_mut().state.top = top;
        h.run();
        let layout = h.state().state.layout.expect("layout");
        let output = h.output().clone();
        let mut seen = 0;
        for clipped in &output.shapes {
            let eframe::egui::Shape::Text(t) = &clipped.shape else {
                continue;
            };
            let text = t.galley.text();
            if !(text.starts_with('v') || text.starts_with('w')) {
                continue;
            }
            seen += 1;
            let painted = clipped.clip_rect.intersect(t.visual_bounding_rect());
            assert!(
                painted.height() <= 0.0 || painted.top() >= layout.header.bottom() - 0.01,
                "top={top}: \"{text}\" desenhado em {painted:?}, sobre o cabeçalho {:?}",
                layout.header
            );
        }
        assert!(seen > 0, "top={top}: nenhum valor desenhado");
    }
}

fn column(h: &mut Harness<'static, App>, col: usize) -> Vec<Option<String>> {
    let store = &mut h.state_mut().store;
    (0..store.len())
        .map(|r| store.cell(r, col).map(str::to_string))
        .collect()
}

fn texts(values: &[&str]) -> Vec<Option<String>> {
    values.iter().map(|v| Some(v.to_string())).collect()
}

/// Um clique no cabeçalho ordena: crescente, decrescente e de volta à
/// ordem em que as linhas chegaram. A coluna continua selecionada, para
/// copiar.
#[test]
fn a_header_click_cycles_the_sort() {
    let mut s = ResultStore::new(vec!["n".into(), "nome".into()]);
    for (n, nome) in [("10", "b"), ("9", "c"), ("100", "a")] {
        s.push(&[Some(n.into()), Some(nome.into())]);
    }
    let mut h = harness(s);
    h.run();
    let header = h.state().state.header_center(0).unwrap();
    click(&mut h, header, Modifiers::NONE);
    h.run();
    // Números ordenam como números, não como texto.
    assert_eq!(column(&mut h, 0), texts(&["9", "10", "100"]));
    assert_eq!(column(&mut h, 1), texts(&["c", "b", "a"]));
    assert_eq!(h.state().state.selection.unwrap().mode, Mode::Columns);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    assert_eq!(h.state().copied.as_deref(), Some("9\n10\n100\n"));

    click(&mut h, header, Modifiers::NONE);
    h.run();
    assert_eq!(column(&mut h, 0), texts(&["100", "10", "9"]));
    click(&mut h, header, Modifiers::NONE);
    h.run();
    assert_eq!(column(&mut h, 0), texts(&["10", "9", "100"]));
    assert_eq!(h.state().state.sort, None);

    // Shift+clique estende a seleção de colunas e não ordena.
    let other = h.state().state.header_center(1).unwrap();
    click(&mut h, other, Modifiers::SHIFT);
    h.run();
    assert_eq!(h.state().state.sort, None);
    assert_eq!(h.state().state.selection.unwrap().cols(2), 0..2);
}

/// O filtro mostra só as linhas com o texto, e a cópia e a seleção passam
/// a valer para o que está à mostra.
#[test]
fn the_filter_keeps_only_matching_rows() {
    let mut h = harness(small());
    h.run();
    h.state_mut().state.set_filter("x".into());
    h.run();
    assert_eq!(column(&mut h, 1), texts(&["Ana", "Carla"]));
    assert_eq!(h.state().store.total_len(), 3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    // O foco ainda não está na grade: clica numa célula e copia tudo.
    let p = h
        .state()
        .state
        .cell_center(Cell { row: 1, col: 1 })
        .unwrap();
    click(&mut h, p, Modifiers::NONE);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    assert_eq!(
        h.state().copied.as_deref(),
        Some("1\tAna\tana@x\n3\tCarla\tcarla@x\n")
    );
    // Trocar o filtro solta a seleção (as linhas mudaram de lugar).
    h.state_mut().state.set_filter("email:null".into());
    h.run();
    assert_eq!(column(&mut h, 1), texts(&["Bruno"]));
    assert_eq!(h.state().state.selection, None);
    // Filtro e ordem juntos; sem filtro volta tudo.
    h.state_mut().state.set_filter("a".into());
    h.state_mut().state.cycle_sort(1);
    h.state_mut().state.cycle_sort(1);
    h.run();
    assert_eq!(column(&mut h, 1), texts(&["Carla", "Ana"]));
    h.state_mut().state.set_filter(String::new());
    h.state_mut().state.sort = None;
    h.run();
    assert_eq!(column(&mut h, 1), texts(&["Ana", "Bruno", "Carla"]));
}

/// Digitar na caixa de filtro filtra; Esc limpa e devolve o foco à grade.
#[test]
fn typing_in_the_filter_box_filters_and_escape_clears() {
    let mut h = harness(small());
    h.run();
    h.get_by_role(eframe::egui::accesskit::Role::TextInput)
        .focus();
    h.run();
    h.event(eframe::egui::Event::Text("bru".into()));
    h.run();
    assert_eq!(h.state().state.filter, "bru");
    assert_eq!(column(&mut h, 1), texts(&["Bruno"]));
    h.get_by_label("1 of 3 rows");
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(h.state().state.filter, "");
    assert_eq!(column(&mut h, 0), texts(&["1", "2", "3"]));
}

/// Com ordem e filtro, a edição e a exclusão continuam apontando para a
/// linha que está na tela, e a visão não muda enquanto uma gravação está
/// em andamento.
#[test]
fn editing_and_deleting_follow_the_sorted_rows() {
    let mut h = edit_harness(vec![true, true, false]);
    h.state_mut().state.cycle_sort(1);
    h.state_mut().state.cycle_sort(1);
    h.run();
    // Decrescente por nome: Carla, Bruno, Ana.
    let p = cell_at(&h, 0, 1);
    double_click(&mut h, p);
    let editing = h.state().state.editing.clone().expect("editing");
    assert_eq!(editing.text, "Carla");
    h.event(eframe::egui::Event::Text("!".into()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(
        h.state().edits.commits,
        vec![(Cell { row: 0, col: 1 }, Some("Carla!".into()))]
    );
    // Quem grava troca o valor pela mesma posição: é a linha da Carla (a
    // terceira do resultado).
    h.state_mut().store.set_cell(0, 1, Some("Carla!".into()));
    h.state_mut().store.remove_row(1);
    h.state_mut().state.sort = None;
    h.run();
    let store = &mut h.state_mut().store;
    assert_eq!(store.len(), 2);
    assert_eq!(store.row(0)[1].as_deref(), Some("Ana"));
    assert_eq!(store.row(1)[1].as_deref(), Some("Carla!"));

    // Gravação em andamento: a ordem pedida espera.
    h.state_mut().state.view_locked = true;
    h.state_mut().state.cycle_sort(0);
    h.state_mut().state.cycle_sort(0);
    h.step();
    h.step();
    assert_eq!(h.state_mut().store.row(0)[1].as_deref(), Some("Ana"));
    h.state_mut().state.view_locked = false;
    h.run();
    assert_eq!(h.state_mut().store.row(0)[1].as_deref(), Some("Carla!"));
}

/// 400 mil linhas: ordenar e filtrar não seguram a janela (cada frame
/// continua curto enquanto a visão é montada) e o resultado sai certo.
#[test]
fn sorting_and_filtering_400k_rows_keeps_frames_short() {
    let n = 400_000u64;
    let mut store = ResultStore::with_budget(
        ["id", "cliente", "total", "status"]
            .map(String::from)
            .to_vec(),
        16 * 1024 * 1024,
    );
    for i in 0..n {
        store.push(&[
            Some(i.to_string()),
            Some(format!("Cliente número {}", (i * 7919) % 9973)),
            (i % 17 != 0).then(|| format!("{}.{:02}", (i * 31) % 5000, i % 100)),
            Some(["aberto", "pago", "enviado"][(i % 3) as usize].into()),
        ]);
    }
    assert!(store.is_spilled());
    let mut h = harness(store);
    h.run();

    let settle = |h: &mut Harness<'static, App>| -> (Duration, usize) {
        let mut worst = Duration::ZERO;
        let mut frames = 0;
        let started = Instant::now();
        loop {
            let t = Instant::now();
            h.step();
            worst = worst.max(t.elapsed());
            frames += 1;
            if !h.state().state.view_working {
                return (worst, frames);
            }
            assert!(started.elapsed() < Duration::from_secs(120), "não terminou");
            // A ordenação roda numa thread: dá tempo a ela.
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    h.state_mut().state.cycle_sort(2);
    let (sort_worst, sort_frames) = settle(&mut h);
    assert_eq!(h.state().store.len(), n);
    let total = |h: &mut Harness<'static, App>, row: u64| -> Option<f64> {
        h.state_mut().store.cell(row, 2).map(|v| v.parse().unwrap())
    };
    let mut last = f64::MIN;
    for row in (0..n - n / 17 - 1).step_by(997) {
        let v = total(&mut h, row).expect("os NULLs ficam no fim");
        assert!(v >= last, "linha {row}: {v} < {last}");
        last = v;
    }
    assert_eq!(total(&mut h, n - 1), None);

    h.state_mut()
        .state
        .set_filter("status:pago \"número 42\"".into());
    let (filter_worst, filter_frames) = settle(&mut h);
    let shown = h.state().store.len();
    assert!(shown > 0 && shown < n / 100, "{shown}");
    for row in [0, shown / 2, shown - 1] {
        let r = h.state_mut().store.row(row);
        assert_eq!(r[3].as_deref(), Some("pago"));
        assert!(r[1].as_deref().unwrap().contains("número 42"), "{r:?}");
    }
    eprintln!(
        "400 mil linhas: ordenar {sort_frames} frames (pior {sort_worst:?}), \
         filtrar {filter_frames} frames (pior {filter_worst:?}), {shown} linhas no filtro"
    );
    // Folgado para build de teste; o trabalho por frame é limitado a 6 ms.
    assert!(sort_worst < Duration::from_millis(250), "{sort_worst:?}");
    assert!(
        filter_worst < Duration::from_millis(250),
        "{filter_worst:?}"
    );
}

fn numbered(rows: u64) -> ResultStore {
    let mut s = ResultStore::new(vec!["n".into(), "nome".into()]);
    for i in 0..rows {
        s.push(&[Some(i.to_string()), Some(format!("linha {i}"))]);
    }
    s
}

/// Uma grade de resultado de consulta: com páginas e a faixa embaixo.
fn paged_harness(rows: u64) -> Harness<'static, App> {
    let mut h = harness(numbered(rows));
    h.state_mut().state.paged = true;
    h.run();
    h
}

fn corner(h: &Harness<'static, App>) -> eframe::egui::Pos2 {
    let l = h.state().state.layout.unwrap();
    eframe::egui::pos2(l.header.left() + 4.0, l.header.center().y)
}

/// Por padrão o resultado vem em páginas de 500 linhas; a faixa embaixo da
/// grade troca de página, e a seleção, a cópia e o teclado valem para a
/// página à mostra.
#[test]
fn results_come_in_pages_and_the_footer_turns_them() {
    use egui_phosphor::regular as icons;
    let mut h = paged_harness(1200);
    assert_eq!(h.state().state.shown, 0..500);
    h.get_by_label("1–500 of 1200");

    // O canto seleciona a página toda; o Ctrl+C copia só ela.
    let p = corner(&h);
    click(&mut h, p, Modifiers::NONE);
    let sel = h.state().state.selection.unwrap();
    assert_eq!(sel.rows(1200), 0..500);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    let copied = h.state().copied.clone().unwrap();
    assert_eq!(copied.lines().count(), 500);
    assert_eq!(copied.lines().last(), Some("499\tlinha 499"));

    // Próxima página: outras linhas, numeradas pelo resultado, sem seleção.
    h.get_by_label(icons::CARET_RIGHT).click();
    h.run();
    assert_eq!(h.state().state.shown, 500..1000);
    assert_eq!(h.state().state.top, 500.0);
    assert_eq!(h.state().state.selection, None);
    h.get_by_label("501–1000 of 1200");
    assert!(
        h.state()
            .state
            .cell_center(Cell { row: 500, col: 0 })
            .is_some()
    );

    // O cabeçalho seleciona a coluna da página.
    let p = h.state().state.header_center(1).unwrap();
    click(&mut h, p, Modifiers::SHIFT);
    h.run();
    let sel = h.state().state.selection.unwrap();
    assert_eq!(sel.mode, Mode::Columns);
    assert_eq!(sel.rows(1200), 500..1000);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    let copied = h.state().copied.clone().unwrap();
    assert_eq!(copied.lines().next(), Some("linha 500"));
    assert_eq!(copied.lines().count(), 500);

    // O teclado fica dentro da página; Alt+PageDown vai para a seguinte.
    let p = h
        .state()
        .state
        .cell_center(Cell { row: 500, col: 0 })
        .unwrap();
    click(&mut h, p, Modifiers::NONE);
    h.key_press(Key::ArrowUp);
    h.run();
    assert_eq!(h.state().state.selection.unwrap().cursor.row, 500);
    h.key_press_modifiers(Modifiers::COMMAND, Key::End);
    h.run();
    assert_eq!(h.state().state.selection.unwrap().cursor.row, 999);
    h.key_press_modifiers(Modifiers::ALT, Key::PageDown);
    h.run();
    assert_eq!(h.state().state.shown, 1000..1200);
    h.get_by_label("1001–1200 of 1200");
    // Voltar uma página pelo teclado para no começo dela, não no fim.
    h.key_press_modifiers(Modifiers::ALT, Key::PageUp);
    h.run();
    assert_eq!(h.state().state.shown, 500..1000);
    assert_eq!(h.state().state.top, 500.0);
    h.get_by_label(icons::CARET_DOUBLE_LEFT).click();
    h.run();
    assert_eq!(h.state().state.shown, 0..500);

    // Ordenar ou filtrar vale para o resultado todo e volta à página 1.
    h.get_by_label(icons::CARET_DOUBLE_RIGHT).click();
    h.run();
    assert_eq!(h.state().state.page, 2);
    // Um clique no cabeçalho ordena e mostra o começo da primeira página.
    let p = h.state().state.header_center(0).unwrap();
    click(&mut h, p, Modifiers::NONE);
    h.run();
    assert_eq!((h.state().state.page, h.state().state.top), (0, 0.0));
    h.get_by_label(icons::CARET_DOUBLE_RIGHT).click();
    h.run();
    h.state_mut().state.set_filter("linha 11".into());
    h.run();
    // As 120 linhas com "11" no número.
    assert_eq!(h.state().state.shown, 0..120);
    h.get_by_label("1–120 of 120");
}

/// O tamanho da página e "todas as linhas" valem para todas as grades, e
/// a primeira linha que estava na tela continua à mostra.
#[test]
fn the_page_size_is_shared_and_all_rows_turns_paging_off() {
    use pgstudio_result_grid::{GridPrefs, prefs};
    let mut h = paged_harness(1200);
    h.state_mut()
        .state
        .go_to_page(&GridPrefs::default(), 1200, 1);
    h.run();
    assert_eq!(h.state().state.shown, 500..1000);
    prefs::set(
        &h.ctx,
        GridPrefs {
            page_size: 100,
            ..GridPrefs::default()
        },
    );
    h.run();
    // A página 2 de 500 não existe mais como era: fica a página pedida.
    assert_eq!(h.state().state.shown, 100..200);
    prefs::set(
        &h.ctx,
        GridPrefs {
            paginate: false,
            ..GridPrefs::default()
        },
    );
    h.run();
    assert_eq!(h.state().state.shown, 0..1200);
    h.get_by_label("1200 rows");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();

    // Uma grade sem páginas (as outras telas) não muda com a escolha.
    let mut plain = harness(numbered(1200));
    plain.run();
    assert_eq!(plain.state().state.shown, 0..1200);
    assert!(plain.query_by_label("1–500 of 1200").is_none());
}

/// O Ctrl+C copia no formato escolhido embaixo da grade.
#[test]
fn ctrl_c_copies_in_the_chosen_format() {
    use pgstudio_result_grid::{CopyFormat, GridPrefs, prefs};
    let mut h = paged_harness(3);
    let p = corner(&h);
    click(&mut h, p, Modifiers::NONE);
    let copy = |h: &mut Harness<'static, App>, format, headers| {
        prefs::set(
            &h.ctx,
            GridPrefs {
                copy_format: format,
                copy_headers: headers,
                ..GridPrefs::default()
            },
        );
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::C);
        h.run();
        h.state().copied.clone().unwrap()
    };
    assert_eq!(
        copy(&mut h, CopyFormat::Csv, true),
        "n,nome\n0,linha 0\n1,linha 1\n2,linha 2\n"
    );
    assert_eq!(
        copy(&mut h, CopyFormat::CsvSemicolon, false),
        "0;linha 0\n1;linha 1\n2;linha 2\n"
    );
    assert_eq!(
        copy(&mut h, CopyFormat::Json, false),
        "[\n  {\"n\": 0, \"nome\": \"linha 0\"},\n  {\"n\": 1, \"nome\": \"linha 1\"},\n  \
         {\"n\": 2, \"nome\": \"linha 2\"}\n]\n"
    );
    assert_eq!(
        copy(&mut h, CopyFormat::Markdown, false),
        "| n | nome |\n| --- | --- |\n| 0 | linha 0 |\n| 1 | linha 1 |\n| 2 | linha 2 |\n"
    );
    // A faixa diz o que foi copiado.
    h.get_by_label("3 rows copied as Markdown");

    // Uma coluna como lista para o IN.
    let p = h.state().state.header_center(1).unwrap();
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(
        copy(&mut h, CopyFormat::InList, false),
        "'linha 0', 'linha 1', 'linha 2'\n"
    );
}
