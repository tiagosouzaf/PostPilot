//! Table Designer: uma aba de documento que edita as colunas de uma tabela
//! (nova ou existente) e salva no banco numa transação.
//!
//! O designer carrega o modelo do banco inteiro (as FKs de outras tabelas
//! acompanham uma coluna renomeada), edita pelo [`Document`] e gera o DDL
//! com [`diff`] entre o modelo carregado e o editado. "Generate Change Script" mostra exatamente o DDL que "Save"
//! aplica.

use pgstudio_i18n::{tr, trf};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::metadata::{DesignTarget, MetadataError, MetadataProvider, Model, NodeId};
use pgstudio_core::schema::diff::{data_loss_warnings, diff};
use pgstudio_core::schema::edit::{Document, EditError};
use pgstudio_core::schema::{Column, Id, Identity};

mod dialogs;
pub use dialogs::{Dialog, DialogKind, Draft, IndexDraft, KeyKind};

/// Altura das linhas da grade de colunas.
const ROW_HEIGHT: f32 = 22.0;
/// Tipo de uma coluna nova, como o SSMS faz com `nchar(10)`.
pub const NEW_COLUMN_TYPE: &str = "text";

/// Todos os Table Designers abertos, por número.
#[derive(Default)]
pub struct Designers {
    open: BTreeMap<u32, TableDesigner>,
    next_id: u32,
    /// Último `Table_N` dado a uma tabela nova, como no SSMS.
    next_table: u32,
}

impl Designers {
    /// Abre um designer para `target` e devolve o número dele. Se a mesma
    /// tabela já está aberta, devolve o existente.
    pub fn open(
        &mut self,
        provider: Arc<dyn MetadataProvider>,
        target: DesignTarget,
        node: NodeId,
        ctx: &egui::Context,
    ) -> u32 {
        if target.table.is_some()
            && let Some((id, _)) = self.open.iter().find(|(_, d)| {
                d.target.database == target.database && d.target.table == target.table
            })
        {
            return *id;
        }
        self.next_id += 1;
        if target.table.is_none() {
            self.next_table += 1;
        }
        let mut designer = TableDesigner::start(provider, target, self.next_table, ctx);
        designer.node = Some(node);
        self.open.insert(self.next_id, designer);
        self.next_id
    }

