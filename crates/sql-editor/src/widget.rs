//! O widget do editor: só as linhas visíveis são montadas e desenhadas, e
//! o realce vem dos tokens do texto inteiro, refeitos só no trecho mudado
//! ([`crate::lexer::retokenize`]). Assim digitar num arquivo de milhares de
//! linhas custa quase o mesmo que num pequeno.
//!
//! O estado pequeno (seleção e rolagem) fica na memória do egui pelo id do
//! editor; o grande (índice de linhas, tokens e histórico) fica no
//! [`Buffer`], dentro do documento.

use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CursorIcon, Event, FontId, Galley, Id, Key, Modifiers, Pos2, Rect, Sense,
    Stroke, Ui, Vec2, pos2, text::CCursor, vec2,
};

use crate::buffer::{Change, EditKind, History, LineIndex, is_word, line_text};
use crate::complete::{Completion, Kind, best, complete};
use crate::diagnostics::{Diagnostic, server_range};
use crate::highlight::{Colors, line_job};
use crate::lexer::{Dialect, Token, tokenize_in};
use pgstudio_core::catalog::Catalog;

/// Espaços de um Tab.
pub const TAB_SIZE: usize = 4;
const SCROLLBAR: f32 = 10.0;
const TEXT_MARGIN: f32 = 4.0;

/// Seleção e rolagem do editor, guardadas na memória do egui.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct View {
    pub anchor: usize,
    pub cursor: usize,
    pub scroll: Vec2,
    /// Posição x desejada ao subir e descer linhas.
    preferred_x: Option<f32>,
    /// Arrastando depois de um duplo clique: seleciona por palavra.
    drag_words: Option<(usize, usize)>,
}

pub(crate) fn load(ctx: &egui::Context, id: Id) -> View {
    ctx.data(|d| d.get_temp::<View>(id)).unwrap_or_default()
}

pub(crate) fn store(ctx: &egui::Context, id: Id, view: View) {
    ctx.data_mut(|d| d.insert_temp(id, view));
}

/// Índice de linhas, tokens e histórico do texto de um editor.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    /// Cópia do texto da última vez que o índice foi montado. Se o texto do
    /// documento mudar por fora (comentar linhas, por exemplo), a diferença
    /// entra no histórico e o índice é refeito.
    shadow: String,
    index: LineIndex,
    tokens: Vec<Token>,
    /// Dialeto do texto (o do servidor da janela).
    pub(crate) dialect: Dialect,
    history: History,
    synced: bool,
    /// Lista do IntelliSense aberta.
    popup: Option<Popup>,
    /// Erros de sintaxe vistos pelos tokens.
    diagnostics: Vec<Diagnostic>,
    /// Erro da última execução, até o texto mudar.
    server_error: Option<Diagnostic>,
    /// Erro pedido por `EditorState::mark_error` (caractere, mensagem).
    pub(crate) pending_error: Option<(usize, String)>,
    /// Onde a lista foi desenhada no último frame: um clique nela tira o
    /// foco do editor, mas não pode fechá-la.
    popup_rect: Option<Rect>,
}

/// A lista de sugestões aberta embaixo do cursor.
#[derive(Clone, Debug)]
struct Popup {
    completion: Completion,
    selected: usize,
    /// Primeira linha visível da lista.
    top: usize,
}

/// Linhas visíveis da lista de sugestões.
const POPUP_ROWS: usize = 10;
const POPUP_WIDTH: f32 = 420.0;

impl Buffer {
    fn sync(&mut self, text: &str, view: &View) {
        if self.synced && self.shadow == text {
            return;
        }
        if self.synced
            && let Some(change) = Change::between(&self.shadow, text)
        {
            let sel = (view.anchor, view.cursor);
            self.history.record(change, EditKind::Other, sel, sel);
            self.history.seal();
        }
        self.rebuild(text);
    }

    /// Depois de uma edição feita aqui: retokeniza só o trecho mudado.
    fn apply(&mut self, text: &str, at: usize, removed: usize, inserted: usize) {
        crate::lexer::retokenize_in(text, &mut self.tokens, at, removed, inserted, self.dialect);
        self.shadow
            .replace_range(at..at + removed, &text[at..at + inserted]);
        self.index = LineIndex::new(text);
        self.text_changed(text);
    }

    fn text_changed(&mut self, text: &str) {
        self.diagnostics = crate::diagnostics::check_in(text, &self.tokens, self.dialect);
        self.server_error = None;
    }

    pub(crate) fn errors(&self) -> Vec<(Range<usize>, String)> {
        self.diagnostics
            .iter()
            .chain(self.server_error.iter())
            .map(|d| (d.range.clone(), d.message.clone()))
            .collect()
    }

    pub(crate) fn is_completing(&self) -> bool {
        self.popup.is_some()
    }

    pub(crate) fn completion_labels(&self) -> Vec<String> {
        self.popup.as_ref().map_or(Vec::new(), |p| {
            p.completion.items.iter().map(|i| i.label.clone()).collect()
        })
    }

    /// Força refazer índice e tokens no próximo frame.
    pub(crate) fn invalidate(&mut self) {
        self.synced = false;
    }

    fn rebuild(&mut self, text: &str) {
        self.shadow.clear();
        self.shadow.push_str(text);
        self.index = LineIndex::new(text);
        self.tokens = tokenize_in(text, self.dialect);
        self.synced = true;
        self.text_changed(text);
    }
}

