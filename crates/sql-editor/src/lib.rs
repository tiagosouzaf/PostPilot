//! Editor de SQL do PGStudio.
//!
//! O editor é um widget próprio ([`widget`]) que monta e desenha só as
//! linhas visíveis, então digitar num script de milhares de linhas não
//! refaz o layout do texto inteiro. Ele tem:
//!
//! - realce de sintaxe do PostgreSQL ([`lexer`], [`highlight`]), com os
//!   tokens refeitos só quando o texto muda;
//! - desfazer e refazer ([`buffer::History`]);
//! - números de linha;
//! - comentar e descomentar linhas ([`comment_selection`]);
//! - barra de busca (Ctrl+F) e "Go To Line" (Ctrl+G).
//!
//! O texto continua sendo uma `String` do documento.

pub mod buffer;
pub mod edit;
pub mod highlight;
pub mod lexer;
mod widget;

use eframe::egui::{self, FontId, Id, Key, TextEdit, Ui};

pub use highlight::Colors;

/// Estado do editor que não é o texto: índice de linhas, tokens,
/// histórico, barra de busca e "Go To Line".
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    buffer: widget::Buffer,
    find: Option<Find>,
    goto: Option<GoTo>,
    /// Pedir foco para o editor no próximo frame.
    focus: bool,
    /// Seleção a aplicar junto com o foco (busca e "Go To Line").
    select: Option<(usize, usize)>,
}

#[derive(Clone, Debug, Default)]
struct Find {
    needle: String,
    match_case: bool,
    /// Pedir foco para o campo de busca no próximo frame.
    focus: bool,
    /// Última busca sem resultado, para avisar.
    not_found: bool,
}

#[derive(Clone, Debug, Default)]
struct GoTo {
    line: String,
    focus: bool,
    error: Option<String>,
}

impl EditorState {
    /// Abre (ou foca) a barra de busca, com o texto selecionado como busca.
    pub fn open_find(&mut self, selected: Option<String>) {
        let find = self.find.get_or_insert_with(Find::default);
        if let Some(s) = selected.filter(|s| !s.is_empty() && !s.contains('\n')) {
            find.needle = s;
        }
        find.focus = true;
        find.not_found = false;
    }

    /// Se a barra de busca está aberta.
    pub fn is_find_open(&self) -> bool {
        self.find.is_some()
    }

    /// Texto buscado.
    pub fn find_text(&self) -> Option<&str> {
        self.find.as_ref().map(|f| f.needle.as_str())
    }

    /// Muda o texto buscado (testes e atalhos).
    pub fn set_find_text(&mut self, needle: &str) {
        self.find.get_or_insert_with(Find::default).needle = needle.to_string();
    }

    /// Se a última busca não achou nada.
    pub fn find_failed(&self) -> bool {
        self.find.as_ref().is_some_and(|f| f.not_found)
    }

    /// Abre o "Go To Line".
    pub fn open_goto(&mut self) {
        self.goto = Some(GoTo {
            focus: true,
            ..Default::default()
        });
    }

    /// Se o "Go To Line" está aberto.
    pub fn is_goto_open(&self) -> bool {
        self.goto.is_some()
    }

    /// Pede foco para o editor no próximo frame.
    pub fn focus_editor(&mut self) {
        self.focus = true;
    }
}

/// Seleção atual do editor `id`, em caracteres: (âncora, cursor).
pub fn selection(ctx: &egui::Context, id: Id) -> Option<(usize, usize)> {
    let view = ctx.data(|d| d.get_temp::<widget::View>(id))?;
    Some((view.anchor, view.cursor))
}

/// Muda a seleção do editor `id`.
pub fn set_selection(ctx: &egui::Context, id: Id, (anchor, cursor): (usize, usize)) {
    let mut view = widget::load(ctx, id);
    view.anchor = anchor;
    view.cursor = cursor;
    widget::store(ctx, id, view);
}

/// Texto selecionado no editor `id`.
pub fn selected_text(ctx: &egui::Context, id: Id, text: &str) -> Option<String> {
    let (a, b) = selection(ctx, id)?;
    let (a, b) = (a.min(b), a.max(b));
    (a != b).then(|| text.chars().skip(a).take(b - a).collect())
}

/// Comenta (ou descomenta) as linhas da seleção, como Ctrl+K, Ctrl+C e
/// Ctrl+K, Ctrl+U, e seleciona as linhas mudadas.
pub fn comment_selection(ctx: &egui::Context, id: Id, text: &mut String, uncomment: bool) {
    let (a, b) = selection(ctx, id).unwrap_or((0, 0));
    let edited = if uncomment {
        edit::uncomment_lines(text, a, b)
    } else {
        edit::comment_lines(text, a, b)
    };
    *text = edited.text;
    set_selection(ctx, id, edited.selection);
}

