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

use crate::buffer::{Change, EditKind, History, LineIndex, Sel, is_word, line_text};
use crate::complete::{Completion, Kind, best, complete};
use crate::diagnostics::{Diagnostic, server_range};
use crate::highlight::{Colors, layout_job_in, line_job};
use crate::hints::{HintAction, HintKind, HintState, HintStatus};
use crate::lexer::{Dialect, Token, tokenize_in};
use pgstudio_core::catalog::Catalog;
use pgstudio_i18n::{tr, trf};

/// Espaços de um Tab.
pub const TAB_SIZE: usize = 4;
const SCROLLBAR: f32 = 10.0;
const TEXT_MARGIN: f32 = 4.0;

/// Seleção e rolagem do editor, guardadas na memória do egui.
#[derive(Clone, Debug, Default)]
pub(crate) struct View {
    pub anchor: usize,
    pub cursor: usize,
    pub scroll: Vec2,
    /// Os outros cursores (âncora, cursor), em ordem e sem se sobrepor. O
    /// principal é `anchor`/`cursor`: é o que o resto do app enxerga.
    pub extra: Vec<Sel>,
    /// Posição x desejada ao subir e descer linhas.
    preferred_x: Option<f32>,
    /// Arrastando depois de um duplo clique: seleciona por palavra.
    drag_words: Option<(usize, usize)>,
    /// Seleção em coluna em andamento (Alt+arrastar, Alt+Shift+setas).
    block: Option<Block>,
    /// O arrasto do mouse está fazendo a seleção em coluna.
    drag_block: bool,
    /// Onde o Alt+clique caiu: arrastar dali vira seleção em coluna.
    block_origin: Option<(usize, usize)>,
    /// O arrasto depois do triplo clique seleciona por linha.
    drag_lines: bool,
    /// Seleções anteriores de "expandir seleção", para encolher de volta, e
    /// a seleção que a última expansão deixou (se mudou, a pilha não vale).
    shrink: Vec<Sel>,
    grown: Option<Sel>,
    /// A tecla anterior foi Ctrl+K: a próxima é o fim de um acorde do app.
    after_ctrl_k: bool,
    /// O último aperto do botão (hora, lugar e quantos seguidos): o duplo e
    /// o triplo clique valem já no aperto, para dar para arrastar.
    last_press: Option<(f64, Pos2, u8)>,
}

/// Os dois cantos de uma seleção em coluna: (linha, coluna). A coluna pode
/// passar do fim da linha, para o bloco atravessar linhas curtas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block {
    anchor: (usize, usize),
    cursor: (usize, usize),
}

impl View {
    /// Fica só com o cursor principal.
    pub(crate) fn collapse(&mut self) {
        self.extra.clear();
        self.block = None;
        self.drag_block = false;
        self.block_origin = None;
    }

    /// Todas as seleções em ordem, e qual delas é a principal.
    pub(crate) fn sels(&self) -> (Vec<Sel>, usize) {
        let key = self.anchor.min(self.cursor);
        let at = self.extra.partition_point(|s| s.0.min(s.1) < key);
        let mut all = self.extra.clone();
        all.insert(at, (self.anchor, self.cursor));
        (all, at)
    }

    /// Troca as seleções por `sels` (a de índice `primary` é a principal),
    /// em ordem e juntando as que se sobrepõem.
    fn set_sels(&mut self, sels: Vec<Sel>, primary: usize) {
        let mut all: Vec<(Sel, bool)> = sels
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s, i == primary))
            .collect();
        all.sort_by_key(|(s, _)| (s.0.min(s.1), s.0.max(s.1)));
        let mut out: Vec<(Sel, bool)> = Vec::with_capacity(all.len());
        for (s, main) in all {
            let (a, b) = (s.0.min(s.1), s.0.max(s.1));
            if let Some((last, last_main)) = out.last_mut() {
                let (la, lb) = (last.0.min(last.1), last.0.max(last.1));
                if a < lb || (a == lb && (a == b || la == lb)) {
                    let end = lb.max(b);
                    // Fica com o sentido da principal (ou da primeira).
                    let forward = if main { s.1 >= s.0 } else { last.1 >= last.0 };
                    *last = if forward { (la, end) } else { (end, la) };
                    *last_main |= main;
                    continue;
                }
            }
            out.push((s, main));
        }
        self.extra.clear();
        for (s, main) in out {
            if main {
                (self.anchor, self.cursor) = s;
            } else {
                self.extra.push(s);
            }
        }
    }
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
    /// Sugestões da IA para o comando do cursor.
    pub(crate) ai: HintState,
    /// Sobe a cada mudança do texto: quem observa o editor (a IA) compara
    /// o número em vez do texto.
    pub(crate) revision: u64,
}

/// A lista de sugestões aberta embaixo do cursor.
#[derive(Clone, Debug)]
struct Popup {
    completion: Completion,
    selected: usize,
    /// Primeira linha visível da lista.
    top: usize,
    /// Largura que a lista quer para mostrar o nome e o detalhe inteiros
    /// da sugestão mais comprida. Medida uma vez por lista; enquanto ela
    /// fica aberta só cresce, para não dançar a cada letra.
    width: Option<f32>,
}

/// Linhas visíveis da lista de sugestões.
const POPUP_ROWS: usize = 10;
const POPUP_WIDTH: f32 = 420.0;
/// Até onde a lista alarga para caber um nome comprido; depois disso o
/// texto é encurtado com reticências.
const POPUP_MAX_WIDTH: f32 = 760.0;
/// Numa linha da lista: onde o nome começa, a margem direita do detalhe e
/// o espaço mínimo entre os dois.
const ROW_LEFT: f32 = 24.0;
const ROW_RIGHT: f32 = 6.0;
const ROW_GAP: f32 = 16.0;
const DETAIL_FONT: f32 = 11.0;

/// A largura da lista: a que o conteúdo pede, entre a largura padrão e o
/// limite, e nunca mais do que cabe na janela.
fn popup_width(wanted: f32, screen: f32) -> f32 {
    wanted
        .clamp(POPUP_WIDTH, POPUP_MAX_WIDTH)
        .min((screen - 16.0).max(120.0))
}

/// Quanto cabe ao nome e ao detalhe numa linha com `avail` de largura
/// útil. Cabendo os dois, cada um fica com o que pede; senão o detalhe
/// cede primeiro (até ficar com 30% da linha) e o nome leva o resto. Os
/// dois nunca se encontram: sobra sempre `ROW_GAP` entre eles.
fn split_row(avail: f32, label: f32, detail: f32) -> (f32, f32) {
    if detail <= 0.0 {
        return (avail.max(0.0), 0.0);
    }
    if label + ROW_GAP + detail <= avail {
        return (label, detail);
    }
    let detail = detail
        .min((avail * 0.3).max(avail - ROW_GAP - label))
        .max(0.0);
    ((avail - ROW_GAP - detail).max(0.0), detail)
}

/// Uma linha só de texto, encurtada com reticências em `max_width`.
fn one_line(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    painter.layout_job(job)
}
/// Letra e margem da explicação embaixo da lista.
const DOC_FONT: f32 = 12.0;
const DOC_MARGIN: f32 = 6.0;

