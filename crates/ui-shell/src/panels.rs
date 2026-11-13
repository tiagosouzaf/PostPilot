//! Painéis da janela principal (tool windows do SSMS).
//!
//! Tool windows (Object Explorer, Properties, Output) e janelas de consulta.

use eframe::egui;
use pgstudio_core::CommandId;
use pgstudio_docking::{Area, Pane, PaneViewer, Workspace};
use pgstudio_object_explorer::{Event, ObjectExplorer};

use crate::documents::Documents;

/// Um painel encaixável.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Panel {
    ObjectExplorer,
    Properties,
    Output,
    /// Janela de consulta, pelo número do documento.
    Query(u32),
}

impl Panel {
    /// Texto da aba, como no SSMS.
    pub fn title(self) -> &'static str {
        match self {
            Panel::ObjectExplorer => "Object Explorer",
            Panel::Properties => "Properties",
            Panel::Output => "Output",
            Panel::Query(_) => "Query",
        }
    }
}

impl Pane for Panel {
    fn area(&self) -> Area {
        match self {
            Panel::ObjectExplorer => Area::Left,
            Panel::Properties => Area::Right,
            Panel::Output => Area::Bottom,
            Panel::Query(_) => Area::Document,
        }
    }
}

/// Layout padrão do SSMS (também usado por "Reset Window Layout").
pub fn default_layout() -> Workspace<Panel> {
    Workspace::new(
        Vec::new(),
        vec![Panel::ObjectExplorer],
        vec![Panel::Properties],
        vec![Panel::Output],
    )
}

/// Dados que os painéis mostram e comandos que eles pedem.
pub struct Viewer<'a> {
    /// Linhas do painel Output.
    pub output: &'a [String],
    /// Árvore do Object Explorer.
    pub explorer: &'a mut ObjectExplorer,
    /// Janelas de consulta.
    pub documents: &'a mut Documents,
    /// Documento cujo editor deve receber o foco neste frame.
    pub focus_editor: Option<u32>,
    /// Comandos pedidos pelos painéis neste frame.
    pub commands: Vec<CommandId>,
    /// Documentos com alterações cujo fechamento foi pedido pela aba.
    pub close_requests: Vec<u32>,
    /// Cores do editor de SQL (seguem o tema).
    pub editor_colors: pgstudio_sql_editor::Colors,
}

impl PaneViewer<Panel> for Viewer<'_> {
    fn title(&mut self, pane: &Panel) -> String {
        match pane {
            Panel::Query(id) => self
                .documents
                .get(*id)
                .map(|d| d.title())
                .unwrap_or_default(),
            _ => pane.title().to_string(),
        }
    }

    fn on_close(&mut self, pane: &Panel) -> bool {
        match pane {
            Panel::Query(id) if self.documents.get(*id).is_some_and(|d| d.is_dirty()) => {
                self.close_requests.push(*id);
                false
            }
            _ => true,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &mut Panel) {
        match pane {
            Panel::ObjectExplorer => {
                ui.horizontal(|ui| {
                    let connected = self.explorer.is_connected();
                    let connect = egui::Button::new(format!(
                        "{} Connect",
                        egui_phosphor::regular::PLUGS_CONNECTED
                    ));
                    if ui
                        .add_enabled(!connected, connect)
                        .on_hover_text("Connect Object Explorer")
                        .clicked()
                    {
                        self.commands.push(CommandId::ConnectObjectExplorer);
                    }
                    let disconnect = egui::Button::new(egui_phosphor::regular::PLUGS);
                    if ui
                        .add_enabled(connected, disconnect)
                        .on_hover_text("Disconnect")
                        .clicked()
                    {
                        self.commands.push(CommandId::DisconnectObjectExplorer);
                    }
                });
                ui.separator();
                for event in self.explorer.show(ui) {
                    match event {
                        Event::NewQuery(_) => self.commands.push(CommandId::NewQuery),
                    }
                }
            }
            Panel::Properties => match self.explorer.selected() {
                Some(node) => {
                    egui::Grid::new("properties")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("Name");
                            ui.label(&node.name);
                            ui.end_row();
                            ui.strong("Type");
                            ui.label(node.kind.label());
                            ui.end_row();
                            ui.strong("Path");
                            ui.label(node.id.to_string());
                            ui.end_row();
                        });
                }
                None => {
                    ui.weak("No object selected.");
                }
            },
            Panel::Query(id) => {
                let Some(doc) = self.documents.get_mut(*id) else {
                    return;
                };
                if self.focus_editor == Some(*id) {
                    doc.editor.focus_editor();
                }
                let out = pgstudio_sql_editor::SqlEditor::new(
                    &mut doc.text,
                    &mut doc.editor,
                    editor_id(*id),
                )
                .colors(self.editor_colors)
                .show(ui);
                if out.cursor.is_some() {
                    doc.cursor = out.cursor;
                }
            }
            Panel::Output => {
                ui.horizontal(|ui| {
                    ui.label("Show output from:");
                    ui.add_enabled_ui(false, |ui| {
                        egui::ComboBox::from_id_salt("output_source")
                            .selected_text("General")
                            .show_ui(ui, |_| {});
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.output {
                            ui.monospace(line);
                        }
                    });
            }
        }
    }
}

/// Id do editor de um documento (para dar foco e para testes).
pub fn editor_id(doc: u32) -> egui::Id {
    egui::Id::new(("query_editor", doc))
}