    pub fn get(&self, id: u32) -> Option<&TableDesigner> {
        self.open.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut TableDesigner> {
        self.open.get_mut(&id)
    }

    pub fn ids(&self) -> Vec<u32> {
        self.open.keys().copied().collect()
    }

    pub fn remove(&mut self, id: u32) {
        self.open.remove(&id);
    }

    /// Designers com alterações não salvas.
    pub fn dirty(&self) -> Vec<u32> {
        self.open
            .iter()
            .filter(|(_, d)| d.is_dirty())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Algum designer ainda carregando ou salvando (screenshots).
    pub fn is_loading(&self) -> bool {
        self.open
            .values()
            .any(|d| d.is_loading() || d.is_saving() || d.types_rx.is_some())
    }

    /// Recebe o que as threads terminaram (carga, tipos, gravação).
    pub fn step(&mut self) {
        for d in self.open.values_mut() {
            d.step();
        }
    }

    /// Designers que pediram para fechar (respondido "No", ou salvo depois
    /// de "Yes").
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

    /// Nós do Object Explorer de onde vieram os designers que salvaram
    /// desde a última chamada (para recarregar).
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

/// Campo de texto da grade ou das propriedades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Schema,
    Name,
    ColumnName,
    Type,
    Default,
    Generated,
    /// Nome e tipo nas Column Properties (a grade tem os seus).
    PropertyName,
    PropertyType,
}

/// Mensagem na faixa do designer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Info(String),
    Error(String),
}

/// Gravação em andamento: a resposta, o modelo salvo e o DDL.
type Saving = (Receiver<Result<(), MetadataError>>, Model, Vec<String>);

/// Mudança numa coluna feita pelas Column Properties.
type ColumnChange = Box<dyn FnOnce(&mut Column)>;

enum State {
    Loading(Receiver<Result<Model, MetadataError>>),
    Failed(String),
    Ready,
}

/// Um Table Designer.
pub struct TableDesigner {
    provider: Arc<dyn MetadataProvider>,
    target: DesignTarget,
    state: State,
    /// O banco como foi carregado ou salvo por último.
    base: Model,
    doc: Document,
    /// A tabela no modelo (depois de carregar).
    table: Option<Id>,
    /// Número do `Table_N` de uma tabela nova ainda não salva.
    new_number: Option<u32>,
    /// Coluna selecionada (a das Column Properties).
    selected: Option<Id>,
    /// Texto sendo digitado num campo, até ele perder o foco.
    buffers: BTreeMap<(Id, Field), String>,
    /// Nome digitado na linha vazia do fim da grade.
    new_column: String,
    /// Distância medida entre duas linhas da grade, para desenhar só as
    /// visíveis.
    row_pitch: f32,
    types: Vec<String>,
    types_rx: Option<Receiver<Result<Vec<String>, MetadataError>>>,
    saving: Option<Saving>,
    notice: Option<Notice>,
    /// "Generate Change Script" aberto, com o script.
    script: Option<String>,
    /// Perguntando se salva antes de fechar.
    confirm_close: bool,
    close_after_save: bool,
    closed: bool,
    log: Vec<String>,
    /// Nó do Object Explorer de onde o designer foi aberto.
    node: Option<NodeId>,
    /// Relationships, Indexes/Keys ou Check Constraints aberta.
    dialog: Option<Dialog>,
    /// Avisos de perda de dados esperando "Yes" para salvar.
    warnings: Option<Vec<String>>,
    /// O nó a recarregar depois de salvar.
    refresh: Option<NodeId>,
}

impl TableDesigner {
    fn start(
        provider: Arc<dyn MetadataProvider>,
        target: DesignTarget,
        number: u32,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, rx) = channel();
        let (types_tx, types_rx) = channel();
        let p = Arc::clone(&provider);
        let db = target.database.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(p.load_model(&db));
            repaint.request_repaint();
            let _ = types_tx.send(p.type_names(&db));
            repaint.request_repaint();
        });
        let new_number = target.table.is_none().then_some(number);
        TableDesigner {
            provider,
            target,
            state: State::Loading(rx),
            base: Model::default(),
            doc: Document::default(),
            table: None,
            new_number,
            selected: None,
            buffers: BTreeMap::new(),
            new_column: String::new(),
            row_pitch: ROW_HEIGHT + 4.0,
            types: pgstudio_core::metadata::COMMON_TYPES
                .iter()
                .map(|t| (*t).to_string())
                .collect(),
            types_rx: Some(types_rx),
            saving: None,
            notice: None,
            script: None,
            confirm_close: false,
            close_after_save: false,
            closed: false,
            log: Vec::new(),
            node: None,
            refresh: None,
            dialog: None,
            warnings: None,
        }
    }

    fn step(&mut self) {
        if let State::Loading(rx) = &self.state {
            match rx.try_recv() {
                Ok(Ok(model)) => self.loaded(model),
                Ok(Err(e)) => self.state = State::Failed(e.to_string()),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.state = State::Failed(tr("The table could not be loaded.").into());
                }
            }
        }
        if let Some(rx) = &self.types_rx {
            match rx.try_recv() {
                Ok(Ok(types)) => {
                    self.types = types;
                    self.types_rx = None;
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => self.types_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some((rx, _, _)) = &self.saving {
            let result = match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    Err(MetadataError(tr("The changes could not be saved.").into()))
                }
            };
            let (_, saved, ddl) = self.saving.take().expect("checado acima");
            self.saved(result, saved, &ddl);
        }
    }

    fn loaded(&mut self, model: Model) {
        self.base = model.clone();
        self.doc = Document::new(model);
        self.state = State::Ready;
        match self.target.table {
            Some(id) => match self.doc.model.table(id) {
                Some(t) => {
                    self.target.schema = t.schema.clone();
                    self.table = Some(id);
                }
                None => self.state = State::Failed(tr("The table no longer exists.").into()),
            },
            None => {
                let n = self.new_number.unwrap_or(1);
                let name = (n..)
                    .map(|n| format!("Table_{n}"))
                    .find(|name| self.doc.model.find(&self.target.schema, name).is_none())
                    .unwrap_or_default();
                match self.doc.add_table(&self.target.schema, &name) {
                    Ok(id) => {
                        // Criar a tabela não é uma edição que se desfaz.
                        self.doc.history.clear();
                        self.table = Some(id);
                    }
                    Err(e) => self.state = State::Failed(e.to_string()),
                }
            }
        }
        self.selected = self
            .table
            .and_then(|t| self.doc.model.table(t))
            .and_then(|t| t.columns.first())
            .map(|c| c.id);
    }

    fn saved(&mut self, result: Result<(), MetadataError>, saved: Model, ddl: &[String]) {
        let name = self.qualified_name();
        match result {
            Ok(()) => {
                self.log
                    .push(trf!("Table Designer: saved {name}.", name = name));
                self.log.extend(ddl.iter().cloned());
                self.base = saved;
                self.doc.history.clear();
                self.new_number = None;
                self.notice = Some(Notice::Info(format!("{name} saved.")));
                self.refresh.clone_from(&self.node);
                if self.close_after_save {
                    self.closed = true;
                }
            }
            Err(e) => {
                self.log.push(trf!(
                    "Table Designer: {name} was not saved: {e}",
                    name = name,
                    e = e
                ));
                self.notice = Some(Notice::Error(trf!(
                    "Nothing was saved (the changes were rolled back): {e}",
                    e = e
                )));
                self.close_after_save = false;
            }
        }
    }

    /// Banco do designer.
    pub fn database(&self) -> &str {
        &self.target.database
    }

    /// `schema.tabela` da tabela sendo editada.
    pub fn qualified_name(&self) -> String {
        self.table
            .and_then(|t| self.doc.model.table(t))
            .map(|t| format!("{}.{}", t.schema, t.name))
            .unwrap_or_else(|| tr("Table Designer").to_string())
    }

    /// Texto da aba: `banco - schema.tabela`, com `*` se há alterações.
    pub fn title(&self) -> String {
        let star = if self.is_dirty() { "*" } else { "" };
        format!("{} - {}{star}", self.target.database, self.qualified_name())
    }

    /// Tabela nova ainda não salva, ou edições desde o último Save.
    pub fn is_dirty(&self) -> bool {
        matches!(self.state, State::Ready)
            && (self.new_number.is_some() || self.doc.history.is_dirty())
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// Mensagem mostrada na faixa (salvo, erro).
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// O modelo editado.
    pub fn model(&self) -> &Model {
        &self.doc.model
    }

    /// Id da tabela no modelo.
    pub fn table_id(&self) -> Option<Id> {
        self.table
    }

    /// O DDL que "Save" aplicaria agora.
    pub fn change_script(&self) -> Vec<String> {
        diff(&self.base, &self.doc.model)
    }

    /// Abre o "Generate Change Script".
    pub fn generate_change_script(&mut self) {
        let ddl = self.change_script();
        self.script = Some(if ddl.is_empty() {
            String::new()
        } else {
            ddl.join("\n")
        });
    }

    /// Script aberto pelo "Generate Change Script".
    pub fn shown_script(&self) -> Option<&str> {
        self.script.as_deref()
    }

    pub fn undo(&mut self) {
        match self.doc.undo() {
            Some(label) => self.notice = Some(Notice::Info(trf!("Undo {label}", label = label))),
            None => self.notice = Some(Notice::Info(tr("Nothing to undo.").into())),
        }
        self.buffers.clear();
        self.fix_selection();
    }

    pub fn redo(&mut self) {
        match self.doc.redo() {
            Some(label) => self.notice = Some(Notice::Info(trf!("Redo {label}", label = label))),
            None => self.notice = Some(Notice::Info(tr("Nothing to redo.").into())),
        }
        self.buffers.clear();
        self.fix_selection();
    }

    fn fix_selection(&mut self) {
        let t = self.table.and_then(|t| self.doc.model.table(t));
        if let Some(t) = t
            && self.selected.is_none_or(|c| t.column_by_id(c).is_none())
        {
            self.selected = t.columns.first().map(|c| c.id);
        }
    }

    /// Aplica as alterações no banco, numa transação, fora da thread da
    /// interface. Se alguma mudança pode perder dados, pergunta antes.
    pub fn save(&mut self, ctx: &egui::Context) {
        if !matches!(self.state, State::Ready) || self.saving.is_some() {
            return;
        }
        let warnings = data_loss_warnings(&self.base, &self.doc.model);
        if warnings.is_empty() {
            self.save_now(ctx);
        } else {
            self.warnings = Some(warnings);
        }
    }

    /// Avisos de perda de dados mostrados antes de salvar.
    pub fn pending_warnings(&self) -> Option<&[String]> {
        self.warnings.as_deref()
    }

    /// Resposta aos avisos: `true` salva mesmo assim.
    pub fn answer_warnings(&mut self, ctx: &egui::Context, save: bool) {
        self.warnings = None;
        if save {
            self.save_now(ctx);
        } else {
            self.close_after_save = false;
            self.notice = Some(Notice::Info(tr("Nothing was saved.").into()));
        }
    }

    fn save_now(&mut self, ctx: &egui::Context) {
        self.buffers.clear();
        let ddl = self.change_script();
        if ddl.is_empty() {
            self.new_number = None;
            self.notice = Some(Notice::Info(tr("There are no changes to save.").into()));
            if self.close_after_save {
                self.closed = true;
            }
            return;
        }
        let (tx, rx) = channel();
        let provider = Arc::clone(&self.provider);
        let db = self.target.database.clone();
        let statements = ddl.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider.apply_ddl(&db, &statements));
            repaint.request_repaint();
        });
        self.notice = Some(Notice::Info(tr("Saving...").into()));
        self.saving = Some((rx, self.doc.model.clone(), ddl));
    }

    /// Fechar a aba: pergunta antes se há alterações. Devolve se pode
    /// fechar já.
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

    /// Resposta ao "salvar alterações?" do fechamento.
    pub fn answer_close(&mut self, ctx: &egui::Context, answer: crate::Answer) {
        self.confirm_close = false;
        match answer {
            crate::Answer::Yes => {
                self.close_after_save = true;
                self.save(ctx);
            }
            crate::Answer::No => self.closed = true,
            crate::Answer::Cancel => {}
        }
    }

    /// Faz uma edição e mostra o erro dela, se houver.
    fn edit<T>(&mut self, f: impl FnOnce(&mut Document, Id) -> Result<T, EditError>) -> Option<T> {
        let table = self.table?;
        match f(&mut self.doc, table) {
            Ok(v) => {
                self.notice = None;
                Some(v)
            }
            Err(e) => {
                self.notice = Some(Notice::Error(e.0));
                None
            }
        }
    }

    /// Acrescenta uma coluna no fim da tabela.
    pub fn add_column(&mut self, name: &str) -> Option<Id> {
        let column = Column {
            name: name.trim().to_string(),
            ty: NEW_COLUMN_TYPE.to_string(),
            nullable: true,
            ..Default::default()
        };
        let id = self.edit(|d, t| d.add_column(t, column))?;
        self.selected = Some(id);
        Some(id)
    }

    pub fn rename_table(&mut self, schema: &str, name: &str) {
        let (schema, name) = (schema.trim().to_string(), name.trim().to_string());
        self.edit(|d, t| d.rename_table(t, &schema, &name));
    }

    pub fn rename_column(&mut self, column: Id, name: &str) {
        let name = name.trim().to_string();
        self.edit(|d, t| d.rename_column(t, column, &name));
    }

    pub fn update_column(&mut self, column: Id, f: impl FnOnce(&mut Column)) {
        self.edit(|d, t| d.update_column(t, column, f));
    }

    pub fn delete_column(&mut self, column: Id) {
        if self.edit(|d, t| d.drop_column(t, column)).is_some() {
            self.fix_selection();
        }
    }

    /// "Set Primary Key" na coluna: ela passa a ser a chave primária; se já
    /// era, a chave sai (como tr("Remove Primary Key") no SSMS).
    pub fn toggle_primary_key(&mut self, column: Id) {
        let Some(t) = self.table.and_then(|t| self.doc.model.table(t)) else {
            return;
        };
        let Some(name) = t.column_by_id(column).map(|c| c.name.clone()) else {
            return;
        };
        let current = t.primary_key.as_ref().map(|k| k.columns.clone());
        let columns = match current {
            Some(cols) if cols.contains(&name) => cols.into_iter().filter(|c| *c != name).collect(),
            _ => vec![name],
        };
        self.edit(|d, t| d.set_primary_key(t, columns));
    }

    pub fn select(&mut self, column: Id) {
        self.selected = Some(column);
    }

    pub fn selected(&self) -> Option<Id> {
        self.selected
    }

    /// Desenha o designer.
    pub fn show(&mut self, ui: &mut egui::Ui, id: u32) {
        match &self.state {
            State::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Loading table..."));
                });
                return;
            }
            State::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e.clone());
                return;
            }
            State::Ready => {}
        }
        self.toolbar(ui);
        if let Some(notice) = &self.notice {
            match notice {
                Notice::Info(s) => ui.label(s),
                Notice::Error(s) => ui.colored_label(ui.visuals().error_fg_color, s),
            };
        }
        ui.separator();
        let height = ui.available_height();
        egui::Panel::bottom(egui::Id::new(("column_properties", id)))
            .resizable(true)
            .default_size(height * 0.4)
            .min_size(80.0)
            .show(ui, |ui| self.column_properties(ui));
        egui::CentralPanel::default().show(ui, |ui| self.grid(ui));
        self.script_window(ui.ctx(), id);
        self.dialog_window(ui.ctx(), id);
        self.warnings_window(ui.ctx(), id);
        self.close_window(ui.ctx(), id);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(table) = self.table.and_then(|t| self.doc.model.table(t)) else {
            return;
        };
        let (schema, name) = (table.schema.clone(), table.name.clone());
        let mut rename = None;
        ui.horizontal(|ui| {
            ui.label(tr("Table:"));
            if let Some(s) = self.text(ui, (0, Field::Schema), &schema, 90.0) {
                rename = Some((s, name.clone()));
            }
            ui.label(".");
            if let Some(n) = self.text(ui, (0, Field::Name), &name, 160.0) {
                rename = Some((schema.clone(), n));
            }
        });
        ui.horizontal_wrapped(|ui| {
            let selected = self.selected;
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new(format!("{} {}", icon::KEY, tr("Set Primary Key"))),
                )
                .clicked()
                && let Some(c) = selected
            {
                self.toggle_primary_key(c);
            }
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new(format!("{} {}", icon::TRASH, tr("Delete Column"))),
                )
                .clicked()
                && let Some(c) = selected
            {
                self.delete_column(c);
            }
            ui.separator();
            let undo = self.doc.history.undo_label().map(str::to_string);
            let redo = self.doc.history.redo_label().map(str::to_string);
            if ui
                .add_enabled(
                    undo.is_some(),
                    egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE),
                )
                .on_hover_text(
                    undo.map_or(tr("Undo").to_string(), |l| trf!("Undo {label}", label = l)),
                )
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(redo.is_some(), egui::Button::new(icon::ARROW_CLOCKWISE))
                .on_hover_text(
                    redo.map_or(tr("Redo").to_string(), |l| trf!("Redo {label}", label = l)),
                )
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            for (kind, glyph, label) in [
                (
                    DialogKind::Relationships,
                    icon::FLOW_ARROW,
                    tr("Relationships"),
                ),
                (
                    DialogKind::IndexesKeys,
                    icon::LIST_NUMBERS,
                    tr("Indexes/Keys"),
                ),
                (
                    DialogKind::Checks,
                    icon::CHECK_SQUARE,
                    tr("Check Constraints"),
                ),
            ] {
                let text = format!("{glyph} {label}"); // i18n-ignore
                if ui.button(text).clicked() {
                    self.open_dialog(kind);
                }
            }
            ui.separator();
            if ui
                .button(format!("{} {}", icon::SCROLL, tr("Generate Change Script")))
                .clicked()
            {
                self.generate_change_script();
            }
            let saving = self.saving.is_some();
            if ui
                .add_enabled(
                    !saving && self.is_dirty(),
                    egui::Button::new(format!("{} {}", icon::FLOPPY_DISK, tr("Save"))),
                )
                .clicked()
            {
                self.save(ui.ctx());
            }
            if saving {
                ui.spinner();
            }
        });
        if let Some((s, n)) = rename {
            self.rename_table(&s, &n);
        }
    }

    /// Campo de texto que só grava a edição quando perde o foco (Enter, Tab
    /// ou clique fora). Devolve o texto novo, se mudou.
    fn text(
        &mut self,
        ui: &mut egui::Ui,
        key: (Id, Field),
        current: &str,
        width: f32,
    ) -> Option<String> {
        let editing = self.buffers.contains_key(&key);
        let mut text = self
            .buffers
            .get(&key)
            .cloned()
            .unwrap_or_else(|| current.to_string());
        let r = ui.add_sized(
            [width, ROW_HEIGHT - 2.0],
            egui::TextEdit::singleline(&mut text).id(egui::Id::new((
                "designer_field",
                key.0,
                key.1 as u8,
            ))),
        );
        if r.lost_focus() {
            self.buffers.remove(&key);
            return (text != current).then_some(text);
        }
        if r.has_focus() || editing {
            self.buffers.insert(key, text);
        }
        None
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        let Some(table) = self.table.and_then(|t| self.doc.model.table(t)) else {
            return;
        };
        let pk: Vec<String> = table
            .primary_key
            .as_ref()
            .map(|k| k.columns.clone())
            .unwrap_or_default();
        let columns: Vec<Column> = table.columns.clone();
        let width = ui.available_width();
        let name_w = (width * 0.3).max(120.0);
        let type_w = (width * 0.3).max(120.0);
        let mut actions: Vec<RowAction> = Vec::new();
        let mut pitch = self.row_pitch;
        let mut tops: Vec<(usize, f32)> = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show_viewport(ui, |ui, viewport| {
                // Só as linhas perto da área visível são desenhadas de
                // verdade; as outras ocupam o mesmo espaço vazias.
                let first = ((viewport.min.y / pitch).floor() as usize).saturating_sub(2);
                let last = (viewport.max.y / pitch).ceil() as usize + 2;
                let gap = ui.spacing().item_spacing.y;
                egui::Grid::new("designer_columns")
                    .num_columns(5)
                    .striped(true)
                    .min_row_height(ROW_HEIGHT)
                    .show(ui, |ui| {
                        ui.label("");
                        ui.strong(tr("Column Name"));
                        ui.strong(tr("Data Type"));
                        ui.label("");
                        ui.strong(tr("Allow Nulls"));
                        ui.end_row();
                        for (i, c) in columns.iter().enumerate() {
                            let selected = self.selected == Some(c.id);
                            if !selected && (i < first || i > last) {
                                ui.allocate_exact_size(
                                    egui::vec2(1.0, (pitch - gap).max(ROW_HEIGHT)),
                                    egui::Sense::hover(),
                                );
                                ui.end_row();
                                continue;
                            }
                            let marker = if pk.contains(&c.name) {
                                icon::KEY
                            } else if selected {
                                icon::CARET_RIGHT
                            } else {
                                " "
                            };
                            let r = ui
                                .add(
                                    egui::Button::selectable(selected, marker)
                                        .min_size(egui::vec2(22.0, ROW_HEIGHT - 4.0)),
                                )
                                .on_hover_text(c.name.clone());
                            tops.push((i, r.rect.top()));
                            if r.clicked() {
                                actions.push(RowAction::Select(c.id));
                            }
                            r.context_menu(|ui| {
                                let label = if pk.contains(&c.name) {
                                    tr("Remove Primary Key")
                                } else {
                                    tr("Set Primary Key")
                                };
                                if ui.button(label).clicked() {
                                    actions.push(RowAction::PrimaryKey(c.id));
                                    ui.close();
                                }
                                if ui.button(tr("Delete Column")).clicked() {
                                    actions.push(RowAction::Delete(c.id));
                                    ui.close();
                                }
                            });
                            if let Some(n) =
                                self.text(ui, (c.id, Field::ColumnName), &c.name, name_w)
                            {
                                actions.push(RowAction::Rename(c.id, n));
                            }
                            if let Some(ty) = self.text(ui, (c.id, Field::Type), &c.ty, type_w) {
                                actions.push(RowAction::Type(c.id, ty));
                            }
                            ui.menu_button(icon::CARET_DOWN, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(260.0)
                                    .show(ui, |ui| {
                                        for t in &self.types {
                                            if ui.button(t).clicked() {
                                                actions.push(RowAction::Type(c.id, t.clone()));
                                                ui.close();
                                            }
                                        }
                                    });
                            })
                            .response
                            .on_hover_text(tr("Data types"));
                            let mut nullable = c.nullable;
                            if ui.checkbox(&mut nullable, "").changed() {
                                actions.push(RowAction::Nulls(c.id, nullable));
                            }
                            if ui.memory(|m| {
                                [Field::ColumnName, Field::Type].iter().any(|f| {
                                    m.has_focus(egui::Id::new(("designer_field", c.id, *f as u8)))
                                })
                            }) && !selected
                            {
                                actions.push(RowAction::Select(c.id));
                            }
                            ui.end_row();
                        }
                        // Linha vazia: digitar um nome acrescenta a coluna.
                        ui.label(icon::ASTERISK);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.new_column)
                                .id(egui::Id::new("designer_new_column"))
                                .hint_text(tr("new column"))
                                .desired_width(name_w),
                        );
                        if r.lost_focus() && !self.new_column.trim().is_empty() {
                            actions.push(RowAction::Add(std::mem::take(&mut self.new_column)));
                        }
                        ui.end_row();
                    });
            });
        if let Some(w) = tops.windows(2).find(|w| w[1].0 == w[0].0 + 1) {
            pitch = w[1].1 - w[0].1;
            if pitch > 1.0 {
                self.row_pitch = pitch;
            }
        }
        for action in actions {
            match action {
                RowAction::Select(c) => self.select(c),
                RowAction::PrimaryKey(c) => self.toggle_primary_key(c),
                RowAction::Delete(c) => self.delete_column(c),
                RowAction::Rename(c, n) => self.rename_column(c, &n),
                RowAction::Type(c, ty) => {
                    let ty = ty.trim().to_string();
                    self.update_column(c, |col| col.ty = ty);
                }
                RowAction::Nulls(c, n) => self.update_column(c, |col| col.nullable = n),
                RowAction::Add(name) => {
                    self.add_column(&name);
                }
            }
        }
    }

    fn column_properties(&mut self, ui: &mut egui::Ui) {
        ui.strong(tr("Column Properties"));
        let column = self
            .table
            .and_then(|t| self.doc.model.table(t))
            .and_then(|t| self.selected.and_then(|c| t.column_by_id(c)))
            .cloned();
        let Some(c) = column else {
            ui.weak(tr("Select a column."));
            return;
        };
        let mut change: Option<ColumnChange> = None;
        let mut rename = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("column_properties_grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(tr("(Name)"));
                        if let Some(n) = self.text(ui, (c.id, Field::PropertyName), &c.name, 220.0)
                        {
                            rename = Some(n);
                        }
                        ui.end_row();
                        ui.label(tr("Allow Nulls"));
                        let mut nullable = c.nullable;
                        if ui.checkbox(&mut nullable, "").changed() {
                            change = Some(Box::new(move |col| col.nullable = nullable));
                        }
                        ui.end_row();
                        ui.label(tr("Data Type"));
                        if let Some(ty) = self.text(ui, (c.id, Field::PropertyType), &c.ty, 220.0) {
                            let ty = ty.trim().to_string();
                            change = Some(Box::new(move |col| col.ty = ty));
                        }
                        ui.end_row();
                        ui.label(tr("Default Value or Binding"));
                        let default = c.default.clone().unwrap_or_default();
                        if let Some(d) = self.text(ui, (c.id, Field::Default), &default, 220.0) {
                            let d = d.trim().to_string();
                            change = Some(Box::new(move |col| {
                                col.default = (!d.is_empty()).then_some(d);
                            }));
                        }
                        ui.end_row();
                        ui.label(tr("Identity Specification"));
                        let mut identity = c.identity;
                        egui::ComboBox::from_id_salt(("identity", c.id))
                            .selected_text(identity_label(identity))
                            .show_ui(ui, |ui| {
                                for option in
                                    [None, Some(Identity::Always), Some(Identity::ByDefault)]
                                {
                                    ui.selectable_value(
                                        &mut identity,
                                        option,
                                        identity_label(option),
                                    );
                                }
                            });
                        if identity != c.identity {
                            change = Some(Box::new(move |col| {
                                col.identity = identity;
                                if identity.is_some() {
                                    col.nullable = false;
                                    col.default = None;
                                }
                            }));
                        }
                        ui.end_row();
                        ui.label(tr("Computed Column Specification"));
                        let generated = c.generated.clone().unwrap_or_default();
                        if let Some(g) = self.text(ui, (c.id, Field::Generated), &generated, 220.0)
                        {
                            let g = g.trim().to_string();
                            change = Some(Box::new(move |col| {
                                col.generated = (!g.is_empty()).then_some(g);
                            }));
                        }
                        ui.end_row();
                    });
            });
        if let Some(n) = rename {
            self.rename_column(c.id, &n);
        }
        if let Some(f) = change {
            self.update_column(c.id, f);
        }
    }

    fn script_window(&mut self, ctx: &egui::Context, id: u32) {
        let Some(script) = &self.script else {
            return;
        };
        let mut open = true;
        let mut close = false;
        let mut text = if script.is_empty() {
            format!("-- {}", tr("There are no changes to save."))
        } else {
            script.clone()
        };
        let modal = crate::theme::modal(egui::Id::new(("change_script", id))).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(tr("Save Change Script"));
            ui.label(trf!(
                "These statements run in one transaction when {name} is saved:",
                name = self.qualified_name()
            ));
            egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .code_editor()
                        .interactive(true)
                        .desired_width(f32::INFINITY),
                );
            });
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", icon::COPY, tr("Copy")))
                    .clicked()
                {
                    ui.ctx().copy_text(text.clone());
                }
                if ui.button(tr("Close")).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            open = false;
        }
        if !open || close {
            self.script = None;
        }
    }

    fn warnings_window(&mut self, ctx: &egui::Context, id: u32) {
        let Some(warnings) = &self.warnings else {
            return;
        };
        let mut answer = None;
        let modal = crate::theme::modal(egui::Id::new(("designer_warnings", id))).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("Validation Warnings"));
            ui.label(tr("Saving these changes might lose data:"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for w in warnings {
                    ui.label(format!("{} {w}", icon::WARNING));
                }
            });
            ui.label(tr("Do you want to save anyway?"));
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("No")).clicked() {
                    answer = Some(false);
                }
                if ui.button(tr("Yes")).clicked() {
                    answer = Some(true);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        if let Some(save) = answer {
            self.answer_warnings(ctx, save);
        }
    }

    fn close_window(&mut self, ctx: &egui::Context, id: u32) {
        if !self.confirm_close {
            return;
        }
        let name = self.qualified_name();
        let mut answer = None;
        let modal = crate::theme::modal(egui::Id::new(("designer_close", id))).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(pgstudio_core::APP_NAME);
            ui.label(tr("Save changes to the following items?"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(&name);
            });
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Cancel")).clicked() {
                    answer = Some(crate::Answer::Cancel);
                }
                if ui.button(tr("No")).clicked() {
                    answer = Some(crate::Answer::No);
                }
                if ui.button(tr("Yes")).clicked() {
                    answer = Some(crate::Answer::Yes);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(crate::Answer::Cancel);
        }
        if let Some(a) = answer {
            self.answer_close(ctx, a);
        }
    }
}

enum RowAction {
    Select(Id),
    PrimaryKey(Id),
    Delete(Id),
    Rename(Id, String),
    Type(Id, String),
    Nulls(Id, bool),
    Add(String),
}

fn identity_label(identity: Option<Identity>) -> &'static str {
    match identity {
        None => tr("(none)"),
        Some(Identity::Always) => "GENERATED ALWAYS",
        Some(Identity::ByDefault) => "GENERATED BY DEFAULT",
    }
}