impl Buffer {
    /// Devolve `true` se o texto mudou por fora do editor.
    fn sync(&mut self, text: &str, view: &View) -> bool {
        if self.synced && self.shadow == text {
            return false;
        }
        if self.synced
            && let Some(change) = Change::between(&self.shadow, text)
        {
            let sel = (view.anchor, view.cursor);
            self.history.record(change, EditKind::Other, sel, sel);
            self.history.seal();
        }
        self.rebuild(text);
        true
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
        self.revision += 1;
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
        let c = match self.buf.dialect {
            Dialect::Redis => crate::redis::complete(self.text, &self.buf.tokens, at, self.catalog),
            Dialect::MongoDb => {
                crate::mongodb::complete(self.text, &self.buf.tokens, at, self.catalog)
            }
            Dialect::Elasticsearch => {
                crate::elasticsearch::complete(self.text, &self.buf.tokens, at, self.catalog)
            }
            _ => complete(self.text, &self.buf.tokens, at, self.catalog),
        };
        if c.items.is_empty() {
            self.buf.popup = None;
            return;
        }
        let selected = best(&c);
        self.buf.ai.open = None;
        // A lista continua aberta enquanto se digita: não encolhe.
        let keep = self.buf.popup.as_ref().and_then(|p| p.width);
        self.buf.popup = Some(Popup {
            completion: c,
            selected,
            top: selected.saturating_sub(POPUP_ROWS / 2),
            width: keep,
        });
    }

    /// Abre a lista das sugestões da IA; sem sugestões, pede à casca que
    /// pergunte à IA.
    fn open_hints(&mut self) {
        self.buf.popup = None;
        if self.buf.ai.count() == 0 {
            self.buf.ai.open = None;
            self.buf.ai.requested = true;
        } else {
            self.buf.ai.open = Some(0);
        }
    }

    fn move_hint(&mut self, delta: i64) {
        let last = self.buf.ai.count() as i64 - 1;
        if let Some(i) = &mut self.buf.ai.open {
            *i = (*i as i64 + delta).clamp(0, last.max(0)) as usize;
        }
    }

    /// Aplica a sugestão escolhida da IA: troca o comando inteiro, ou
    /// acrescenta outro depois dele. Um passo só do desfazer.
    fn apply_hint(&mut self) {
        let Some(i) = self.buf.ai.open.take() else {
            return;
        };
        let Some(hints) = &self.buf.ai.hints else {
            return;
        };
        let Some(item) = hints.items.get(i).cloned() else {
            return;
        };
        let range = hints.range.clone();
        // O comando mudou desde que a IA leu: a sugestão não vale mais.
        if self.text.get(range.clone()) != Some(hints.base.as_str()) {
            self.buf.ai.hints = None;
            return;
        }
        self.view.collapse();
        self.buf.history.seal();
        match &item.action {
            HintAction::Replace(code) => {
                let a = crate::edit::char_of(self.text, range.start);
                let b = crate::edit::char_of(self.text, range.end);
                self.replace(a..b, code, EditKind::Other);
                self.buf.ai.hints = None;
            }
            HintAction::Insert(code) => {
                // Depois do `;` do comando, se já houver um.
                let rest = &self.text[range.end..];
                let blank = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                let sql = matches!(self.buf.dialect, Dialect::Postgres | Dialect::TSql);
                let (at, lead) = if sql && rest[blank..].starts_with(';') {
                    (range.end + blank + 1, "\n\n")
                } else if sql && self.buf.dialect == Dialect::Postgres {
                    (range.end, ";\n\n")
                } else {
                    (range.end, "\n\n")
                };
                let c = crate::edit::char_of(self.text, at);
                self.replace(c..c, &format!("{lead}{code}"), EditKind::Other);
                if let Some(hints) = &mut self.buf.ai.hints {
                    hints.items.remove(i);
                }
            }
            HintAction::None => {
                // Um conselho: não há o que aplicar; a lista fica.
                self.buf.ai.open = Some(i);
                return;
            }
        }
        self.buf.history.seal();
        self.buf.ai.applied = Some(item);
    }

