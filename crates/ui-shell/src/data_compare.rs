//! "Data Compare" (Tools > Data Compare...): compara as linhas das tabelas
//! de dois bancos pela chave primária, mostra o que só existe na origem,
//! o que só existe no destino e o que difere, e gera o script
//! (`DELETE`/`UPDATE`/`INSERT`) que deixa o destino como a origem.
//!
//! Cada lado é um banco de um servidor conectado no Object Explorer; os
//! dois lados têm de ser do mesmo tipo de servidor. A janela nunca altera
//! nada: o script abre numa janela de consulta no servidor de destino,
//! onde vale a confirmação do ambiente (Produção), ou vai para um arquivo
//! quando é grande demais para o editor.
//!
//! A comparação roda fora da thread da interface e lê as duas tabelas ao
//! mesmo tempo, em ordem de chave, sem guardá-las na memória (veja
//! [`engine`]); as diferenças vão para um arquivo temporário e só as
//! primeiras [`engine::SAMPLE`] de cada tabela ficam na grade.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui::{self, Align, Layout};
use pgstudio_core::connection::{Engine, Feature};
use pgstudio_core::query::QueryService;
use pgstudio_i18n::{tr, trf};
use serde::{Deserialize, Serialize};

use crate::ShellApp;
use crate::file_dialogs::Question;
use crate::servers::server_key;

pub mod engine;

use engine::{Counts, Diff, Plan, Progress, Table, TableResult, Wanted};

/// Acima disto o script vai para um arquivo: o editor não foi feito para
/// centenas de milhares de linhas.
pub const WINDOW_STATEMENTS: u64 = 20_000;

/// Um servidor conectado, onde um lado pode estar.
#[derive(Clone)]
pub struct Server {
    /// Chave do servidor (a mesma das janelas de consulta).
    pub key: String,
    /// Como aparece na lista.
    pub label: String,
    pub engine: Engine,
    pub queries: Arc<dyn QueryService>,
    /// Banco da conexão, de onde sai a lista de bancos.
    pub home: String,
}

/// Um lado da comparação.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Side {
    /// Chave do servidor.
    pub server: String,
    pub database: String,
}

/// O que a sessão guarda da janela, para ela reabrir como estava.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Setup {
    pub source: Side,
    pub target: Side,
    /// Tabelas desmarcadas.
    pub skipped: BTreeSet<String>,
    pub no_insert: bool,
    pub no_update: bool,
    pub no_delete: bool,
}

/// Qual lado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Source,
    Target,
}

/// Um par de tabelas (a da origem e a de mesmo nome no destino).
pub struct Pair {
    /// `schema.tabela`.
    pub label: String,
    /// Como comparar, ou por que não dá.
    pub plan: Result<Plan, String>,
}

/// As tabelas dos dois lados, lidas.
struct Listing {
    source: Side,
    target: Side,
    pairs: Vec<Pair>,
}

/// O que as threads leram dos catálogos.
type Listed = Result<(Engine, Vec<Table>, Vec<Table>), String>;

enum Phase {
    Idle,
    Listing {
        rx: Receiver<Listed>,
        source: Side,
        target: Side,
        then_compare: bool,
    },
    Comparing {
        rx: Receiver<TableResult>,
        progress: Arc<Progress>,
        labels: Vec<String>,
    },
    Saving(Receiver<Result<(PathBuf, u64), String>>),
}

/// O que a janela pediu à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Abrir o script numa janela de consulta neste servidor e banco.
    Script {
        sql: String,
        server: String,
        database: String,
    },
    Log(Vec<String>),
    Closed,
}

/// A lista de bancos de um servidor.
enum Databases {
    Loading(Receiver<Vec<String>>),
    Ready(Vec<String>),
}

/// A janela "Data Compare".
pub struct DataCompare {
    servers: Vec<Server>,
    pub source: Side,
    pub target: Side,
    databases: HashMap<String, Databases>,
    listing: Option<Listing>,
    /// Tabelas desmarcadas (ficam fora da comparação e do script).
    pub skipped: BTreeSet<String>,
    pub wanted: Wanted,
    phase: Phase,
    /// Os lados da última comparação e o resultado de cada tabela.
    compared: Option<(Side, Side)>,
    results: Vec<TableResult>,
    /// A tabela cujas diferenças aparecem ao lado.
    pub selected: Option<String>,
    filter: String,
    error: Option<String>,
    status: Option<String>,
    asking: Option<Question>,
}