/// Geometria do editor neste frame.
struct Frame {
    text_rect: Rect,
    line_h: f32,
    char_w: f32,
    font: FontId,
    colors: Colors,
}

impl Frame {
    fn visible_lines(&self) -> usize {
        (self.text_rect.height() / self.line_h).floor().max(1.0) as usize
    }
}

/// Uma edição em andamento: texto, buffer e seleção juntos.
struct Editing<'a> {
    text: &'a mut String,
    buf: &'a mut Buffer,
    view: &'a mut View,
    moved: bool,
    catalog: Option<&'a Catalog>,
}

impl Editing<'_> {
    /// Recalcula a lista do IntelliSense no cursor. Com `open`, abre se
    /// estiver fechada; sem, só atualiza a que já está aberta. Fecha se não
    /// houver sugestões.
    fn refresh_popup(&mut self, open: bool) {
        if self.buf.popup.is_none() && !open {
            return;
        }
        if !self.range().is_empty() {
            self.buf.popup = None;
            return;
        }
        let at = self.buf.index.byte_of(self.text, self.view.cursor);
        let c = complete(self.text, &self.buf.tokens, at, self.catalog);
        if c.items.is_empty() {
            self.buf.popup = None;
            return;
        }
        let selected = best(&c);
        self.buf.popup = Some(Popup {
            completion: c,
            selected,
            top: selected.saturating_sub(POPUP_ROWS / 2),
        });
    }

    /// Troca a palavra digitada pela sugestão escolhida.
    fn accept(&mut self) {
        let Some(p) = self.buf.popup.take() else {
            return;
        };
        let Some(item) = p.completion.items.get(p.selected) else {
            return;
        };
        let n = p.completion.prefix.chars().count();
        let cur = self.view.cursor;
        self.replace(cur.saturating_sub(n)..cur, &item.insert, EditKind::Other);
        self.buf.history.seal();
    }

    /// Se o que foi digitado já é exatamente a sugestão escolhida.
    fn typed_selection(&self) -> bool {
        self.buf.popup.as_ref().is_some_and(|p| {
            p.completion
                .items
                .get(p.selected)
                .is_some_and(|i| i.insert == p.completion.prefix)
        })
    }

    fn move_popup(&mut self, delta: i64) {
        if let Some(p) = &mut self.buf.popup {
            let last = p.completion.items.len() as i64 - 1;
            p.selected = (p.selected as i64 + delta).clamp(0, last) as usize;
            if p.selected < p.top {
                p.top = p.selected;
            } else if p.selected >= p.top + POPUP_ROWS {
                p.top = p.selected + 1 - POPUP_ROWS;
            }
        }
    }

    fn range(&self) -> Range<usize> {
        self.view.anchor.min(self.view.cursor)..self.view.anchor.max(self.view.cursor)
    }

    fn index(&self) -> &LineIndex {
        &self.buf.index
    }

    fn len(&self) -> usize {
        self.buf.index.len_chars()
    }

    fn selected(&self) -> String {
        let r = self.range();
        let a = self.buf.index.byte_of(self.text, r.start);
        let b = self.buf.index.byte_of(self.text, r.end);
        self.text[a..b].to_string()
    }

    /// Troca os caracteres de `range` por `insert` e põe o cursor depois.
    fn replace(&mut self, range: Range<usize>, insert: &str, kind: EditKind) {
        let before = (self.view.anchor, self.view.cursor);
        let a = self.buf.index.byte_of(self.text, range.start);
        let b = self.buf.index.byte_of(self.text, range.end);
        let removed = self.text[a..b].to_string();
        if removed.is_empty() && insert.is_empty() {
            return;
        }
        self.text.replace_range(a..b, insert);
        self.buf.apply(self.text, a, removed.len(), insert.len());
        let c = range.start + insert.chars().count();
        self.view.anchor = c;
        self.view.cursor = c;
        self.buf.history.record(
            Change {
                at: range.start,
                removed,
                inserted: insert.to_string(),
            },
            kind,
            before,
            (c, c),
        );
        self.moved = true;
    }

    fn insert(&mut self, s: &str, kind: EditKind) {
        let r = self.range();
        self.replace(r, s, kind);
    }

    fn move_to(&mut self, c: usize, extend: bool) {
        self.view.cursor = c.min(self.len());
        if !extend {
            self.view.anchor = self.view.cursor;
        }
        self.moved = true;
    }

    fn select(&mut self, range: Range<usize>) {
        self.view.anchor = range.start;
        self.view.cursor = range.end;
        self.moved = true;
    }

    fn line_chars(&self, line: usize) -> Vec<char> {
        line_text(self.text, &self.buf.index, line)
            .chars()
            .collect()
    }

    /// Começo da palavra (ou do bloco de espaços/sinais) antes de `c`.
    fn word_left(&self, c: usize) -> usize {
        let (line, col) = self.index().line_col(c);
        if col == 0 {
            return c.saturating_sub(1);
        }
        let chars = self.line_chars(line);
        let mut i = col;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        if i > 0 {
            let word = is_word(chars[i - 1]);
            while i > 0 && !chars[i - 1].is_whitespace() && is_word(chars[i - 1]) == word {
                i -= 1;
            }
        }
        c - (col - i)
    }

    /// Fim da palavra depois de `c`, e os espaços que vêm depois dela.
    fn word_right(&self, c: usize) -> usize {
        let (line, col) = self.index().line_col(c);
        let chars = self.line_chars(line);
        if col >= chars.len() {
            return (c + 1).min(self.len());
        }
        let mut i = col;
        if !chars[i].is_whitespace() {
            let word = is_word(chars[i]);
            while i < chars.len() && !chars[i].is_whitespace() && is_word(chars[i]) == word {
                i += 1;
            }
        }
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        c + (i - col)
    }

    /// A palavra em volta de `c` (duplo clique).
    fn word_at(&self, c: usize) -> Range<usize> {
        let (line, col) = self.index().line_col(c);
        let chars = self.line_chars(line);
        if chars.is_empty() {
            return c..c;
        }
        let at = col.min(chars.len() - 1);
        let class = |ch: char| (ch.is_whitespace(), is_word(ch));
        let k = class(chars[at]);
        let mut a = at;
        while a > 0 && class(chars[a - 1]) == k {
            a -= 1;
        }
        let mut b = at;
        while b < chars.len() && class(chars[b]) == k {
            b += 1;
        }
        let start = self.index().line_start(line);
        start + a..start + b
    }

    /// A linha inteira de `c`, com o `\n` (triplo clique).
    fn line_at(&self, c: usize) -> Range<usize> {
        let line = self.index().line_of(c);
        let start = self.index().line_start(line);
        let end = if line + 1 < self.index().line_count() {
            self.index().line_start(line + 1)
        } else {
            self.len()
        };
        start..end
    }

    /// Indentação (espaços e tabs) do começo da linha de `c`, até `c`.
    fn indent_before(&self, c: usize) -> String {
        let (line, col) = self.index().line_col(c);
        self.line_chars(line)
            .into_iter()
            .take(col)
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect()
    }

    /// Linhas tocadas pela seleção (uma seleção que termina no começo de uma
    /// linha não a inclui).
    fn selected_lines(&self) -> Range<usize> {
        let r = self.range();
        let first = self.index().line_of(r.start);
        let mut last = self.index().line_of(r.end);
        if r.end > r.start && last > first && self.index().line_start(last) == r.end {
            last -= 1;
        }
        first..last + 1
    }

    /// Tab com várias linhas selecionadas: indenta; Shift+Tab: tira até
    /// [`TAB_SIZE`] espaços (ou um tab) do começo.
    fn indent_lines(&mut self, outdent: bool) {
        let lines = self.selected_lines();
        let start = self.index().line_start(lines.start);
        let end_line = lines.end - 1;
        let end = self.index().line_start(end_line) + self.index().line_len(end_line);
        let old: Vec<String> = lines
            .clone()
            .map(|l| line_text(self.text, &self.buf.index, l).to_string())
            .collect();
        let new: Vec<String> = old
            .iter()
            .map(|l| {
                if outdent {
                    let n = if l.starts_with('\t') {
                        1
                    } else {
                        l.chars().take(TAB_SIZE).take_while(|c| *c == ' ').count()
                    };
                    l.chars().skip(n).collect()
                } else if l.is_empty() {
                    String::new()
                } else {
                    format!("{}{l}", " ".repeat(TAB_SIZE))
                }
            })
            .collect();
        let joined = new.join("\n");
        if joined == old.join("\n") {
            return;
        }
        self.replace(start..end, &joined, EditKind::Other);
        self.buf.history.seal();
        let new_end = start + joined.chars().count();
        self.select(start..new_end);
    }

    fn undo(&mut self) {
        if let Some((a, c)) = self.buf.history.undo(self.text) {
            self.buf.rebuild(self.text);
            self.view.anchor = a.min(self.len());
            self.view.cursor = c.min(self.len());
            self.moved = true;
        }
    }

    fn redo(&mut self) {
        if let Some((a, c)) = self.buf.history.redo(self.text) {
            self.buf.rebuild(self.text);
            self.view.anchor = a.min(self.len());
            self.view.cursor = c.min(self.len());
            self.moved = true;
        }
    }
}

