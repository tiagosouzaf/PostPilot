//! Painel de resultados da janela de consulta, como no SSMS: abas Results
//! e Messages e a faixa de status embaixo (situação, servidor, banco, tempo
//! e linhas).
//!
//! A tabela aqui é simples (linhas virtualizadas, larguras fixas pelo
//! conteúdo). A grade completa, com buffer paginado, seleção e cópia, é o
//! marco 2.5.

use eframe::egui::{self, Color32, FontId, RichText};
use egui_phosphor::regular as icons;

use crate::execution::{Execution, MessageKind, Outcome, ResultSet, Tab, format_elapsed};

/// O que o usuário pediu no painel.
#[derive(Default)]
pub struct Action {
    /// Clicou numa mensagem de erro: levar o cursor a esta posição.
    pub jump_to: Option<usize>,
}

/// Desenha o painel de resultados. `server` é o texto da conexão.
pub fn show(ui: &mut egui::Ui, doc: u32, exec: &mut Execution, server: &str) -> Action {
    let mut action = Action::default();
    egui::Panel::bottom(egui::Id::new(("query_status", doc)))
        .resizable(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 2)))
        .show(ui, |ui| status_strip(ui, exec, server));
    // Sem resultados (ainda), só há o que mostrar em Messages; a escolha
    // do usuário fica guardada para quando eles chegarem.
    let tab = if exec.results.is_empty() {
        Tab::Messages
    } else {
        exec.tab
    };
    ui.horizontal(|ui| {
        let results = format!("{} Results", icons::TABLE);
        if !exec.results.is_empty() && ui.selectable_label(tab == Tab::Results, results).clicked() {
            exec.tab = Tab::Results;
        }
        let messages = format!("{} Messages", icons::CHAT_TEXT);
        if ui
            .selectable_label(tab == Tab::Messages, messages)
            .clicked()
        {
            exec.tab = Tab::Messages;
        }
    });
    ui.separator();
    match tab {
        Tab::Results => results(ui, doc, &exec.results),
        Tab::Messages => messages(ui, doc, exec, &mut action),
    }
    action
}

fn status_strip(ui: &mut egui::Ui, exec: &Execution, server: &str) {
    let outcome = exec.outcome();
    let (icon, color) = match outcome {
        Outcome::Running => (icons::HOURGLASS, ui.visuals().text_color()),
        Outcome::Succeeded => (icons::CHECK_CIRCLE, Color32::from_rgb(0x10, 0x7C, 0x10)),
        Outcome::Failed => (icons::X_CIRCLE, ui.visuals().error_fg_color),
        Outcome::Canceled => (icons::PROHIBIT, ui.visuals().warn_fg_color),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).color(color));
        ui.label(outcome.label());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let rows = exec.total_rows();
            ui.label(if rows == 1 {
                "1 row".to_string()
            } else {
                format!("{rows} rows")
            });
            ui.separator();
            ui.label(format_elapsed(exec.elapsed(), false));
            ui.separator();
            ui.label(&exec.database);
            ui.separator();
            ui.label(server);
        });
    });
}

fn messages(ui: &mut egui::Ui, doc: u32, exec: &Execution, action: &mut Action) {
    egui::ScrollArea::both()
        .id_salt(("messages", doc))
        .auto_shrink(false)
        .show(ui, |ui| {
            for m in &exec.messages {
                let color = match m.kind {
                    MessageKind::Error => ui.visuals().error_fg_color,
                    _ => ui.visuals().text_color(),
                };
                let text = RichText::new(&m.text).monospace().color(color);
                if m.position.is_some() {
                    let r = ui
                        .add(egui::Label::new(text).sense(egui::Sense::click()))
                        .on_hover_text("Click to go to the error in the editor.");
                    if r.clicked() {
                        action.jump_to = m.position;
                    }
                } else {
                    ui.label(text);
                }
            }
        });
}

fn results(ui: &mut egui::Ui, doc: u32, sets: &[ResultSet]) {
    let gap = ui.spacing().item_spacing.y;
    // Cada tabela ganha uma fatia da altura; a moldura ocupa uns pixels a
    // mais que a área pedida.
    let frame = 4.0;
    let n = sets.len() as f32;
    let each = ((ui.available_height() - (gap + frame) * n) / n).max(80.0);
    egui::ScrollArea::vertical()
        .id_salt(("results", doc))
        .auto_shrink(false)
        .show(ui, |ui| {
            for (i, set) in sets.iter().enumerate() {
                ui.allocate_ui(egui::vec2(ui.available_width(), each), |ui| {
                    table(ui, egui::Id::new(("result", doc, i)), set);
                });
            }
        });
}