/// O editor de um documento.
pub struct SqlEditor<'a> {
    text: &'a mut String,
    state: &'a mut EditorState,
    id: Id,
    colors: Colors,
    font: FontId,
}

/// O que aconteceu no editor neste frame.
pub struct EditorResponse {
    /// Resposta do editor (foco, cliques).
    pub response: egui::Response,
    /// Linha e coluna do cursor (a partir de 1), para a status bar.
    pub cursor: Option<(usize, usize)>,
}

impl<'a> SqlEditor<'a> {
    pub fn new(text: &'a mut String, state: &'a mut EditorState, id: Id) -> Self {
        SqlEditor {
            text,
            state,
            id,
            colors: Colors::LIGHT,
            font: FontId::monospace(13.0),
        }
    }

    /// Usa estas cores (o tema escuro passa [`Colors::DARK`]).
    pub fn colors(mut self, colors: Colors) -> Self {
        self.colors = colors;
        self
    }

    /// Desenha a barra de busca, o "Go To Line" e o editor.
    pub fn show(self, ui: &mut Ui) -> EditorResponse {
        let SqlEditor {
            text,
            state,
            id,
            colors,
            font,
        } = self;
        // Foco e seleção pedidos em frames anteriores. Os pedidos deste frame
        // (das barras abaixo) ficam para o próximo: o clique ou o Enter que
        // os causou não pode chegar ao editor.
        let focus = std::mem::take(&mut state.focus);
        let select = state.select.take();
        find_bar(ui, id, text, state);
        goto_bar(ui, id, text, state);

        let out = widget::show(ui, id, text, &mut state.buffer, colors, font, select, focus);
        EditorResponse {
            response: out.response,
            cursor: Some(out.cursor),
        }
    }
}

fn find_bar(ui: &mut Ui, id: Id, text: &str, state: &mut EditorState) {
    let Some(find) = &mut state.find else {
        return;
    };
    let mut close = false;
    let mut search = false;
    ui.horizontal(|ui| {
        ui.label("Find:");
        let field = ui.add(
            TextEdit::singleline(&mut find.needle)
                .id(id.with("find"))
                .desired_width(220.0),
        );
        if find.focus {
            field.request_focus();
            find.focus = false;
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            search = true;
        }
        if field.changed() {
            find.not_found = false;
        }
        ui.checkbox(&mut find.match_case, "Match case");
        if ui.button("Find Next").clicked() {
            search = true;
        }
        if find.not_found {
            ui.colored_label(ui.visuals().warn_fg_color, "The text was not found.");
        }
        if ui.button("✕").on_hover_text("Close (Esc)").clicked() {
            close = true;
        }
        if field.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
    });
    if search {
        let from = selection(ui.ctx(), id).map_or(0, |(a, b)| a.max(b));
        match edit::find_next(text, &find.needle, from, find.match_case) {
            Some(r) => {
                find.not_found = false;
                state.select = Some((r.start, r.end));
                state.focus = true;
            }
            None => find.not_found = true,
        }
    }
    if close {
        state.find = None;
        state.focus = true;
    }
}

fn goto_bar(ui: &mut Ui, id: Id, text: &str, state: &mut EditorState) {
    let Some(goto) = &mut state.goto else {
        return;
    };
    let lines = edit::line_count(text);
    let mut close = false;
    let mut go = None;
    ui.horizontal(|ui| {
        ui.label(format!("Line number (1 - {lines}):"));
        let field = ui.add(
            TextEdit::singleline(&mut goto.line)
                .id(id.with("goto"))
                .desired_width(80.0),
        );
        if goto.focus {
            field.request_focus();
            goto.focus = false;
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if ui.button("OK").clicked() || enter {
            match goto.line.trim().parse::<usize>() {
                Ok(n) if (1..=lines).contains(&n) => go = Some(n),
                _ => {
                    goto.error = Some(format!("Enter a line number between 1 and {lines}."));
                    goto.focus = true;
                }
            }
        }
        if let Some(e) = &goto.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if ui.button("Cancel").clicked()
            || (field.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)))
        {
            close = true;
        }
    });
    if let Some(n) = go {
        let start = edit::line_start(text, n - 1);
        state.select = Some((start, start));
        close = true;
    }
    if close {
        state.goto = None;
        state.focus = true;
    }
}
