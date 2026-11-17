//! Painel de resultados da janela de consulta, como no SSMS: abas Results
//! e Messages e a faixa de status embaixo (situação, servidor, banco, tempo
//! e linhas).
//!
//! Os resultados aparecem na grade do `pgstudio-result-grid` ou, com
//! "Results to Text" (Ctrl+T), como texto de largura fixa.

use crate::fit;
use crate::registered::ServerTag;
use eframe::egui::{self, Color32, RichText};
use egui_phosphor::regular as icons;
use pgstudio_i18n::{tr, trf};

use crate::execution::{
    Execution, MessageKind, Outcome, ResultSet, ResultsMode, Tab, format_elapsed,
};
use pgstudio_result_grid::{grid, text};

use crate::result_edit::ResultEditor;

/// Salvar o resultado com este índice, com a extensão sugerida (`csv` em
/// "Save Results As...", `xlsx` em "Export to Excel...").
pub type SaveRequest = (usize, &'static str);
/// Abrir o JSON Viewer nesta célula do resultado com este índice.
pub type JsonRequest = (usize, pgstudio_result_grid::Cell);

/// O que o usuário pediu no painel.
#[derive(Default)]
pub struct Action {
    /// Clicou numa mensagem de erro: levar o cursor a esta posição.
    pub jump_to: Option<usize>,
    /// "Save Results As..." (`csv`) ou "Export to Excel..." (`xlsx`) no
    /// resultado com este índice, com a extensão sugerida.
    pub save_results: Option<(usize, &'static str)>,
    /// Abrir o JSON Viewer nesta célula do resultado com este índice.
    pub open_json: Option<(usize, pgstudio_result_grid::Cell)>,
}

/// O servidor na faixa de status: o texto mostrado (o endereço ou, num
/// servidor registrado, a pasta e o nome do registro) e, quando o texto
/// não é o endereço, o endereço para a dica.
#[derive(Clone, Copy)]
pub struct Server<'a> {
    pub label: &'a str,
    pub host: Option<&'a str>,
}

impl<'a> From<&'a str> for Server<'a> {
    fn from(label: &'a str) -> Self {
        Server { label, host: None }
    }
}

