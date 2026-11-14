//! A grade de resultados, como a do SSMS: número da linha, cabeçalho com
//! largura ajustável, NULL destacado, seleção de células, linhas ou colunas
//! e cópia com ou sem cabeçalho.
//!
//! A rolagem é própria, em linhas (não em pixels), para continuar precisa
//! com milhões de linhas: só as linhas visíveis são lidas do
//! [`ResultStore`] e desenhadas.

use std::ops::Range;

use eframe::egui::{self, Color32, CursorIcon, FontId, Key, Pos2, Rect, Sense, Stroke, vec2};

use crate::json;
use crate::store::ResultStore;
use crate::text;

/// Uma célula: linha e coluna, a partir de 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub row: u64,
    pub col: usize,
}

/// O que a seleção cobre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Retângulo de células.
    Cells,
    /// Linhas inteiras (clique no número da linha).
    Rows,
    /// Colunas inteiras (clique no cabeçalho).
    Columns,
}

/// Seleção da grade: do `anchor` até o `cursor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cell,
    pub cursor: Cell,
    pub mode: Mode,
}

impl Selection {
    fn at(cell: Cell, mode: Mode) -> Self {
        Selection {
            anchor: cell,
            cursor: cell,
            mode,
        }
    }

    /// Linhas selecionadas, num resultado de `rows` linhas.
    pub fn rows(&self, rows: u64) -> Range<u64> {
        match self.mode {
            Mode::Columns => 0..rows,
            _ => {
                let a = self.anchor.row.min(self.cursor.row);
                let b = self.anchor.row.max(self.cursor.row);
                a..(b + 1).min(rows)
            }
        }
    }

    /// Colunas selecionadas, num resultado de `cols` colunas.
    pub fn cols(&self, cols: usize) -> Range<usize> {
        match self.mode {
            Mode::Rows => 0..cols,
            _ => {
                let a = self.anchor.col.min(self.cursor.col);
                let b = self.anchor.col.max(self.cursor.col);
                a..(b + 1).min(cols)
            }
        }
    }

    fn contains(&self, cell: Cell, rows: u64, cols: usize) -> bool {
        self.rows(rows).contains(&cell.row) && self.cols(cols).contains(&cell.col)
    }
}

/// Estado da grade de um resultado, guardado entre frames.
#[derive(Clone, Debug, Default)]
pub struct GridState {
    /// Largura de cada coluna, em pontos. Vazio: calcula pelo conteúdo.
    pub widths: Vec<f32>,
    /// Primeira linha visível (com fração, para rolagem suave).
    pub top: f64,
    /// Deslocamento horizontal, em pontos.
    pub left: f32,
    pub selection: Option<Selection>,
    /// Geometria do último frame, para achar células pela posição.
    pub layout: Option<Layout>,
    /// Colunas reconhecidas como JSON pela amostra das primeiras linhas.
    pub json_columns: Vec<bool>,
    /// Linhas da amostra quando [`GridState::json_columns`] foi calculado.
    json_sampled: u64,
}

/// Onde a grade foi desenhada no último frame.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub header: Rect,
    pub body: Rect,
    /// Onde começam as células (depois da coluna de números).
    pub cells_x: f32,
    pub row_h: f32,
}

impl GridState {
    /// Centro da célula na tela, se estiver visível.
    pub fn cell_center(&self, cell: Cell) -> Option<Pos2> {
        let l = self.layout?;
        let y = l.body.top() + ((cell.row as f64 - self.top) as f32 + 0.5) * l.row_h;
        let x = self.column_x(cell.col)? + self.widths[cell.col] / 2.0;
        (l.body.y_range().contains(y)).then(|| egui::pos2(x, y))
    }

    /// Retângulo da célula na tela (pode estar fora da área visível).
    pub fn cell_rect(&self, cell: Cell) -> Option<Rect> {
        let l = self.layout?;
        let y = l.body.top() + (cell.row as f64 - self.top) as f32 * l.row_h;
        let x = self.column_x(cell.col)?;
        Some(Rect::from_min_size(
            egui::pos2(x, y),
            vec2(self.widths[cell.col], l.row_h),
        ))
    }