const MAX_COLUMN_CHARS: usize = 40;

/// Tabela de um resultado: número da linha, cabeçalho e células, desenhando
/// só as linhas visíveis.
fn table(ui: &mut egui::Ui, id: egui::Id, set: &ResultSet) {
    let font = FontId::monospace(12.0);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 4.0;
    let pad = 8.0;
    let number_w = char_w * set.rows.len().max(1).to_string().len().max(2) as f32 + pad * 2.0;
    let widths: Vec<f32> = set
        .columns
        .iter()
        .enumerate()
        .map(|(c, name)| {
            let longest = set
                .rows
                .iter()
                .take(200)
                .map(|r| {
                    r.get(c)
                        .and_then(|v| v.as_deref())
                        .map_or(4, |v| v.chars().count())
                })
                .chain([name.chars().count()])
                .max()
                .unwrap_or(4)
                .min(MAX_COLUMN_CHARS);
            char_w * longest.max(4) as f32 + pad * 2.0
        })
        .collect();
    let total_w = number_w + widths.iter().sum::<f32>();
    let visuals = ui.visuals().clone();
    let grid = visuals.widgets.noninteractive.bg_stroke.color;
    let header_bg = visuals.faint_bg_color;
    let null_bg = Color32::from_rgba_unmultiplied(255, 255, 0xC0, 90);

    egui::Frame::new()
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt(id.with("h"))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // Cabeçalho.
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(total_w, row_h), egui::Sense::hover());
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, 0.0, header_bg);
                    let mut x = rect.left() + number_w;
                    for (name, w) in set.columns.iter().zip(&widths) {
                        let cell = egui::Rect::from_min_size(
                            egui::pos2(x, rect.top()),
                            egui::vec2(*w, row_h),
                        );
                        cell_text(
                            &painter,
                            cell,
                            name,
                            &font,
                            visuals.strong_text_color(),
                            pad,
                        );
                        painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, grid));
                        x += w;
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(id.with("v"))
                        .auto_shrink([false, false])
                        .show_rows(ui, row_h, set.rows.len(), |ui, range| {
                            for r in range {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(total_w, row_h),
                                    egui::Sense::hover(),
                                );
                                let painter = ui.painter_at(rect);
                                let num = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(number_w, row_h),
                                );
                                painter.rect_filled(num, 0.0, header_bg);
                                cell_text(
                                    &painter,
                                    num,
                                    &(r + 1).to_string(),
                                    &font,
                                    visuals.weak_text_color(),
                                    pad,
                                );
                                let mut x = rect.left() + number_w;
                                for (c, w) in widths.iter().enumerate() {
                                    let cell = egui::Rect::from_min_size(
                                        egui::pos2(x, rect.top()),
                                        egui::vec2(*w, row_h),
                                    );
                                    match set.rows[r].get(c).and_then(|v| v.as_deref()) {
                                        Some(v) => cell_text(
                                            &painter,
                                            cell,
                                            v,
                                            &font,
                                            visuals.text_color(),
                                            pad,
                                        ),
                                        None => {
                                            painter.rect_filled(cell.shrink(1.0), 0.0, null_bg);
                                            cell_text(
                                                &painter,
                                                cell,
                                                "NULL",
                                                &font,
                                                visuals.weak_text_color(),
                                                pad,
                                            );
                                        }
                                    }
                                    painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, grid));
                                    x += w;
                                }
                                painter.hline(
                                    rect.x_range(),
                                    rect.bottom() - 0.5,
                                    egui::Stroke::new(1.0, grid),
                                );
                            }
                        });
                });
        });
}

/// Texto de uma célula, cortado na largura (com `…`) e sem quebras.
fn cell_text(
    painter: &egui::Painter,
    cell: egui::Rect,
    text: &str,
    font: &FontId,
    color: Color32,
    pad: f32,
) {
    let one_line: String = text
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .take(MAX_COLUMN_CHARS * 4)
        .collect();
    let mut job = egui::text::LayoutJob::simple_singleline(one_line, font.clone(), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(cell.width() - pad * 2.0);
    let galley = painter.layout_job(job);
    let pos = egui::pos2(cell.left() + pad, cell.center().y - galley.size().y / 2.0);
    painter.with_clip_rect(cell).galley(pos, galley, color);
}