/// Layout de uma linha, com o realce (o egui guarda os layouts usados, então
/// linhas que não mudaram saem do cache).
fn line_galley(ui: &Ui, text: &str, buf: &Buffer, line: usize, f: &Frame) -> Arc<Galley> {
    let bytes = buf.index.line_bytes(line);
    let job = line_job(
        &text[bytes.clone()],
        bytes.start,
        &buf.tokens,
        &f.colors,
        &f.font,
    );
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// x do caractere `col` da linha, relativo ao começo do texto.
fn col_x(galley: &Galley, col: usize) -> f32 {
    galley.pos_from_cursor(CCursor::new(col)).min.x
}

/// Coluna mais perto de `x` na linha.
fn x_col(galley: &Galley, x: f32) -> usize {
    galley.cursor_from_pos(vec2(x, 0.0)).index.into()
}

/// O que o editor fez neste frame.
pub(crate) struct Output {
    pub response: egui::Response,
    /// Linha e coluna do cursor (a partir de 1).
    pub cursor: (usize, usize),
}

/// Desenha o editor e trata teclado e mouse. `select` e `focus` vêm de
/// pedidos de frames anteriores (busca, "Go To Line").
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    buf: &mut Buffer,
    colors: Colors,
    font: FontId,
    select: Option<(usize, usize)>,
    focus: bool,
    catalog: Option<&Catalog>,
    list_members: bool,
) -> Output {
    let ctx = ui.ctx().clone();
    let mut view = load(&ctx, id);
    buf.sync(text, &view);
    if let Some((at, message)) = buf.pending_error.take() {
        let byte = buf.index.byte_of(text, at);
        buf.server_error = Some(Diagnostic {
            range: server_range(text, &buf.tokens, byte),
            message,
        });
    }
    let total = buf.index.len_chars();
    view.anchor = view.anchor.min(total);
    view.cursor = view.cursor.min(total);

    let line_h = ui.fonts_mut(|f| f.row_height(&font));
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let digits = buf.index.line_count().to_string().len().max(3);
    let gutter_w = char_w * digits as f32 + 16.0;

    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let gutter = Rect::from_min_size(rect.min, vec2(gutter_w, rect.height() - SCROLLBAR));
    let text_rect = Rect::from_min_max(
        pos2(gutter.right() + TEXT_MARGIN, rect.top()),
        pos2(rect.right() - SCROLLBAR, rect.bottom() - SCROLLBAR),
    );
    let f = Frame {
        text_rect,
        line_h,
        char_w,
        font,
        colors,
    };
    let response = ui.interact(
        Rect::from_min_max(gutter.min, text_rect.max),
        id,
        Sense::click_and_drag(),
    );
    response.widget_info(|| egui::WidgetInfo::text_edit(true, &*text, &*text, ""));
    if response.hovered() {
        ctx.set_cursor_icon(CursorIcon::Text);
    }
    if focus || response.drag_started() || response.clicked() {
        ui.memory_mut(|m| m.request_focus(id));
    }
    let focused = ui.memory(|m| m.has_focus(id));

    let over_popup = buf.popup.is_some()
        && buf
            .popup_rect
            .is_some_and(|r| ui.input(|i| i.pointer.interact_pos().is_some_and(|p| r.contains(p))));
    let mut ed = Editing {
        text,
        buf,
        view: &mut view,
        moved: false,
        catalog,
    };
    if let Some((a, c)) = select {
        ed.view.anchor = a.min(total);
        ed.view.cursor = c.min(total);
        ed.moved = true;
    }
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    // Com a lista aberta, Esc fecha a lista e o foco fica.
                    escape: ed.buf.popup.is_some(),
                },
            )
        });
        if list_members {
            ed.refresh_popup(true);
        }
        let events = ui.input(|i| i.events.clone());
        let mut alt_chord = false;
        for event in events {
            // No Linux, Shift+Alt+F (Format Document) também manda o texto
            // "F" depois da tecla: um atalho com Alt (sem Ctrl, que é AltGr
            // no Windows) não digita. No macOS, Option digita caracteres.
            if let Event::Key {
                pressed: true,
                modifiers,
                ..
            } = &event
            {
                alt_chord = modifiers.alt && !modifiers.ctrl && !cfg!(target_os = "macos");
            } else if alt_chord && matches!(event, Event::Text(_)) {
                alt_chord = false;
                continue;
            }
            handle_event(ui, &mut ed, &f, event);
        }
    } else if !over_popup {
        ed.buf.popup = None;
    }
    mouse(ui, &response, &mut ed, &f);

    // Rolagem.
    let content = vec2(
        (ed.index().max_line_chars() as f32 + 4.0) * f.char_w,
        ed.index().line_count() as f32 * f.line_h,
    );
    if response.hovered() {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        ed.view.scroll -= delta;
    }
    if ed.moved {
        reveal(ui, &mut ed, &f);
    }
    scrollbars(ui, id, rect, &f, content, ed.view);
    // Dá para rolar até a última linha ficar no topo.
    let max = vec2(
        (content.x - text_rect.width()).max(0.0),
        (content.y - f.line_h).max(0.0),
    );
    ed.view.scroll = ed.view.scroll.clamp(Vec2::ZERO, max);

    paint(ui, &ed, &f, gutter, focused);
    squiggles(ui, &response, &ed, &f);
    ed.buf.popup_rect = None;
    if (focused || over_popup) && popup(ui, id, &mut ed, &f) {
        ui.memory_mut(|m| m.request_focus(id));
        ctx.request_repaint();
    }

    let (line, col) = ed.index().line_col(ed.view.cursor);
    if focused {
        let galley = line_galley(ui, ed.text, ed.buf, line, &f);
        let x = text_rect.left() + col_x(&galley, col) - ed.view.scroll.x;
        let y = text_rect.top() + line as f32 * f.line_h - ed.view.scroll.y;
        let cursor_rect = Rect::from_min_size(pos2(x, y), vec2(2.0, f.line_h));
        ui.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::viewport::IMEPurpose::Normal,
                rect: text_rect,
                cursor_rect,
                should_interrupt_composition: false,
            });
        });
    }
    store(&ctx, id, view);
    Output {
        response,
        cursor: (line + 1, col + 1),
    }
}

