//! Painel de resultados da janela de consulta, como no SSMS: abas Results
//! e Messages e a faixa de status embaixo (situação, servidor, banco, tempo
//! e linhas).
//!
//! Os resultados aparecem na grade do `pgstudio-result-grid` ou, com
//! "Results to Text" (Ctrl+T), como texto de largura fixa.

use eframe::egui::{self, Color32, RichText};
use egui_phosphor::regular as icons;

use crate::execution::{
    Execution, MessageKind, Outcome, ResultSet, ResultsMode, Tab, format_elapsed,
};
use pgstudio_result_grid::{grid, text};

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
    let tab = match exec.tab {
        Tab::Plan if !exec.plans.is_empty() => Tab::Plan,
        Tab::Results if !exec.results.is_empty() => Tab::Results,
        _ => Tab::Messages,
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
        let plan = format!("{} Execution plan", icons::FLOW_ARROW);
        if !exec.plans.is_empty() && ui.selectable_label(tab == Tab::Plan, plan).clicked() {
            exec.tab = Tab::Plan;
        }
    });
    ui.separator();
    match tab {
        Tab::Results => match exec.mode {
            ResultsMode::Grid => grids(ui, doc, &mut exec.results),
            ResultsMode::Text => text_results(ui, doc, &mut exec.results),
        },
        Tab::Messages => messages(ui, doc, exec, &mut action),
        Tab::Plan => plans(ui, doc, exec),
    }
    action
}

/// Os planos do Ctrl+L, um embaixo do outro, cada um com o cabeçalho do
/// SSMS: número, custo relativo ao lote e o comando.
fn plans(ui: &mut egui::Ui, doc: u32, exec: &mut Execution) {
    let shares = pgstudio_execution_plan::batch_shares(exec.plans.iter().map(|(p, _)| p));
    let count = exec.plans.len();
    let height = (ui.available_height() / count as f32).max(180.0);
    egui::ScrollArea::vertical()
        .id_salt(("plans", doc))
        .auto_shrink(false)
        .show(ui, |ui| {
            for (i, (plan, view)) in exec.plans.iter_mut().enumerate() {
                ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                    let mut header = format!(
                        "Query {}: Query cost (relative to the batch): {:.0}%",
                        i + 1,
                        shares[i] * 100.0
                    );
                    if let (Some(p), Some(e)) = (plan.planning_ms, plan.execution_ms) {
                        header.push_str(&format!(
                            "   Planning time: {p:.3} ms   Execution time: {e:.3} ms"
                        ));
                    }
                    ui.label(header);
                    let statement: String = plan
                        .statement
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    ui.add(
                        egui::Label::new(RichText::new(statement).monospace().weak()).truncate(),
                    );
                    ui.separator();
                    ui.set_min_height(height - 8.0);
                    view.show(ui, plan, ("plan_view", doc, i));
                });
                ui.separator();
            }
        });
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

/// Id da grade do resultado `index` do documento `doc`.
pub fn grid_id(doc: u32, index: usize) -> egui::Id {
    egui::Id::new(("result_grid", doc, index))
}

/// Uma grade por resultado, dividindo a altura entre elas.
fn grids(ui: &mut egui::Ui, doc: u32, sets: &mut [ResultSet]) {
    let gap = ui.spacing().item_spacing.y;
    let n = sets.len() as f32;
    let each = ((ui.available_height() - gap * (n - 1.0)) / n).max(90.0);
    egui::ScrollArea::vertical()
        .id_salt(("results", doc))
        .auto_shrink(false)
        .show(ui, |ui| {
            for (i, set) in sets.iter_mut().enumerate() {
                let size = egui::vec2(ui.available_width(), each);
                ui.allocate_ui(size, |ui| {
                    ui.set_min_size(size);
                    let (store, state) = set.parts();
                    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
                    grid::show(ui, grid_id(doc, i), &mut store, state);
                });
            }
        });
}

/// Linhas do modo texto de um resultado: cabeçalho, traços, linhas, uma
/// em branco, "(n rows affected)" e outra em branco.
fn text_lines(set: &ResultSet) -> u64 {
    set.len() + 5
}

/// "Results to Text": cada resultado como texto de largura fixa, como no
/// SSMS. Só as linhas visíveis são montadas.
fn text_results(ui: &mut egui::Ui, doc: u32, sets: &mut [ResultSet]) {
    for set in sets.iter_mut() {
        // As linhas chegam enquanto o modo texto já aparece: recalcula até a
        // amostra completar, senão a largura fica a do cabeçalho.
        let sampled = set.len().min(text::WIDTH_SAMPLE);
        if set.text_widths.is_none() || set.text_width_rows != sampled {
            let widths = text::widths(&mut set.store());
            set.text_widths = Some(widths);
            set.text_width_rows = sampled;
        }
    }
    let total: u64 = sets.iter().map(text_lines).sum();
    let font = egui::FontId::monospace(12.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let color = ui.visuals().text_color();
    egui::ScrollArea::both()
        .id_salt(("results_text", doc))
        .auto_shrink(false)
        .show_rows(ui, row_h, total as usize, |ui, range| {
            for line in range {
                let mut k = line as u64;
                let mut text = String::new();
                for set in sets.iter_mut() {
                    let n = text_lines(set);
                    if k >= n {
                        k -= n;
                        continue;
                    }
                    let widths = set.text_widths.clone().unwrap_or_default();
                    let rows = set.len();
                    text = match k {
                        0 => text::header_line(&set.store(), &widths),
                        1 => text::rule_line(&widths),
                        k if k < rows + 2 => text::row_line(&mut set.store(), k - 2, &widths),
                        k if k == rows + 3 => crate::execution::rows_affected(rows),
                        _ => String::new(),
                    };
                    break;
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(text).font(font.clone()).color(color))
                        .extend(),
                );
            }
        });
}
