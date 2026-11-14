//! Painéis da janela principal (tool windows do SSMS).
//!
//! Tool windows (Object Explorer, Properties, Output) e janelas de consulta.

use eframe::egui;
use pgstudio_core::CommandId;
use pgstudio_docking::{Area, Pane, PaneViewer, Workspace};
use pgstudio_object_explorer::{Event, ObjectExplorer, Properties};

use crate::documents::Documents;

/// Um painel encaixável.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Panel {
    ObjectExplorer,
    Properties,
    Output,
    /// Janela de consulta, pelo número do documento.
    Query(u32),
    /// Table Designer, pelo número dele.
    TableDesigner(u32),
    /// Database Diagram, pelo número dele.
    Diagram(u32),
    /// Activity Monitor (um só, do servidor conectado).
    ActivityMonitor,
}

impl Panel {
    /// Texto da aba, como no SSMS.
    pub fn title(self) -> &'static str {
        match self {
            Panel::ObjectExplorer => "Object Explorer",
            Panel::Properties => "Properties",
            Panel::Output => "Output",
            Panel::Query(_) => "Query",
            Panel::TableDesigner(_) => "Table Designer",
            Panel::Diagram(_) => "Database Diagram",
            Panel::ActivityMonitor => "Activity Monitor",
        }
    }
}

impl Pane for Panel {
    fn area(&self) -> Area {
        match self {
            Panel::ObjectExplorer => Area::Left,
            Panel::Properties => Area::Right,
            Panel::Output => Area::Bottom,
            Panel::Query(_)
            | Panel::TableDesigner(_)
            | Panel::Diagram(_)
            | Panel::ActivityMonitor => Area::Document,
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
    /// Table Designers.
    pub designers: &'a mut crate::designer::Designers,
    /// Database Diagrams.
    pub diagrams: &'a mut crate::diagrams::Diagrams,
    /// Activity Monitor, se aberto.
    pub activity: &'a mut Option<crate::activity::ActivityMonitor>,
    /// Documento cujo editor deve receber o foco neste frame.
    pub focus_editor: Option<u32>,
    /// Comandos pedidos pelos painéis neste frame.
    pub commands: Vec<CommandId>,
    /// Ações pedidas pelo menu de contexto do Object Explorer.
    pub explorer_events: Vec<Event>,
    /// Documentos com alterações cujo fechamento foi pedido pela aba.
    pub close_requests: Vec<u32>,
    /// Cores do editor de SQL (seguem o tema).
    pub editor_colors: pgstudio_sql_editor::Colors,
    /// Servidor e usuário, para a faixa de status das janelas de consulta.
    pub server: String,
    /// Catálogos do IntelliSense.
    pub catalogs: &'a crate::intellisense::Catalogs,
    /// Banco das janelas sem conexão própria.
    pub default_database: Option<String>,
}

impl PaneViewer<Panel> for Viewer<'_> {
    fn title(&mut self, pane: &Panel) -> String {
        match pane {
            Panel::Query(id) => self
                .documents
                .get(*id)
                .map(|d| d.title())
                .unwrap_or_default(),
            Panel::TableDesigner(id) => self
                .designers
                .get(*id)
                .map(|d| d.title())
                .unwrap_or_default(),
            Panel::Diagram(id) => self
                .diagrams
                .get(*id)
                .map(|d| d.title())
                .unwrap_or_default(),
            Panel::ActivityMonitor => self
                .activity
                .as_ref()
                .map(|a| a.title())
                .unwrap_or_else(|| pane.title().to_string()),
            _ => pane.title().to_string(),
        }
    }

    fn on_close(&mut self, pane: &Panel) -> bool {
        match pane {
            Panel::Query(id) if self.documents.get(*id).is_some_and(|d| d.is_dirty()) => {
                self.close_requests.push(*id);
                false
            }
            Panel::TableDesigner(id) => self
                .designers
                .get_mut(*id)
                .is_none_or(crate::designer::TableDesigner::request_close),
            Panel::Diagram(id) => self
                .diagrams
                .get_mut(*id)
                .is_none_or(crate::diagrams::DiagramView::request_close),
            Panel::ActivityMonitor => {
                *self.activity = None;
                true
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
                let events = self.explorer.show(ui);
                self.explorer_events.extend(events);
            }
            Panel::Properties => match self.explorer.selected().cloned() {
                Some(node) => {
                    let props = self.explorer.properties(&node.id).cloned();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("properties")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                let row = |ui: &mut egui::Ui, name: &str, value: &str| {
                                    ui.strong(name);
                                    ui.add(egui::Label::new(value).wrap());
                                    ui.end_row();
                                };
                                row(ui, "Name", &node.name);
                                row(ui, "Type", node.kind.label());
                                if let Some(Properties::Loaded(props)) = &props {
                                    for p in props {
                                        row(ui, &p.name, &p.value);
                                    }
                                }
                                row(ui, "Path", &node.id.to_string());
                            });
                        match props {
                            Some(Properties::Loading) => {
                                ui.weak("Loading...");
                                // A leitura não acorda a interface: olha de
                                // novo daqui a pouco.
                                ui.ctx()
                                    .request_repaint_after(std::time::Duration::from_millis(30));
                            }
                            Some(Properties::Failed(e)) => {
                                ui.colored_label(ui.visuals().error_fg_color, e);
                            }
                            _ => {}
                        }
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
                if let Some(exec) = &mut doc.execution {
                    let height = ui.available_height();
                    let action = egui::Panel::bottom(egui::Id::new(("results_pane", *id)))
                        .resizable(true)
                        .default_size(height * 0.45)
                        .min_size(90.0)
                        .show(ui, |ui| crate::results::show(ui, *id, exec, &self.server))
                        .inner;
                    if let Some(pos) = action.jump_to {
                        pgstudio_sql_editor::set_selection(ui.ctx(), editor_id(*id), (pos, pos));
                        doc.editor.focus_editor();
                    }
                }
                let db = match &doc.session {
                    Some(s) => Some(s.0.database()),
                    None => doc.database.clone().or(self.default_database.clone()),
                };
                let catalog = db.and_then(|db| self.catalogs.get(&db));
                let out = pgstudio_sql_editor::SqlEditor::new(
                    &mut doc.text,
                    &mut doc.editor,
                    editor_id(*id),
                )
                .colors(self.editor_colors)
                .catalog(catalog)
                .show(ui);
                if out.cursor.is_some() {
                    doc.cursor = out.cursor;
                }
            }
            Panel::TableDesigner(id) => {
                if let Some(d) = self.designers.get_mut(*id) {
                    d.show(ui, *id);
                }
            }
            Panel::Diagram(id) => {
                if let Some(d) = self.diagrams.get_mut(*id) {
                    d.show(ui, *id);
                }
            }
            Panel::ActivityMonitor => match self.activity {
                Some(a) => a.show(ui),
                None => {
                    ui.weak("Connect to a server to monitor its activity.");
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

/// Id do editor de um documento (para dar foco e para testes).
pub fn editor_id(doc: u32) -> egui::Id {
    egui::Id::new(("query_editor", doc))
}