fn handle_event(ui: &Ui, ed: &mut Editing<'_>, f: &Frame, event: Event) {
    match event {
        Event::Text(s) => {
            // Quebras de linha e tabs vêm de texto colado ou de testes; os
            // outros caracteres de controle são ignorados.
            let s: String = s
                .replace("\r\n", "\n")
                .chars()
                .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                .collect();
            if !s.is_empty() {
                ed.insert(&s, EditKind::Typing);
                ed.view.preferred_x = None;
                // Letras e `.` abrem (ou atualizam) a lista, como no SSMS;
                // o resto fecha.
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if is_word(c) || c == '.' => ed.refresh_popup(true),
                    _ => ed.buf.popup = None,
                }
            }
        }
        Event::Ime(egui::ImeEvent::Commit(s)) if !s.is_empty() => {
            ed.insert(&s, EditKind::Other);
            ed.refresh_popup(false);
        }
        Event::Copy => {
            let s = ed.selected();
            if !s.is_empty() {
                ui.ctx().copy_text(s);
            }
        }
        Event::Cut => {
            let s = ed.selected();
            ed.buf.popup = None;
            if !s.is_empty() {
                ui.ctx().copy_text(s);
                ed.insert("", EditKind::Other);
                ed.buf.history.seal();
            }
        }
        Event::Paste(s) => {
            let s = s.replace("\r\n", "\n").replace('\r', "\n");
            ed.buf.popup = None;
            ed.insert(&s, EditKind::Other);
            ed.buf.history.seal();
        }
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => key_pressed(ui, ed, f, key, modifiers),
        _ => {}
    }
}

