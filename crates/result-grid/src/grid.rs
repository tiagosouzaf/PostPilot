//! A grade de resultados, como a do SSMS: número da linha, cabeçalho com
//! largura ajustável, NULL destacado, seleção de células, linhas ou colunas
//! e cópia com ou sem cabeçalho. Um clique no cabeçalho ordena pela coluna
//! e a caixa acima da grade filtra as linhas já recebidas ([`crate::view`]).
//!
//! A rolagem é própria, em linhas (não em pixels), para continuar precisa
//! com milhões de linhas: só as linhas visíveis são lidas do
//! [`ResultStore`] e desenhadas.

use pgstudio_i18n::{tr, trf};
use std::ops::Range;

use eframe::egui::{self, Color32, CursorIcon, FontId, Key, Pos2, Rect, Sense, Stroke, vec2};
use egui_phosphor::regular as icons;

use crate::copy::{self, CopyFormat};
use crate::json;
use crate::prefs::{self, GridPrefs};
use crate::store::ResultStore;
use crate::view::{self, Sort, ViewProgress, ViewSpec};

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

    /// Linhas selecionadas, num resultado de `rows` linhas. Uma seleção de
    /// colunas inteiras vai da primeira à última linha à mostra (a página,
    /// com paginação), que a grade mantém no `anchor` e no `cursor`.
    pub fn rows(&self, rows: u64) -> Range<u64> {
        let a = self.anchor.row.min(self.cursor.row);
        let b = self.anchor.row.max(self.cursor.row);
        a.min(rows)..(b + 1).min(rows)
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
    /// Colunas que dá para editar na própria grade (vazio: resultado só
    /// leitura). Quem preenche é quem sabe de onde o resultado veio.
    pub editable: Vec<bool>,
    /// Por que o resultado (ou a coluna) não é editável, para dizer ao
    /// usuário quando ele tenta.
    pub read_only_reason: Option<String>,
    /// Colunas JSON (reconhecidas pela amostra das primeiras linhas): o
    /// duplo clique nelas abre o JSON Viewer, não a edição.
    pub json_columns: Vec<bool>,
    /// Colunas que a grade reconheceu como JSON pela amostra das primeiras
    /// linhas (somam-se às de [`GridState::json_columns`]).
    pub json_detected: Vec<bool>,
    /// Linhas da amostra quando [`GridState::json_detected`] foi calculado.
    pub json_sampled: u64,
    /// Célula em edição e o texto digitado.
    pub editing: Option<CellEditor>,
    /// Uma alteração está sendo gravada: não começa outra.
    pub busy: bool,
    /// Ordenação pedida (um clique no cabeçalho).
    pub sort: Option<Sort>,
    /// Texto do filtro das linhas.
    pub filter: String,
    /// Quem grava as alterações está no meio de uma (ou esperando a
    /// confirmação de uma exclusão) e guardou posições de linhas: a ordem
    /// e o filtro só mudam depois.
    pub view_locked: bool,
    /// [`ResultStore::view_generation`] visto no último frame.
    pub view_generation: u64,
    /// A ordem ou o filtro ainda está sendo aplicado.
    pub view_working: bool,
    /// A grade mostra o resultado em páginas (quando [`GridPrefs::paginate`]
    /// está ligado) e tem embaixo o controle das páginas e do formato de
    /// cópia. Quem liga é quem desenha resultados de consulta.
    pub paged: bool,
    /// Página à mostra, a partir de 0.
    pub page: u64,
    /// Linhas à mostra no último frame (a página; sem paginação, todas).
    pub shown: Range<u64>,
    /// O que a última cópia levou, para dizer embaixo da grade, e quando
    /// (tempo do egui, em segundos).
    pub copied_note: Option<(String, f64)>,
    /// O servidor ainda tem linhas deste resultado que não foram buscadas
    /// (quem desenha diz a cada frame).
    pub more: bool,
    /// As linhas pedidas em [`GridState::fetch`] estão chegando.
    pub loading: bool,
    /// Pedido a quem desenha, que o tira daqui: buscar mais linhas do
    /// servidor.
    pub fetch: Option<Fetch>,
    /// Para onde ir quando as linhas pedidas chegarem.
    pub turn: Option<Turn>,
}

/// O que a grade pede do servidor quando o resultado veio só em parte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Mais uma página.
    Page,
    /// Todo o resto.
    All,
}

/// Troca de página à espera das linhas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Para esta página (a partir de 0).
    To(u64),
    /// Para a última, quando o resultado chegar inteiro.
    Last,
}

