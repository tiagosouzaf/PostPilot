//! Database Diagrams: uma aba de documento com o diagrama de algumas tabelas
//! do banco, como o "New Database Diagram" do SSMS.
//!
//! O diagrama carrega o modelo do banco inteiro numa thread, abre o "Add
//! Table" para escolher as tabelas e desenha com o
//! [`pgstudio_diagram_engine`]. Dá para mover e redimensionar tabelas,
//! criar tabela, coluna e relacionamento (arrastando a chave até a coluna
//! que vai referenciá-la), com undo/redo. "Save" mostra o DDL antes de
//! aplicar no banco numa transação, e grava o diagrama (quais tabelas, onde
//! e o zoom) num arquivo `.pgsdiagram`, que File > Open abre de novo.

use pgstudio_i18n::{tr, trf};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::metadata::{MetadataError, MetadataProvider, Model, NodeId};
use pgstudio_core::schema::diff::{diff, unsupported};
use pgstudio_core::schema::edit::{Document, EditError};
use pgstudio_core::schema::{Column, ForeignKey, Id};
use pgstudio_diagram_engine::{Camera, Diagram, DiagramEvent};
use serde::{Deserialize, Serialize};

/// Extensão dos arquivos de diagrama.
pub const EXTENSION: &str = "pgsdiagram";

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
        node: Option<NodeId>,
        all_tables: bool,
        ctx: &egui::Context,
    ) -> u32 {
        let rx = load(&provider, &database, Some(ctx));
        let id = self.open_with(database, Some(provider), State::Loading(rx), all_tables);
        if let Some(d) = self.open.get_mut(&id) {
            d.node = node;
        }
        id
    }

    /// Abre um arquivo `.pgsdiagram` salvo antes; o modelo vem do banco
    /// dele pelo `provider`.
    pub fn open_file(
        &mut self,
        provider: Arc<dyn MetadataProvider>,
        path: &Path,
    ) -> Result<u32, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let file: DiagramFile =
            serde_json::from_str(&text).map_err(|e| format!("not a diagram file: {e}"))?;
        let rx = load(&provider, &file.database, None);
        let id = self.open_with(
            file.database.clone(),
            Some(provider),
            State::Loading(rx),
            false,
        );
        if let Some(d) = self.open.get_mut(&id) {
            d.path = Some(path.to_path_buf());
            d.file = Some(file);
        }
        Ok(id)
    }

    /// Abre um diagrama de um modelo já pronto (medição).
    pub fn open_model(&mut self, database: String, model: Model, all_tables: bool) -> u32 {
        let id = self.open_with(database, None, State::Loading(channel().1), all_tables);
        if let Some(d) = self.open.get_mut(&id) {
            d.loaded(model);
        }
        id
    }

    fn open_with(
        &mut self,
        database: String,
        provider: Option<Arc<dyn MetadataProvider>>,
        state: State,
        all_tables: bool,
    ) -> u32 {
        self.next_id += 1;
        let number = self.next_number;
        self.next_number += 1;
        self.open.insert(
            self.next_id,
            DiagramView {
                database,
                number,
                provider,
                state,
                base: Model::default(),
                doc: Document::default(),
                tables: Vec::new(),
                diagram: Diagram::default(),
                picker: None,
                all_tables,
                path: None,
                file: None,
                layout_dirty: false,
                notice: None,
                script: None,
                saving: None,
                confirm_close: false,
                closed: false,
                log: Vec::new(),
                node: None,
                refresh: None,
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

    /// Diagramas com alterações não salvas.
    pub fn dirty(&self) -> Vec<u32> {
        self.open
            .iter()
            .filter(|(_, d)| d.is_dirty())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Algum diagrama ainda carregando ou salvando (screenshots).
    pub fn is_loading(&self) -> bool {
        self.open.values().any(|d| d.is_loading() || d.is_saving())
    }

    /// Recebe os modelos e gravações que as threads terminaram.
    pub fn step(&mut self) {
        for d in self.open.values_mut() {
            d.step();
        }
    }

    /// Diagramas que pediram para fechar.
    pub fn take_closed(&mut self) -> Vec<u32> {
        let closed: Vec<u32> = self
            .open
            .iter()
            .filter(|(_, d)| d.closed)
            .map(|(id, _)| *id)
            .collect();
        for id in &closed {
            self.open.remove(id);
        }
        closed
    }

    /// Nós do Object Explorer a recarregar depois de salvar no banco.
    pub fn take_saved(&mut self) -> Vec<NodeId> {
        self.open
            .values_mut()
            .filter_map(|d| d.refresh.take())
            .collect()
    }

    /// Linhas para o painel Output, desde a última chamada.
    pub fn take_log(&mut self) -> Vec<String> {
        self.open
            .values_mut()
            .flat_map(|d| std::mem::take(&mut d.log))
            .collect()
    }
}

/// Carrega o modelo do banco numa thread.
fn load(
    provider: &Arc<dyn MetadataProvider>,
    database: &str,
    ctx: Option<&egui::Context>,
) -> Receiver<Result<Model, MetadataError>> {
    let (tx, rx) = channel();
    let p = Arc::clone(provider);
    let db = database.to_string();
    let repaint = ctx.cloned();
    std::thread::spawn(move || {
        let _ = tx.send(p.load_model(&db));
        if let Some(ctx) = repaint {
            ctx.request_repaint();
        }
    });
    rx
}

/// O arquivo `.pgsdiagram`: o banco, as tabelas (por nome, para valer
/// mesmo se os oids mudarem) com posição e largura, e a câmera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagramFile {
    pub version: u32,
    pub database: String,
    pub zoom: f32,
    pub pan: [f32; 2],
    pub tables: Vec<FileTable>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileTable {
    pub schema: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
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

/// Mensagem na faixa do diagrama.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Info(String),
    Error(String),
}

/// O script que o "Save" vai aplicar, esperando confirmação.
struct PendingScript {
    statements: Vec<String>,
    /// Só mostrar (Generate Change Script) ou salvar ao confirmar.
    save: bool,
}

type Saving = (Receiver<Result<(), MetadataError>>, Model, Vec<String>);

/// Um diagrama aberto.
pub struct DiagramView {
    database: String,
    number: u32,
    provider: Option<Arc<dyn MetadataProvider>>,
    state: State,
    /// O banco como foi carregado (ou salvo pela última vez).
    base: Model,
    /// O banco com as edições do diagrama.
    doc: Document,
    /// Tabelas no diagrama (pode ter ids que um undo tirou do modelo).
    tables: Vec<Id>,
    diagram: Diagram,
    picker: Option<Picker>,
    all_tables: bool,
    /// Arquivo onde o diagrama está salvo.
    path: Option<PathBuf>,
    /// Arquivo lido, aplicado quando o modelo chegar.
    file: Option<DiagramFile>,
    /// Tabelas movidas, postas ou tiradas desde o último arquivo salvo.
    layout_dirty: bool,
    notice: Option<Notice>,
    script: Option<PendingScript>,
    saving: Option<Saving>,
    confirm_close: bool,
    closed: bool,
    log: Vec<String>,
    /// Nó do Object Explorer de onde veio (para recarregar depois de
    /// salvar).
    node: Option<NodeId>,
    refresh: Option<NodeId>,
}

impl DiagramView {
    /// Texto da aba: `banco - Diagram_0`, ou o nome do arquivo; `*` com
    /// alterações.
    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("Diagram_{}", self.number));
        let dirty = if self.is_dirty() { "*" } else { "" };
        format!("{} - {name}{dirty}", self.database)
    }

    /// Nome sugerido para o arquivo.
    pub fn file_name(&self) -> String {
        format!("Diagram_{}.{EXTENSION}", self.number)
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// Mensagem de erro da carga, se falhou.
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn diagram(&self) -> &Diagram {
        &self.diagram
    }

    pub fn diagram_mut(&mut self) -> &mut Diagram {
        &mut self.diagram
    }

    /// O banco com as edições do diagrama.
    pub fn model(&self) -> &Model {
        &self.doc.model
    }

    /// DDL que leva o banco ao modelo editado.
    pub fn change_script(&self) -> Vec<String> {
        diff(&self.base, &self.doc.model)
    }

    /// Script mostrado (Generate Change Script ou a confirmação do Save).
    pub fn shown_script(&self) -> Option<&[String]> {
        self.script.as_ref().map(|s| s.statements.as_slice())
    }

    /// Há alterações no banco ou no desenho ainda não salvas.
    pub fn is_dirty(&self) -> bool {
        matches!(self.state, State::Ready) && (self.layout_dirty || self.doc.history.is_dirty())
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
        if let State::Loading(rx) = &self.state {
            match rx.try_recv() {
                Ok(Ok(model)) => self.loaded(model),
                Ok(Err(e)) => self.state = State::Failed(e.0),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.state = State::Failed(tr("The model could not be loaded.").to_string());
                }
            }
        }
        if let Some((rx, model, ddl)) = &self.saving {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.base = model.clone();
                    self.doc.history.clear();
                    self.log.push(format!(
                        "-- {}",
                        trf!(
                            "Database Diagram {number}: {count} statement(s) applied to {database}",
                            number = self.number,
                            count = ddl.len(),
                            database = self.database
                        )
                    ));
                    self.log.extend(ddl.iter().cloned());
                    self.refresh.clone_from(&self.node);
                    self.saving = None;
                    self.write_file();
                }
                Ok(Err(e)) => {
                    self.notice = Some(Notice::Error(trf!(
                        "Nothing was saved. The database refused the changes: {error}",
                        error = e.0
                    )));
                    self.saving = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.notice = Some(Notice::Error(tr("The save was interrupted.").into()));
                    self.saving = None;
                }
            }
        }
    }

    fn loaded(&mut self, model: Model) {
        self.base = model.clone();
        self.doc = Document::new(model);
        self.state = State::Ready;
        if let Some(file) = self.file.take() {
            self.apply_file(&file);
        } else if self.all_tables {
            let all: Vec<Id> = self.doc.model.tables.iter().map(|t| t.id).collect();
            self.add_tables(&all);
            self.layout_dirty = false;
        } else {
            self.open_picker();
        }
    }

    /// Põe as tabelas do arquivo onde estavam; as que não existem mais no
    /// banco ficam de fora, com um aviso.
    fn apply_file(&mut self, file: &DiagramFile) {
        let mut missing = Vec::new();
        let mut tables = Vec::new();
        for t in &file.tables {
            match self.doc.model.find(&t.schema, &t.name) {
                Some(found) => tables.push(found.id),
                None => missing.push(format!("{}.{}", t.schema, t.name)),
            }
        }
        self.tables = tables;
        self.diagram = Diagram::from_model(&self.doc.model, &self.tables);
        for (n, t) in self.diagram.nodes.iter_mut().zip(
            file.tables
                .iter()
                .filter(|t| !missing.contains(&format!("{}.{}", t.schema, t.name))),
        ) {
            n.pos = egui::pos2(t.x, t.y);
            n.width = t.width;
        }
        self.diagram.camera = Camera {
            pan: egui::vec2(file.pan[0], file.pan[1]),
            zoom: file.zoom,
        };
        self.diagram.keep_camera();
        if !missing.is_empty() {
            self.notice = Some(Notice::Info(trf!(
                "These tables no longer exist and were left out: {tables}",
                tables = missing.join(", ")
            )));
        }
    }

    /// O que vai para o arquivo.
    pub fn to_file(&self) -> DiagramFile {
        DiagramFile {
            version: 1,
            database: self.database.clone(),
            zoom: self.diagram.camera.zoom,
            pan: [self.diagram.camera.pan.x, self.diagram.camera.pan.y],
            tables: self
                .diagram
                .nodes
                .iter()
                .map(|n| FileTable {
                    schema: n.schema.clone(),
                    name: n.name.clone(),
                    x: n.pos.x,
                    y: n.pos.y,
                    width: n.width,
                })
                .collect(),
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
            .doc
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

    /// Põe tabelas no diagrama. O primeiro lote ganha o layout automático;
    /// os seguintes vão para o centro da tela, sem mexer nas outras.
    pub fn add_tables(&mut self, ids: &[Id]) {
        let new: Vec<Id> = ids
            .iter()
            .filter(|id| !self.tables.contains(id))
            .copied()
            .collect();
        if new.is_empty() {
            return;
        }
        self.tables.extend(new);
        if self.diagram.nodes.is_empty() {
            self.diagram = Diagram::from_model(&self.doc.model, &self.tables);
        } else {
            self.diagram.sync(&self.doc.model, &self.tables);
        }
        self.layout_dirty = true;
    }

    /// Adiciona as tabelas marcadas no "Add Table" e fecha ele.
    pub fn add_checked(&mut self) {
        if let Some(p) = self.picker.take() {
            let ids: Vec<Id> = p.checked.into_iter().collect();
            self.add_tables(&ids);
        }
    }

    /// Tira a tabela selecionada do diagrama (ela continua no banco).
    pub fn remove_selected(&mut self) {
        if let Some(id) = self.diagram.selected_table() {
            self.tables.retain(|t| *t != id);
            self.diagram.selected = None;
            self.sync();
            self.layout_dirty = true;
        }
    }

    fn sync(&mut self) {
        self.diagram.sync(&self.doc.model, &self.tables);
    }

    /// Schema para uma tabela nova: o da selecionada, `public` se existir,
    /// ou o da primeira tabela.
    fn new_table_schema(&self) -> String {
        let m = &self.doc.model;
        self.diagram
            .selected_table()
            .and_then(|id| m.table(id))
            .map(|t| t.schema.clone())
            .or_else(|| {
                let default = m.dialect.default_schema();
                m.tables
                    .iter()
                    .any(|t| t.schema == default)
                    .then(|| default.to_string())
            })
            .or_else(|| m.tables.first().map(|t| t.schema.clone()))
            .unwrap_or_else(|| m.dialect.default_schema().to_string())
    }

    /// "New Table": `Table_N` com uma coluna `id` e chave primária, no
    /// centro da tela, já selecionada.
    pub fn new_table(&mut self) -> Option<Id> {
        let schema = self.new_table_schema();
        let name = (1..)
            .map(|n| format!("Table_{n}"))
            .find(|n| self.doc.model.find(&schema, n).is_none())
            .expect("algum nome está livre");
        match self.doc.add_table(&schema, &name) {
            Ok(id) => {
                self.tables.push(id);
                self.sync();
                self.diagram.selected = self.diagram.index_of(id);
                self.layout_dirty = true;
                Some(id)
            }
            Err(e) => {
                self.notice = Some(Notice::Error(e.0));
                None
            }
        }
    }

    /// "Add Column" na tabela selecionada: `Column_N`, `text`, aceita
    /// nulos.
    pub fn add_column(&mut self) {
        let Some(table) = self.diagram.selected_table() else {
            return;
        };
        let Some(t) = self.doc.model.table(table) else {
            return;
        };
        let name = (1..)
            .map(|n| format!("Column_{n}"))
            .find(|n| t.column(n).is_none())
            .expect("algum nome está livre");
        let column = Column {
            name,
            ty: self.doc.model.dialect.new_column_type().into(),
            nullable: true,
            ..Default::default()
        };
        if let Err(e) = self.doc.add_column(table, column) {
            self.notice = Some(Notice::Error(e.0));
        }
        self.sync();
    }

    /// Relacionamento arrastado de `from_column` (a chave) até
    /// `to_column`: a FK fica em `to`.
    pub fn link(&mut self, from: Id, from_column: &str, to: Id, to_column: &str) {
        let Some(target) = self.doc.model.table(from) else {
            return;
        };
        let fk = ForeignKey {
            columns: vec![to_column.to_string()],
            ref_schema: target.schema.clone(),
            ref_table: target.name.clone(),
            ref_columns: vec![from_column.to_string()],
            ..Default::default()
        };
        match self.doc.add_foreign_key(to, fk) {
            Ok(()) => self.notice = None,
            Err(e) => self.notice = Some(Notice::Error(e.0)),
        }
        self.sync();
    }

    /// Uma edição qualquer no modelo (o diagrama acompanha; um erro vai
    /// para a faixa).
    pub fn edit(&mut self, f: impl FnOnce(&mut Document) -> Result<(), EditError>) {
        match f(&mut self.doc) {
            Ok(()) => self.notice = None,
            Err(e) => self.notice = Some(Notice::Error(e.0)),
        }
        self.sync();
    }

    pub fn undo(&mut self) {
        if self.doc.undo().is_some() {
            self.sync();
        }
    }

    pub fn redo(&mut self) {
        if self.doc.redo().is_some() {
            self.sync();
        }
    }

    /// "Generate Change Script": mostra o DDL sem aplicar.
    pub fn generate_change_script(&mut self) {
        self.script = Some(PendingScript {
            statements: self.change_script(),
            save: false,
        });
    }

    /// "Save" para o arquivo `path` (None: o usuário cancelou a escolha).
    /// Com alterações no banco, mostra o script e espera confirmação.
    pub fn save(&mut self, path: Option<PathBuf>) {
        let Some(path) = path else {
            return;
        };
        if !matches!(self.state, State::Ready) || self.saving.is_some() {
            return;
        }
        if let Some(e) = unsupported(&self.base, &self.doc.model).into_iter().next() {
            self.notice = Some(Notice::Error(e));
            return;
        }
        self.path = Some(path);
        let statements = self.change_script();
        if statements.is_empty() {
            self.write_file();
        } else {
            self.script = Some(PendingScript {
                statements,
                save: true,
            });
        }
    }

    /// Resposta à confirmação do script do "Save".
    pub fn confirm_save(&mut self, ctx: &egui::Context, yes: bool) {
        let Some(script) = self.script.take() else {
            return;
        };
        if !yes || !script.save {
            return;
        }
        let Some(provider) = self.provider.clone() else {
            self.notice = Some(Notice::Error(tr("Not connected.").into()));
            return;
        };
        let (tx, rx) = channel();
        let db = self.database.clone();
        let statements = script.statements.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider.apply_ddl(&db, &statements));
            repaint.request_repaint();
        });
        self.notice = Some(Notice::Info(tr("Saving...").into()));
        self.saving = Some((rx, self.doc.model.clone(), script.statements));
    }

    fn write_file(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let text = serde_json::to_string_pretty(&self.to_file()).expect("sempre serializa");
        match std::fs::write(path, text) {
            Ok(()) => {
                self.layout_dirty = false;
                self.notice = Some(Notice::Info(trf!(
                    "Saved to {path}.",
                    path = path.display()
                )));
            }
            Err(e) => {
                self.notice = Some(Notice::Error(trf!(
                    "Could not write {path}: {e}",
                    path = path.display(),
                    e = e
                )));
            }
        }
    }

    /// Fechar a aba: com alterações, pergunta antes. Devolve se pode fechar
    /// já.
    pub fn request_close(&mut self) -> bool {
        if self.is_dirty() {
            self.confirm_close = true;
            false
        } else {
            true
        }
    }

    pub fn is_confirming_close(&self) -> bool {
        self.confirm_close
    }

    /// Resposta ao "descartar alterações?": fecha ou continua.
    pub fn answer_close(&mut self, discard: bool) {
        self.confirm_close = false;
        if discard {
            self.closed = true;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, id: u32) {
        match &self.state {
            State::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Loading tables..."));
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
        if let Some(n) = &self.notice {
            match n {
                Notice::Info(t) => ui.label(t),
                Notice::Error(t) => ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("{} {t}", icon::WARNING),
                ),
            };
        }
        ui.separator();
        if self.diagram.nodes.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.weak(tr(
                    "The diagram is empty. Use Add Table to show tables of the database, \
                     or New Table to create one.",
                ));
            });
        } else {
            self.diagram.show(ui);
        }
        for event in self.diagram.take_events() {
            match event {
                DiagramEvent::Moved(_) => self.layout_dirty = true,
                DiagramEvent::Toggle { .. } => {}
                DiagramEvent::Link {
                    from,
                    from_column,
                    to,
                    to_column,
                } => self.link(from, &from_column, to, &to_column),
            }
        }
        let ctx = ui.ctx().clone();
        self.picker_window(&ctx, id);
        self.script_window(&ctx, id);
        self.close_window(&ctx, id);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(format!("{} {}", icon::PLUS, tr("Add Table...")))
                .on_hover_text(tr("Add tables of the database to the diagram"))
                .clicked()
            {
                self.open_picker();
            }
            if ui
                .button(format!("{} {}", icon::TABLE, tr("New Table")))
                .on_hover_text(tr("Create a table in the database (applied on Save)"))
                .clicked()
            {
                self.new_table();
            }
            let selected = self.diagram.selected_table().is_some();
            if ui
                .add_enabled(
                    selected,
                    egui::Button::new(format!("{} {}", icon::COLUMNS, tr("Add Column"))),
                )
                .clicked()
            {
                self.add_column();
            }
            if ui
                .add_enabled(
                    selected,
                    egui::Button::new(format!("{} {}", icon::X, tr("Remove from Diagram"))),
                )
                .on_hover_text(tr("The table stays in the database"))
                .clicked()
            {
                self.remove_selected();
            }
            ui.add_space(6.0);
            let undo = self.doc.history.undo_label().map(str::to_string);
            let redo = self.doc.history.redo_label().map(str::to_string);
            let b = ui.add_enabled(
                undo.is_some(),
                egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE),
            );
            if b.on_hover_text(trf!("Undo {label}", label = undo.unwrap_or_default()))
                .clicked()
            {
                self.undo();
            }
            let b = ui.add_enabled(redo.is_some(), egui::Button::new(icon::ARROW_CLOCKWISE));
            if b.on_hover_text(trf!("Redo {label}", label = redo.unwrap_or_default()))
                .clicked()
            {
                self.redo();
            }
            ui.add_space(6.0);
            let has = !self.diagram.nodes.is_empty();
            if ui
                .add_enabled(
                    has,
                    egui::Button::new(format!("{} {}", icon::GRID_FOUR, tr("Arrange Tables"))),
                )
                .clicked()
            {
                self.diagram.auto_layout();
                self.layout_dirty = true;
            }
            if ui
                .add_enabled(
                    has,
                    egui::Button::new(format!("{} {}", icon::CORNERS_OUT, tr("Zoom to Fit"))),
                )
                .clicked()
            {
                self.diagram.fit();
            }
            ui.label(format!("{:.0}%", self.diagram.camera.zoom * 100.0));
            ui.add_space(6.0);
            if ui
                .button(format!("{} {}", icon::SCROLL, tr("Generate Change Script")))
                .clicked()
            {
                self.generate_change_script();
            }
            ui.weak(trf!(
                "{tables} tables, {relationships} relationships",
                tables = self.diagram.nodes.len(),
                relationships = self.diagram.edges.len()
            ));
        });
    }

    fn picker_window(&mut self, ctx: &egui::Context, id: u32) {
        if self.picker.is_none() {
            return;
        }
        let candidates = self.candidates();
        let picker = self.picker.as_mut().expect("checado acima");
        match table_picker(
            ctx,
            egui::Id::new(("diagram_add_table", id)),
            picker,
            &candidates,
        ) {
            Some(true) => self.add_checked(),
            Some(false) => self.picker = None,
            None => {}
        }
    }

    fn script_window(&mut self, ctx: &egui::Context, id: u32) {
        let Some(script) = &self.script else {
            return;
        };
        let save = script.save;
        let mut text = if script.statements.is_empty() {
            format!("-- {}", tr("There are no changes to the database."))
        } else {
            script.statements.join("\n")
        };
        let mut answer = None;
        let modal = crate::theme::modal(egui::Id::new(("diagram_script", id))).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(if save {
                tr("Save Diagram")
            } else {
                tr("Save Change Script")
            });
            ui.label(if save {
                tr("Saving the diagram runs these statements in one transaction:")
            } else {
                tr("These statements run in one transaction when the diagram is saved:")
            });
            egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if save {
                    if ui.button(tr("Cancel")).clicked() {
                        answer = Some(false);
                    }
                    if ui.button(tr("Save")).clicked() {
                        answer = Some(true);
                    }
                } else if ui.button(tr("Close")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            self.confirm_save(ctx, yes);
        }
    }

    fn close_window(&mut self, ctx: &egui::Context, id: u32) {
        if !self.confirm_close {
            return;
        }
        let mut answer = None;
        let title = self.title();
        let modal = crate::theme::modal(egui::Id::new(("diagram_close", id))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(tr("Unsaved Changes"));
            ui.label(trf!(
                "{title} has changes that were not saved. Close it anyway?",
                title = title.trim_end_matches('*')
            ));
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    answer = Some(false);
                }
                if ui.button(tr("Close Without Saving")).clicked() {
                    answer = Some(true);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(discard) = answer {
            self.answer_close(discard);
        }
    }
}