fn key_pressed(ui: &Ui, ed: &mut Editing<'_>, f: &Frame, key: Key, m: Modifiers) {
    let ctrl = m.ctrl || m.command || m.mac_cmd;
    let shift = m.shift;
    if m.alt {
        return;
    }
    if ed.buf.popup.is_some() && !ctrl && !shift {
        match key {
            Key::Escape => {
                ed.buf.popup = None;
                return;
            }
            Key::ArrowUp => return ed.move_popup(-1),
            Key::ArrowDown => return ed.move_popup(1),
            Key::PageUp => return ed.move_popup(-(POPUP_ROWS as i64)),
            Key::PageDown => return ed.move_popup(POPUP_ROWS as i64),
            Key::Enter | Key::Tab => {
                // A palavra já está inteira: a tecla faz o que faria sem a
                // lista (Enter quebra a linha).
                if ed.typed_selection() {
                    ed.buf.popup = None;
                } else {
                    return ed.accept();
                }
            }
            _ => {}
        }
    }
    // Ctrl+Espaço completa a palavra (direto, se só houver uma opção);
    // Ctrl+J lista os membros.
    if ctrl && !shift && matches!(key, Key::Space | Key::J) {
        ed.refresh_popup(true);
        if key == Key::Space
            && ed
                .buf
                .popup
                .as_ref()
                .is_some_and(|p| p.completion.items.len() == 1)
        {
            ed.accept();
        }
        return;
    }
    key_action(ui, ed, f, key, m);
    if matches!(key, Key::Backspace | Key::Delete) && !ctrl {
        ed.refresh_popup(false);
    } else {
        ed.buf.popup = None;
    }
}

fn key_action(ui: &Ui, ed: &mut Editing<'_>, f: &Frame, key: Key, m: Modifiers) {
    let ctrl = m.ctrl || m.command || m.mac_cmd;
    let shift = m.shift;
    let vertical = matches!(
        key,
        Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown
    );
    if !vertical {
        ed.view.preferred_x = None;
    }
    let cur = ed.view.cursor;
    let r = ed.range();
    match key {
        Key::ArrowLeft => {
            if !shift && !r.is_empty() && !ctrl {
                ed.move_to(r.start, false);
            } else if ctrl {
                ed.move_to(ed.word_left(cur), shift);
            } else {
                ed.move_to(cur.saturating_sub(1), shift);
            }
        }
        Key::ArrowRight => {
            if !shift && !r.is_empty() && !ctrl {
                ed.move_to(r.end, false);
            } else if ctrl {
                ed.move_to(ed.word_right(cur), shift);
            } else {
                ed.move_to(cur + 1, shift);
            }
        }
        Key::ArrowUp | Key::ArrowDown if ctrl => {
            // Ctrl+setas rolam sem mover o cursor, como no SSMS.
            let dir = if key == Key::ArrowUp { -1.0 } else { 1.0 };
            ed.view.scroll.y += dir * f.line_h;
        }
        Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown => {
            let page = f.visible_lines() as i64;
            let delta = match key {
                Key::ArrowUp => -1,
                Key::ArrowDown => 1,
                Key::PageUp => -page,
                _ => page,
            };
            let (line, col) = ed.index().line_col(cur);
            let x = ed
                .view
                .preferred_x
                .unwrap_or_else(|| col_x(&line_galley(ui, ed.text, ed.buf, line, f), col));
            let last = ed.index().line_count() as i64 - 1;
            let target = line as i64 + delta;
            let c = if target < 0 {
                0
            } else if target > last {
                ed.len()
            } else {
                let t = target as usize;
                let g = line_galley(ui, ed.text, ed.buf, t, f);
                ed.index().char_at(t, x_col(&g, x))
            };
            ed.move_to(c, shift);
            ed.view.preferred_x = Some(x);
            if matches!(key, Key::PageUp | Key::PageDown) {
                ed.view.scroll.y += delta as f32 * f.line_h;
            }
        }
        Key::Home => {
            if ctrl {
                ed.move_to(0, shift);
            } else {
                // Home inteligente: primeiro caractere não branco, depois a
                // coluna 0.
                let (line, col) = ed.index().line_col(cur);
                let indent = ed
                    .line_chars(line)
                    .iter()
                    .take_while(|c| c.is_whitespace())
                    .count();
                let target = if col == indent { 0 } else { indent };
                ed.move_to(ed.index().char_at(line, target), shift);
            }
        }
        Key::End => {
            if ctrl {
                ed.move_to(ed.len(), shift);
            } else {
                let line = ed.index().line_of(cur);
                ed.move_to(ed.index().char_at(line, usize::MAX), shift);
            }
        }
        Key::Backspace => {
            if r.is_empty() {
                let from = if ctrl {
                    ed.word_left(cur)
                } else {
                    cur.saturating_sub(1)
                };
                ed.replace(from..cur, "", EditKind::Deleting);
            } else {
                ed.insert("", EditKind::Other);
                ed.buf.history.seal();
            }
        }
        Key::Delete => {
            if r.is_empty() {
                let to = if ctrl {
                    ed.word_right(cur)
                } else {
                    (cur + 1).min(ed.len())
                };
                ed.replace(cur..to, "", EditKind::Deleting);
            } else {
                ed.insert("", EditKind::Other);
                ed.buf.history.seal();
            }
        }
        Key::Enter => {
            let indent = ed.indent_before(r.start);
            ed.insert(&format!("\n{indent}"), EditKind::Other);
            ed.buf.history.seal();
        }
        Key::Tab => {
            let multi = ed.selected_lines().len() > 1;
            if shift || multi {
                ed.indent_lines(shift);
            } else {
                let (_, col) = ed.index().line_col(r.start);
                let n = TAB_SIZE - col % TAB_SIZE;
                ed.insert(&" ".repeat(n), EditKind::Other);
                ed.buf.history.seal();
            }
        }
        Key::A if ctrl => ed.select(0..ed.len()),
        Key::Z if ctrl && shift => ed.redo(),
        Key::Z if ctrl => ed.undo(),
        Key::Y if ctrl => ed.redo(),
        _ => {}
    }
}