    /// Botão `{…}` de uma célula de coluna JSON, que abre o JSON Viewer.
    pub fn json_button(&self, cell: Cell) -> Option<Rect> {
        if self.json_columns.get(cell.col) != Some(&true) {
            return None;
        }
        let r = self.cell_rect(cell)?;
        if r.width() < 48.0 {
            return None;
        }
        Some(Rect::from_min_max(
            egui::pos2(r.right() - JSON_BUTTON_W - 2.0, r.top() + 2.0),
            egui::pos2(r.right() - 2.0, r.bottom() - 2.0),
        ))
    }

    /// Centro do cabeçalho da coluna.
    pub fn header_center(&self, col: usize) -> Option<Pos2> {
        let l = self.layout?;
        Some(egui::pos2(
            self.column_x(col)? + self.widths[col] / 2.0,
            l.header.center().y,
        ))
    }

    /// Centro do número da linha (que precisa estar visível).
    pub fn row_header_center(&self, row: u64) -> Option<Pos2> {
        let l = self.layout?;
        let y = l.body.top() + ((row as f64 - self.top) as f32 + 0.5) * l.row_h;
        Some(egui::pos2((l.body.left() + l.cells_x) / 2.0, y))
    }

    /// Borda direita do cabeçalho da coluna (onde se arrasta a largura).
    pub fn column_border(&self, col: usize) -> Option<Pos2> {
        let l = self.layout?;
        Some(egui::pos2(
            self.column_x(col)? + self.widths[col],
            l.header.center().y,
        ))
    }

    fn column_x(&self, col: usize) -> Option<f32> {
        let l = self.layout?;
        (col < self.widths.len())
            .then(|| l.cells_x + self.widths[..col].iter().sum::<f32>() - self.left)
    }

    /// O JSON da célula, se ela tiver um objeto ou array.
    pub fn json_at(store: &mut ResultStore, cell: Cell) -> Option<json::Node> {
        json::container(store.cell(cell.row, cell.col)?)
    }

    /// Recalcula as colunas JSON enquanto a amostra cresce.
    fn detect_json(&mut self, store: &mut ResultStore) {
        let ncols = store.columns().len();
        let sample = store.len().min(JSON_SAMPLE);
        if self.json_columns.len() == ncols && self.json_sampled == sample {
            return;
        }
        self.json_sampled = sample;
        self.json_columns = (0..ncols)
            .map(|c| {
                let values: Vec<String> = (0..sample)
                    .filter_map(|r| store.cell(r, c).map(str::to_string))
                    .collect();
                json::is_json_column(values.iter().map(String::as_str))
            })
            .collect();
    }

    /// Seleciona tudo.
    pub fn select_all(&mut self, store: &ResultStore) {
        if store.is_empty() || store.columns().is_empty() {
            return;
        }
        self.selection = Some(Selection {
            anchor: Cell { row: 0, col: 0 },
            cursor: Cell {
                row: store.len() - 1,
                col: store.columns().len() - 1,
            },
            mode: Mode::Cells,
        });
    }

    /// Texto da seleção, para a área de transferência.
    pub fn copy(&self, store: &mut ResultStore, headers: bool) -> Option<String> {
        let s = self.selection?;
        let rows = s.rows(store.len());
        let cols = s.cols(store.columns().len());
        Some(text::copy(store, rows, cols, headers))
    }

    /// A seleção como comandos INSERT (uma tabela `table_name` para trocar
    /// depois de colar).
    pub fn copy_insert(&self, store: &mut ResultStore) -> Option<String> {
        let s = self.selection?;
        let rows = s.rows(store.len());
        let cols = s.cols(store.columns().len());
        Some(crate::export::insert_text(store, rows, cols, INSERT_TABLE))
    }
}

/// Tabela dos INSERTs copiados ou salvos da grade.
pub const INSERT_TABLE: &str = "table_name";

/// O que aconteceu na grade neste frame.
pub struct GridOutput {
    pub response: egui::Response,
    /// Texto copiado neste frame (já mandado para a área de transferência).
    pub copied: Option<String>,
    /// Pediu "Save Results As..." pelo menu de contexto.
    pub save_as: bool,
    /// Pediu para ver o JSON desta célula (duplo clique, Enter ou "View
    /// JSON..." no menu de contexto). A célula tem um objeto ou array.
    pub open_json: Option<Cell>,
}

/// Linhas lidas para reconhecer as colunas JSON.
const JSON_SAMPLE: u64 = 50;
/// Largura do botão `{…}` das células JSON.
const JSON_BUTTON_W: f32 = 26.0;