    /// Se a linha do cursor tem algo escrito antes dele.
    fn line_has_text(&self) -> bool {
        let at = self.buf.index.byte_of(self.text, self.view.cursor);
        let line = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        !self.text[line..at].trim().is_empty()
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
        if self.multi() {
            // Completa em todos os cursores que têm a mesma palavra antes.
            let prefix = p.completion.prefix.to_lowercase();
            let (sels, _) = self.view.sels();
            let edits = sels
                .iter()
                .map(|&(_, c)| {
                    let from = c.saturating_sub(n);
                    let a = self.buf.index.byte_of(self.text, from);
                    let b = self.buf.index.byte_of(self.text, c);
                    if self.text[a..b].to_lowercase() == prefix {
                        (from..c, item.insert.clone())
                    } else {
                        (c..c, String::new())
                    }
                })
                .collect();
            self.edit_each(edits, EditKind::Other);
        } else {
            self.replace(cur.saturating_sub(n)..cur, &item.insert, EditKind::Other);
        }
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

    /// Se há mais de um cursor.
    fn multi(&self) -> bool {
        !self.view.extra.is_empty()
    }

    /// Aplica `edits` (em ordem, sem se sobrepor) ao texto de uma vez.
    /// Devolve a mudança, do começo da primeira ao fim da última, e onde
    /// termina o que cada uma inseriu. `None` se nada mudou.
    fn splice(&mut self, edits: &[(Range<usize>, String)]) -> Option<(Change, Vec<usize>)> {
        let start = edits.first()?.0.start;
        let end = edits.last()?.0.end;
        let a = self.buf.index.byte_of(self.text, start);
        let b = self.buf.index.byte_of(self.text, end);
        let mut inserted = String::new();
        let mut chars = 0;
        let mut copied = a;
        let mut ends = Vec::with_capacity(edits.len());
        for (r, s) in edits {
            let from = self.buf.index.byte_of(self.text, r.start);
            let gap = &self.text[copied..from];
            chars += gap.chars().count() + s.chars().count();
            inserted.push_str(gap);
            inserted.push_str(s);
            copied = self.buf.index.byte_of(self.text, r.end);
            ends.push(start + chars);
        }
        let removed = self.text[a..b].to_string();
        if removed == inserted {
            return None;
        }
        self.text.replace_range(a..b, &inserted);
        self.buf.apply(self.text, a, removed.len(), inserted.len());
        let change = Change {
            at: start,
            removed,
            inserted,
        };
        Some((change, ends))
    }

    /// Faz uma troca por cursor, todas num passo só do desfazer. `edits`
    /// segue a ordem de [`View::sels`]; cada cursor fica depois do que foi
    /// inserido nele. Trocas que se sobrepõem são aparadas.
    fn edit_each(&mut self, mut edits: Vec<(Range<usize>, String)>, kind: EditKind) {
        let (_, primary) = self.view.sels();
        let before = (
            (self.view.anchor, self.view.cursor),
            self.view.extra.clone(),
        );
        let mut end = 0;
        for (r, _) in &mut edits {
            r.start = r.start.max(end);
            r.end = r.end.max(r.start);
            end = r.end;
        }
        let Some((change, ends)) = self.splice(&edits) else {
            return;
        };
        self.view
            .set_sels(ends.into_iter().map(|c| (c, c)).collect(), primary);
        self.view.block = None;
        let after = (
            (self.view.anchor, self.view.cursor),
            self.view.extra.clone(),
        );
        self.buf.history.record_multi(change, kind, before, after);
        self.moved = true;
    }

    /// Faz `edits` (em ordem, sem se sobrepor) num passo só do desfazer e
    /// mantém as seleções, deslocadas junto com o texto.
    fn edit_keep(&mut self, edits: Vec<(Range<usize>, String)>) {
        let (sels, primary) = self.view.sels();
        let before = (
            (self.view.anchor, self.view.cursor),
            self.view.extra.clone(),
        );
        let map = |p: usize| {
            let mut delta = 0isize;
            for (r, s) in &edits {
                let len = s.chars().count();
                if p >= r.end {
                    delta += len as isize - r.len() as isize;
                } else if p > r.start {
                    return (r.start as isize + delta) as usize + len;
                } else {
                    break;
                }
            }
            (p as isize + delta) as usize
        };
        let sels = sels.iter().map(|&(a, c)| (map(a), map(c))).collect();
        let Some((change, _)) = self.splice(&edits) else {
            return;
        };
        self.view.set_sels(sels, primary);
        self.view.block = None;
        let after = (
            (self.view.anchor, self.view.cursor),
            self.view.extra.clone(),
        );
        self.buf
            .history
            .record_multi(change, EditKind::Other, before, after);
        self.buf.history.seal();
        self.moved = true;
    }

    /// Tab e Shift+Tab com vários cursores: indenta (ou tira a indentação
    /// de) todas as linhas tocadas por alguma seleção.
    fn indent_each(&mut self, outdent: bool) {
        let (sels, _) = self.view.sels();
        let mut lines: Vec<usize> = Vec::new();
        for &(a, c) in &sels {
            for line in crate::edit::selected_lines(self.text, a, c) {
                if lines.last().is_none_or(|l| *l < line) {
                    lines.push(line);
                }
            }
        }
        let edits = lines
            .into_iter()
            .filter_map(|line| {
                let start = self.index().line_start(line);
                let chars = self.line_chars(line);
                if outdent {
                    let n = if chars.first() == Some(&'\t') {
                        1
                    } else {
                        chars
                            .iter()
                            .take(TAB_SIZE)
                            .take_while(|c| **c == ' ')
                            .count()
                    };
                    (n > 0).then(|| (start..start + n, String::new()))
                } else {
                    (!chars.is_empty()).then(|| (start..start, " ".repeat(TAB_SIZE)))
                }
            })
            .collect();
        self.edit_keep(edits);
    }

    /// Insere `s` em todos os cursores (trocando o que estiver selecionado).
    fn insert_each(&mut self, s: &str, kind: EditKind) {
        let (sels, _) = self.view.sels();
        let edits = sels
            .iter()
            .map(|&(a, c)| (a.min(c)..a.max(c), s.to_string()))
            .collect();
        self.edit_each(edits, kind);
    }

    /// Texto de todas as seleções, uma por linha.
    fn selected_each(&self) -> String {
        let (sels, _) = self.view.sels();
        let parts: Vec<&str> = sels
            .iter()
            .filter(|(a, c)| a != c)
            .map(|&(a, c)| {
                let from = self.buf.index.byte_of(self.text, a.min(c));
                let to = self.buf.index.byte_of(self.text, a.max(c));
                &self.text[from..to]
            })
            .collect();
        parts.join("\n")
    }

    /// Cola em todos os cursores. Se o texto tem uma linha por cursor, cada
    /// cursor recebe a sua (é o que volta de copiar uma seleção em coluna).
    fn paste_each(&mut self, s: &str) {
        let (sels, _) = self.view.sels();
        let lines: Vec<&str> = s.strip_suffix('\n').unwrap_or(s).split('\n').collect();
        let spread = lines.len() == sels.len();
        let edits = sels
            .iter()
            .enumerate()
            .map(|(i, &(a, c))| {
                let piece = if spread { lines[i] } else { s };
                (a.min(c)..a.max(c), piece.to_string())
            })
            .collect();
        self.edit_each(edits, EditKind::Other);
        self.buf.history.seal();
    }

    /// Faz `action` (um movimento do cursor) em cada cursor.
    fn each_cursor(&mut self, mut action: impl FnMut(&mut Self)) {
        let (sels, primary) = self.view.sels();
        let preferred_x = self.view.preferred_x;
        self.view.extra.clear();
        let mut moved = Vec::with_capacity(sels.len());
        let mut primary_x = None;
        for (i, s) in sels.into_iter().enumerate() {
            (self.view.anchor, self.view.cursor) = s;
            // Só o principal lembra a coluna ao subir e descer.
            self.view.preferred_x = if i == primary { preferred_x } else { None };
            action(self);
            if i == primary {
                primary_x = self.view.preferred_x;
            }
            moved.push((self.view.anchor, self.view.cursor));
        }
        self.view.set_sels(moved, primary);
        self.view.preferred_x = primary_x;
        self.view.block = None;
    }

    /// Canto de uma seleção em coluna a partir de um caractere.
    fn corner(&self, c: usize) -> (usize, usize) {
        self.index().line_col(c)
    }

    /// Faz a seleção em coluna entre os dois cantos: uma seleção por linha.
    /// Com largura, as linhas que não chegam à coluna ficam de fora.
    fn set_block(&mut self, block: Block) {
        let last = self.index().line_count() - 1;
        let block = Block {
            anchor: (block.anchor.0.min(last), block.anchor.1),
            cursor: (block.cursor.0.min(last), block.cursor.1),
        };
        let lines = block.anchor.0.min(block.cursor.0)..=block.anchor.0.max(block.cursor.0);
        let left = block.anchor.1.min(block.cursor.1);
        let right = block.anchor.1.max(block.cursor.1);
        let forward = block.cursor.1 >= block.anchor.1;
        let mut sels = Vec::new();
        let mut primary = 0;
        for line in lines {
            let start = self.index().line_start(line);
            let len = self.index().line_len(line);
            let (a, b) = (left.min(len), right.min(len));
            let main = line == block.cursor.0;
            if a == b && left != right && !main {
                continue;
            }
            if main {
                primary = sels.len();
            }
            sels.push(if forward {
                (start + a, start + b)
            } else {
                (start + b, start + a)
            });
        }
        self.view.set_sels(sels, primary);
        self.view.block = Some(block);
        self.view.preferred_x = None;
        self.buf.popup = None;
        self.moved = true;
    }

    /// Alt+Shift+setas: começa (ou estica) a seleção em coluna.
    fn move_block(&mut self, key: Key) {
        let mut block = self.view.block.unwrap_or(Block {
            anchor: self.corner(self.view.anchor),
            cursor: self.corner(self.view.cursor),
        });
        let (line, col) = &mut block.cursor;
        match key {
            Key::ArrowUp => *line = line.saturating_sub(1),
            Key::ArrowDown => *line += 1,
            Key::ArrowLeft => *col = col.saturating_sub(1),
            _ => *col = (*col + 1).min(self.index().max_line_chars()),
        }
        self.set_block(block);
    }

    /// A palavra do cursor, se ele está numa palavra (ou logo depois).
    fn word_under_cursor(&self) -> Option<Range<usize>> {
        let c = self.view.cursor;
        [c, c.saturating_sub(1)]
            .into_iter()
            .map(|at| self.word_at(at))
            .find(|w| {
                !w.is_empty()
                    && w.start <= c
                    && c <= w.end
                    && self
                        .line_chars(self.index().line_of(w.start))
                        .get(w.start - self.index().line_start(self.index().line_of(w.start)))
                        .is_some_and(|ch| is_word(*ch))
            })
    }

    /// Ctrl+D: seleciona também a próxima ocorrência do texto selecionado.
    /// Sem seleção, seleciona a palavra do cursor.
    fn add_next_match(&mut self) {
        if self.range().is_empty() {
            if let Some(word) = self.word_under_cursor() {
                self.select(word);
            }
            return;
        }
        let needle = self.selected();
        let Some(found) = crate::edit::find_next(self.text, &needle, self.range().end, true) else {
            return;
        };
        let (mut sels, _) = self.view.sels();
        if sels
            .iter()
            .any(|&(a, c)| a.min(c) < found.end && found.start < a.max(c))
        {
            return;
        }
        sels.push((found.start, found.end));
        let primary = sels.len() - 1;
        self.view.set_sels(sels, primary);
        self.view.block = None;
        self.moved = true;
    }

    /// Ctrl+F2: seleciona todas as ocorrências do texto selecionado (ou
    /// da palavra do cursor).
    fn select_all_matches(&mut self) {
        let range = if self.range().is_empty() {
            match self.word_under_cursor() {
                Some(w) => w,
                None => return,
            }
        } else {
            self.range()
        };
        let a = self.buf.index.byte_of(self.text, range.start);
        let b = self.buf.index.byte_of(self.text, range.end);
        let needle = self.text[a..b].to_string();
        let mut sels = Vec::new();
        let mut primary = 0;
        let mut from = 0;
        let mut chars = 0;
        while let Some(at) = self.text[from..].find(&needle) {
            let start = chars + self.text[from..from + at].chars().count();
            let end = start + needle.chars().count();
            if start == range.start {
                primary = sels.len();
            }
            sels.push((start, end));
            from += at + needle.len();
            chars = end;
        }
        if sels.is_empty() {
            return;
        }
        self.view.collapse();
        self.view.set_sels(sels, primary);
        self.moved = true;
    }

    /// O menor trecho com sentido que contém `range` e é maior que ele:
    /// a palavra, o texto entre aspas, o que está entre parênteses, o
    /// comando (até o `;`) e, por fim, o texto todo. Em bytes.
    fn wider(&self, range: Range<usize>) -> Option<Range<usize>> {
        use crate::lexer::TokenKind;
        let text: &str = self.text;
        let mut found: Vec<Range<usize>> = Vec::new();
        found.push(0..text.len());
        // A palavra em volta (dentro de um texto entre aspas não há tokens).
        let word = |c: char| is_word(c);
        let before = text[..range.start].trim_end_matches(word).len();
        let after =
            range.end + text[range.end..].len() - text[range.end..].trim_start_matches(word).len();
        if text[range.clone()].chars().all(word) {
            found.push(before..after);
        }
        let mut open: Vec<usize> = Vec::new();
        let mut statement = None;
        let mut last = 0;
        for t in &self.buf.tokens {
            let piece = &text[t.range.clone()];
            match t.kind {
                TokenKind::Whitespace => continue,
                TokenKind::Punctuation if matches!(piece, "(" | "[") => open.push(t.range.start),
                TokenKind::Punctuation if matches!(piece, ")" | "]") => {
                    if let Some(from) = open.pop() {
                        found.push(from + 1..t.range.start);
                        found.push(from..t.range.end);
                    }
                }
                TokenKind::Punctuation if piece == ";" => {
                    if let Some(from) = statement.take() {
                        found.push(from..last);
                        found.push(from..t.range.end);
                    }
                    continue;
                }
                TokenKind::String | TokenKind::QuotedIdentifier => {
                    let quote = piece.chars().next().filter(|q| matches!(q, '\'' | '"'));
                    if let Some(q) = quote
                        && piece.len() >= 2
                        && piece.ends_with(q)
                    {
                        found.push(t.range.start + 1..t.range.end - 1);
                    }
                    found.push(t.range.clone());
                }
                TokenKind::Punctuation | TokenKind::Operator => {}
                _ => found.push(t.range.clone()),
            }
            statement.get_or_insert(t.range.start);
            last = t.range.end;
        }
        if let Some(from) = statement {
            found.push(from..last);
        }
        found
            .into_iter()
            .filter(|r| r.start <= range.start && range.end <= r.end && r.len() > range.len())
            .min_by_key(|r| r.len())
    }

    /// Shift+Alt+→: expande a seleção para o próximo trecho com sentido.
    fn expand_selection(&mut self) {
        let now = (self.view.anchor, self.view.cursor);
        let r = self.range();
        let bytes =
            self.buf.index.byte_of(self.text, r.start)..self.buf.index.byte_of(self.text, r.end);
        let Some(next) = self.wider(bytes) else {
            return;
        };
        let next =
            crate::edit::char_of(self.text, next.start)..crate::edit::char_of(self.text, next.end);
        if self.view.grown != Some(now) {
            self.view.shrink.clear();
        }
        self.view.shrink.push(now);
        self.view.collapse();
        self.view.grown = Some((next.start, next.end));
        self.select(next);
    }

    /// Shift+Alt+←: volta à seleção de antes da última expansão.
    fn shrink_selection(&mut self) {
        if self.view.grown != Some((self.view.anchor, self.view.cursor)) {
            self.view.shrink.clear();
            return;
        }
        if let Some(prev) = self.view.shrink.pop() {
            (self.view.anchor, self.view.cursor) = prev;
            self.view.grown = Some(prev);
            self.moved = true;
        }
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
        if let Some(sels) = self.buf.history.undo_multi(self.text) {
            self.restore(sels);
        }
    }

    fn redo(&mut self) {
        if let Some(sels) = self.buf.history.redo_multi(self.text) {
            self.restore(sels);
        }
    }

    /// Depois de desfazer ou refazer: refaz o índice e volta os cursores.
    fn restore(&mut self, ((a, c), extra): (Sel, Vec<Sel>)) {
        self.buf.rebuild(self.text);
        let len = self.len();
        self.view.collapse();
        let mut sels = vec![(a.min(len), c.min(len))];
        sels.extend(extra.into_iter().map(|(a, c)| (a.min(len), c.min(len))));
        self.view.set_sels(sels, 0);
        self.moved = true;
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
    /// Onde o cursor está no texto, em bytes.
    pub cursor_byte: usize,
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
    if buf.sync(text, &view) {
        // As posições dos outros cursores não valem para o texto novo.
        view.collapse();
    }
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
    if view.extra.iter().any(|s| s.0 > total || s.1 > total) {
        view.collapse();
    }

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

    let over_popup = (buf.popup.is_some() || buf.ai.open.is_some())
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
        ed.view.collapse();
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
                    // Com a lista aberta, Esc fecha a lista e o foco fica;
                    // com vários cursores, volta a um só.
                    escape: ed.buf.popup.is_some() || ed.buf.ai.open.is_some() || ed.multi(),
                },
            )
        });
        if list_members {
            ed.refresh_popup(true);
        }
        if std::mem::take(&mut ed.buf.ai.want_open) {
            ed.open_hints();
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
        ed.buf.ai.open = None;
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
    let chip = hint_chip(ui, id, &mut ed, &f);
    if ((focused || over_popup) && hint_popup(ui, id, &mut ed, &f)) || chip {
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
    let cursor_byte = ed.buf.index.byte_of(ed.text, ed.view.cursor);
    store(&ctx, id, view);
    Output {
        response,
        cursor: (line + 1, col + 1),
        cursor_byte,
    }
}

/// Teclas da seleção em coluna: Alt+Shift+setas e, para pôr um cursor na
/// linha de cima ou de baixo, Ctrl+Alt+↑/↓.
///
/// Como no VS Code, Shift+Alt+←/→ encolhe e expande a seleção; com um bloco
/// já começado (`block`), elas mudam a largura dele. Ctrl+Shift+Alt+setas é
/// sempre seleção em coluna.
fn is_block_key(key: Key, m: Modifiers, block: bool) -> bool {
    let ctrl = m.ctrl || m.command || m.mac_cmd;
    let vertical = matches!(key, Key::ArrowUp | Key::ArrowDown);
    let horizontal = matches!(key, Key::ArrowLeft | Key::ArrowRight);
    if !m.alt || !(vertical || horizontal) {
        return false;
    }
    match (m.shift, ctrl) {
        (true, true) => true,
        (true, false) => vertical || block,
        (false, true) => vertical,
        (false, false) => false,
    }
}

fn handle_event(ui: &Ui, ed: &mut Editing<'_>, f: &Frame, event: Event) {
    // Qualquer outra edição ou movimento termina a seleção em coluna (os
    // cursores ficam).
    match &event {
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } if is_block_key(*key, *modifiers, ed.view.block.is_some()) => {}
        Event::Key { pressed: true, .. }
        | Event::Text(_)
        | Event::Ime(_)
        | Event::Cut
        | Event::Paste(_) => ed.view.block = None,
        _ => {}
    }
    // Só o que escreve fecha a lista: no Wayland, o método de entrada manda
    // avisos sem texto nenhum (ligou, pré-edição vazia) a qualquer hora.
    let writes = match &event {
        Event::Text(_) | Event::Cut | Event::Paste(_) => true,
        Event::Ime(egui::ImeEvent::Commit(s) | egui::ImeEvent::Preedit { text: s, .. }) => {
            !s.is_empty()
        }
        Event::Ime(egui::ImeEvent::DeleteSurrounding { .. }) => true,
        _ => false,
    };
    if writes {
        ed.buf.ai.open = None;
    }
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
                if ed.multi() {
                    // Como com um cursor: um espaço, ou trocar uma seleção,
                    // começa outro passo do desfazer.
                    let (sels, _) = ed.view.sels();
                    if s.starts_with(char::is_whitespace) || sels.iter().any(|(a, c)| a != c) {
                        ed.buf.history.seal();
                    }
                    ed.insert_each(&s, EditKind::Typing);
                } else {
                    ed.insert(&s, EditKind::Typing);
                }
                ed.view.preferred_x = None;
                // Letras e `.` abrem (ou atualizam) a lista, como no SSMS;
                // o resto fecha.
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if is_word(c) || c == '.' => ed.refresh_popup(true),
                    // Redis: `:` faz parte do nome da chave, e o espaço
                    // depois do comando já mostra as chaves.
                    (Some(':' | ' '), None)
                        if ed.buf.dialect == Dialect::Redis && ed.line_has_text() =>
                    {
                        ed.refresh_popup(true)
                    }
                    // Elasticsearch: a aspa abre uma chave ou um valor, a
                    // barra um pedaço do caminho.
                    (Some('"' | '/' | ',' | '?' | '&'), None)
                        if ed.buf.dialect == Dialect::Elasticsearch =>
                    {
                        ed.refresh_popup(true)
                    }
                    (Some(' '), None)
                        if ed.buf.dialect == Dialect::Elasticsearch && ed.line_has_text() =>
                    {
                        ed.refresh_popup(true)
                    }
                    // MongoDB: os operadores começam com `$`.
                    (Some('$'), None) if ed.buf.dialect == Dialect::MongoDb => {
                        ed.refresh_popup(true)
                    }
                    _ => ed.buf.popup = None,
                }
            }
        }
        Event::Ime(egui::ImeEvent::Commit(s)) if !s.is_empty() => {
            if ed.multi() {
                ed.insert_each(&s, EditKind::Other);
            } else {
                ed.insert(&s, EditKind::Other);
            }
            ed.refresh_popup(false);
        }
        Event::Copy => {
            let s = ed.selected_each();
            if !s.is_empty() {
                ui.ctx().copy_text(s);
            }
        }
        Event::Cut => {
            let s = ed.selected_each();
            ed.buf.popup = None;
            if !s.is_empty() {
                ui.ctx().copy_text(s);
                if ed.multi() {
                    ed.insert_each("", EditKind::Other);
                } else {
                    ed.insert("", EditKind::Other);
                }
                ed.buf.history.seal();
            }
        }
        Event::Paste(s) => {
            let s = s.replace("\r\n", "\n").replace('\r', "\n");
            ed.buf.popup = None;
            if ed.multi() {
                ed.paste_each(&s);
            } else {
                ed.insert(&s, EditKind::Other);
                ed.buf.history.seal();
            }
        }
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            key_pressed(ui, ed, f, key, modifiers);
            ed.view.after_ctrl_k = key == Key::K
                && (modifiers.ctrl || modifiers.command || modifiers.mac_cmd)
                && !modifiers.alt;
        }
        _ => {}
    }
}

