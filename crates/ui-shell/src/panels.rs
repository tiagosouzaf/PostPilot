//! Painéis da janela principal (tool windows do SSMS).
//!
//! O conteúdo ainda é provisório: o Object Explorer de verdade entra no marco
//! 1.5 e as janelas de consulta no 1.6.

use eframe::egui;
use pgstudio_core::CommandId;
use pgstudio_docking::{Area, Pane, PaneViewer, Workspace};
use pgstudio_object_explorer::{Event, ObjectExplorer};

/// Um painel encaixável.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    ObjectExplorer,
    Properties,
    Output,
}

impl Panel {
    /// Texto da aba, como no SSMS.
    pub fn title(self) -> &'static str {
        match self {
            Panel::ObjectExplorer => "Object Explorer",
            Panel::Properties => "Properties",
            Panel::Output => "Output",
        }
    }
}

impl Pane for Panel {
    fn area(&self) -> Area {
        match self {
            Panel::ObjectExplorer => Area::Left,
            Panel::Properties => Area::Right,
            Panel::Output => Area::Bottom,
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
    /// Comandos pedidos pelos painéis neste frame.
    pub commands: Vec<CommandId>,
}

impl PaneViewer<Panel> for Viewer<'_> {
    fn title(&mut self, pane: &Panel) -> String {
        pane.title().to_string()
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