/// Desenha o painel de resultados. `server` é o texto da conexão.
pub fn show<'a>(
    ui: &mut egui::Ui,
    doc: u32,
    exec: &mut Execution,
    server: impl Into<Server<'a>>,
    tag: Option<&ServerTag>,
) -> Action {
    let server = server.into();
    let mut action = Action::default();
    egui::Panel::bottom(egui::Id::new(("query_status", doc)))
        .resizable(false)
        .frame(strip_frame(ui, tag))
        .show(ui, |ui| status_strip(ui, exec, server, tag));
    // Sem resultados (ainda), só há o que mostrar em Messages; a escolha
    // do usuário fica guardada para quando eles chegarem.
    if let Some(restored) = exec.restored() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::CLOCK_COUNTER_CLOCKWISE).weak());
            ui.add(egui::Label::new(RichText::new(restored.notice()).weak()).truncate());
        });
    }
    let tab = match exec.tab {
        Tab::Plan if !exec.plans.is_empty() => Tab::Plan,
        Tab::Results if !exec.results.is_empty() => Tab::Results,
        _ => Tab::Messages,
    };
    ui.horizontal(|ui| {
        let results = format!("{} {}", icons::TABLE, tr("Results"));
        if !exec.results.is_empty() && ui.selectable_label(tab == Tab::Results, results).clicked() {
            exec.tab = Tab::Results;
        }
        let messages = format!("{} {}", icons::CHAT_TEXT, tr("Messages"));
        if ui
            .selectable_label(tab == Tab::Messages, messages)
            .clicked()
        {
            exec.tab = Tab::Messages;
        }
        let plan = format!("{} {}", icons::FLOW_ARROW, tr("Execution plan"));
        if !exec.plans.is_empty() && ui.selectable_label(tab == Tab::Plan, plan).clicked() {
            exec.tab = Tab::Plan;
        }
    });
    ui.separator();
    match tab {
        Tab::Results => match exec.mode {
            ResultsMode::Grid | ResultsMode::File => {
                let mut log = Vec::new();
                let place = crate::result_edit::Place {
                    server: server.label.to_string(),
                    environment: tag.map(|t| (t.environment.clone().unwrap_or_default(), t.color)),
                };
                (action.save_results, action.open_json) =
                    grids(ui, doc, &mut exec.results, &mut log, &place);
                for text in log {
                    exec.push_info(text);
                }
            }
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
                    let mut header = trf!(
                        "Query {n}: Query cost (relative to the batch): {cost}%",
                        n = i + 1,
                        cost = format!("{:.0}", shares[i] * 100.0)
                    );
                    if let (Some(p), Some(e)) = (plan.planning_ms, plan.execution_ms) {
                        header.push_str(&trf!(
                            "   Planning time: {planning} ms   Execution time: {execution} ms",
                            planning = format!("{p:.3}"),
                            execution = format!("{e:.3}")
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

/// Faixa de status de uma janela de consulta que ainda não executou:
/// o servidor e o banco dela.
pub fn connected_strip<'a>(
    ui: &mut egui::Ui,
    database: &str,
    server: impl Into<Server<'a>>,
    tag: Option<&ServerTag>,
) {
    let server = server.into();
    let icon = RichText::new(icons::PLUG).color(Color32::from_rgb(0x10, 0x7C, 0x10));
    strip(ui, icon, tr("Connected."), server, tag, |ui| {
        fit::text(ui, database);
    });
}

/// Uma faixa de status que cabe em qualquer largura. Da direita para a
/// esquerda: os itens de `fixed`, o servidor, a etiqueta do ambiente e,
/// no que sobrar à esquerda, o ícone com a mensagem. Quando falta espaço
/// o servidor encurta primeiro (o nome inteiro fica na dica), depois a
/// mensagem; nada é desenhado por cima do vizinho.
fn strip(
    ui: &mut egui::Ui,
    icon: RichText,
    message: &str,
    server: Server<'_>,
    tag: Option<&ServerTag>,
    fixed: impl FnOnce(&mut egui::Ui),
) {
    fit::ends(
        ui,
        |ui| {
            ui.label(icon.clone());
            fit::text(ui, message);
        },
        |ui| {
            fixed(ui);
            fit::separator(ui);
            let spacing = ui.spacing().item_spacing.x;
            let badge = match tag {
                Some(ServerTag {
                    environment: Some(name),
                    ..
                }) => crate::registered::environment_badge_width(ui, name) + spacing,
                _ => 0.0,
            };
            // A mensagem inteira tem preferência sobre o nome do servidor,
            // que ainda assim fica com pelo menos 40% do espaço.
            let left = fit::text_width(ui, icon.clone()) + fit::text_width(ui, message);
            let free = ui.available_width() - badge;
            let server_max = (free - left - 3.0 * spacing).max(free * 0.4);
            let label = fit::text_within(ui, server.label, server_max);
            if let (Some(label), Some(host)) = (label, server.host) {
                label.on_hover_text(host);
            }
            if badge > 0.0 && ui.available_width() >= badge - spacing {
                environment(ui, tag);
            }
        },
    );
}

/// Fundo da faixa de status: tingido com a cor do ambiente (ou da pasta)
/// do servidor da janela.
pub fn strip_frame(ui: &egui::Ui, tag: Option<&ServerTag>) -> egui::Frame {
    let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 2));
    match tag {
        Some(t) => {
            let alpha = if ui.visuals().dark_mode { 0.30 } else { 0.22 };
            frame.fill(t.color.color32().gamma_multiply(alpha))
        }
        None => frame,
    }
}

/// A etiqueta do ambiente, se o servidor tem um.
fn environment(ui: &mut egui::Ui, tag: Option<&ServerTag>) {
    if let Some(ServerTag {
        environment: Some(name),
        color,
        ..
    }) = tag
    {
        crate::registered::environment_badge(ui, name, *color);
    }
}

fn status_strip(ui: &mut egui::Ui, exec: &Execution, server: Server<'_>, tag: Option<&ServerTag>) {
    let outcome = exec.outcome();
    let (icon, color) = match outcome {
        Outcome::Running => (icons::HOURGLASS, ui.visuals().text_color()),
        Outcome::Succeeded => (icons::CHECK_CIRCLE, Color32::from_rgb(0x10, 0x7C, 0x10)),
        Outcome::Failed => (icons::X_CIRCLE, ui.visuals().error_fg_color),
        Outcome::Canceled => (icons::PROHIBIT, ui.visuals().warn_fg_color),
    };
    let icon = RichText::new(icon).color(color);
    strip(ui, icon, outcome.label(), server, tag, |ui| {
        let rows = exec.total_rows();
        fit::text(
            ui,
            if rows == 1 {
                tr("1 row").to_string()
            } else {
                trf!("{rows} rows", rows = rows)
            },
        );
        fit::separator(ui);
        fit::text(ui, format_elapsed(exec.elapsed(), false));
        fit::separator(ui);
        fit::text(ui, exec.database.as_str());
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
                        .on_hover_text(tr("Click to go to the error in the editor."));
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
/// As grades, uma embaixo da outra. Devolve o resultado em que o usuário
/// pediu "Save Results As..." e a célula em que pediu o JSON Viewer.
fn grids(
    ui: &mut egui::Ui,
    doc: u32,
    sets: &mut [ResultSet],
    log: &mut Vec<String>,
    place: &crate::result_edit::Place,
) -> (Option<SaveRequest>, Option<JsonRequest>) {
    let mut save = None;
    let mut json = None;
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
                    let id = grid_id(doc, i);
                    let editor = &mut set.editor;
                    let store = set.store.clone();
                    let state = &mut set.grid;
                    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(editor) = editor.as_mut() {
                        editor.step(&mut store, state);
                        edit_strip(ui, editor);
                    }
                    let out = grid::show(ui, id, &mut store, state);
                    if out.export_excel {
                        save = Some((i, "xlsx"));
                    } else if out.save_as {
                        save = Some((i, "csv"));
                    }
                    if let Some(editor) = editor.as_mut() {
                        editor.handle(&out, &mut store, state);
                        editor.confirm_window(ui.ctx(), &mut store, id, place);
                        log.extend(editor.take_log());
                    }
                    if let Some(cell) = out.open_json {
                        json = Some((i, cell));
                    }
                });
            }
        });
    (save, json)
}

/// Linha acima da grade com a situação da edição: a tabela editável e
/// o que aconteceu por último, ou por que o resultado é só leitura (depois
/// que o usuário tentou editar).
fn edit_strip(ui: &mut egui::Ui, editor: &ResultEditor) {
    let message = editor.message();
    let editable = editor.editing();
    if message.is_none() && editable.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        if let Some(t) = &editable {
            ui.label(RichText::new(icons::PENCIL_SIMPLE).weak())
                .on_hover_text(tr(
                    "Double-click a cell (or press F2) to edit it; Enter asks for confirmation \
                     and saves, Esc cancels. Delete removes the selected rows after confirmation.",
                ));
            ui.weak(trf!("Editing {table}", table = t));
            if message.is_some() {
                ui.separator();
            }
        }
        if editor.is_busy() && editable.is_some() {
            ui.spinner();
        }
        if let Some(m) = message {
            ui.add(egui::Label::new(m).truncate());
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