fn key_pressed(ui: &Ui, ed: &mut Editing<'_>, f: &Frame, key: Key, m: Modifiers) {
    let ctrl = m.ctrl || m.command || m.mac_cmd;
    let shift = m.shift;
    if m.alt {
        if is_block_key(key, m, ed.view.block.is_some()) {
            ed.move_block(key);
        } else if shift && !ctrl {
            ed.buf.popup = None;
            match key {
                Key::Period => ed.add_next_match(),
                Key::ArrowRight => ed.expand_selection(),
                Key::ArrowLeft => ed.shrink_selection(),
                _ => {}
            }
        }
        return;
    }
    // Ctrl+D e Ctrl+F2, como no VS Code. Depois de Ctrl+K a tecla é o fim de
    // um acorde do app (Ctrl+K, Ctrl+D formata).
    let chord = ed.view.after_ctrl_k;
    if ctrl && !shift && matches!(key, Key::D | Key::F2) {
        if !chord {
            ed.buf.popup = None;
            ed.buf.ai.open = None;
            if key == Key::D {
                ed.add_next_match();
            } else {
                ed.select_all_matches();
            }
        }
        return;
    }
    if ed.buf.ai.open.is_some() {
        if ctrl && !shift && key == Key::Period {
            // O atalho das sugestões: a casca abre e fecha a lista.
            return;
        } else if ctrl || shift {
            ed.buf.ai.open = None;
        } else {
            match key {
                Key::Escape => {
                    ed.buf.ai.open = None;
                    return;
                }
                Key::ArrowUp => return ed.move_hint(-1),
                Key::ArrowDown => return ed.move_hint(1),
                Key::Enter | Key::Tab => return ed.apply_hint(),
                _ => ed.buf.ai.open = None,
            }
        }
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
    if !ed.multi() || !key_action_each(ui, ed, f, key, m) {
        key_action(ui, ed, f, key, m);
    }
    if matches!(key, Key::Backspace | Key::Delete) && !ctrl {
        ed.refresh_popup(false);
    } else {
        ed.buf.popup = None;
    }
}

/// A tecla com vários cursores. Devolve `false` se ela não é por cursor
/// (desfazer, rolar): aí segue o caminho de um cursor só.
fn key_action_each(ui: &Ui, ed: &mut Editing<'_>, f: &Frame, key: Key, m: Modifiers) -> bool {
    let ctrl = m.ctrl || m.command || m.mac_cmd;
    let (sels, _) = ed.view.sels();
    let ranges = sels.iter().map(|&(a, c)| a.min(c)..a.max(c));
    let all_empty = sels.iter().all(|(a, c)| a == c);
    match key {
        Key::Escape => ed.view.collapse(),
        // Vão para um lugar só: fica um cursor.
        Key::PageUp | Key::PageDown => {
            ed.view.collapse();
            return false;
        }
        Key::Home | Key::End | Key::A if ctrl => {
            ed.view.collapse();
            return false;
        }
        Key::ArrowUp | Key::ArrowDown if ctrl => return false,
        Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End => {
            ed.each_cursor(|ed| key_action(ui, ed, f, key, m));
            ed.moved = true;
        }
        Key::Backspace | Key::Delete => {
            let back = key == Key::Backspace;
            let edits = ranges
                .zip(&sels)
                .map(|(r, &(_, cur))| {
                    let r = if !r.is_empty() {
                        r
                    } else if back {
                        let from = if ctrl {
                            ed.word_left(cur)
                        } else {
                            cur.saturating_sub(1)
                        };
                        from..cur
                    } else {
                        let to = if ctrl {
                            ed.word_right(cur)
                        } else {
                            (cur + 1).min(ed.len())
                        };
                        cur..to
                    };
                    (r, String::new())
                })
                .collect();
            if all_empty {
                ed.edit_each(edits, EditKind::Deleting);
            } else {
                ed.edit_each(edits, EditKind::Other);
                ed.buf.history.seal();
            }
        }
        Key::Enter => {
            let edits = ranges
                .map(|r| {
                    let indent = ed.indent_before(r.start);
                    (r, format!("\n{indent}"))
                })
                .collect();
            ed.edit_each(edits, EditKind::Other);
            ed.buf.history.seal();
        }
        Key::Tab if m.shift => ed.indent_each(true),
        Key::Tab
            if sels
                .iter()
                .any(|&(a, c)| ed.index().line_of(a) != ed.index().line_of(c)) =>
        {
            ed.indent_each(false);
        }
        Key::Tab => {
            let edits = ranges
                .map(|r| {
                    let (_, col) = ed.index().line_col(r.start);
                    (r, " ".repeat(TAB_SIZE - col % TAB_SIZE))
                })
                .collect();
            ed.edit_each(edits, EditKind::Other);
            ed.buf.history.seal();
        }
        _ => return false,
    }
    if !matches!(
        key,
        Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown
    ) {
        ed.view.preferred_x = None;
    }
    true
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

/// Linha e coluna sob o ponteiro, para a seleção em coluna: a coluna pode
/// passar do fim da linha.
fn block_corner(ed: &Editing<'_>, f: &Frame, pos: Pos2) -> (usize, usize) {
    let y = pos.y - f.text_rect.top() + ed.view.scroll.y;
    let line = (y / f.line_h).floor().max(0.0) as usize;
    let x = (pos.x - f.text_rect.left() + ed.view.scroll.x).max(0.0);
    (
        line.min(ed.index().line_count() - 1),
        (x / f.char_w).round() as usize,
    )
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
    let (pressed, shift, alt, ctrl) = ui.input(|i| {
        let m = i.modifiers;
        (
            i.pointer.primary_pressed(),
            m.shift,
            m.alt,
            m.ctrl || m.command || m.mac_cmd,
        )
    });
    // Com Ctrl (ou Alt, como no VS Code), o clique acrescenta um cursor: os
    // outros ficam. Com Shift junto (Shift+Alt ou Ctrl+Shift), o clique faz a
    // seleção em coluna do cursor até ali.
    let column = shift && (alt || ctrl);
    let adding = (ctrl || alt) && !shift;
    // Quantos apertos seguidos no mesmo lugar: o egui só avisa do duplo e do
    // triplo clique quando o botão solta, tarde demais para arrastar.
    let mut presses = 0;
    if pressed {
        let now = ui.input(|i| i.time);
        presses = match ed.view.last_press {
            Some((at, p, n)) if now - at < 0.4 && p.distance(pos) < 6.0 => n % 3 + 1,
            _ => 1,
        };
        ed.view.last_press = Some((now, pos, presses));
    }
    if column {
        presses = presses.min(1);
    }
    let triple = response.triple_clicked() || presses == 3;
    if triple || response.double_clicked() || presses == 2 {
        let (mut sels, primary) = ed.view.sels();
        let range = if triple { ed.line_at(c) } else { ed.word_at(c) };
        // Arrastar depois continua por palavra, ou por linha.
        ed.view.drag_lines = triple;
        ed.view.drag_words = Some((range.start, range.end));
        ed.view.drag_block = false;
        if adding {
            sels[primary] = (range.start, range.end);
            ed.view.set_sels(sels, primary);
            ed.moved = true;
        } else {
            ed.view.collapse();
            ed.select(range);
        }
        return;
    }
    if pressed && column {
        // Do cursor (ou do começo do bloco que já existe) até o clique;
        // arrastar continua mudando o canto.
        let anchor = match ed.view.block {
            Some(block) => block.anchor,
            None => ed.corner(ed.view.anchor),
        };
        ed.view.drag_words = None;
        ed.view.drag_block = true;
        ed.view.block_origin = Some(anchor);
        ed.set_block(Block {
            anchor,
            cursor: block_corner(ed, f, pos),
        });
        ed.buf.history.seal();
    } else if pressed {
        ed.buf.popup = None;
        ed.buf.ai.open = None;
        ed.view.drag_words = None;
        ed.view.preferred_x = None;
        ed.view.block = None;
        // Alt+arrastar faz a seleção em coluna a partir daqui.
        ed.view.drag_block = alt && !ctrl && !shift;
        ed.view.block_origin = ed.view.drag_block.then(|| block_corner(ed, f, pos));
        if adding {
            let (mut sels, primary) = ed.view.sels();
            if let Some(hit) = sels
                .iter()
                .position(|&(a, s)| (a.min(s)..=a.max(s)).contains(&c))
            {
                // Ctrl+clique num cursor que já existe tira esse cursor (o
                // principal fica: o duplo clique começa por ele).
                if hit != primary {
                    sels.remove(hit);
                    let primary = primary - usize::from(hit < primary);
                    ed.view.set_sels(sels, primary);
                }
            } else {
                sels.push((c, c));
                let primary = sels.len() - 1;
                ed.view.set_sels(sels, primary);
            }
            ed.moved = true;
        } else {
            ed.view.collapse();
            ed.move_to(c, shift);
        }
        ed.buf.history.seal();
    } else if response.dragged() && ed.view.drag_block {
        let cursor = block_corner(ed, f, pos);
        match (ed.view.block, ed.view.block_origin) {
            (Some(block), _) if cursor != block.cursor => ed.set_block(Block { cursor, ..block }),
            (None, Some(anchor)) if cursor != anchor => ed.set_block(Block { anchor, cursor }),
            _ => {}
        }
    } else if response.dragged() {
        if ed.multi() {
            // Arrastando um cursor acrescentado: junta com os que alcançar.
            ed.view.cursor = c.min(ed.len());
            let (sels, primary) = ed.view.sels();
            ed.view.set_sels(sels, primary);
            ed.moved = true;
            return;
        }
        match ed.view.drag_words {
            Some((a, b)) => {
                let w = if ed.view.drag_lines {
                    ed.line_at(c)
                } else {
                    ed.word_at(c)
                };
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
    let (sels, _) = ed.view.sels();
    let multi = ed.multi();
    let (cur_line, _) = ed.index().line_col(ed.view.cursor);
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
        if line == cur_line && ed.range().is_empty() && focused && !multi {
            let row = Rect::from_min_size(
                pos2(f.text_rect.left() - TEXT_MARGIN, y),
                vec2(f.text_rect.width() + TEXT_MARGIN, f.line_h),
            );
            text_painter.rect_filled(row, 0.0, v.faint_bg_color);
        }
        // Seleção nesta linha (o `\n` selecionado aparece como meio caractere).
        // As seleções estão em ordem: só as que tocam esta linha.
        let from = sels.partition_point(|s| s.0.max(s.1) < start);
        let on_line = || {
            sels[from..]
                .iter()
                .take_while(|s| s.0.min(s.1) <= start + len)
        };
        for s in on_line() {
            let sel = s.0.min(s.1)..s.0.max(s.1);
            let a = sel.start.max(start);
            let b = sel.end.min(start + len);
            if !sel.is_empty() && a <= b {
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
        }
        text_painter.galley(pos2(left, y), galley.clone(), f.colors.text);
        if focused {
            for s in on_line().filter(|s| (start..=start + len).contains(&s.1)) {
                let x = left + col_x(&galley, s.1 - start);
                text_painter.line_segment(
                    [pos2(x, y), pos2(x, y + f.line_h)],
                    Stroke::new(2.0, v.text_cursor.stroke.color),
                );
            }
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
/// A largura que a lista pede para mostrar inteiros o nome e o detalhe de
/// todas as sugestões. Fica guardada na lista: mede uma vez só.
fn wanted_width(ui: &Ui, p: &mut Popup, f: &Frame) -> f32 {
    let mut details: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    let font = FontId::proportional(DETAIL_FONT);
    let mut widest = 0.0_f32;
    for item in &p.completion.items {
        let label = item.label.chars().count() as f32 * f.char_w;
        let detail = if item.detail.is_empty() {
            0.0
        } else {
            *details.entry(item.detail.as_str()).or_insert_with(|| {
                ui.painter()
                    .layout_no_wrap(item.detail.clone(), font.clone(), Color32::WHITE)
                    .size()
                    .x
                    + ROW_GAP
            })
        };
        widest = widest.max(label + detail);
    }
    let wanted = (ROW_LEFT + widest + ROW_RIGHT).max(p.width.unwrap_or(0.0));
    p.width = Some(wanted);
    wanted
}

fn popup(ui: &Ui, id: Id, ed: &mut Editing<'_>, f: &Frame) -> bool {
    let Some(p) = &mut ed.buf.popup else {
        return false;
    };
    // A largura que a sugestão mais comprida pede (nome + detalhe).
    let wanted = wanted_width(ui, p, f);
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
    let v = ui.visuals().clone();
    let screen = ui.ctx().content_rect();
    let width = popup_width(wanted, screen.width());
    let avail = width - ROW_LEFT - ROW_RIGHT;
    let detail_font = FontId::proportional(DETAIL_FONT);
    // A explicação da sugestão escolhida, embaixo da lista. O espaço é de
    // pelo menos duas linhas, para a lista não mudar de tamanho a cada
    // seta. Quando o nome escolhido não coube na linha, ele vem inteiro
    // antes da explicação.
    let doc = p.completion.items.get(p.selected).and_then(|item| {
        let text = crate::docs::explain(ed.buf.dialect, item, ed.catalog);
        let detail_w = if item.detail.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(item.detail.clone(), detail_font.clone(), v.text_color())
                .size()
                .x
        };
        let label_w = item.label.chars().count() as f32 * f.char_w;
        let cut = split_row(avail, label_w, detail_w).0 < label_w;
        if text.is_none() && !cut {
            return None;
        }
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = width - 2.0 * DOC_MARGIN;
        if cut {
            job.append(
                &item.label,
                0.0,
                egui::TextFormat::simple(f.font.clone(), v.strong_text_color()),
            );
            if text.is_some() {
                job.append("\n", 0.0, egui::TextFormat::default());
            }
        }
        if let Some(text) = text {
            job.append(
                &text,
                0.0,
                egui::TextFormat::simple(FontId::proportional(DOC_FONT), v.text_color()),
            );
        }
        Some(ui.painter().layout_job(job))
    });
    let doc_h = doc.as_ref().map_or(0.0, |g| {
        g.size().y.max(2.0 * (DOC_FONT + 3.0)) + 2.0 * DOC_MARGIN
    });
    let height = rows as f32 * row_h + doc_h;
    // Embaixo da linha; se não couber, em cima.
    let pos = if y + height + 8.0 > screen.bottom() {
        pos2(x, y - f.line_h - height - 8.0)
    } else {
        pos2(x, y)
    };
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
                            ui.allocate_exact_size(vec2(width, row_h), Sense::click());
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
                        // O nome e o detalhe medidos antes de desenhar:
                        // cada um fica na sua parte da linha, encurtado
                        // com reticências se não couber.
                        let detail_color = if selected {
                            Color32::WHITE
                        } else {
                            v.weak_text_color()
                        };
                        let detail_w = if item.detail.is_empty() {
                            0.0
                        } else {
                            painter
                                .layout_no_wrap(
                                    item.detail.clone(),
                                    detail_font.clone(),
                                    detail_color,
                                )
                                .size()
                                .x
                        };
                        let label_w = painter
                            .layout_no_wrap(item.label.clone(), font.clone(), text_color)
                            .size()
                            .x;
                        let (label_max, detail_max) = split_row(avail, label_w, detail_w);
                        let label =
                            one_line(painter, &item.label, font.clone(), text_color, label_max);
                        let mut cut = label.elided;
                        painter.galley(
                            pos2(rect.left() + ROW_LEFT, mid.y - label.size().y / 2.0),
                            label,
                            text_color,
                        );
                        if !item.detail.is_empty() && detail_max > 0.0 {
                            let detail = one_line(
                                painter,
                                &item.detail,
                                detail_font.clone(),
                                detail_color,
                                detail_max,
                            );
                            cut |= detail.elided;
                            painter.galley(
                                pos2(
                                    rect.right() - ROW_RIGHT - detail.size().x,
                                    mid.y - detail.size().y / 2.0,
                                ),
                                detail,
                                detail_color,
                            );
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        // O texto inteiro de uma linha encurtada, no hover.
                        if cut {
                            resp.on_hover_text(if item.detail.is_empty() {
                                item.label.clone()
                            } else {
                                format!("{}\n{}", item.label, item.detail)
                            });
                        }
                    }
                    if let Some(doc) = doc {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, doc_h), Sense::hover());
                        ui.painter().hline(
                            rect.x_range(),
                            rect.top(),
                            v.widgets.noninteractive.bg_stroke,
                        );
                        ui.put(
                            Rect::from_min_size(
                                rect.min + vec2(DOC_MARGIN, DOC_MARGIN),
                                doc.size(),
                            ),
                            egui::Label::new(doc).selectable(false),
                        );
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

/// Largura da lista das sugestões da IA e linhas do código mostrado.
const HINT_WIDTH: f32 = 560.0;
const HINT_PREVIEW_ROWS: usize = 12;
/// O ícone das sugestões da IA (da fonte de ícones da casca).
const HINT_GLYPH: &str = egui_phosphor::regular::SPARKLE;

/// O aviso das sugestões da IA no canto de baixo do editor: quantas há,
/// ou que a IA está lendo o comando. Um clique abre a lista. Devolve
/// `true` se foi clicado.
fn hint_chip(ui: &Ui, id: Id, ed: &mut Editing<'_>, f: &Frame) -> bool {
    let ai = &ed.buf.ai;
    let n = ai.count();
    let v = ui.visuals().clone();
    let mut hover = None;
    let text = if n == 1 {
        tr("1 AI suggestion").to_string()
    } else if n > 1 {
        trf!("{n} AI suggestions", n = n)
    } else if ai.hints.is_some() {
        tr("AI: nothing to suggest here").to_string()
    } else {
        match &ai.status {
            HintStatus::Off | HintStatus::Idle => return false,
            HintStatus::Ask => tr("Ask the AI for suggestions").to_string(),
            HintStatus::Thinking => tr("The AI is reading the query…").to_string(),
            HintStatus::Failed(e) => {
                hover = Some(e.clone());
                tr("The AI did not answer").to_string()
            }
        }
    };
    let ready = n > 0;
    let asks = ready || ai.status == HintStatus::Ask;
    let font = FontId::proportional(12.0);
    let color = if ready {
        v.strong_text_color()
    } else {
        v.weak_text_color()
    };
    let mut job = egui::text::LayoutJob::default();
    let format = |color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    job.append(
        HINT_GLYPH,
        0.0,
        format(if ready { f.colors.keyword } else { color }),
    );
    job.append(&text, 6.0, format(color));
    if asks && !ai.shortcut.is_empty() {
        job.append(&ai.shortcut, 10.0, format(v.weak_text_color()));
    }
    let galley = ui.painter().layout_job(job);
    let size = galley.size() + vec2(16.0, 8.0);
    let rect = Rect::from_min_size(f.text_rect.right_bottom() - size - vec2(8.0, 8.0), size);
    if rect.left() < f.text_rect.left() || rect.top() < f.text_rect.top() {
        return false;
    }
    // Clique e arrasto: só com o clique, o botão apertado em cima do aviso
    // ainda chegava ao editor embaixo dele e levava o cursor do texto para
    // lá (e as sugestões eram de onde o cursor estava).
    let resp = ui.interact(rect, id.with("ai_chip"), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &text));
    let painter = ui.painter();
    let fill = if resp.hovered() {
        v.widgets.hovered.weak_bg_fill
    } else {
        v.window_fill
    };
    let stroke = if ready {
        Stroke::new(1.0, f.colors.keyword)
    } else {
        v.widgets.noninteractive.bg_stroke
    };
    painter.rect(rect, 4.0, fill, stroke, egui::StrokeKind::Inside);
    painter.galley(rect.min + vec2(8.0, 4.0), galley, color);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let resp = match hover {
        Some(e) => resp.on_hover_text(e),
        None => resp,
    };
    if !resp.clicked() {
        return false;
    }
    if ed.buf.ai.open.is_some() {
        ed.buf.ai.open = None;
    } else {
        ed.open_hints();
    }
    true
}

/// Desenha a lista das sugestões da IA embaixo do cursor, com a
/// explicação e o código da escolhida. Devolve `true` se houve um clique
/// nela.
fn hint_popup(ui: &Ui, id: Id, ed: &mut Editing<'_>, f: &Frame) -> bool {
    let Some(selected) = ed.buf.ai.open else {
        return false;
    };
    let Some(hints) = ed.buf.ai.hints.clone().filter(|h| !h.items.is_empty()) else {
        ed.buf.ai.open = None;
        return false;
    };
    let selected = selected.min(hints.items.len() - 1);
    let (line, col) = ed.index().line_col(ed.view.cursor);
    let galley = line_galley(ui, ed.text, ed.buf, line, f);
    let screen = ui.ctx().content_rect();
    let x = (f.text_rect.left() + col_x(&galley, col) - ed.view.scroll.x)
        .min(screen.right() - HINT_WIDTH - 12.0)
        .max(screen.left() + 4.0);
    let y = f.text_rect.top() + (line + 1) as f32 * f.line_h - ed.view.scroll.y;
    // Embaixo da linha; se sobrar pouco, em cima.
    let below = screen.bottom() - y;
    let above = y - f.line_h - screen.top();
    let (pos, pivot) = if below < 340.0 && above > below {
        (pos2(x, y - f.line_h - 4.0), egui::Align2::LEFT_BOTTOM)
    } else {
        (pos2(x, y), egui::Align2::LEFT_TOP)
    };
    let v = ui.visuals().clone();
    let colors = f.colors;
    let mono = f.font.clone();
    let dialect = ed.buf.dialect;
    let row_h = f.line_h + 6.0;
    let mut choose = None;
    let mut apply = false;
    let mut clicked = false;
    let area = egui::Area::new(id.with("ai_hints"))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .pivot(pivot)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(2.0)
                .show(ui, |ui| {
                    ui.set_width(HINT_WIDTH);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for (i, item) in hints.items.iter().enumerate() {
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(HINT_WIDTH, row_h), Sense::click());
                        let painter = ui.painter().with_clip_rect(rect);
                        let chosen = i == selected;
                        if chosen {
                            painter.rect_filled(rect, 2.0, v.selection.bg_fill);
                        } else if resp.hovered() {
                            painter.rect_filled(rect, 2.0, v.widgets.hovered.weak_bg_fill);
                        }
                        let pick = |c: Color32| if chosen { Color32::WHITE } else { c };
                        let glyph = match item.kind {
                            HintKind::Fix | HintKind::Safety => v.warn_fg_color,
                            HintKind::Performance | HintKind::Index => colors.number,
                            HintKind::Join | HintKind::Complete => colors.keyword,
                            _ => colors.identifier_quoted,
                        };
                        let mid = rect.left_center();
                        painter.text(
                            mid + vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            HINT_GLYPH,
                            FontId::proportional(13.0),
                            pick(glyph),
                        );
                        let kind = painter.text(
                            rect.right_center() - vec2(6.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            item.kind.label(),
                            FontId::proportional(11.0),
                            pick(v.weak_text_color()),
                        );
                        painter
                            .with_clip_rect(rect.with_max_x(kind.left() - 8.0))
                            .text(
                                mid + vec2(26.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                &item.title,
                                FontId::proportional(13.0),
                                pick(colors.text),
                            );
                        resp.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                chosen,
                                &item.title,
                            )
                        });
                        if resp.double_clicked() {
                            apply = true;
                        }
                        if resp.clicked() {
                            choose = Some(i);
                        }
                    }
                    let item = &hints.items[selected];
                    let (sep, _) = ui.allocate_exact_size(vec2(HINT_WIDTH, 1.0), Sense::hover());
                    ui.painter().hline(
                        sep.x_range(),
                        sep.top(),
                        v.widgets.noninteractive.bg_stroke,
                    );
                    egui::Frame::new().inner_margin(DOC_MARGIN).show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                        if !item.detail.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&item.detail).size(DOC_FONT + 0.5),
                                )
                                .wrap()
                                .selectable(false),
                            );
                        }
                        if let Some(code) = item.action.code() {
                            egui::Frame::new()
                                .fill(v.extreme_bg_color)
                                .stroke(v.widgets.noninteractive.bg_stroke)
                                .corner_radius(3.0)
                                .inner_margin(6.0)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    let mut job =
                                        layout_job_in(code, &colors, mono.clone(), dialect);
                                    job.wrap = egui::text::TextWrapping {
                                        max_width: ui.available_width(),
                                        max_rows: HINT_PREVIEW_ROWS,
                                        break_anywhere: false,
                                        overflow_character: Some('…'),
                                    };
                                    ui.add(egui::Label::new(job).selectable(false));
                                });
                        }
                        ui.horizontal(|ui| {
                            let button = match item.action {
                                HintAction::Replace(_) => Some(tr("Apply")),
                                HintAction::Insert(_) => Some(tr("Add after the query")),
                                HintAction::None => None,
                            };
                            match button {
                                Some(label) => {
                                    if ui
                                        .add(egui::Button::new(label).shortcut_text("Enter")) // i18n-ignore
                                        .clicked()
                                    {
                                        apply = true;
                                    }
                                }
                                None => {
                                    ui.weak(tr("Advice only: there is nothing to apply."));
                                }
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(trf!(
                                                "{source} · arrows choose · Esc closes",
                                                source = hints.source
                                            ))
                                            .size(11.0)
                                            .weak(),
                                        )
                                        .selectable(false),
                                    );
                                },
                            );
                        });
                    });
                });
        });
    ed.buf.popup_rect = Some(area.response.rect);
    if area.response.clicked() || area.response.dragged() {
        clicked = true;
    }
    if let Some(i) = choose {
        ed.buf.ai.open = Some(i);
        clicked = true;
    }
    if apply {
        ed.buf.ai.open = Some(choose.unwrap_or(selected));
        ed.apply_hint();
        clicked = true;
    }
    clicked
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

