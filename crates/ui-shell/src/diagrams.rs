//! Database Diagrams: uma aba de documento com o diagrama de algumas tabelas
//! do banco, como o "New Database Diagram" do SSMS.
//!
//! O diagrama carrega o modelo do banco inteiro numa thread, abre o "Add
//! Table" para escolher as tabelas e desenha com o
//! [`pgstudio_diagram_engine`]. Por enquanto é só visualização: mover,
//! criar e salvar vêm na edição.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::metadata::{MetadataError, MetadataProvider, Model};
use pgstudio_core::schema::Id;
use pgstudio_diagram_engine::Diagram;

/// Todos os diagramas abertos, por número.
#[derive(Default)]
pub struct Diagrams {
    open: BTreeMap<u32, DiagramView>,
    next_id: u32,
    /// Último `Diagram_N`, como no SSMS.
    next_number: u32,
}

impl Diagrams {
    /// Abre um diagrama novo do banco `database` e devolve o número dele.
    /// Com `all_tables`, põe todas as tabelas sem perguntar (screenshots e
    /// medição).
    pub fn open(
        &mut self,
        provider: Arc<dyn MetadataProvider>,
        database: String,
        all_tables: bool,
        ctx: &egui::Context,
    ) -> u32 {
        let (tx, rx) = channel();
        let db = database.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider.load_model(&db));
            repaint.request_repaint();
        });
        self.open_with(database, State::Loading(rx), all_tables)
    }

    /// Abre um diagrama de um modelo já pronto (medição).
    pub fn open_model(&mut self, database: String, model: Model, all_tables: bool) -> u32 {
        let id = self.open_with(database, State::Loading(channel().1), all_tables);
        if let Some(d) = self.open.get_mut(&id) {
            d.loaded(model);
        }
        id
    }

    fn open_with(&mut self, database: String, state: State, all_tables: bool) -> u32 {
        self.next_id += 1;
        let number = self.next_number;
        self.next_number += 1;
        self.open.insert(
            self.next_id,
            DiagramView {
                database,
                number,
                state,
                model: Model::default(),
                diagram: Diagram::default(),
                picker: None,
                all_tables,
            },
        );
        self.next_id
    }

    pub fn get(&self, id: u32) -> Option<&DiagramView> {
        self.open.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut DiagramView> {
        self.open.get_mut(&id)
    }

    pub fn ids(&self) -> Vec<u32> {
        self.open.keys().copied().collect()
    }

    pub fn remove(&mut self, id: u32) {
        self.open.remove(&id);
    }

    /// Algum diagrama ainda carregando (screenshots).
    pub fn is_loading(&self) -> bool {
        self.open
            .values()
            .any(|d| matches!(d.state, State::Loading(_)))
    }

    /// Recebe os modelos que as threads carregaram.
    pub fn step(&mut self) {
        for d in self.open.values_mut() {
            d.step();
        }
    }
}

enum State {
    Loading(Receiver<Result<Model, MetadataError>>),
    Failed(String),
    Ready,
}

/// O "Add Table": filtro e tabelas marcadas.
#[derive(Debug, Default)]
pub struct Picker {
    pub filter: String,
    pub checked: BTreeSet<Id>,
    /// O filtro já recebeu o foco (ele ganha ao abrir).
    focused: bool,
}

/// Um diagrama aberto.
pub struct DiagramView {
    database: String,
    number: u32,
    state: State,
    model: Model,
    diagram: Diagram,
    picker: Option<Picker>,
    all_tables: bool,
}

