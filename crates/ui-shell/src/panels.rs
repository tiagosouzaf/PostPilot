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
    /// Edit Top N Rows de uma tabela, pelo número da janela.
    EditRows(u32),
    /// Registered Servers (Ctrl+Alt+G).
    RegisteredServers,
    /// Workspace Explorer (Ctrl+Alt+L), como o Solution Explorer.
    WorkspaceExplorer,
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
            Panel::EditRows(_) => "Edit Rows",
            Panel::RegisteredServers => "Registered Servers",
            Panel::WorkspaceExplorer => "Workspace Explorer",
        }
    }
}

impl Pane for Panel {
    fn area(&self) -> Area {
        match self {
            Panel::ObjectExplorer | Panel::RegisteredServers | Panel::WorkspaceExplorer => {
                Area::Left
            }
            Panel::Properties => Area::Right,
            Panel::Output => Area::Bottom,
            Panel::Query(_)
            | Panel::TableDesigner(_)
            | Panel::Diagram(_)
            | Panel::ActivityMonitor
            | Panel::EditRows(_) => Area::Document,
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
    /// Janelas Edit Top N Rows.
    pub edit_rows: &'a mut std::collections::BTreeMap<u32, crate::edit_rows::EditRows>,
    /// Documento cujo editor deve receber o foco neste frame.
    pub focus_editor: Option<u32>,
    /// Comandos pedidos pelos painéis neste frame.
    pub commands: Vec<CommandId>,
    /// Ações pedidas pelo menu de contexto do Object Explorer.
    pub explorer_events: Vec<Event>,
    /// Documentos com alterações cujo fechamento foi pedido pela aba.
    pub close_requests: Vec<u32>,
    /// "Save Results As...": janela e índice do resultado.
    pub save_results: Vec<(u32, usize)>,
    /// Atalhos em uso, para o menu de contexto do editor.
    pub keymap: &'a pgstudio_core::keymap::Keymap,
    /// Cores do editor de SQL (seguem o tema).
    pub editor_colors: pgstudio_sql_editor::Colors,
    /// Servidor e usuário, para a faixa de status das janelas de consulta.
    pub server: String,
    /// Servidor de cada janela de consulta, para a faixa de status dela
    /// (o ativo pode ser outro).
    pub doc_servers: std::collections::HashMap<u32, String>,
    /// Catálogos do IntelliSense.
    pub catalogs: &'a crate::intellisense::Catalogs,
    /// Banco das janelas sem conexão própria.
    pub default_database: Option<String>,
    /// Os outros servidores conectados no Object Explorer.
    pub servers: &'a mut crate::servers::Servers,
    /// Janela Registered Servers.
    pub registered: &'a mut crate::registered::RegisteredPanel,
    /// Cofre das senhas lembradas.
    pub secrets: Option<&'a std::sync::Arc<dyn crate::secrets::SecretStore>>,
    /// Servidor guardado que o usuário usou neste frame (vira o ativo).
    pub activate_server: Option<usize>,
    /// Pedidos de conexão da janela Registered Servers.
    pub registered_actions: Vec<crate::registered::Action>,
    /// Workspace Explorer.
    pub workspace_panel: &'a mut crate::repo::panel::WorkspacePanel,
    /// Pedidos do Workspace Explorer.
    pub workspace_actions: Vec<crate::repo::panel::Action>,
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
            Panel::EditRows(id) => self
                .edit_rows
                .get(id)
                .map(|e| e.title())
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
            Panel::EditRows(id) => self
                .edit_rows
                .get_mut(id)
                .is_none_or(crate::edit_rows::EditRows::request_close),
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
                    // Como no SSMS, dá para conectar mais servidores.
                    if ui
                        .add(connect)
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
                let shown = crate::servers::show(ui, self.explorer, self.servers);
                self.explorer_events.extend(shown.events);
                self.activate_server = self.activate_server.or(shown.activate);
            }
            Panel::RegisteredServers => {
                let actions = self.registered.show(ui, self.secrets);
                self.registered_actions.extend(actions);
            }
            Panel::WorkspaceExplorer => {
                let actions = self.workspace_panel.show(ui, self.secrets);
                self.workspace_actions.extend(actions);
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
                let server = self.doc_servers.get(id).unwrap_or(&self.server);
                if let Some(exec) = &mut doc.execution {
                    let height = ui.available_height();
                    let action = egui::Panel::bottom(egui::Id::new(("results_pane", *id)))
                        .resizable(true)
                        .default_size(height * 0.45)
                        .min_size(90.0)
                        .show(ui, |ui| crate::results::show(ui, *id, exec, server))
                        .inner;
                    if let Some(i) = action.save_results {
                        self.save_results.push((*id, i));
                    }
                    if let Some(pos) = action.jump_to {
                        pgstudio_sql_editor::set_selection(ui.ctx(), editor_id(*id), (pos, pos));
                        doc.editor.focus_editor();
                    }
                }
                let db = match &doc.session {
                    Some(s) => Some(s.0.database()),
                    None => doc.database.clone().or(self.default_database.clone()),
                };
                if doc.execution.is_none()
                    && let Some(server) = self.doc_servers.get(id)
                {
                    // Antes de executar, a faixa já diz onde a janela roda.
                    egui::Panel::bottom(egui::Id::new(("query_status", *id)))
                        .resizable(false)
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 2)))
                        .show(ui, |ui| {
                            crate::results::connected_strip(ui, db.as_deref().unwrap_or(""), server)
                        });
                }
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
                out.response.context_menu(|ui| {
                    editor_context_menu(ui, self.keymap, &mut self.commands);
                });
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
            Panel::EditRows(id) => {
                if let Some(e) = self.edit_rows.get_mut(id) {
                    e.show(ui, *id);
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
/// Menu de contexto do editor: cada item vira um comando.
fn editor_context_menu(
    ui: &mut egui::Ui,
    keymap: &pgstudio_core::keymap::Keymap,
    commands: &mut Vec<CommandId>,
) {
    for entry in crate::menus::EDITOR_CONTEXT {
        match *entry {
            crate::menus::Entry::Item(cmd) => {
                let mut button = egui::Button::new(cmd.label());
                if let Some(s) = keymap.shortcut_for(cmd) {
                    button = button.shortcut_text(s.to_string());
                }
                if ui.add(button).clicked() {
                    commands.push(cmd);
                    ui.close();
                }
            }
            crate::menus::Entry::Separator => {
                ui.separator();
            }
            crate::menus::Entry::Submenu { .. } => {}
        }
    }
}

pub fn editor_id(doc: u32) -> egui::Id {
    egui::Id::new(("query_editor", doc))
}