/// Caractere sob o ponteiro.
fn char_at_pos(ui: &Ui, ed: &Editing<'_>, f: &Frame, pos: Pos2) -> usize {
    let y = pos.y - f.text_rect.top() + ed.view.scroll.y;
    let line = (y / f.line_h).floor().max(0.0) as usize;
    let line = line.min(ed.index().line_count() - 1);
    let x = pos.x - f.text_rect.left() + ed.view.scroll.x;
    let g = line_galley(ui, ed.text, ed.buf, line, f);
    ed.index().char_at(line, x_col(&g, x.max(0.0)))
}

fn mouse(ui: &Ui, response: &egui::Response, ed: &mut Editing<'_>, f: &Frame) {
    // No frame em que o botão solta (duplo e triplo clique) a resposta já
    // não tem a posição do ponteiro; a do contexto ainda tem.
    let pos = response.interact_pointer_pos().or_else(|| {
        (response.double_clicked() || response.triple_clicked())
            .then(|| ui.ctx().pointer_interact_pos())
            .flatten()
    });
    let Some(pos) = pos else {
        return;
    };
    let c = char_at_pos(ui, ed, f, pos);
    let (pressed, shift) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
    if response.triple_clicked() {
        let line = ed.line_at(c);
        ed.select(line);
        ed.view.drag_words = None;
        return;
    }
    if response.double_clicked() {
        let word = ed.word_at(c);
        ed.view.drag_words = Some((word.start, word.end));
        ed.select(word);
        return;
    }
    if pressed {
        ed.buf.popup = None;
        ed.view.drag_words = None;
        ed.view.preferred_x = None;
        ed.move_to(c, shift);
        ed.buf.history.seal();
    } else if response.dragged() {
        match ed.view.drag_words {
            Some((a, b)) => {
                let w = ed.word_at(c);
                if c < a {
                    ed.view.anchor = b;
                    ed.view.cursor = w.start;
                } else {
                    ed.view.anchor = a;
                    ed.view.cursor = w.end.max(b);
                }
                ed.moved = true;
            }
            None => ed.move_to(c, true),
        }
    }
}

/// Rola para o cursor ficar visível.
fn reveal(ui: &Ui, ed: &mut Editing<'_>, f: &Frame) {
    let (line, col) = ed.index().line_col(ed.view.cursor);
    let top = line as f32 * f.line_h;
    let view_h = f.text_rect.height();
    if top < ed.view.scroll.y {
        ed.view.scroll.y = top;
    } else if top + f.line_h > ed.view.scroll.y + view_h {
        ed.view.scroll.y = top + f.line_h - view_h;
    }
    let g = line_galley(ui, ed.text, ed.buf, line, f);
    let x = col_x(&g, col);
    let view_w = f.text_rect.width();
    let margin = f.char_w * 4.0;
    if x < ed.view.scroll.x {
        ed.view.scroll.x = (x - margin).max(0.0);
    } else if x + f.char_w > ed.view.scroll.x + view_w {
        ed.view.scroll.x = x + f.char_w - view_w + margin;
    }
}

