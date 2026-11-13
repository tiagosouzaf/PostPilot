//! Painéis da janela principal (tool windows do SSMS).
//!
//! O conteúdo ainda é provisório: o Object Explorer de verdade entra no marco
//! 1.5 e as janelas de consulta no 1.6.

use eframe::egui;
use pgstudio_docking::{Area, Pane, PaneViewer, Workspace};

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

/// Dados que os painéis mostram.
pub struct Viewer<'a> {
    /// Linhas do painel Output.
    pub output: &'a [String],
}

impl PaneViewer<Panel> for Viewer<'_> {
    fn title(&mut self, pane: &Panel) -> String {
        pane.title().to_string()
    }

    fn ui(&mut self, ui: &mut egui::Ui, pane: &mut Panel) {
        match pane {
            Panel::ObjectExplorer => {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(false, |ui| {
                        let _ =
                            ui.button(format!("Connect {}", egui_phosphor::regular::CARET_DOWN));
                    });
                });
                ui.separator();
                ui.weak("Not connected.");
            }
            Panel::Properties => {
                ui.weak("No object selected.");
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