impl DataCompare {
    /// Abre a janela com `servers` conectados; o primeiro é o ativo.
    pub fn open(servers: Vec<Server>) -> Self {
        let side = |s: Option<&Server>| Side {
            server: s.map(|s| s.key.clone()).unwrap_or_default(),
            database: s.map(|s| s.home.clone()).unwrap_or_default(),
        };
        let source = side(servers.first());
        // O destino mais provável é o mesmo banco noutro servidor.
        let mut target = side(servers.get(1).or(servers.first()));
        if servers.len() < 2 {
            target.database.clear();
        } else {
            target.database = source.database.clone();
        }
        Self::with(servers, source, target)
    }

    /// Reabre a janela como a sessão guardou.
    pub fn restore(servers: Vec<Server>, setup: Setup) -> Self {
        let mut d = Self::with(servers, setup.source, setup.target);
        d.skipped = setup.skipped;
        d.wanted = Wanted {
            insert: !setup.no_insert,
            update: !setup.no_update,
            delete: !setup.no_delete,
        };
        d
    }

    fn with(servers: Vec<Server>, source: Side, target: Side) -> Self {
        Self {
            servers,
            source,
            target,
            databases: HashMap::new(),
            listing: None,
            skipped: BTreeSet::new(),
            wanted: Wanted::default(),
            phase: Phase::Idle,
            compared: None,
            results: Vec::new(),
            selected: None,
            filter: String::new(),
            error: None,
            status: None,
            asking: None,
        }
    }

    /// O que a sessão guarda.
    pub fn setup(&self) -> Setup {
        Setup {
            source: self.source.clone(),
            target: self.target.clone(),
            skipped: self.skipped.clone(),
            no_insert: !self.wanted.insert,
            no_update: !self.wanted.update,
            no_delete: !self.wanted.delete,
        }
    }

    /// Os servidores conectados agora (a casca atualiza a cada frame).
    pub fn set_servers(&mut self, servers: Vec<Server>) {
        self.servers = servers;
    }

    pub fn side_mut(&mut self, which: Which) -> &mut Side {
        match which {
            Which::Source => &mut self.source,
            Which::Target => &mut self.target,
        }
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Os pares de tabelas listados para os lados atuais.
    pub fn pairs(&self) -> &[Pair] {
        match &self.listing {
            Some(l) if l.source == self.source && l.target == self.target => &l.pairs,
            _ => &[],
        }
    }

    /// O resultado de cada tabela comparada.
    pub fn results(&self) -> &[TableResult] {
        &self.results
    }

    fn server(&self, key: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.key == key)
    }

    fn side_label(&self, side: &Side) -> String {
        let server = self
            .server(&side.server)
            .map_or(side.server.as_str(), |s| s.label.as_str());
        format!("{server}/{}", side.database)
    }

    /// Confere os lados e devolve os servidores deles.
    fn ends(&self) -> Result<(Server, Server), String> {
        let find = |side: &Side, missing: &str, database: &str| {
            let server = self.server(&side.server).ok_or(missing.to_string())?;
            if side.database.trim().is_empty() {
                return Err(database.to_string());
            }
            Ok(server.clone())
        };
        let s = find(
            &self.source,
            tr("Choose a connected server as the source."),
            tr("Choose the source database."),
        )?;
        let t = find(
            &self.target,
            tr("Choose a connected server as the target."),
            tr("Choose the target database."),
        )?;
        if s.engine != t.engine {
            return Err(
                tr("A PostgreSQL database cannot be compared with a SQL Server database.").into(),
            );
        }
        if self.source == self.target {
            return Err(tr("The source and the target are the same database.").into());
        }
        Ok((s, t))
    }

    /// Lê as tabelas dos dois lados.
    pub fn list(&mut self, ctx: &egui::Context) {
        self.start_listing(ctx, false);
    }

