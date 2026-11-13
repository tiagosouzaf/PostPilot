//! Editor de SQL do PGStudio.
//!
//! O editor usa o `TextEdit` do egui para digitação, seleção, desfazer e
//! IME, e acrescenta o que um editor de SQL precisa:
//!
//! - realce de sintaxe do PostgreSQL ([`lexer`], [`highlight`]), calculado
//!   só quando o texto muda;
//! - números de linha;
//! - comentar e descomentar linhas ([`comment_selection`]);
//! - barra de busca (Ctrl+F) e "Go To Line" (Ctrl+G).
//!
//! O texto continua sendo uma `String` do documento. Um buffer próprio
//! (`ropey`) só vale a pena se arquivos grandes ficarem lentos; o teste de
//! 5 mil linhas mede isso.

pub mod edit;
pub mod highlight;
pub mod lexer;

use eframe::egui::{
    self, Color32, FontId, Id, Key, TextEdit, Ui,
    text::{CCursor, CCursorRange},
};

pub use highlight::Colors;

/// Estado do editor que não é o texto: barra de busca e "Go To Line".
#[derive(Clone, Debug, Default)]
pub struct EditorState {
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
    let state = egui::text_edit::TextEditState::load(ctx, id)?;
    let range = state.cursor.char_range()?;
    Some((range.secondary.index.into(), range.primary.index.into()))
}

/// Muda a seleção do editor `id`.
pub fn set_selection(ctx: &egui::Context, id: Id, (anchor, cursor): (usize, usize)) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange {
        secondary: CCursor::new(anchor),
        primary: CCursor::new(cursor),
        h_pos: None,
    }));
    state.store(ctx, id);
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
    /// Resposta do `TextEdit`.
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

        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            let job = ui.ctx().memory_mut(|m| {
                m.caches
                    .cache::<highlight::HighlightCache>()
                    .get((buf.as_str(), colors, font.size.to_bits()))
                    .clone()
            });
            ui.fonts_mut(|f| f.layout_job(job))
        };

        let gutter_digits = edit::line_count(text).to_string().len().max(3);
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let gutter_w = char_w * gutter_digits as f32 + 12.0;
        let mut cursor = None;

        // O foco precisa chegar antes do `TextEdit`: sem foco ele reduz a
        // seleção ao cursor, e a busca perderia o trecho achado.
        if let Some(range) = select {
            set_selection(ui.ctx(), id, range);
        }
        if focus {
            ui.memory_mut(|m| m.request_focus(id));
        }
        // Ctrl+K e Ctrl+U são acordes do SSMS (comentar e descomentar); o
        // `TextEdit` os trataria como apagar até o fim ou o começo da linha.
        ui.input_mut(|i| {
            i.events.retain(|e| {
                !matches!(e, egui::Event::Key { key: Key::K | Key::U, modifiers, .. }
                    if (modifiers.ctrl || modifiers.command) && !modifiers.alt)
            });
        });

        let output = egui::ScrollArea::both()
            .id_salt(id.with("scroll"))
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (gutter, _) = ui.allocate_exact_size(
                        egui::vec2(gutter_w, ui.available_height()),
                        egui::Sense::hover(),
                    );
                    let output = TextEdit::multiline(text)
                        .id(id)
                        .font(font.clone())
                        .code_editor()
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .min_size(ui.available_size())
                        .layouter(&mut layouter)
                        .show(ui);
                    paint_line_numbers(ui, gutter, &output, &font, colors.operator);
                    output
                })
                .inner
            })
            .inner;

        if let Some(range) = output.cursor_range {
            let idx: usize = range.primary.index.into();
            let line = edit::line_of(text, idx);
            let col = idx - edit::line_start(text, line);
            cursor = Some((line + 1, col + 1));
        }
        EditorResponse {
            response: output.response.response,
            cursor,
        }
    }
}

fn paint_line_numbers(
    ui: &Ui,
    gutter: egui::Rect,
    output: &egui::text_edit::TextEditOutput,
    font: &FontId,
    color: Color32,
) {
    let painter = ui.painter_at(gutter.expand2(egui::vec2(0.0, 10_000.0)));
    let clip = ui.clip_rect();
    let mut line = 1;
    let mut row_starts_line = true;
    for row in &output.galley.rows {
        let rect = row.rect().translate(output.galley_pos.to_vec2());
        if row_starts_line && rect.bottom() >= clip.top() && rect.top() <= clip.bottom() {
            painter.text(
                egui::pos2(gutter.right() - 8.0, rect.top()),
                egui::Align2::RIGHT_TOP,
                line.to_string(),
                font.clone(),
                color,
            );
        }
        row_starts_line = row.ends_with_newline;
        if row.ends_with_newline {
            line += 1;
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