#[cfg(test)]
mod popup_layout_tests {
    use super::*;

    /// O nome e o detalhe nunca se encontram, com qualquer tamanho.
    #[test]
    fn the_name_and_the_detail_never_meet() {
        for avail in [40.0_f32, 104.0, 390.0, 730.0] {
            for label in [0.0_f32, 30.0, 200.0, 420.0, 5000.0] {
                for detail in [0.0_f32, 30.0, 180.0, 5000.0] {
                    let (l, d) = split_row(avail, label, detail);
                    assert!(l >= 0.0 && d >= 0.0, "{avail} {label} {detail}");
                    assert!(l <= label.max(avail) && d <= detail);
                    if detail > 0.0 {
                        assert!(
                            l + ROW_GAP + d <= avail + 0.01 || d == 0.0,
                            "{avail} {label} {detail}: {l} + {d}"
                        );
                    } else {
                        assert!(l <= avail);
                    }
                }
            }
        }
    }

    #[test]
    fn what_fits_is_not_cut_and_the_detail_gives_way_first() {
        assert_eq!(split_row(390.0, 200.0, 100.0), (200.0, 100.0));
        // Não cabe: o nome fica inteiro e o detalhe encolhe.
        assert_eq!(split_row(390.0, 250.0, 180.0), (250.0, 124.0));
        // Nome enorme: o detalhe ainda fica com 30% da linha.
        let (l, d) = split_row(400.0, 5000.0, 300.0);
        assert!((d - 120.0).abs() < 0.01, "{d}");
        assert!((l + ROW_GAP + d - 400.0).abs() < 0.01, "{l}");
    }

    #[test]
    fn the_list_widens_up_to_a_limit_and_never_past_the_window() {
        assert_eq!(popup_width(100.0, 1600.0), POPUP_WIDTH);
        assert_eq!(popup_width(600.0, 1600.0), 600.0);
        assert_eq!(popup_width(3000.0, 1600.0), POPUP_MAX_WIDTH);
        assert_eq!(popup_width(600.0, 500.0), 484.0);
    }
}