/// Largura máxima inicial de uma coluna, em caracteres.
const MAX_INITIAL_CHARS: usize = 40;
const SCROLLBAR: f32 = 12.0;
const PAD: f32 = 6.0;

struct Metrics {
    font: FontId,
    row_h: f32,
    char_w: f32,
}

fn metrics(ui: &egui::Ui) -> Metrics {
    let font = FontId::monospace(12.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 6.0;
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    Metrics {
        font,
        row_h,
        char_w,
    }
}

/// Larguras iniciais pelo nome e pelas primeiras linhas.
fn initial_widths(store: &mut ResultStore, char_w: f32) -> Vec<f32> {
    let sample = store.len().min(200);
    (0..store.columns().len())
        .map(|c| {
            let mut chars = store.columns()[c].chars().count().max(4);
            for r in 0..sample {
                chars = chars.max(store.cell(r, c).map_or(4, |v| v.chars().count()));
            }
            char_w * chars.min(MAX_INITIAL_CHARS) as f32 + PAD * 2.0
        })
        .collect()
}

/// Onde o ponteiro está na grade.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Corner,
    Column(usize),
    RowHeader(u64),
    Cell(Cell),
}

/// Desenha a grade de `store` ocupando o espaço disponível.
pub fn show(
    ui: &mut egui::Ui,
    id: egui::Id,
    store: &mut ResultStore,
    state: &mut GridState,
) -> GridOutput {
    let m = metrics(ui);
    let ncols = store.columns().len();
    let nrows = store.len();
    if state.widths.len() != ncols {
        state.widths = initial_widths(store, m.char_w);
    }
    state.detect_json(store);
    let rect = ui.available_rect_before_wrap();
    let response = ui.interact(rect, id, Sense::click_and_drag());
    ui.allocate_rect(rect, Sense::hover());

    let number_w = m.char_w * nrows.max(1).to_string().len().max(2) as f32 + PAD * 2.0;
    let header = Rect::from_min_size(rect.min, vec2(rect.width() - SCROLLBAR, m.row_h));
    let body = Rect::from_min_max(
        egui::pos2(rect.left(), header.bottom()),
        egui::pos2(rect.right() - SCROLLBAR, rect.bottom() - SCROLLBAR),
    );
    let cells_x = body.left() + number_w;
    let content_w: f32 = state.widths.iter().sum();
    let view_w = (body.right() - cells_x).max(1.0);
    let visible = (body.height() / m.row_h).max(1.0) as f64;
    let max_top = (nrows as f64 - visible.floor()).max(0.0);
    let max_left = (content_w - view_w).max(0.0);

    // Rolagem pela roda do mouse.
    if response.hovered() {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        state.top -= f64::from(delta.y / m.row_h);
        state.left -= delta.x;
    }

    // Barras de rolagem.
    let vtrack = Rect::from_min_max(
        egui::pos2(rect.right() - SCROLLBAR, body.top()),
        egui::pos2(rect.right(), body.bottom()),
    );
    let htrack = Rect::from_min_max(
        egui::pos2(cells_x, body.bottom()),
        egui::pos2(body.right(), rect.bottom()),
    );
    let vthumb_h = if nrows == 0 {
        vtrack.height()
    } else {
        (vtrack.height() * (visible / nrows as f64).min(1.0) as f32).max(20.0)
    };
    let hthumb_w = (htrack.width() * (view_w / content_w.max(1.0)).min(1.0)).max(20.0);
    let vbar = ui.interact(vtrack, id.with("vbar"), Sense::click_and_drag());
    if let Some(p) = vbar
        .interact_pointer_pos()
        .filter(|_| vbar.dragged() || vbar.clicked())
    {
        let t = ((p.y - vtrack.top() - vthumb_h / 2.0) / (vtrack.height() - vthumb_h).max(1.0))
            .clamp(0.0, 1.0);
        state.top = f64::from(t) * max_top;
    }
    let hbar = ui.interact(htrack, id.with("hbar"), Sense::click_and_drag());
    if let Some(p) = hbar
        .interact_pointer_pos()
        .filter(|_| hbar.dragged() || hbar.clicked())
    {
        let t = ((p.x - htrack.left() - hthumb_w / 2.0) / (htrack.width() - hthumb_w).max(1.0))
            .clamp(0.0, 1.0);
        state.left = t * max_left;
    }

    // Ajuste de largura pelas bordas do cabeçalho.
    let mut resizing = false;
    let mut x = cells_x - state.left;
    for (c, width) in state.widths.iter_mut().enumerate() {
        x += *width;
        if x < cells_x || x > header.right() {
            continue;
        }
        let handle =
            Rect::from_center_size(egui::pos2(x, header.center().y), vec2(8.0, header.height()));
        let r = ui.interact(handle, id.with(("resize", c)), Sense::drag());
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if r.dragged() {
            *width = (*width + r.drag_delta().x).max(24.0);
            resizing = true;
        }
    }

    // Seleção com o mouse.
    let hit = |pos: Pos2, state: &GridState| -> Option<Hit> {
        if !rect.contains(pos) || pos.x > header.right() || pos.y > body.bottom() {
            return None;
        }
        let col = || {
            let mut x = cells_x - state.left;
            for (c, w) in state.widths.iter().enumerate() {
                if pos.x < x + w {
                    return Some(c);
                }
                x += w;
            }
            None
        };
        let row = || {
            let r = state.top + f64::from((pos.y - body.top()) / m.row_h);
            (r >= 0.0 && (r as u64) < nrows).then_some(r as u64)
        };
        let in_header = pos.y < header.bottom();
        let in_numbers = pos.x < cells_x;
        match (in_header, in_numbers) {
            (true, true) => Some(Hit::Corner),
            (true, false) => col().map(Hit::Column),
            (false, true) => row().map(Hit::RowHeader),
            (false, false) => Some(Hit::Cell(Cell {
                row: row()?,
                col: col()?,
            })),
        }
    };
    if response.clicked() || response.drag_started() || response.secondary_clicked() {
        response.request_focus();
    }
    let mut open_json = None;
    let pointer_cell = response
        .interact_pointer_pos()
        .or(response.hover_pos())
        .and_then(|p| match hit(p, state) {
            Some(Hit::Cell(cell)) => Some(cell),
            _ => None,
        });
    if response.secondary_clicked()
        && let Some(cell) = pointer_cell
        && !state
            .selection
            .is_some_and(|s| s.contains(cell, nrows, ncols))
    {
        // O botão direito fora da seleção seleciona a célula, como no SSMS.
        state.selection = Some(Selection::at(cell, Mode::Cells));
    }
    // Nas colunas JSON, o duplo clique e o botão `{…}` abrem o JSON
    // Viewer; nas outras o duplo clique fica livre.
    // Um triplo clique rápido também conta (o segundo clique já abriu).
    let multi = response.double_clicked() || response.triple_clicked();
    if (multi || response.clicked())
        && !resizing
        && let Some(cell) = pointer_cell
        && state.json_columns.get(cell.col) == Some(&true)
    {
        let on_button = state.layout.is_some()
            && state
                .json_button(cell)
                .zip(response.interact_pointer_pos())
                .is_some_and(|(b, p)| b.contains(p));
        if (multi || on_button) && GridState::json_at(store, cell).is_some() {
            open_json = Some(cell);
        }
    }
    if !resizing {
        let (pressed, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
        let pos = response.interact_pointer_pos().or(response.hover_pos());
        let target = pos.and_then(|p| hit(p, state));
        if pressed && response.hovered() {
            let last_row = nrows.saturating_sub(1);
            let last_col = ncols.saturating_sub(1);
            let new = match target {
                Some(Hit::Corner) => {
                    state.select_all(store);
                    None
                }
                Some(Hit::Column(c)) => Some(Selection::at(Cell { row: 0, col: c }, Mode::Columns)),
                Some(Hit::RowHeader(r)) => Some(Selection::at(Cell { row: r, col: 0 }, Mode::Rows)),
                Some(Hit::Cell(cell)) => Some(Selection::at(cell, Mode::Cells)),
                None => None,
            };
            if let Some(mut new) = new {
                if let Some(old) = state.selection.filter(|o| shift && o.mode == new.mode) {
                    new.anchor = old.anchor;
                }
                if new.mode == Mode::Rows {
                    new.cursor.col = last_col;
                }
                if new.mode == Mode::Columns {
                    new.cursor.row = last_row;
                }
                state.selection = Some(new);
            }
        } else if response.dragged() {
            if let (Some(sel), Some(t)) = (&mut state.selection, target) {
                match (sel.mode, t) {
                    (Mode::Cells, Hit::Cell(cell)) => sel.cursor = cell,
                    (Mode::Rows, Hit::RowHeader(r))
                    | (Mode::Rows, Hit::Cell(Cell { row: r, .. })) => sel.cursor.row = r,
                    (Mode::Columns, Hit::Column(c))
                    | (Mode::Columns, Hit::Cell(Cell { col: c, .. })) => sel.cursor.col = c,
                    _ => {}
                }
            }
        }
    }

    // Teclado.
    let mut copied = None;
    let mut save_as = false;
    if response.has_focus() {
        let keys = ui.input(|i| {
            let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || i.key_pressed(Key::C) && i.modifiers.command;
            (
                copy.then_some(i.modifiers.shift),
                i.key_pressed(Key::A) && i.modifiers.command,
                [
                    (Key::ArrowUp, -1i64),
                    (Key::ArrowDown, 1),
                    (Key::PageUp, -(visible as i64).max(1)),
                    (Key::PageDown, (visible as i64).max(1)),
                ]
                .iter()
                .filter(|(k, _)| i.key_pressed(*k))
                .map(|(_, d)| *d)
                .sum::<i64>(),
                i.key_pressed(Key::ArrowRight) as i64 - i.key_pressed(Key::ArrowLeft) as i64,
                (i.key_pressed(Key::Home), i.key_pressed(Key::End)),
                i.modifiers,
            )
        });
        let (copy, all, dy, dx, (home, end), mods) = keys;
        if all {
            state.select_all(store);
        }
        if ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.is_none())
            && let Some(s) = state.selection
            && GridState::json_at(store, s.cursor).is_some()
        {
            open_json = Some(s.cursor);
        }
        if (dy != 0 || dx != 0 || home || end) && nrows > 0 && ncols > 0 {
            let cur = state
                .selection
                .map_or(Cell { row: 0, col: 0 }, |s| s.cursor);
            let mut next = Cell {
                row: (cur.row as i64 + dy).clamp(0, nrows as i64 - 1) as u64,
                col: (cur.col as i64 + dx).clamp(0, ncols as i64 - 1) as usize,
            };
            if home {
                next.col = 0;
                if mods.command {
                    next.row = 0;
                }
            }
            if end {
                next.col = ncols - 1;
                if mods.command {
                    next.row = nrows - 1;
                }
            }
            let mut sel = Selection::at(next, Mode::Cells);
            if let Some(old) = state.selection.filter(|_| mods.shift) {
                sel.anchor = old.anchor;
                sel.mode = Mode::Cells;
            }
            state.selection = Some(sel);
            scroll_to(state, next, visible, cells_x, view_w);
        }
        if let Some(headers) = copy {
            copied = state.copy(store, headers);
        }
    }
    let json_cursor = state.selection.map(|s| s.cursor);
    response.context_menu(|ui| {
        let json_cell = json_cursor.filter(|&c| GridState::json_at(store, c).is_some());
        if ui
            .add_enabled(json_cell.is_some(), egui::Button::new("View JSON..."))
            .on_disabled_hover_text("The selected cell has no JSON object or array.")
            .clicked()
        {
            open_json = json_cell;
            ui.close();
        }
        ui.separator();
        if ui.button("Copy").clicked() {
            copied = state.copy(store, false);
            ui.close();
        }
        if ui.button("Copy with Headers").clicked() {
            copied = state.copy(store, true);
            ui.close();
        }
        if ui.button("Copy as INSERT").clicked() {
            copied = state.copy_insert(store);
            ui.close();
        }
        ui.separator();
        if ui.button("Save Results As...").clicked() {
            save_as = true;
            ui.close();
        }
        if ui.button("Select All").clicked() {
            state.select_all(store);
            ui.close();
        }
    });
    if let Some(text) = &copied {
        ui.ctx().copy_text(text.clone());
    }

    state.top = state.top.clamp(0.0, max_top);
    state.left = state.left.clamp(0.0, max_left);

    state.layout = Some(Layout {
        header,
        body,
        cells_x,
        row_h: m.row_h,
    });
    paint(
        ui,
        &m,
        rect,
        header,
        body,
        number_w,
        store,
        state,
        response.has_focus(),
        pointer_cell.filter(|_| response.hovered()),
    );
    paint_scrollbars(
        ui, vtrack, htrack, vthumb_h, hthumb_w, state, max_top, max_left,
    );
    let on_json = pointer_cell.is_some_and(|c| state.json_columns.get(c.col) == Some(&true));
    let response = if on_json && !response.dragged() {
        response.on_hover_text_at_pointer("Double-click or click {…} to open the JSON Viewer.")
    } else {
        response
    };
    GridOutput {
        response,
        copied,
        save_as,
        open_json,
    }
}