/// Edição de uma célula na própria grade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellEditor {
    pub cell: Cell,
    pub text: String,
    /// A caixa de texto já recebeu o foco (perder o foco depois disso
    /// termina a edição).
    focused: bool,
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

    /// Botão `{…}` de uma célula de coluna JSON, que abre o JSON Viewer.
    pub fn json_button(&self, cell: Cell) -> Option<Rect> {
        if !self.is_json(cell.col) {
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

    /// Retângulo da célula na tela, se estiver visível.
    pub fn cell_rect(&self, cell: Cell) -> Option<Rect> {
        let l = self.layout?;
        let y = l.body.top() + (cell.row as f64 - self.top) as f32 * l.row_h;
        let x = self.column_x(cell.col)?;
        let rect = Rect::from_min_size(egui::pos2(x, y), vec2(self.widths[cell.col], l.row_h));
        (rect.top() >= l.body.top() - 0.5 && rect.bottom() <= l.body.bottom() + 0.5).then_some(rect)
    }

    /// A coluna pode ser editada na grade.
    pub fn is_editable(&self, col: usize) -> bool {
        self.editable.get(col).copied().unwrap_or(false)
    }

    /// Coluna JSON (duplo clique abre o visualizador).
    pub fn is_json(&self, col: usize) -> bool {
        self.json_columns.get(col).copied().unwrap_or(false)
            || self.json_detected.get(col).copied().unwrap_or(false)
    }

    /// Começa a editar `cell` com o valor atual. Devolve `false` (sem
    /// mudar nada) quando a coluna não é editável ou há uma gravação em
    /// andamento.
    pub fn start_edit(&mut self, store: &mut ResultStore, cell: Cell) -> bool {
        if self.busy || !self.is_editable(cell.col) || cell.row >= store.len() {
            return false;
        }
        let value = store.cell(cell.row, cell.col);
        let text = value.unwrap_or("").to_string();
        self.reopen_edit(cell, text);
        true
    }

    /// Volta a editar `cell` com `text` (a gravação falhou e o usuário
    /// corrige o que digitou).
    pub fn reopen_edit(&mut self, cell: Cell, text: String) {
        self.editing = Some(CellEditor {
            cell,
            text,
            focused: false,
        });
        self.selection = Some(Selection::at(cell, Mode::Cells));
    }

    /// Linhas que o Delete exclui: as da seleção, a não ser que ela seja de
    /// colunas inteiras (aí seria o resultado todo sem querer).
    pub fn rows_to_delete(&self, store: &ResultStore) -> Option<Range<u64>> {
        let s = self.selection?;
        if s.mode == Mode::Columns || self.editable.iter().all(|e| !e) {
            return None;
        }
        let rows = s.rows(store.len());
        (!rows.is_empty()).then_some(rows)
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
        if self.json_detected.len() == ncols && self.json_sampled == sample {
            return;
        }
        self.json_sampled = sample;
        self.json_detected = (0..ncols)
            .map(|c| {
                let values: Vec<String> = (0..sample)
                    .filter_map(|r| store.cell(r, c).map(str::to_string))
                    .collect();
                json::is_json_column(values.iter().map(String::as_str))
            })
            .collect();
    }

    /// A ordem e o filtro pedidos.
    pub fn view_spec(&self) -> ViewSpec {
        ViewSpec {
            sort: self.sort,
            filter: self.filter.clone(),
        }
    }

    /// Um clique no cabeçalho: crescente, decrescente, sem ordem.
    pub fn cycle_sort(&mut self, col: usize) {
        self.sort = match self.sort {
            Some(s) if s.col == col && !s.descending => Some(Sort {
                col,
                descending: true,
            }),
            Some(s) if s.col == col => None,
            _ => Some(Sort {
                col,
                descending: false,
            }),
        };
        self.first_page();
    }

    /// Troca o filtro.
    pub fn set_filter(&mut self, text: String) {
        self.filter = text;
        self.first_page();
    }

    /// Volta para o começo do resultado (a ordem ou o filtro mudou).
    fn first_page(&mut self) {
        self.top = 0.0;
        self.page = 0;
    }

    /// Linhas da página à mostra, num resultado de `len` linhas. A página
    /// pedida além do fim vira a última.
    pub fn page_rows(&self, prefs: &GridPrefs, len: u64) -> Range<u64> {
        match prefs.page_rows().filter(|_| self.paged) {
            Some(size) => {
                let last = len.saturating_sub(1) / size;
                let start = self.page.min(last) * size;
                start..(start + size).min(len)
            }
            None => 0..len,
        }
    }

    /// Quantas páginas tem um resultado de `len` linhas (pelo menos uma).
    pub fn page_count(&self, prefs: &GridPrefs, len: u64) -> u64 {
        match prefs.page_rows().filter(|_| self.paged) {
            Some(size) => len.div_ceil(size).max(1),
            None => 1,
        }
    }

    /// Vai para a página `page` (a partir de 0), soltando a seleção: ela
    /// era das linhas da outra página.
    pub fn go_to_page(&mut self, prefs: &GridPrefs, len: u64, page: u64) {
        let page = page.min(self.page_count(prefs, len) - 1);
        if page != self.page {
            self.page = page;
            self.selection = None;
        }
        self.top = self.page_rows(prefs, len).start as f64;
    }

    /// A página seguinte; no fim do que já veio, pede mais uma ao servidor
    /// e vai para ela quando chegar.
    pub fn next_page(&mut self, prefs: &GridPrefs, len: u64) {
        if self.page + 1 < self.page_count(prefs, len) {
            self.go_to_page(prefs, len, self.page + 1);
        } else if self.more && self.turn.is_none() {
            self.fetch = Some(Fetch::Page);
            self.turn = Some(Turn::To(self.page + 1));
        }
    }

    /// A última página; com linhas ainda no servidor, busca todas antes.
    pub fn last_page(&mut self, prefs: &GridPrefs, len: u64) {
        if self.more {
            self.fetch = Some(Fetch::All);
            self.turn = Some(Turn::Last);
        } else {
            self.go_to_page(prefs, len, self.page_count(prefs, len) - 1);
        }
    }

    /// Completa a troca de página que esperava linhas do servidor.
    fn finish_turn(&mut self, prefs: &GridPrefs, len: u64) {
        let pages = self.page_count(prefs, len);
        // O pedido ainda não foi atendido nem recolhido: espera.
        let waiting = self.loading || self.fetch.is_some();
        match self.turn {
            Some(Turn::To(page)) if page < pages => {
                self.turn = None;
                self.go_to_page(prefs, len, page);
            }
            Some(Turn::Last) if !self.more => {
                self.turn = None;
                self.go_to_page(prefs, len, pages - 1);
            }
            // Não veio mais nada (ou o filtro escondeu o que veio).
            Some(_) if !waiting => self.turn = None,
            _ => {}
        }
    }

    /// Aplica a ordem e o filtro pedidos, um pouco por frame, e solta a
    /// seleção quando as linhas mudam de lugar.
    fn sync_view(&mut self, ctx: &egui::Context, store: &mut ResultStore) -> ViewProgress {
        let mut progress = ViewProgress {
            working: false,
            fraction: 1.0,
        };
        let spec = self.view_spec();
        if !self.view_locked && self.editing.is_none() {
            progress = store.sync_view(&spec, VIEW_BUDGET);
        } else if store.view_spec().cloned().unwrap_or_default() != spec {
            progress.working = true;
        }
        if progress.working {
            ctx.request_repaint();
        }
        self.view_working = progress.working;
        if store.view_generation() != self.view_generation {
            self.view_generation = store.view_generation();
            // Colunas inteiras continuam selecionadas (dá para ordenar e
            // copiar a coluna); o resto apontava para outras linhas.
            // As linhas dela são acertadas a cada frame, em `show`.
            if !self
                .selection
                .is_some_and(|s| s.mode == Mode::Columns && !store.is_empty())
            {
                self.selection = None;
            }
            self.editing = None;
        }
        progress
    }

    /// Seleciona tudo o que está à mostra: o resultado ou, com paginação,
    /// a página (as outras páginas não estão na tela).
    pub fn select_all(&mut self, store: &ResultStore) {
        let rows = if self.shown.end > self.shown.start && self.shown.end <= store.len() {
            self.shown.clone()
        } else {
            0..store.len()
        };
        if rows.is_empty() || store.columns().is_empty() {
            return;
        }
        self.selection = Some(Selection {
            anchor: Cell {
                row: rows.start,
                col: 0,
            },
            cursor: Cell {
                row: rows.end - 1,
                col: store.columns().len() - 1,
            },
            mode: Mode::Cells,
        });
    }

    /// Texto da seleção separado por tab, com ou sem cabeçalho.
    pub fn copy(&self, store: &mut ResultStore, headers: bool) -> Option<String> {
        self.copy_as(store, CopyFormat::Tsv, headers)
    }

    /// A seleção no formato `format`, para a área de transferência.
    pub fn copy_as(
        &self,
        store: &mut ResultStore,
        format: CopyFormat,
        headers: bool,
    ) -> Option<String> {
        let s = self.selection?;
        let rows = s.rows(store.len());
        let cols = s.cols(store.columns().len());
        Some(copy::copy(store, rows, cols, format, headers, INSERT_TABLE))
    }

    /// A seleção como comandos INSERT (uma tabela `table_name` para trocar
    /// depois de colar).
    pub fn copy_insert(&self, store: &mut ResultStore) -> Option<String> {
        self.copy_as(store, CopyFormat::Insert, false)
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
    /// Pediu "Export to Excel..." pelo menu de contexto.
    pub export_excel: bool,
    /// Pediu para ver o JSON desta célula (duplo clique, Enter ou "View
    /// JSON..." no menu de contexto). A célula tem um objeto ou array.
    pub open_json: Option<Cell>,
    /// Terminou a edição de uma célula com este valor (diferente do que
    /// havia): quem chamou grava no banco e, dando certo, troca no
    /// [`ResultStore`].
    pub commit: Option<(Cell, Option<String>)>,
    /// Delete (ou o menu) nestas linhas: quem chamou confirma e exclui.
    pub delete_rows: Option<Range<u64>>,
    /// Tentou editar ou excluir num resultado (ou coluna) só leitura; o
    /// motivo está em [`GridState::read_only_reason`].
    pub edit_refused: bool,
}

/// Tempo por frame para aplicar a ordem e o filtro.
const VIEW_BUDGET: std::time::Duration = std::time::Duration::from_millis(6);

/// Id da caixa de filtro da grade `id`.
pub fn filter_id(id: egui::Id) -> egui::Id {
    id.with("filter")
}

/// Leva o foco para a caixa de filtro da grade `id` (Ctrl+F).
pub fn focus_filter(ctx: &egui::Context, id: egui::Id) {
    ctx.memory_mut(|m| m.request_focus(filter_id(id)));
}

/// A caixa de filtro acima da grade, com quantas linhas passam.
fn filter_bar(
    ui: &mut egui::Ui,
    id: egui::Id,
    store: &ResultStore,
    state: &mut GridState,
    progress: ViewProgress,
) {
    ui.horizontal(|ui| {
        let before = state.filter.clone();
        let edit = ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .id(filter_id(id))
                .hint_text(tr("Filter rows: text or column:text"))
                .desired_width(260.0),
        );
        if state.filter != before {
            state.first_page();
        }
        if edit.lost_focus() {
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if escape {
                state.set_filter(String::new());
            }
            if enter || escape {
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
        if !state.filter.is_empty()
            && ui
                .small_button("×") // i18n-ignore
                .on_hover_text(tr("Clear the filter"))
                .clicked()
        {
            state.set_filter(String::new());
        }
        if progress.working {
            ui.spinner();
            if progress.fraction < 1.0 {
                ui.weak(format!("{:.0}%", progress.fraction * 100.0));
            }
        }
        if store.len() != store.total_len() || !state.filter.trim().is_empty() {
            ui.weak(trf!(
                "{shown} of {total} rows",
                shown = store.len(),
                total = store.total_len()
            ));
        }
    });
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
    let progress = state.sync_view(ui.ctx(), store);
    if store.total_len() > 1 || !state.filter.is_empty() {
        filter_bar(ui, id, store, state, progress);
    }
    let mut prefs = prefs::get(ui.ctx());
    let len = store.len();
    if state.widths.len() != ncols {
        state.widths = initial_widths(store, m.char_w);
    }
    state.detect_json(store);
    // O controle das páginas e do formato de cópia fica embaixo da grade;
    // numa grade baixa demais ele some e as linhas ficam com o espaço.
    let full = ui.available_rect_before_wrap();
    let footer_h = ui.spacing().interact_size.y + 6.0;
    let has_footer = state.paged && full.height() >= footer_h + m.row_h * 2.0;
    let rect = if has_footer {
        Rect::from_min_max(full.min, egui::pos2(full.right(), full.bottom() - footer_h))
    } else {
        full
    };
    let response = ui.interact(rect, id, Sense::click_and_drag());
    ui.allocate_rect(full, Sense::hover());

    // A página à mostra: a grade só desenha, rola e seleciona dentro dela.
    state.finish_turn(&prefs, len);
    state.page = state.page.min(state.page_count(&prefs, len) - 1);
    let shown = state.page_rows(&prefs, len);
    let (first_row, nrows) = (shown.start, shown.end);
    let page_len = nrows - first_row;
    let paging = state.paged && prefs.paginate;
    match &mut state.selection {
        // Colunas inteiras: da primeira à última linha à mostra.
        Some(s) if s.mode == Mode::Columns => {
            s.anchor.row = first_row;
            s.cursor.row = nrows.saturating_sub(1);
        }
        // O tamanho da página mudou e a seleção ficou em outra página.
        Some(s) if paging => {
            let (a, b) = (s.anchor.row, s.cursor.row);
            if a.min(b) < first_row || a.max(b) >= nrows.max(1) {
                state.selection = None;
            }
        }
        _ => {}
    }
    state.shown = shown;

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
    let min_top = first_row as f64;
    let max_top = (nrows as f64 - visible.floor()).max(min_top);
    let max_left = (content_w - view_w).max(0.0);

    // Rolagem pela roda do mouse.
    // A rolagem que a grade usa não passa para a área de resultados em volta
    // (com várias grades ela também rola, e o cabeçalho sumia junto); no fim
    // da grade, a roda continua rolando a área de fora.
    if response.hovered() {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        let (top, left) = (state.top, state.left);
        state.top = (state.top - f64::from(delta.y / m.row_h)).clamp(min_top, max_top);
        state.left = (state.left - delta.x).clamp(0.0, max_left);
        ui.input_mut(|i| {
            if state.top != top {
                i.smooth_scroll_delta.y = 0.0;
            }
            if state.left != left {
                i.smooth_scroll_delta.x = 0.0;
            }
        });
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
    let vthumb_h = if page_len == 0 {
        vtrack.height()
    } else {
        (vtrack.height() * (visible / page_len as f64).min(1.0) as f32).max(20.0)
    };
    let hthumb_w = (htrack.width() * (view_w / content_w.max(1.0)).min(1.0)).max(20.0);
    let vbar = ui.interact(vtrack, id.with("vbar"), Sense::click_and_drag());
    if let Some(p) = vbar
        .interact_pointer_pos()
        .filter(|_| vbar.dragged() || vbar.clicked())
    {
        let t = ((p.y - vtrack.top() - vthumb_h / 2.0) / (vtrack.height() - vthumb_h).max(1.0))
            .clamp(0.0, 1.0);
        state.top = min_top + f64::from(t) * (max_top - min_top);
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
    if response.has_focus() {
        // As setas são da grade (andam pelas células): sem isto o egui as
        // usa para levar o foco aos controles em volta, como a faixa das
        // páginas.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
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
        && page_len > 0
        && let Some(Hit::Column(c)) = response.interact_pointer_pos().and_then(|p| hit(p, state))
        && !state
            .selection
            .is_some_and(|s| s.mode == Mode::Columns && s.cols(ncols).contains(&c))
    {
        // No cabeçalho, o botão direito seleciona a coluna (o menu ordena
        // por ela).
        let mut sel = Selection::at(
            Cell {
                row: first_row,
                col: c,
            },
            Mode::Columns,
        );
        sel.cursor.row = nrows - 1;
        state.selection = Some(sel);
    }
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
        && state.is_json(cell.col)
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
                Some(Hit::Column(c)) => Some(Selection::at(
                    Cell {
                        row: first_row,
                        col: c,
                    },
                    Mode::Columns,
                )),
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
        } else if response.dragged()
            && let (Some(sel), Some(t)) = (&mut state.selection, target)
        {
            match (sel.mode, t) {
                (Mode::Cells, Hit::Cell(cell)) => sel.cursor = cell,
                (Mode::Rows, Hit::RowHeader(r)) | (Mode::Rows, Hit::Cell(Cell { row: r, .. })) => {
                    sel.cursor.row = r
                }
                (Mode::Columns, Hit::Column(c))
                | (Mode::Columns, Hit::Cell(Cell { col: c, .. })) => sel.cursor.col = c,
                _ => {}
            }
        }
        // Um clique (sem arrastar e sem Shift) no cabeçalho ordena pela
        // coluna; a borda entre duas colunas é de ajustar a largura.
        if response.clicked()
            && !shift
            && let Some(p) = pos
            && let Some(Hit::Column(c)) = target
        {
            let mut x = cells_x - state.left;
            let on_border = state.widths.iter().any(|w| {
                x += w;
                (p.x - x).abs() <= 4.0
            });
            if !on_border {
                state.cycle_sort(c);
            }
        }
    }

    // Teclado.
    let mut copied = None;
    let mut save_as = false;
    let mut export_excel = false;
    let mut edit_refused = false;
    let mut delete_rows = None;
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
        // Alt+PageDown e Alt+PageUp trocam de página.
        let turn = if mods.alt { dy.signum() } else { 0 };
        let dy = if mods.alt { 0 } else { dy };
        if turn > 0 && paging {
            state.next_page(&prefs, len);
        } else if turn < 0 && paging {
            state.go_to_page(&prefs, len, state.page.saturating_sub(1));
        }
        if all {
            state.select_all(store);
        }
        if ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.is_none())
            && let Some(s) = state.selection
            && GridState::json_at(store, s.cursor).is_some()
        {
            open_json = Some(s.cursor);
        }
        if (dy != 0 || dx != 0 || home || end) && page_len > 0 && ncols > 0 {
            let cur = state.selection.map_or(
                Cell {
                    row: first_row,
                    col: 0,
                },
                |s| s.cursor,
            );
            let mut next = Cell {
                row: (cur.row as i64 + dy).clamp(first_row as i64, nrows as i64 - 1) as u64,
                col: (cur.col as i64 + dx).clamp(0, ncols as i64 - 1) as usize,
            };
            if home {
                next.col = 0;
                if mods.command {
                    next.row = first_row;
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
        if let Some(shift) = copy {
            // Shift pede o cabeçalho mesmo com a opção desligada.
            copied = copy_selection(state, store, prefs.copy_format, shift || prefs.copy_headers);
        }
        let (f2, delete) = ui.input(|i| {
            (
                i.key_pressed(Key::F2) && i.modifiers.is_none(),
                i.key_pressed(Key::Delete) && i.modifiers.is_none(),
            )
        });
        if f2 && let Some(sel) = state.selection {
            edit_refused |= !state.is_json(sel.cursor.col) && !state.start_edit(store, sel.cursor);
        }
        if delete && !state.busy {
            delete_rows = state.rows_to_delete(store);
            edit_refused |=
                delete_rows.is_none() && state.selection.is_some_and(|s| s.mode != Mode::Columns);
        }
    }
    // Dois cliques numa célula: edita ali mesmo (colunas JSON ficam com o
    // visualizador).
    if response.double_clicked()
        && let Some(Hit::Cell(cell)) = response
            .interact_pointer_pos()
            .or(response.hover_pos())
            .and_then(|p| hit(p, state))
        && !state.is_json(cell.col)
    {
        edit_refused |= !state.start_edit(store, cell);
    }
    let can_edit = !state.busy && state.editable.iter().any(|e| *e);
    let json_cursor = state.selection.map(|s| s.cursor);
    response.context_menu(|ui| {
        let json_cell = json_cursor.filter(|&c| GridState::json_at(store, c).is_some());
        if ui
            .add_enabled(json_cell.is_some(), egui::Button::new(tr("View JSON...")))
            .on_disabled_hover_text(tr("The selected cell has no JSON object or array."))
            .clicked()
        {
            open_json = json_cell;
            ui.close();
        }
        ui.separator();
        if can_edit {
            let cursor = state.selection.map(|s| s.cursor);
            let cell_editable = cursor.is_some_and(|c| state.is_editable(c.col));
            if ui
                .add_enabled(cell_editable, egui::Button::new(tr("Edit Cell")))
                .clicked()
                && let Some(c) = cursor
            {
                state.start_edit(store, c);
                ui.close();
            }
            let rows = state.rows_to_delete(store);
            let label = match &rows {
                Some(r) if r.end - r.start > 1 => trf!("Delete {n} Rows", n = r.end - r.start),
                _ => tr("Delete Row").to_string(),
            };
            if ui
                .add_enabled(rows.is_some(), egui::Button::new(label))
                .clicked()
            {
                delete_rows = rows;
                ui.close();
            }
            ui.separator();
        }
        let cursor = state.selection.map(|s| s.cursor).filter(|c| c.col < ncols);
        for (label, descending) in [(tr("Sort Ascending"), false), (tr("Sort Descending"), true)] {
            if ui
                .add_enabled(cursor.is_some(), egui::Button::new(label))
                .clicked()
                && let Some(c) = cursor
            {
                state.sort = Some(Sort {
                    col: c.col,
                    descending,
                });
                state.top = 0.0;
                ui.close();
            }
        }
        if ui
            .add_enabled(state.sort.is_some(), egui::Button::new(tr("Clear Sort")))
            .clicked()
        {
            state.sort = None;
            state.top = 0.0;
            ui.close();
        }
        let by_value = cursor.filter(|c| c.row < store.len());
        if ui
            .add_enabled(
                by_value.is_some(),
                egui::Button::new(tr("Filter by This Value")),
            )
            .clicked()
            && let Some(c) = by_value
        {
            let value = store.cell(c.row, c.col).unwrap_or("NULL").to_string();
            let text = view::filter_for_value(&store.columns()[c.col], &value);
            state.set_filter(text);
            ui.close();
        }
        if ui
            .add_enabled(
                !state.filter.is_empty(),
                egui::Button::new(tr("Clear Filter")),
            )
            .clicked()
        {
            state.set_filter(String::new());
            ui.close();
        }
        ui.separator();
        // "Copy" é o Ctrl+C: sai no formato escolhido embaixo da grade.
        let format = prefs.copy_format;
        if ui
            .button(tr("Copy"))
            .on_hover_text(trf!(
                "Copies in the format chosen below the grid: {format}",
                format = tr(format.label())
            ))
            .clicked()
        {
            copied = copy_selection(state, store, format, prefs.copy_headers);
            ui.close();
        }
        if ui.button(tr("Copy with Headers")).clicked() {
            let format = if format.headers_optional() {
                format
            } else {
                CopyFormat::Tsv
            };
            copied = copy_selection(state, store, format, true);
            ui.close();
        }
        if ui.button(tr("Copy as INSERT")).clicked() {
            copied = copy_selection(state, store, CopyFormat::Insert, false);
            ui.close();
        }
        ui.menu_button(tr("Copy As"), |ui| {
            for f in CopyFormat::ALL {
                if ui.button(tr(f.label())).clicked() {
                    copied = copy_selection(state, store, f, prefs.copy_headers);
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui.button(tr("Save Results As...")).clicked() {
            save_as = true;
            ui.close();
        }
        if ui.button(tr("Export to Excel...")).clicked() {
            export_excel = true;
            ui.close();
        }
        if ui.button(tr("Select All")).clicked() {
            state.select_all(store);
            ui.close();
        }
    });
    if let Some(text) = &copied {
        ui.ctx().copy_text(text.clone());
    }

    if state.page_rows(&prefs, len) == state.shown {
        state.top = state.top.clamp(min_top, max_top);
    } else {
        // A página mudou neste frame (teclado, ordem ou filtro): os
        // limites acima eram da outra; o próximo frame acerta.
        ui.ctx().request_repaint();
    }
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
    let vfraction =
        (max_top > min_top).then(|| ((state.top - min_top) / (max_top - min_top)) as f32);
    paint_scrollbars(
        ui, vtrack, htrack, vthumb_h, hthumb_w, state, vfraction, max_left,
    );
    let commit = cell_editor(ui, id, &m, store, state, &response);
    if has_footer {
        let bar = Rect::from_min_max(egui::pos2(full.left(), rect.bottom()), full.max);
        footer(ui, id, bar, len, state, &mut prefs);
    }
    let on_json = pointer_cell.is_some_and(|c| state.is_json(c.col));
    let on_corner = response
        .hover_pos()
        .is_some_and(|p| hit(p, state) == Some(Hit::Corner));
    let response = if on_json && !response.dragged() {
        response.on_hover_text_at_pointer("Double-click or click {…} to open the JSON Viewer.")
    } else if on_corner && page_len > 0 {
        response.on_hover_text_at_pointer(tr(
            "Click to select everything shown, then Ctrl+C copies it in the format chosen below the grid.",
        ))
    } else {
        response
    };
    GridOutput {
        response,
        copied,
        save_as,
        export_excel,
        open_json,
        commit,
        delete_rows,
        edit_refused,
    }
}

/// Copia a seleção no formato `format` e guarda o que dizer embaixo da
/// grade.
fn copy_selection(
    state: &mut GridState,
    store: &mut ResultStore,
    format: CopyFormat,
    headers: bool,
) -> Option<String> {
    let text = state.copy_as(store, format, headers)?;
    let rows = state.selection.map_or(0, |s| {
        let r = s.rows(store.len());
        r.end - r.start
    });
    let note = if rows == 1 {
        trf!(
            "1 row copied as {format}",
            format = tr(format.short_label())
        )
    } else {
        trf!(
            "{rows} rows copied as {format}",
            rows = rows,
            format = tr(format.short_label())
        )
    };
    state.copied_note = Some((note, f64::NAN));
    Some(text)
}

/// Por quanto tempo o aviso da cópia fica embaixo da grade, em segundos.
const NOTE_SECONDS: f64 = 4.0;

/// A faixa embaixo da grade: as páginas à esquerda (ir e voltar, quais
/// linhas estão à mostra e quantas por página) e o formato do Ctrl+C à
/// direita. O que não cabe some, da direita da parte esquerda para dentro;
/// nada é desenhado por cima do vizinho.
fn footer(
    ui: &mut egui::Ui,
    id: egui::Id,
    bar: Rect,
    len: u64,
    state: &mut GridState,
    prefs: &mut GridPrefs,
) {
    let before = *prefs;
    let v = ui.visuals();
    let line = Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color);
    ui.painter().rect_filled(bar, 0.0, v.faint_bg_color);
    ui.painter().hline(bar.x_range(), bar.top(), line);
    let inner = bar.shrink2(vec2(6.0, 2.0));
    let wide = bar.width() >= 620.0;
    let clip = ui.clip_rect();

    // À direita, o formato de cópia.
    let mut right = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right.set_clip_rect(clip.intersect(bar));
    let mut format = prefs.copy_format;
    let mut headers = prefs.copy_headers;
    egui::ComboBox::from_id_salt(id.with("copy_format"))
        .selected_text(tr(format.short_label()))
        .width(10.0)
        .height(400.0)
        .show_ui(&mut right, |ui| {
            for f in CopyFormat::ALL {
                ui.selectable_value(&mut format, f, tr(f.label()));
            }
            ui.separator();
            ui.add_enabled(
                format.headers_optional(),
                egui::Checkbox::new(&mut headers, tr("Include column headers")),
            )
            .on_disabled_hover_text(tr(
                "This format always carries the column names, or has no place for them.",
            ));
        })
        .response
        .on_hover_text(tr(
            "Format of what Ctrl+C copies from the grid. Click the top-left corner to select everything shown, or a column header to select the column.",
        ));
    prefs.copy_format = format;
    prefs.copy_headers = headers;
    if wide {
        right.weak(tr("Copy as:"));
    }
    let left_end = right.min_rect().left() - 8.0;

    // À esquerda, as páginas.
    let left_rect = Rect::from_min_max(inner.min, egui::pos2(left_end, inner.bottom()));
    if left_rect.width() < 24.0 {
        return apply_prefs(ui, state, before, *prefs, len);
    }
    let mut left = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(left_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    left.set_clip_rect(clip.intersect(left_rect));
    let ui_left = &mut left;
    let pages = state.page_count(prefs, len);
    let shown = state.page_rows(prefs, len);
    let mut go = None;
    let (mut next, mut last) = (false, false);
    if prefs.paginate {
        let roomy = bar.width() >= 420.0;
        let button = |ui: &mut egui::Ui, enabled: bool, icon: &str, hint: &str| {
            ui.add_enabled(enabled, egui::Button::new(icon).small())
                .on_hover_text(hint)
                .clicked()
        };
        if roomy
            && button(
                ui_left,
                state.page > 0,
                icons::CARET_DOUBLE_LEFT,
                tr("First page"),
            )
        {
            go = Some(0);
        }
        if button(
            ui_left,
            state.page > 0,
            icons::CARET_LEFT,
            tr("Previous page (Alt+PageUp)"),
        ) {
            go = Some(state.page.saturating_sub(1));
        }
        let range = if shown.is_empty() {
            "0".to_string()
        } else {
            trf!(
                "{from}–{to} of {total}",
                from = shown.start + 1,
                to = shown.end,
                // Com linhas ainda no servidor, o total é "pelo menos".
                total = if state.more {
                    format!("{len}+")
                } else {
                    len.to_string()
                }
            )
        };
        let range = ui_left.label(range);
        if state.more {
            range.on_hover_text(tr(
                "Only the rows shown so far came from the server; the query is still open there. The next page fetches more, and the last page fetches everything. Sorting and filtering apply to the rows already fetched.",
            ));
        } else {
            range.on_hover_text(trf!(
                "Page {page} of {pages}",
                page = state.page + 1,
                pages = pages
            ));
        }
        if state.turn.is_some() {
            ui_left.spinner();
        }
        let more = state.page + 1 < pages || state.more;
        next = button(
            ui_left,
            more,
            icons::CARET_RIGHT,
            tr("Next page (Alt+PageDown)"),
        );
        last = roomy
            && button(
                ui_left,
                more,
                icons::CARET_DOUBLE_RIGHT,
                if state.more {
                    tr("Last page (fetches all the rows)")
                } else {
                    tr("Last page")
                },
            );
    } else {
        ui_left.label(if len == 1 {
            tr("1 row").to_string()
        } else {
            trf!("{rows} rows", rows = len)
        });
    }
    if let Some(page) = go {
        state.turn = None;
        state.go_to_page(prefs, len, page);
    }
    if next {
        state.next_page(prefs, len);
    }
    if last {
        state.last_page(prefs, len);
    }
    ui_left.add_space(6.0);
    // Com o aviso da cópia na faixa, o rótulo cede o lugar a ele.
    let now = ui_left.input(|i| i.time);
    let noting = state
        .copied_note
        .as_ref()
        .is_some_and(|(_, at)| at.is_nan() || now - *at < NOTE_SECONDS);
    if wide && (!noting || bar.width() >= 900.0) {
        ui_left.weak(tr("Rows per page:"));
    }
    let size_text = |p: &GridPrefs| match p.page_rows() {
        Some(n) => n.to_string(),
        None => tr("All rows").to_string(),
    };
    let mut choice = prefs.page_rows();
    egui::ComboBox::from_id_salt(id.with("page_size"))
        .selected_text(size_text(prefs))
        .width(10.0)
        .height(400.0)
        .show_ui(ui_left, |ui| {
            let mut sizes = prefs::PAGE_SIZES.to_vec();
            // Um tamanho que veio das opções e não está na lista.
            if !sizes.contains(&prefs.page_size) {
                sizes.push(prefs.page_size);
                sizes.sort_unstable();
            }
            for n in sizes {
                ui.selectable_value(&mut choice, Some(u64::from(n)), n.to_string());
            }
            ui.separator();
            ui.selectable_value(&mut choice, None, tr("All rows"));
        })
        .response
        .on_hover_text(tr(
            "How many rows each page shows. \"All rows\" turns paging off. The choice is kept for every result and for the next time PostPilot opens.",
        ));
    match choice {
        Some(n) => {
            prefs.paginate = true;
            prefs.page_size = n as u32;
        }
        None => prefs.paginate = false,
    }
    // O aviso da última cópia, por alguns segundos.
    if let Some((text, at)) = &mut state.copied_note {
        if at.is_nan() {
            *at = now;
        }
        if now - *at < NOTE_SECONDS {
            ui_left.add_space(6.0);
            ui_left.add(egui::Label::new(egui::RichText::new(text.as_str()).weak()).truncate());
            ui_left
                .ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    NOTE_SECONDS - (now - *at) + 0.05,
                ));
        } else {
            state.copied_note = None;
        }
    }
    apply_prefs(ui, state, before, *prefs, len);
}

/// Guarda as escolhas mudadas na faixa (valem para todas as grades) e
/// mantém à mostra a primeira linha que estava na tela.
fn apply_prefs(ui: &egui::Ui, state: &mut GridState, before: GridPrefs, now: GridPrefs, len: u64) {
    if before == now {
        return;
    }
    prefs::set(ui.ctx(), now);
    if before.page_rows() != now.page_rows() {
        let first = (state.top.floor().max(0.0) as u64).min(len.saturating_sub(1));
        state.page = now.page_rows().map_or(0, |size| first / size);
        state.top = match now.page_rows() {
            Some(_) => state.page_rows(&now, len).start as f64,
            None => first as f64,
        };
    }
    ui.ctx().request_repaint();
}

/// A caixa de texto da célula em edição, por cima da grade. Enter (ou
/// clicar fora) termina e devolve o valor novo; Esc desiste; Ctrl+0 grava
/// NULL, como na janela Edit Rows.
fn cell_editor(
    ui: &mut egui::Ui,
    id: egui::Id,
    m: &Metrics,
    store: &mut ResultStore,
    state: &mut GridState,
    grid: &egui::Response,
) -> Option<(Cell, Option<String>)> {
    let editor = state.editing.as_mut()?;
    let cell = editor.cell;
    if cell.row >= store.len() || cell.col >= store.columns().len() {
        state.editing = None;
        return None;
    }
    let Some(rect) = state.cell_rect(cell) else {
        // Rolou para longe: termina a edição como se tivesse clicado fora.
        return finish_edit(store, state);
    };
    let edit_id = id.with("cell_editor");
    let editor = state.editing.as_mut()?;
    let resp = ui.put(
        rect,
        egui::TextEdit::singleline(&mut editor.text)
            .id(edit_id)
            .font(m.font.clone())
            .margin(vec2(PAD - 2.0, 2.0)),
    );
    let (enter, escape, null) = ui.input(|i| {
        (
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
            i.modifiers.command && i.key_pressed(Key::Num0),
        )
    });
    // O foco pela memória, não por `has_focus`: trocar de janela (Alt+Tab)
    // tira o foco da janela, não da edição.
    let focused = ui.memory(|m| m.has_focus(edit_id));
    if focused && null {
        // O Ctrl+0 é desta célula (NULL), não do "Reset Zoom" da casca.
        ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Num0));
        state.editing = None;
        grid.request_focus();
        return (store.cell(cell.row, cell.col).is_some()).then_some((cell, None));
    }
    if !editor.focused {
        editor.focused = true;
        resp.request_focus();
        // O cursor vai para o fim do texto.
        let mut s = egui::TextEdit::load_state(ui.ctx(), edit_id).unwrap_or_default();
        let end = egui::text::CCursor::new(editor.text.chars().count());
        s.cursor
            .set_char_range(Some(egui::text::CCursorRange::one(end)));
        s.store(ui.ctx(), edit_id);
        return None;
    }
    if !focused {
        if escape {
            state.editing = None;
            grid.request_focus();
            return None;
        }
        let out = finish_edit(store, state);
        if enter {
            grid.request_focus();
        }
        return out;
    }
    None
}

/// Fecha a edição; devolve o valor novo se ele mudou. Texto vazio numa
/// célula que era NULL continua NULL.
fn finish_edit(store: &mut ResultStore, state: &mut GridState) -> Option<(Cell, Option<String>)> {
    let editor = state.editing.take()?;
    let current = store.cell(editor.cell.row, editor.cell.col);
    let unchanged = match current {
        None => editor.text.is_empty(),
        Some(v) => v == editor.text,
    };
    (!unchanged).then_some((editor.cell, Some(editor.text)))
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
    // Fim da página à mostra (sem paginação, do resultado).
    let nrows = state.shown.end;
    let cells_x = body.left() + number_w;
    // O cabeçalho e as linhas têm recortes separados: com rolagem
    // fracionária a primeira linha visível começa acima de `body.top()` e,
    // se dividissem o mesmo recorte, seria desenhada por cima dos nomes das
    // colunas.
    let heads = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(cells_x, header.top()),
        egui::pos2(header.right(), header.bottom()),
    ));
    let cells = painter.with_clip_rect(Rect::from_min_max(
        egui::pos2(cells_x, body.top()),
        egui::pos2(header.right(), body.bottom()),
    ));
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
    if nrows > state.shown.start {
        // O canto seleciona tudo: um triângulo, como nas planilhas.
        let c = egui::pos2(cells_x - 3.0, header.bottom() - 3.0);
        let side = (header.height() * 0.4).clamp(4.0, 8.0);
        painter.add(egui::Shape::convex_polygon(
            vec![c, egui::pos2(c.x - side, c.y), egui::pos2(c.x, c.y - side)],
            weak,
            Stroke::NONE,
        ));
    }
    let mut x = cells_x - state.left;
    let selected_cols = state.selection.map(|s| s.cols(ncols));
    for (c, &w) in state.widths.iter().enumerate() {
        let cell = Rect::from_min_size(egui::pos2(x, header.top()), vec2(w, header.height()));
        if selected_cols.as_ref().is_some_and(|r| r.contains(&c)) {
            heads.rect_filled(cell, 0.0, sel_bg);
        }
        let is_json = state.is_json(c);
        let name_cell = if is_json {
            // Marca `{}` à direita do nome das colunas JSON.
            let badge = Rect::from_min_max(egui::pos2(x + w - 22.0, cell.top()), cell.max);
            heads.text(
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
        let name_cell = match state.sort.filter(|s| s.col == c) {
            Some(sort) if name_cell.width() > 28.0 => {
                // Seta da ordenação, à direita do nome.
                let center = egui::pos2(name_cell.right() - 9.0, cell.center().y);
                let d = if sort.descending { 3.0 } else { -3.0 };
                heads.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(center.x - 4.0, center.y - d),
                        egui::pos2(center.x + 4.0, center.y - d),
                        egui::pos2(center.x, center.y + d),
                    ],
                    strong,
                    Stroke::NONE,
                ));
                Rect::from_min_max(name_cell.min, egui::pos2(center.x - 2.0, cell.bottom()))
            }
            _ => name_cell,
        };
        cell_text(
            &heads,
            name_cell,
            &store.columns()[c].clone(),
            &m.font,
            strong,
        );
        heads.vline(x + w, header.y_range(), grid);
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
    vfraction: Option<f32>,
    max_left: f32,
) {
    let v = ui.visuals();
    let painter = ui.painter();
    let track = v.extreme_bg_color;
    let thumb = v.widgets.inactive.bg_fill;
    painter.rect_filled(vtrack, 0.0, track);
    painter.rect_filled(htrack, 0.0, track);
    if let Some(t) = vfraction {
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