/// Barras de rolagem: arrastar o polegar ou clicar na trilha.
fn scrollbars(ui: &Ui, id: Id, rect: Rect, f: &Frame, content: Vec2, view: &mut View) {
    let v = ui.visuals();
    let track_color = v.extreme_bg_color;
    let thumb_color = v.widgets.inactive.bg_fill;
    let hot_color = v.widgets.active.bg_fill;
    let viewport = f.text_rect.size();
    for axis in [1usize, 0] {
        let (track, len, view_len, total) = if axis == 1 {
            let t = Rect::from_min_max(
                pos2(rect.right() - SCROLLBAR, rect.top()),
                pos2(rect.right(), rect.bottom() - SCROLLBAR),
            );
            (t, t.height(), viewport.y, content.y + viewport.y - f.line_h)
        } else {
            let t = Rect::from_min_max(
                pos2(rect.left(), rect.bottom() - SCROLLBAR),
                pos2(rect.right() - SCROLLBAR, rect.bottom()),
            );
            (t, t.width(), viewport.x, content.x)
        };
        ui.painter().rect_filled(track, 0.0, track_color);
        if total <= view_len || len <= 0.0 {
            continue;
        }
        let max_scroll = total - view_len;
        let thumb_len = (len * view_len / total).max(20.0).min(len);
        let free = len - thumb_len;
        let pos = view.scroll[axis] / max_scroll * free;
        let thumb = if axis == 1 {
            Rect::from_min_size(
                pos2(track.left() + 2.0, track.top() + pos),
                vec2(SCROLLBAR - 4.0, thumb_len),
            )
        } else {
            Rect::from_min_size(
                pos2(track.left() + pos, track.top() + 2.0),
                vec2(thumb_len, SCROLLBAR - 4.0),
            )
        };
        let r = ui.interact(track, id.with(("scrollbar", axis)), Sense::click_and_drag());
        if r.dragged() {
            let d = r.drag_delta()[axis];
            if free > 0.0 {
                view.scroll[axis] += d / free * max_scroll;
            }
        } else if r.clicked()
            && let Some(p) = r.interact_pointer_pos()
        {
            // Clicar na trilha pula uma página para aquele lado.
            if p[axis] < thumb.min[axis] {
                view.scroll[axis] -= view_len;
            } else if p[axis] > thumb.max[axis] {
                view.scroll[axis] += view_len;
            }
        }
        let color = if r.dragged() || r.hovered() {
            hot_color
        } else {
            thumb_color
        };
        ui.painter().rect_filled(thumb, 3.0, color);
    }
}

fn paint(ui: &Ui, ed: &Editing<'_>, f: &Frame, gutter: Rect, focused: bool) {
    let v = ui.visuals();
    let painter = ui.painter();
    let full = Rect::from_min_max(gutter.min, f.text_rect.max);
    painter.rect_filled(full, 0.0, v.extreme_bg_color);
    let text_painter = painter.with_clip_rect(f.text_rect.expand2(vec2(TEXT_MARGIN, 0.0)));
    let gutter_painter = painter.with_clip_rect(gutter);

    let first = (ed.view.scroll.y / f.line_h).floor().max(0.0) as usize;
    let count = (f.text_rect.height() / f.line_h).ceil() as usize + 1;
    let last = (first + count).min(ed.index().line_count());
    let sel = ed.range();
    let (cur_line, cur_col) = ed.index().line_col(ed.view.cursor);
    let sel_color = if focused {
        v.selection.bg_fill
    } else {
        v.selection.bg_fill.linear_multiply(0.5)
    };
    let left = f.text_rect.left() - ed.view.scroll.x;
    for line in first..last {
        let y = f.text_rect.top() + line as f32 * f.line_h - ed.view.scroll.y;
        let galley = line_galley(ui, ed.text, ed.buf, line, f);
        let start = ed.index().line_start(line);
        let len = ed.index().line_len(line);
        if line == cur_line && sel.is_empty() && focused {
            let row = Rect::from_min_size(
                pos2(f.text_rect.left() - TEXT_MARGIN, y),
                vec2(f.text_rect.width() + TEXT_MARGIN, f.line_h),
            );
            text_painter.rect_filled(row, 0.0, v.faint_bg_color);
        }
        // Seleção nesta linha (o `\n` selecionado aparece como meio caractere).
        let a = sel.start.max(start);
        let b = sel.end.min(start + len);
        if !sel.is_empty() && a <= b && sel.start <= start + len && sel.end >= start {
            let x0 = left + col_x(&galley, a - start);
            let mut x1 = left + col_x(&galley, b - start);
            if sel.end > start + len {
                x1 += f.char_w * 0.5;
            }
            if x1 > x0 {
                text_painter.rect_filled(
                    Rect::from_min_max(pos2(x0, y), pos2(x1, y + f.line_h)),
                    0.0,
                    sel_color,
                );
            }
        }
        text_painter.galley(pos2(left, y), galley.clone(), f.colors.text);
        if focused && line == cur_line {
            let x = left + col_x(&galley, cur_col);
            text_painter.line_segment(
                [pos2(x, y), pos2(x, y + f.line_h)],
                Stroke::new(2.0, v.text_cursor.stroke.color),
            );
        }
        gutter_painter.text(
            pos2(gutter.right() - 8.0, y),
            egui::Align2::RIGHT_TOP,
            (line + 1).to_string(),
            f.font.clone(),
            if line == cur_line {
                v.strong_text_color()
            } else {
                f.colors.operator
            },
        );
    }
}