/// Rola para a célula ficar visível.
fn scroll_to(state: &mut GridState, cell: Cell, visible: f64, cells_x: f32, view_w: f32) {
    let row = cell.row as f64;
    let page = visible.floor().max(1.0);
    if row < state.top {
        state.top = row;
    } else if row >= state.top + page {
        state.top = row - page + 1.0;
    }
    let start: f32 = state.widths[..cell.col].iter().sum();
    let end = start + state.widths[cell.col];
    if start < state.left {
        state.left = start;
    } else if end > state.left + view_w {
        state.left = end - view_w;
    }
    let _ = cells_x;
}

#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &egui::Ui,
    m: &Metrics,
    rect: Rect,
    header: Rect,
    body: Rect,
    number_w: f32,
    store: &mut ResultStore,
    state: &GridState,
    focused: bool,
    hovered: Option<Cell>,
) {
    let v = ui.visuals();
    let grid = Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color);
    let header_bg = v.faint_bg_color;
    let null_bg = if v.dark_mode {
        Color32::from_rgb(0x3A, 0x3A, 0x20)
    } else {
        Color32::from_rgb(0xFF, 0xFF, 0xE1)
    };
    let sel_bg = v
        .selection
        .bg_fill
        .linear_multiply(if focused { 0.5 } else { 0.25 });
    let text = v.text_color();
    let weak = v.weak_text_color();
    let strong = v.strong_text_color();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, v.extreme_bg_color);

    let ncols = store.columns().len();
    let nrows = store.len();
    let cells_x = body.left() + number_w;
    let cells_clip = Rect::from_min_max(
        egui::pos2(cells_x, rect.top()),
        egui::pos2(header.right(), body.bottom()),
    );
    let cells = painter.with_clip_rect(cells_clip);
    let numbers = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(rect.left(), body.top()),
        egui::pos2(cells_x, body.bottom()),
    ));

    // Cabeçalho.
    painter.rect_filled(header, 0.0, header_bg);
    painter.rect_filled(
        Rect::from_min_size(header.min, vec2(number_w, header.height())),
        0.0,
        header_bg,
    );
    let mut x = cells_x - state.left;
    let selected_cols = state.selection.map(|s| s.cols(ncols));
    for (c, &w) in state.widths.iter().enumerate() {
        let cell = Rect::from_min_size(egui::pos2(x, header.top()), vec2(w, header.height()));
        if selected_cols.as_ref().is_some_and(|r| r.contains(&c)) {
            cells.rect_filled(cell, 0.0, sel_bg);
        }
        let is_json = state.json_columns.get(c) == Some(&true);
        let name_cell = if is_json {
            // Marca `{}` à direita do nome das colunas JSON.
            let badge = Rect::from_min_max(egui::pos2(x + w - 22.0, cell.top()), cell.max);
            cells.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                "{}",
                m.font.clone(),
                weak,
            );
            Rect::from_min_max(cell.min, egui::pos2(badge.left() + PAD, cell.bottom()))
        } else {
            cell
        };
        cell_text(
            &cells,
            name_cell,
            &store.columns()[c].clone(),
            &m.font,
            strong,
        );
        cells.vline(x + w, header.y_range(), grid);
        x += w;
    }
    painter.hline(rect.x_range(), header.bottom(), grid);

    // Linhas visíveis.
    let first = state.top.floor() as u64;
    let offset = (state.top - state.top.floor()) as f32 * m.row_h;
    let count = (body.height() / m.row_h).ceil() as u64 + 1;
    let selected_rows = state.selection.map(|s| s.rows(nrows));
    for i in 0..count {
        let r = first + i;
        if r >= nrows {
            break;
        }
        let y = body.top() - offset + i as f32 * m.row_h;
        let num = Rect::from_min_size(egui::pos2(rect.left(), y), vec2(number_w, m.row_h));
        numbers.rect_filled(num, 0.0, header_bg);
        if selected_rows.as_ref().is_some_and(|s| s.contains(&r)) {
            numbers.rect_filled(num, 0.0, sel_bg);
        }
        cell_text(&numbers, num, &(r + 1).to_string(), &m.font, weak);
        let mut x = cells_x - state.left;
        for (c, &w) in state.widths.iter().enumerate() {
            if x + w >= cells_x && x <= header.right() {
                let cell = Rect::from_min_size(egui::pos2(x, y), vec2(w, m.row_h));
                let here = Cell { row: r, col: c };
                let value = store.cell(r, c);
                if value.is_none() {
                    cells.rect_filled(cell.shrink(0.5), 0.0, null_bg);
                }
                if state
                    .selection
                    .is_some_and(|s| s.contains(here, nrows, ncols))
                {
                    cells.rect_filled(cell, 0.0, sel_bg);
                }
                match value {
                    Some(v) => {
                        let v = v.to_string();
                        cell_text(&cells, cell, &v, &m.font, text);
                    }
                    None => cell_text(&cells, cell, "NULL", &m.font, weak),
                }
                let is_cursor = state.selection.is_some_and(|s| s.cursor == here);
                if value.is_some()
                    && (is_cursor || hovered == Some(here))
                    && let Some(b) = state.json_button(here)
                {
                    cells.rect_filled(b, 2.0, v.widgets.inactive.weak_bg_fill);
                    cells.rect_stroke(
                        b,
                        2.0,
                        Stroke::new(1.0, v.widgets.inactive.bg_stroke.color),
                        egui::StrokeKind::Inside,
                    );
                    cells.text(
                        b.center(),
                        egui::Align2::CENTER_CENTER,
                        "{…}",
                        m.font.clone(),
                        text,
                    );
                }
                if focused && is_cursor {
                    cells.rect_stroke(
                        cell.shrink(1.0),
                        0.0,
                        Stroke::new(1.0, strong),
                        egui::StrokeKind::Inside,
                    );
                }
                cells.vline(x + w, cell.y_range(), grid);
            }
            x += w;
        }
        painter.hline(
            egui::Rangef::new(rect.left(), header.right()),
            y + m.row_h,
            grid,
        );
    }
    numbers.vline(cells_x, body.y_range(), grid);
}