/// O "Add Table": filtro, lista de `candidates` com caixinhas, Select All,
/// Add e Close. Devolve Some(true) para Add, Some(false) para fechar.
pub(crate) fn table_picker(
    ctx: &egui::Context,
    id: egui::Id,
    picker: &mut Picker,
    candidates: &[(Id, String)],
) -> Option<bool> {
    let mut answer = None;
    let modal = crate::theme::modal(id).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading(tr("Add Table"));
        ui.horizontal(|ui| {
            ui.label(tr("Filter:"));
            let filter =
                ui.add(egui::TextEdit::singleline(&mut picker.filter).desired_width(f32::INFINITY));
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
                ui.weak(tr("No tables to add."));
            }
        });
        ui.horizontal(|ui| {
            if ui.button(tr("Select All")).clicked() {
                picker.checked.extend(candidates.iter().map(|(t, _)| *t));
            }
            if ui.button(tr("Clear")).clicked() {
                picker.checked.clear();
            }
            ui.weak(trf!("{n} selected", n = picker.checked.len()));
        });
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("Close")).clicked() {
                answer = Some(false);
            }
            let n = picker.checked.len();
            if ui
                .add_enabled(n > 0, egui::Button::new(tr("Add")))
                .clicked()
            {
                answer = Some(true);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(false);
    }
    answer
}