/// Desenha a lista de sugestões embaixo da palavra. Devolve `true` se uma
/// sugestão foi escolhida com o mouse.
fn popup(ui: &Ui, id: Id, ed: &mut Editing<'_>, f: &Frame) -> bool {
    let Some(p) = &ed.buf.popup else {
        return false;
    };
    let start = ed
        .view
        .cursor
        .saturating_sub(p.completion.prefix.chars().count());
    let (line, col) = ed.index().line_col(start);
    let galley = line_galley(ui, ed.text, ed.buf, line, f);
    let x = f.text_rect.left() + col_x(&galley, col) - ed.view.scroll.x;
    let y = f.text_rect.top() + (line + 1) as f32 * f.line_h - ed.view.scroll.y;
    let row_h = f.line_h + 4.0;
    let rows = p.completion.items.len().min(POPUP_ROWS);
    let height = rows as f32 * row_h;
    // Embaixo da linha; se não couber, em cima.
    let screen = ui.ctx().content_rect();
    let pos = if y + height + 8.0 > screen.bottom() {
        pos2(x, y - f.line_h - height - 8.0)
    } else {
        pos2(x, y)
    };
    let v = ui.visuals().clone();
    let mut clicked = None;
    let mut scroll = 0.0;
    let font = f.font.clone();
    let colors = f.colors;
    let area = egui::Area::new(id.with("intellisense"))
        .order(egui::Order::Foreground)
        // Sem o fade: a lista aparece inteira já no primeiro frame.
        .fade_in(false)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(2.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let items = &p.completion.items;
                    for (i, item) in items.iter().enumerate().skip(p.top).take(rows) {
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(POPUP_WIDTH, row_h), Sense::click());
                        let painter = ui.painter();
                        let selected = i == p.selected;
                        if selected {
                            painter.rect_filled(rect, 2.0, v.selection.bg_fill);
                        } else if resp.hovered() {
                            painter.rect_filled(rect, 2.0, v.widgets.hovered.weak_bg_fill);
                        }
                        // Texto branco na linha escolhida, como no SSMS.
                        let text_color = if selected {
                            Color32::WHITE
                        } else {
                            colors.text
                        };
                        let glyph_color = match item.kind {
                            Kind::Column | Kind::Alias => colors.identifier_quoted,
                            Kind::Table | Kind::View | Kind::Schema => colors.keyword,
                            Kind::Function => colors.number,
                            Kind::Keyword => colors.operator,
                        };
                        let glyph_color = if selected {
                            Color32::WHITE
                        } else {
                            glyph_color
                        };
                        let mid = rect.left_center();
                        painter.text(
                            mid + vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            item.kind.glyph(),
                            font.clone(),
                            glyph_color,
                        );
                        painter.text(
                            mid + vec2(24.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &item.label,
                            font.clone(),
                            text_color,
                        );
                        if !item.detail.is_empty() {
                            painter.text(
                                rect.right_center() - vec2(6.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                &item.detail,
                                FontId::proportional(11.0),
                                if selected {
                                    Color32::WHITE
                                } else {
                                    v.weak_text_color()
                                },
                            );
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                    }
                });
        });
    ed.buf.popup_rect = Some(area.response.rect);
    if area.response.hovered() {
        scroll = ui.input(|i| i.smooth_scroll_delta.y);
    }
    if scroll != 0.0
        && let Some(p) = &mut ed.buf.popup
    {
        let max = p.completion.items.len().saturating_sub(POPUP_ROWS);
        let step = (scroll / row_h).round() as i64;
        p.top = (p.top as i64 - step).clamp(0, max as i64) as usize;
    }
    match clicked {
        Some(i) => {
            if let Some(p) = &mut ed.buf.popup {
                p.selected = i;
            }
            ed.accept();
            true
        }
        None => false,
    }
}

/// Sublinha os erros com uma linha ondulada vermelha; parar o mouse em cima
/// mostra a mensagem.
fn squiggles(ui: &Ui, response: &egui::Response, ed: &Editing<'_>, f: &Frame) {
    let buf = &ed.buf;
    if buf.diagnostics.is_empty() && buf.server_error.is_none() {
        return;
    }
    let painter = ui.painter().with_clip_rect(f.text_rect);
    let color = Color32::from_rgb(0xE5, 0x14, 0x00);
    let first = (ed.view.scroll.y / f.line_h).floor().max(0.0) as usize;
    let count = (f.text_rect.height() / f.line_h).ceil() as usize + 1;
    let last = (first + count).min(ed.index().line_count());
    let hover = response.hover_pos();
    let mut tip = None;
    let left = f.text_rect.left() - ed.view.scroll.x;
    for d in buf.diagnostics.iter().chain(buf.server_error.iter()) {
        for line in first..last {
            let bytes = ed.index().line_bytes(line);
            // Um erro no `\n` (ou vazio) ainda aparece: meio caractere.
            let a = d.range.start.max(bytes.start);
            let b = d.range.end.min(bytes.end);
            let before = d.range.end <= bytes.start && d.range.start < bytes.start;
            if a > b || d.range.start > bytes.end || before {
                continue;
            }
            let galley = line_galley(ui, ed.text, ed.buf, line, f);
            let col = |byte: usize| ed.text[bytes.start..byte].chars().count();
            let x0 = left + col_x(&galley, col(a));
            let x1 = (left + col_x(&galley, col(b))).max(x0 + f.char_w * 0.5);
            let y = f.text_rect.top() + (line + 1) as f32 * f.line_h - ed.view.scroll.y - 1.5;
            let mut points = Vec::new();
            let mut x = x0;
            let mut up = false;
            while x < x1 {
                points.push(pos2(x, if up { y - 2.0 } else { y }));
                x += 2.0;
                up = !up;
            }
            points.push(pos2(x1, if up { y - 2.0 } else { y }));
            painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
            let rect = Rect::from_min_max(pos2(x0, y - f.line_h), pos2(x1, y + 2.0));
            if hover.is_some_and(|p| rect.contains(p)) {
                tip = Some(d.message.clone());
            }
        }
    }
    if let Some(message) = tip
        && ed.buf.popup.is_none()
    {
        response.show_tooltip_text(message);
    }
}