    fn start_listing(&mut self, ctx: &egui::Context, then_compare: bool) {
        let (s, t) = match self.ends() {
            Ok(e) => e,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.error = None;
        self.status = None;
        let (tx, rx) = mpsc::channel();
        let source = s.queries.open(&self.source.database);
        let target = t.queries.open(&self.target.database);
        let engine = s.engine;
        let (source_label, target_label) =
            (self.side_label(&self.source), self.side_label(&self.target));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = engine::read_tables(&source)
                .map_err(|e| format!("{source_label}: {e}"))
                .and_then(|a| {
                    let b =
                        engine::read_tables(&target).map_err(|e| format!("{target_label}: {e}"))?;
                    Ok((engine, a, b))
                });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.phase = Phase::Listing {
            rx,
            source: self.source.clone(),
            target: self.target.clone(),
            then_compare,
        };
    }

    /// Compara as tabelas marcadas (lendo a lista antes, se preciso).
    pub fn compare(&mut self, ctx: &egui::Context) {
        if self.pairs().is_empty() {
            self.start_listing(ctx, true);
            return;
        }
        let (s, t) = match self.ends() {
            Ok(e) => e,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let plans: Vec<Plan> = self
            .pairs()
            .iter()
            .filter(|p| !self.skipped.contains(&p.label))
            .filter_map(|p| p.plan.clone().ok())
            .collect();
        if plans.is_empty() {
            self.error = Some(tr("Select at least one table with a primary key.").into());
            return;
        }
        self.error = None;
        self.status = None;
        self.results.clear();
        self.compared = Some((self.source.clone(), self.target.clone()));
        let labels = plans.iter().map(Plan::label).collect();
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let source = s.queries.open(&self.source.database);
        let target = t.queries.open(&self.target.database);
        let (ctx, shared) = (ctx.clone(), progress.clone());
        std::thread::spawn(move || {
            for plan in plans {
                if shared.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let result = engine::compare_table(plan, &source, &target, &shared);
                shared.tables.fetch_add(1, Ordering::Relaxed);
                if tx.send(result).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            ctx.request_repaint();
        });
        self.phase = Phase::Comparing {
            rx,
            progress,
            labels,
        };
    }

    /// Para a comparação em andamento.
    pub fn cancel(&mut self) {
        if let Phase::Comparing { progress, .. } = &self.phase {
            progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// As tabelas que entram no script: comparadas, com diferenças e ainda
    /// marcadas.
    fn scripted(&self) -> Vec<&TableResult> {
        self.results
            .iter()
            .filter(|r| r.spill.is_some() && !self.skipped.contains(&r.plan.label()))
            .collect()
    }

    /// Quantos comandos o script terá.
    pub fn statements(&self) -> u64 {
        self.scripted()
            .iter()
            .map(|r| {
                let c = &r.counts;
                [
                    (self.wanted.insert, c.only_source),
                    (self.wanted.update, c.different),
                    (self.wanted.delete, c.only_target),
                ]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, n)| n)
                .sum::<u64>()
            })
            .sum()
    }

    fn header(&self) -> Vec<String> {
        let Some((source, target)) = &self.compared else {
            return Vec::new();
        };
        vec![
            trf!(
                "Data Compare: changes that make {target} like {source}.",
                target = self.side_label(target),
                source = self.side_label(source)
            ),
            tr("Review the script before running it. It runs in one transaction: if a command fails, nothing is changed (in PostgreSQL, run ROLLBACK afterwards).").into(),
            tr("Tables are in alphabetical order. With foreign keys between them, reorder the commands if the server refuses one.").into(),
        ]
    }

    /// Escreve o script em `out`.
    fn write_script(&self, out: &mut dyn std::io::Write) -> std::io::Result<u64> {
        let tables = self.script_tables();
        let engine = tables.first().map(|(p, _)| p.engine).unwrap_or_default();
        engine::write_script(out, engine, &self.header(), &tables, self.wanted)
    }

    /// O plano e o arquivo de diferenças de cada tabela do script.
    fn script_tables(&self) -> Vec<(Plan, PathBuf)> {
        self.scripted()
            .into_iter()
            .filter_map(|r| Some((r.plan.clone(), r.spill.as_ref()?.path().to_path_buf())))
            .collect()
    }

    /// O script inteiro, como texto.
    pub fn script_text(&self) -> Result<String, String> {
        let mut out = Vec::new();
        self.write_script(&mut out).map_err(|e| e.to_string())?;
        String::from_utf8(out).map_err(|e| e.to_string())
    }

    /// "Generate Script": abre numa janela de consulta no destino; um
    /// script grande demais para o editor vai para um arquivo.
    pub fn generate(&mut self) -> Option<Request> {
        let (_, target) = self.compared.clone()?;
        let statements = self.statements();
        if statements == 0 {
            self.error = Some(tr("There is nothing to put in the script.").into());
            return None;
        }
        self.error = None;
        if statements > WINDOW_STATEMENTS {
            self.asking = Some(Question::save("DataCompare.sql"));
            return None;
        }
        match self.script_text() {
            Ok(sql) => Some(Request::Script {
                sql,
                server: target.server,
                database: target.database,
            }),
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    /// O diálogo de arquivo que a janela pediu, para a casca perguntar.
    pub fn take_question(&mut self) -> Option<Question> {
        self.asking.take()
    }

    /// Salva o script em `path`, fora da thread da interface.
    pub fn save_script(&mut self, path: PathBuf, ctx: &egui::Context) {
        // As diferenças já estão em arquivos: a thread só precisa dos
        // planos e dos caminhos, que copiamos para ela.
        let tables = self.script_tables();
        let Some(engine) = tables.first().map(|(p, _)| p.engine) else {
            return;
        };
        let (header, wanted) = (self.header(), self.wanted);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let write = || -> std::io::Result<u64> {
                let mut out = std::io::BufWriter::new(std::fs::File::create(&path)?);
                engine::write_script(&mut out, engine, &header, &tables, wanted)
            };
            let r = write()
                .map(|n| (path.clone(), n))
                .map_err(|e| format!("{}: {e}", path.display()));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.phase = Phase::Saving(rx);
    }

    fn request_databases(&mut self, ctx: &egui::Context) {
        for key in [self.source.server.clone(), self.target.server.clone()] {
            if self.databases.contains_key(&key) {
                continue;
            }
            let Some(server) = self.server(&key) else {
                continue;
            };
            let session = server.queries.open(&server.home);
            let sql = server.engine.list_databases_sql();
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let names = engine::collect(&session, sql)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|r| r.into_iter().next().flatten())
                    .collect();
                let _ = tx.send(names);
                ctx.request_repaint();
            });
            self.databases.insert(key, Databases::Loading(rx));
        }
    }

    /// Recebe o que as threads mandaram.
    pub fn step(&mut self, ctx: &egui::Context) -> Option<Request> {
        self.request_databases(ctx);
        for list in self.databases.values_mut() {
            if let Databases::Loading(rx) = list
                && let Ok(names) = rx.try_recv()
            {
                *list = Databases::Ready(names);
            }
        }
        match &self.phase {
            Phase::Idle => None,
            Phase::Listing {
                rx,
                source,
                target,
                then_compare,
            } => {
                let r = rx.try_recv().ok()?;
                let (source, target, then_compare) =
                    (source.clone(), target.clone(), *then_compare);
                self.phase = Phase::Idle;
                match r {
                    Ok((engine, a, b)) => {
                        let pairs = pairs(engine, &a, &b);
                        let empty = pairs.is_empty();
                        self.listing = Some(Listing {
                            source,
                            target,
                            pairs,
                        });
                        if empty {
                            self.error = Some(tr("The source has no tables.").into());
                        } else if then_compare && !self.pairs().is_empty() {
                            self.compare(ctx);
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
                None
            }
            Phase::Comparing { rx, progress, .. } => {
                let mut finished = false;
                loop {
                    match rx.try_recv() {
                        Ok(r) => self.results.push(r),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            finished = true;
                            break;
                        }
                    }
                }
                if !finished {
                    ctx.request_repaint_after(Duration::from_millis(200));
                    return None;
                }
                let canceled = progress.cancel.load(Ordering::Relaxed);
                self.phase = Phase::Idle;
                if self.selected.is_none() {
                    self.selected = self
                        .results
                        .iter()
                        .find(|r| r.counts.differences() > 0)
                        .or(self.results.first())
                        .map(|r| r.plan.label());
                }
                let (source, target) = self.compared.clone()?;
                let differences: u64 = self.results.iter().map(|r| r.counts.differences()).sum();
                let line = trf!(
                    "Data Compare: {source} to {target}: {differences} different rows in {tables} tables.",
                    source = self.side_label(&source),
                    target = self.side_label(&target),
                    differences = differences,
                    tables = self.results.len()
                );
                if canceled {
                    self.status = Some(tr("The comparison was canceled.").into());
                }
                Some(Request::Log(vec![line]))
            }
            Phase::Saving(rx) => {
                let r = rx.try_recv().ok()?;
                self.phase = Phase::Idle;
                match r {
                    Ok((path, statements)) => {
                        self.status = Some(trf!(
                            "The script ({statements} commands) was saved to {path}.",
                            statements = statements,
                            path = path.display()
                        ));
                        Some(Request::Log(vec![trf!(
                            "Data Compare: the script was saved to {path}.",
                            path = path.display()
                        )]))
                    }
                    Err(e) => {
                        self.error = Some(e);
                        None
                    }
                }
            }
        }
    }

    /// Desenha a janela.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step(ctx);
        let mut open = true;
        // Numa janela pequena do app (ou com zoom grande) a ferramenta
        // encolhe para caber inteira.
        let screen = ctx.content_rect().size() - egui::vec2(32.0, 80.0);
        egui::Window::new(tr("Data Compare"))
            .id(egui::Id::new("data_compare"))
            .open(&mut open)
            .collapsible(false)
            .resizable([true, false])
            .frame(crate::theme::dialog_frame(&ctx.global_style()))
            .default_width(screen.x.min(1040.0))
            .max_width(screen.x.max(240.0))
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .min_size([screen.x.clamp(240.0, 640.0), screen.y.clamp(160.0, 360.0)])
            .show(ctx, |ui| {
                if let Some(r) = self.ui(ui, ctx) {
                    request = Some(r);
                }
            });
        if !open && request.is_none() {
            self.cancel();
            request = Some(Request::Closed);
        }
        request
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<Request> {
        let mut request = None;
        let idle = !self.is_busy();
        ui.add_enabled_ui(idle, |ui| {
            egui::Grid::new("data_compare_sides")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    for which in [Which::Source, Which::Target] {
                        ui.label(tr(match which {
                            Which::Source => "Source:",
                            Which::Target => "Target:",
                        }));
                        ui.horizontal(|ui| self.side_ui(ui, which));
                        ui.end_row();
                    }
                });
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(idle, egui::Button::new(tr("List Tables")))
                .clicked()
            {
                self.list(ctx);
            }
            if ui
                .add_enabled(
                    idle,
                    egui::Button::new(format!(
                        "{} {}",
                        egui_phosphor::regular::GIT_DIFF,
                        tr("Compare")
                    )),
                )
                .clicked()
            {
                self.compare(ctx);
            }
            self.progress_ui(ui);
        });
        ui.separator();
        // A altura do miolo é fixa (pelo tamanho da janela do app): uma
        // área de rolagem que ocupa "o que sobrar" faria a janela crescer.
        let h = (ctx.content_rect().height() - 330.0).clamp(100.0, 460.0);
        let body = egui::vec2(ui.available_width(), h);
        ui.allocate_ui_with_layout(body, Layout::left_to_right(Align::Min), |ui| {
            ui.set_height(h);
            let list = egui::vec2(crate::fit::side_width(ui, 360.0, 0.4), h);
            ui.allocate_ui_with_layout(list, Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(list);
                ui.set_max_width(list.x);
                ui.set_max_height(h);
                self.tables_ui(ui, h);
            });
            ui.separator();
            let rest = egui::vec2(ui.available_width(), h);
            ui.allocate_ui_with_layout(rest, Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(rest);
                ui.set_max_height(h);
                self.details_ui(ui);
            });
        });
        ui.separator();
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        } else if let Some(s) = &self.status {
            ui.label(format!("{} {s}", egui_phosphor::regular::CHECK_CIRCLE));
        } else {
            ui.weak(tr(
                "Nothing is changed here: the script opens in a query window for you to review and run.",
            ));
        }
        ui.horizontal(|ui| {
            ui.label(tr("In the script:"));
            ui.checkbox(&mut self.wanted.insert, tr("Inserts"));
            ui.checkbox(&mut self.wanted.update, tr("Updates"));
            ui.checkbox(&mut self.wanted.delete, tr("Deletes"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let statements = self.statements();
                let ready = idle && statements > 0;
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!(
                            "{} {}",
                            egui_phosphor::regular::SCROLL,
                            tr("Generate Script")
                        )),
                    )
                    .clicked()
                {
                    request = self.generate();
                }
                if statements > 0 {
                    ui.weak(trf!("{count} commands", count = statements));
                }
            });
        });
        request
    }

    fn progress_ui(&mut self, ui: &mut egui::Ui) {
        match &self.phase {
            Phase::Idle => {}
            Phase::Listing { .. } => {
                ui.spinner();
                ui.label(tr("Reading the tables..."));
            }
            Phase::Saving(_) => {
                ui.spinner();
                ui.label(tr("Saving the script..."));
            }
            Phase::Comparing {
                progress, labels, ..
            } => {
                let done = progress.tables.load(Ordering::Relaxed) as usize;
                let table = labels.get(done).or(labels.last()).cloned();
                ui.spinner();
                ui.label(trf!(
                    "Comparing {table} ({n} of {total}): {source} rows read from the source, {target} from the target",
                    table = table.unwrap_or_default(),
                    n = (done + 1).min(labels.len()),
                    total = labels.len(),
                    source = progress.source_rows.load(Ordering::Relaxed),
                    target = progress.target_rows.load(Ordering::Relaxed)
                ));
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                }
            }
        }
    }

    /// Os campos de um lado: servidor e banco.
    fn side_ui(&mut self, ui: &mut egui::Ui, which: Which) {
        let id = match which {
            Which::Source => "source",
            Which::Target => "target",
        };
        let servers: Vec<(String, String)> = self
            .servers
            .iter()
            .map(|s| (s.key.clone(), s.label.clone()))
            .collect();
        let key = self.side_mut(which).server.clone();
        let databases: Vec<String> = match self.databases.get(&key) {
            Some(Databases::Ready(names)) => names.clone(),
            _ => Vec::new(),
        };
        let side = self.side_mut(which);
        let shown = servers.iter().find(|(k, _)| *k == side.server).map_or_else(
            || {
                if side.server.is_empty() {
                    String::new()
                } else {
                    trf!("{server} (not connected)", server = side.server)
                }
            },
            |(_, label)| label.clone(),
        );
        // As listas encolhem com a janela do app para a linha caber.
        let screen = ui.ctx().content_rect().width();
        egui::ComboBox::from_id_salt(format!("data_compare_{id}_server"))
            .width((screen * 0.38).clamp(110.0, 300.0))
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (key, label) in &servers {
                    if ui
                        .selectable_label(side.server == *key, label.as_str())
                        .clicked()
                        && side.server != *key
                    {
                        side.server = key.clone();
                        side.database.clear();
                    }
                }
            });
        egui::ComboBox::from_id_salt(format!("data_compare_{id}_database"))
            .width((screen * 0.26).clamp(90.0, 220.0))
            .selected_text(side.database.as_str())
            .show_ui(ui, |ui| {
                for d in &databases {
                    ui.selectable_value(&mut side.database, d.clone(), d.as_str());
                }
            });
        if servers.len() < 2 && which == Which::Target {
            let hint = tr("To compare with another server, connect to it in the Object Explorer.");
            if screen >= 1100.0 {
                ui.weak(hint);
            } else {
                // Sem largura para a frase, ela vai para a dica de um ícone.
                ui.weak(egui_phosphor::regular::INFO).on_hover_text(hint);
            }
        }
    }

    /// A lista de tabelas, com o que deu em cada uma.
    fn tables_ui(&mut self, ui: &mut egui::Ui, height: f32) {
        if self.pairs().is_empty() {
            ui.label(tr("Choose the source and the target and press Compare."));
            return;
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(tr("Search tables"))
                    .desired_width(180.0),
            );
            let labels: Vec<String> = self.pairs().iter().map(|p| p.label.clone()).collect();
            if ui.small_button(tr("All")).clicked() {
                self.skipped.clear();
            }
            if ui.small_button(tr("None")).clicked() {
                self.skipped = labels.into_iter().collect();
            }
        });
        let needle = self.filter.trim().to_lowercase();
        let green = egui::Color32::from_rgb(40, 150, 70);
        let (red, orange) = (ui.visuals().error_fg_color, ui.visuals().warn_fg_color);
        let mut toggled = None;
        let mut selected = None;
        egui::ScrollArea::vertical()
            .id_salt("data_compare_tables")
            .max_height(height - 30.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("data_compare_tables_grid")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for pair in self.pairs() {
                            if !needle.is_empty() && !pair.label.to_lowercase().contains(&needle) {
                                continue;
                            }
                            let comparable = pair.plan.is_ok();
                            let mut on = comparable && !self.skipped.contains(&pair.label);
                            if ui
                                .add_enabled(comparable, egui::Checkbox::without_text(&mut on))
                                .changed()
                            {
                                toggled = Some((pair.label.clone(), on));
                            }
                            let is_selected = self.selected.as_deref() == Some(&pair.label);
                            if ui
                                .selectable_label(is_selected, pair.label.as_str())
                                .clicked()
                            {
                                selected = Some(pair.label.clone());
                            }
                            let result = self.results.iter().find(|r| r.plan.label() == pair.label);
                            match (&pair.plan, result) {
                                (Err(reason), _) => {
                                    ui.weak(reason.as_str());
                                }
                                (_, Some(r)) if r.error.is_some() => {
                                    ui.colored_label(red, tr("failed"));
                                }
                                (_, Some(r)) if r.counts.differences() == 0 => {
                                    ui.weak(trf!("equal ({rows} rows)", rows = r.counts.equal));
                                }
                                (_, Some(r)) => {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        let c = &r.counts;
                                        ui.colored_label(green, format!("+{}", c.only_source))
                                            .on_hover_text(tr("Only in source"));
                                        ui.colored_label(orange, format!("≠{}", c.different))
                                            .on_hover_text(tr("Different"));
                                        ui.colored_label(red, format!("−{}", c.only_target))
                                            .on_hover_text(tr("Only in target"));
                                    });
                                }
                                (_, None) => {
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some((label, on)) = toggled {
            if on {
                self.skipped.remove(&label);
            } else {
                self.skipped.insert(label);
            }
        }
        if selected.is_some() {
            self.selected = selected;
        }
    }

    /// As diferenças da tabela selecionada.
    fn details_ui(&mut self, ui: &mut egui::Ui) {
        let result = self
            .selected
            .as_ref()
            .and_then(|l| self.results.iter().find(|r| r.plan.label() == *l));
        let Some(r) = result else {
            let pair = self
                .selected
                .as_ref()
                .and_then(|l| self.pairs().iter().find(|p| p.label == *l));
            match pair.map(|p| &p.plan) {
                Some(Err(reason)) => {
                    ui.label(trf!(
                        "This table cannot be compared: {reason}.",
                        reason = reason
                    ));
                }
                Some(Ok(plan)) => {
                    ui.label(trf!(
                        "{table}: {columns} columns will be compared by the primary key.",
                        table = plan.label(),
                        columns = plan.columns.len()
                    ));
                    ignored_ui(ui, plan);
                }
                None => {
                    ui.weak(tr("Select a table to see its differences."));
                }
            }
            return;
        };
        if let Some(e) = &r.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        let c: &Counts = &r.counts;
        ui.strong(trf!(
            "{table}: {only_source} only in the source, {different} different, {only_target} only in the target, {equal} equal",
            table = r.plan.label(),
            only_source = c.only_source,
            different = c.different,
            only_target = c.only_target,
            equal = c.equal
        ));
        ignored_ui(ui, &r.plan);
        if c.differences() > r.sample.len() as u64 {
            ui.weak(trf!(
                "Showing the first {shown} differences; the script has all of them.",
                shown = r.sample.len()
            ));
        }
        if r.sample.is_empty() {
            return;
        }
        let green = egui::Color32::from_rgb(40, 150, 70);
        let (red, orange) = (ui.visuals().error_fg_color, ui.visuals().warn_fg_color);
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        egui::ScrollArea::both()
            .id_salt("data_compare_rows")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show_rows(ui, row_height, r.sample.len() + 1, |ui, range| {
                egui::Grid::new("data_compare_rows_grid")
                    .num_columns(r.plan.columns.len() + 1)
                    .striped(true)
                    .min_row_height(row_height)
                    .show(ui, |ui| {
                        // O cabeçalho rola junto: é a linha 0.
                        for i in range {
                            if i == 0 {
                                ui.strong(tr("Difference"));
                                for (n, column) in r.plan.columns.iter().enumerate() {
                                    if r.plan.key.contains(&n) {
                                        ui.strong(format!(
                                            "{} {}",
                                            egui_phosphor::regular::KEY,
                                            column.target
                                        ));
                                    } else {
                                        ui.strong(column.target.as_str());
                                    }
                                }
                                ui.end_row();
                                continue;
                            }
                            match &r.sample[i - 1] {
                                Diff::OnlySource(row) => {
                                    ui.colored_label(green, tr("Only in source"));
                                    for v in row {
                                        cell(ui, v.as_deref(), None);
                                    }
                                }
                                Diff::OnlyTarget(row) => {
                                    ui.colored_label(red, tr("Only in target"));
                                    for v in row {
                                        cell(ui, v.as_deref(), None);
                                    }
                                }
                                Diff::Different(source, target) => {
                                    ui.colored_label(orange, tr("Different"));
                                    for (s, t) in source.iter().zip(target) {
                                        let changed = (s != t).then_some((t.as_deref(), orange));
                                        cell(ui, s.as_deref(), changed);
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }
}

fn ignored_ui(ui: &mut egui::Ui, plan: &Plan) {
    if !plan.ignored.is_empty() {
        ui.weak(trf!(
            "Not compared (computed or only on one side): {columns}",
            columns = plan.ignored.join(", ")
        ));
    }
}

/// Uma célula da grade. Numa linha diferente, `changed` traz o valor do
/// destino e a cor: a célula mostra "destino → origem".
fn cell(ui: &mut egui::Ui, value: Option<&str>, changed: Option<(Option<&str>, egui::Color32)>) {
    const WIDTH: usize = 48;
    let short = |v: Option<&str>| match v {
        None => "NULL".to_string(),
        Some(v) => {
            let line = v.lines().next().unwrap_or_default();
            if line.chars().count() > WIDTH || line.len() < v.len() {
                format!("{}…", line.chars().take(WIDTH).collect::<String>())
            } else {
                line.to_string()
            }
        }
    };
    match changed {
        Some((target, color)) => {
            ui.colored_label(
                color,
                format!(
                    "{} {} {}",
                    short(target),
                    egui_phosphor::regular::ARROW_RIGHT,
                    short(value)
                ),
            )
            .on_hover_text(trf!(
                "Target: {target}\nSource: {source}",
                target = target.unwrap_or("NULL"),
                source = value.unwrap_or("NULL")
            ));
        }
        None if value.is_none() => {
            ui.weak("NULL"); // i18n-ignore
        }
        None => {
            ui.label(short(value));
        }
    }
}

/// Os pares de tabelas: cada tabela da origem com a de mesmo nome no
/// destino. As que não dá para comparar ficam na lista, com o motivo.
fn pairs(engine: Engine, source: &[Table], target: &[Table]) -> Vec<Pair> {
    source
        .iter()
        .map(|s| Pair {
            label: format!("{}.{}", s.schema, s.name),
            plan: match engine::find_table(engine, target, s) {
                Some(t) => Plan::new(engine, s, t),
                None => Err(tr("not in the target").into()),
            },
        })
        .collect()
}

impl ShellApp {
    /// Os servidores conectados no Object Explorer, o ativo primeiro.
    fn data_compare_servers(&self) -> Vec<Server> {
        self.connection
            .iter()
            .chain(
                self.servers
                    .parked
                    .iter()
                    .filter_map(|p| p.connection.as_ref()),
            )
            // Só bancos relacionais têm tabelas para comparar.
            .filter(|c| c.settings.engine.supports(Feature::SchemaCompare))
            .map(|c| {
                let key = server_key(&c.settings);
                let name = self
                    .registered_name(&key)
                    .unwrap_or_else(|| c.settings.server_name());
                Server {
                    label: format!("{name} ({})", c.settings.user),
                    key,
                    engine: c.settings.engine,
                    queries: c.queries.clone(),
                    home: c.settings.database.clone(),
                }
            })
            .collect()
    }

    /// Abre o "Data Compare".
    pub fn open_data_compare(&mut self) {
        if self.data_compare.is_some() {
            return;
        }
        let servers = self.data_compare_servers();
        if servers.is_empty() {
            self.status = tr("Connect to a server to compare data.").to_string();
            return;
        }
        self.data_compare = Some(DataCompare::open(servers));
    }

    /// "Data Compare", para testes.
    pub fn data_compare(&self) -> Option<&DataCompare> {
        self.data_compare.as_ref()
    }

    /// "Data Compare", mutável (testes).
    pub fn data_compare_mut(&mut self) -> Option<&mut DataCompare> {
        self.data_compare.as_mut()
    }

    /// O que a sessão guarda do "Data Compare" aberto.
    pub(crate) fn data_compare_setup(&self) -> Option<Setup> {
        self.data_compare.as_ref().map(DataCompare::setup)
    }

    /// Reabre o "Data Compare" da sessão anterior. Os servidores chegam
    /// depois, conforme reconectam.
    pub(crate) fn restore_data_compare(&mut self, setup: Setup) {
        self.data_compare = Some(DataCompare::restore(Vec::new(), setup));
    }

    pub(crate) fn data_compare_window(&mut self, ctx: &egui::Context) {
        if self.data_compare.is_none() {
            return;
        }
        let servers = self.data_compare_servers();
        let Some(dialog) = &mut self.data_compare else {
            return;
        };
        dialog.set_servers(servers);
        let request = dialog.show(ctx);
        if let Some(question) = dialog.take_question() {
            self.ask_path(question, |app, ctx, path| {
                if let (Some(d), Some(path)) = (app.data_compare.as_mut(), path) {
                    d.save_script(path, ctx);
                }
            });
        }
        self.data_compare_request(request);
    }

    pub(crate) fn data_compare_request(&mut self, request: Option<Request>) {
        match request {
            None => {}
            Some(Request::Log(lines)) => self.output.extend(lines),
            Some(Request::Script {
                sql,
                server,
                database,
            }) => {
                let id = self.documents.new_script(sql, Some(database));
                self.doc_servers.insert(id, server);
                self.activate_document(id);
                self.output
                    .push(tr("Data Compare: the script was opened in a new query window.").into());
            }
            Some(Request::Closed) => self.data_compare = None,
        }
    }
}