impl DiagramView {
    /// Texto da aba: `banco - Diagram_0`.
    pub fn title(&self) -> String {
        format!("{} - Diagram_{}", self.database, self.number)
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    /// Mensagem de erro da carga, se falhou.
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn diagram(&self) -> &Diagram {
        &self.diagram
    }

    pub fn diagram_mut(&mut self) -> &mut Diagram {
        &mut self.diagram
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    /// O "Add Table" aberto, se estiver.
    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    pub fn picker_mut(&mut self) -> Option<&mut Picker> {
        self.picker.as_mut()
    }

    /// Abre o "Add Table".
    pub fn open_picker(&mut self) {
        if matches!(self.state, State::Ready) {
            self.picker = Some(Picker::default());
        }
    }

    fn step(&mut self) {
        let State::Loading(rx) = &self.state else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(model)) => self.loaded(model),
            Ok(Err(e)) => self.state = State::Failed(e.0),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state = State::Failed("The model could not be loaded.".to_string());
            }
        }
    }

    fn loaded(&mut self, model: Model) {
        self.model = model;
        self.state = State::Ready;
        if self.all_tables {
            let all: Vec<Id> = self.model.tables.iter().map(|t| t.id).collect();
            self.add_tables(&all);
        } else {
            self.open_picker();
        }
    }

    /// Tabelas que o "Add Table" mostra: as que ainda não estão no
    /// diagrama e que passam no filtro, por nome.
    pub fn candidates(&self) -> Vec<(Id, String)> {
        let filter = self
            .picker
            .as_ref()
            .map(|p| p.filter.to_lowercase())
            .unwrap_or_default();
        let shown: BTreeSet<Id> = self.diagram.tables().into_iter().collect();
        let mut out: Vec<(Id, String)> = self
            .model
            .tables
            .iter()
            .filter(|t| !shown.contains(&t.id))
            .map(|t| (t.id, format!("{}.{}", t.schema, t.name)))
            .filter(|(_, name)| name.to_lowercase().contains(&filter))
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    /// Põe tabelas no diagrama e refaz o layout.
    pub fn add_tables(&mut self, ids: &[Id]) {
        let mut tables = self.diagram.tables();
        tables.extend(
            ids.iter()
                .filter(|id| !tables.contains(id))
                .copied()
                .collect::<Vec<_>>(),
        );
        let camera_kept = !self.diagram.nodes.is_empty();
        let selected = self.diagram.selected;
        self.diagram = Diagram::from_model(&self.model, &tables);
        if camera_kept {
            self.diagram.selected = selected;
        }
    }

    /// Adiciona as tabelas marcadas no "Add Table" e fecha ele.
    pub fn add_checked(&mut self) {
        if let Some(p) = self.picker.take() {
            let ids: Vec<Id> = p.checked.into_iter().collect();
            self.add_tables(&ids);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, id: u32) {
        match &self.state {
            State::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading tables...");
                });
                return;
            }
            State::Failed(e) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("{} {e}", icon::WARNING),
                );
                return;
            }
            State::Ready => {}
        }
        self.toolbar(ui);
        ui.separator();
        if self.diagram.nodes.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.weak("The diagram is empty. Use Add Table to show tables of the database.");
            });
        } else {
            self.diagram.show(ui);
        }
        self.picker_window(ui.ctx(), id);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(format!("{} Add Table...", icon::PLUS))
                .on_hover_text("Add tables of the database to the diagram")
                .clicked()
            {
                self.open_picker();
            }
            let has = !self.diagram.nodes.is_empty();
            if ui
                .add_enabled(
                    has,
                    egui::Button::new(format!("{} Arrange Tables", icon::GRID_FOUR)),
                )
                .clicked()
            {
                self.diagram.auto_layout();
            }
            if ui
                .add_enabled(
                    has,
                    egui::Button::new(format!("{} Zoom to Fit", icon::CORNERS_OUT)),
                )
                .clicked()
            {
                self.diagram.fit();
            }
            ui.add_space(8.0);
            ui.label(format!("{:.0}%", self.diagram.camera.zoom * 100.0));
            ui.add_space(8.0);
            ui.weak(format!(
                "{} tables, {} relationships",
                self.diagram.nodes.len(),
                self.diagram.edges.len()
            ));
        });
    }

    fn picker_window(&mut self, ctx: &egui::Context, id: u32) {
        if self.picker.is_none() {
            return;
        }
        let candidates = self.candidates();
        let mut add = false;
        let mut close = false;
        let picker = self.picker.as_mut().expect("checado acima");
        let modal = egui::Modal::new(egui::Id::new(("diagram_add_table", id))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Add Table");
            ui.horizontal(|ui| {
                ui.label("Filter:");
                let filter = ui.add(
                    egui::TextEdit::singleline(&mut picker.filter).desired_width(f32::INFINITY),
                );
                if !picker.focused {
                    filter.request_focus();
                    picker.focused = true;
                }
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, 20.0, candidates.len(), |ui, range| {
                        for (table, name) in &candidates[range] {
                            let mut on = picker.checked.contains(table);
                            if ui.checkbox(&mut on, name).changed() {
                                if on {
                                    picker.checked.insert(*table);
                                } else {
                                    picker.checked.remove(table);
                                }
                            }
                        }
                    });
                if candidates.is_empty() {
                    ui.weak("No tables to add.");
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Select All").clicked() {
                    picker.checked.extend(candidates.iter().map(|(t, _)| *t));
                }
                if ui.button("Clear").clicked() {
                    picker.checked.clear();
                }
                ui.weak(format!("{} selected", picker.checked.len()));
            });
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
                let n = picker.checked.len();
                if ui.add_enabled(n > 0, egui::Button::new("Add")).clicked() {
                    add = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if add {
            self.add_checked();
        } else if close {
            self.picker = None;
        }
    }
}