#[allow(clippy::too_many_arguments)]
fn paint_scrollbars(
    ui: &egui::Ui,
    vtrack: Rect,
    htrack: Rect,
    vthumb_h: f32,
    hthumb_w: f32,
    state: &GridState,
    max_top: f64,
    max_left: f32,
) {
    let v = ui.visuals();
    let painter = ui.painter();
    let track = v.extreme_bg_color;
    let thumb = v.widgets.inactive.bg_fill;
    painter.rect_filled(vtrack, 0.0, track);
    painter.rect_filled(htrack, 0.0, track);
    if max_top > 0.0 {
        let t = (state.top / max_top) as f32;
        let y = vtrack.top() + (vtrack.height() - vthumb_h) * t;
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(vtrack.left() + 2.0, y),
                vec2(SCROLLBAR - 4.0, vthumb_h),
            ),
            3.0,
            thumb,
        );
    }
    if max_left > 0.0 {
        let t = state.left / max_left;
        let x = htrack.left() + (htrack.width() - hthumb_w) * t;
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(x, htrack.top() + 2.0),
                vec2(hthumb_w, SCROLLBAR - 4.0),
            ),
            3.0,
            thumb,
        );
    }
}

/// Texto de uma célula, numa linha só, cortado na largura com `…`.
fn cell_text(painter: &egui::Painter, cell: Rect, value: &str, font: &FontId, color: Color32) {
    let one_line: String = value
        .chars()
        .take(MAX_INITIAL_CHARS * 8)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut job = egui::text::LayoutJob::simple_singleline(one_line, font.clone(), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width((cell.width() - PAD * 2.0).max(1.0));
    let galley = painter.layout_job(job);
    let pos = egui::pos2(cell.left() + PAD, cell.center().y - galley.size().y / 2.0);
    painter
        .with_clip_rect(painter.clip_rect().intersect(cell))
        .galley(pos, galley, color);
}
