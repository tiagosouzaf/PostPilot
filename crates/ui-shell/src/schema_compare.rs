//! "Schema Compare" (Tools > Schema Compare..., ou Tasks > Compare Schema...
//! num banco): compara dois schemas, mostra o que difere, gera o script que
//! deixa o destino como a origem e aplica numa transação.
//!
//! Cada lado é um banco do servidor conectado, um banco de outro servidor
//! (conecta só para comparar e aplicar) ou um snapshot `.pgsschema` salvo
//! antes ("Save Snapshot..." em qualquer lado que seja um banco).
//!
//! Tabelas, colunas, chaves, índices e checks vêm do `schema-model`; views,
//! funções, procedures, triggers, sequências e tipos são lidos pelo
//! [`catalog`] e comparados pela definição. O [`Outcome`] junta tudo.

use pgstudio_i18n::{tr, trc, trf};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align, Layout};
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode, parse_server_name};
use pgstudio_core::metadata::MetadataProvider;
use pgstudio_core::query::{QueryEvent, QueryService};
use pgstudio_core::schema::Dialect;
pub use pgstudio_core::schema::compare::{Item, Outcome, Status};
pub use pgstudio_core::schema::objects::{Key, Kind};
use pgstudio_core::schema::snapshot::{self, Schema, Snapshot};

use crate::file_dialogs::Question;

pub mod catalog;

/// De onde vem um lado da comparação.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Place {
    /// Um banco do servidor conectado.
    #[default]
    Database,
    /// Um banco de outro servidor.
    Server,
    /// Um snapshot `.pgsschema`.
    Snapshot,
}

impl Place {
    pub const ALL: [Place; 3] = [Place::Database, Place::Server, Place::Snapshot];

    pub fn label(self) -> &'static str {
        tr(match self {
            Place::Database => "Database",
            Place::Server => "Other server",
            Place::Snapshot => "Snapshot file",
        })
    }
}

/// Um lado da comparação, como está na janela.
#[derive(Clone, Default)]
pub struct Side {
    pub place: Place,
    /// O banco (no servidor conectado ou no outro).
    pub database: String,
    /// Outro servidor: "Server name" como no Connect to Server.
    pub server: String,
    pub user: String,
    /// Só na memória, enquanto a janela está aberta.
    pub password: String,
    pub ssl: SslMode,
    /// O arquivo `.pgsschema`.
    pub file: String,
}

/// Qual lado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Source,
    Target,
}

/// Um lado pronto para ler, conferido.
#[derive(Clone)]
enum Endpoint {
    Here(String),
    There(ConnectionSettings, String),
    File(PathBuf),
}

impl Endpoint {
    /// Como aparece na janela e no log.
    fn label(&self) -> String {
        match self {
            Endpoint::Here(db) => db.clone(),
            Endpoint::There(s, _) => format!("{}/{}", s.server_name(), s.database),
            Endpoint::File(p) => p
                .file_name()
                .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into()),
        }
    }

    fn same_as(&self, other: &Endpoint) -> bool {
        match (self, other) {
            (Endpoint::Here(a), Endpoint::Here(b)) => a == b,
            (Endpoint::There(a, _), Endpoint::There(b, _)) => {
                (&a.host, a.port, &a.database) == (&b.host, b.port, &b.database)
            }
            (Endpoint::File(a), Endpoint::File(b)) => a == b,
            _ => false,
        }
    }
}

impl Side {
    fn endpoint(&self, what: &str) -> Result<Endpoint, String> {
        match self.place {
            Place::Database if self.database.is_empty() => {
                Err(trf!("Choose the {what} database.", what = what))
            }
            Place::Database => Ok(Endpoint::Here(self.database.clone())),
            Place::Server => {
                let (host, port) = parse_server_name(&self.server)
                    .map_err(|e| trf!("{what} server: {e}", what = what, e = e))?;
                if self.database.trim().is_empty() {
                    return Err(trf!("Enter the {what} database.", what = what));
                }
                let settings = ConnectionSettings {
                    engine: Default::default(),
                    host,
                    port,
                    database: self.database.trim().into(),
                    user: self.user.trim().into(),
                    ssl: self.ssl,
                };
                Ok(Endpoint::There(settings, self.password.clone()))
            }
            Place::Snapshot if self.file.trim().is_empty() => {
                Err(trf!("Choose the {what} snapshot file.", what = what))
            }
            Place::Snapshot => Ok(Endpoint::File(PathBuf::from(self.file.trim()))),
        }
    }
}

/// O que as threads precisam para ler um lado.
#[derive(Clone)]
struct Env {
    provider: Arc<dyn MetadataProvider>,
    queries: Arc<dyn QueryService>,
    connector: Option<Arc<dyn Connector>>,
    /// O servidor conectado, para o cabeçalho do snapshot.
    server: String,
}

impl Env {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<pgstudio_core::connection::Connected, String> {
        let Some(connector) = &self.connector else {
            return Err(tr("This build cannot connect to other servers.").into());
        };
        connector
            .connect(settings, password)
            .map_err(|e| format!("{}: {e}", settings.server_name()))
    }

    /// O schema de um lado (roda numa thread).
    fn load(&self, e: &Endpoint) -> Result<Schema, String> {
        let label = e.label();
        let r = match e {
            Endpoint::Here(db) => catalog::load(&*self.provider, &*self.queries, db),
            Endpoint::There(settings, password) => {
                let c = self.connect(settings, password)?;
                catalog::load(&*c.metadata, &*c.queries, &settings.database)
            }
            Endpoint::File(path) => std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|text| Snapshot::from_json(&text))
                .map(|s| s.schema),
        };
        r.map_err(|e| format!("{label}: {e}"))
    }

    /// Salva o schema de um banco num `.pgsschema`.
    fn save(&self, e: &Endpoint, path: &std::path::Path) -> Result<(), String> {
        let schema = self.load(e)?;
        let (server, db) = match e {
            Endpoint::Here(db) => (self.server.clone(), db.clone()),
            Endpoint::There(s, _) => (s.server_name(), s.database.clone()),
            Endpoint::File(_) => return Err(tr("Choose a database to save.").into()),
        };
        let taken = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let snap = Snapshot::new(&server, &db, taken, schema);
        std::fs::write(path, snap.to_json()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Aplica o script num banco, numa transação.
    fn apply(&self, e: &Endpoint, statements: &[String]) -> Result<(), String> {
        match e {
            Endpoint::Here(db) => self.provider.apply_ddl(db, statements).map_err(|e| e.0),
            Endpoint::There(settings, password) => {
                let c = self.connect(settings, password)?;
                c.metadata
                    .apply_ddl(&settings.database, statements)
                    .map_err(|e| e.0)
            }
            Endpoint::File(_) => Err(tr("A snapshot file cannot be changed.").into()),
        }
    }
}

/// Compara os dois lados, que têm de ser do mesmo tipo de servidor: o
/// script de um não roda no outro.
fn outcome(source: &Schema, target: &Schema) -> Result<Outcome, String> {
    if source.model.dialect != target.model.dialect {
        return Err(
            tr("A PostgreSQL database cannot be compared with a SQL Server database.").into(),
        );
    }
    Ok(Outcome::new(source, target))
}

/// O que foi comparado.
struct Compared {
    outcome: Outcome,
    source: String,
    target: Endpoint,
}

enum Phase {
    Idle,
    Comparing(Receiver<Result<Outcome, String>>, String, Endpoint),
    Applying(Receiver<Result<(), String>>, Vec<String>),
    Saving(Receiver<Result<(), String>>, String),
}

/// O que a janela pediu à casca.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Abrir o script numa janela de consulta no banco destino.
    Script(String, String),
    Log(Vec<String>),
    /// Aplicou num banco do servidor conectado.
    Applied(String, Vec<String>),
    Closed,
}

/// A janela "Schema Compare".
pub struct SchemaCompare {
    env: Env,
    pub source: Side,
    pub target: Side,
    databases: Option<Vec<String>>,
    databases_rx: Receiver<Vec<String>>,
    phase: Phase,
    compared: Option<Compared>,
    /// O que entra no script (tudo, depois de comparar).
    pub chosen: BTreeSet<Key>,
    /// O objeto cujo DDL aparece ao lado.
    pub selected: Option<Key>,
    confirming: bool,
    error: Option<String>,
    status: Option<String>,
    /// Diálogo de arquivo pedido neste frame (a casca pergunta e devolve
    /// a resposta em [`SchemaCompare::answered`]).
    asking: Option<(Ask, Question)>,
}

/// Para que serve o arquivo pedido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Onde salvar o script (Generate Script fora do servidor conectado).
    Script,
    /// Onde salvar o snapshot de um lado.
    Snapshot(Which),
    /// O snapshot a comparar num lado (Browse...).
    Browse(Which),
}

impl SchemaCompare {
    /// Abre a janela; `source` é o banco de origem sugerido. `home` é o
    /// banco da conexão, de onde vem a lista de bancos.
    pub fn open(
        source: Option<String>,
        home: &str,
        provider: Arc<dyn MetadataProvider>,
        queries: Arc<dyn QueryService>,
        ctx: &egui::Context,
    ) -> Self {
        let (tx, databases_rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        let mut names = Vec::new();
        let session = queries.open(home);
        session.execute(
            session.engine().list_databases_sql().to_string(),
            Box::new(move |e| match e {
                QueryEvent::Row(r) => {
                    if let Some(Some(n)) = r.into_iter().next() {
                        names.push(n);
                    }
                }
                QueryEvent::Finished { .. } => {
                    let _ = tx.send(std::mem::take(&mut names));
                    ctx2.request_repaint();
                }
                _ => {}
            }),
        );
        Self {
            env: Env {
                provider,
                queries,
                connector: None,
                server: String::new(),
            },
            source: Side {
                database: source.unwrap_or_default(),
                ..Default::default()
            },
            target: Side::default(),
            databases: None,
            databases_rx,
            phase: Phase::Idle,
            compared: None,
            chosen: BTreeSet::new(),
            selected: None,
            confirming: false,
            error: None,
            status: None,
            asking: None,
        }
    }

    /// Permite comparar com outros servidores, conectando com `connector`;
    /// `home` é a conexão atual, que preenche servidor e usuário.
    pub fn with_connector(
        mut self,
        connector: Option<Arc<dyn Connector>>,
        home: &ConnectionSettings,
    ) -> Self {
        self.env.connector = connector;
        self.env.server = home.server_name();
        for side in [&mut self.source, &mut self.target] {
            side.server = home.server_name();
            side.user = home.user.clone();
            side.ssl = home.ssl;
        }
        self
    }

    pub fn databases(&self) -> Option<&[String]> {
        self.databases.as_deref()
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.compared.as_ref().map(|c| &c.outcome)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    pub fn side_mut(&mut self, which: Which) -> &mut Side {
        match which {
            Which::Source => &mut self.source,
            Which::Target => &mut self.target,
        }
    }

    /// Compara origem e destino.
    pub fn compare(&mut self, ctx: &egui::Context) {
        let ends = self
            .source
            .endpoint(trc("side", "source"))
            .and_then(|s| Ok((s, self.target.endpoint(trc("side", "target"))?)));
        let (s, t) = match ends {
            Ok(e) => e,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if s.same_as(&t) {
            self.error = Some(tr("The source and the target are the same database.").into());
            return;
        }
        self.error = None;
        self.status = None;
        self.confirming = false;
        let (tx, rx) = mpsc::channel();
        let env = self.env.clone();
        let (s2, t2) = (s.clone(), t.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = env.load(&s2).and_then(|a| outcome(&a, &env.load(&t2)?));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.phase = Phase::Comparing(rx, s.label(), t);
    }

    /// Salva o schema de um lado num `.pgsschema`.
    pub fn save_snapshot(&mut self, which: Which, path: PathBuf, ctx: &egui::Context) {
        let side = match which {
            Which::Source => &self.source,
            Which::Target => &self.target,
        };
        let e = match side.endpoint(if which == Which::Source {
            trc("side", "source")
        } else {
            trc("side", "target")
        }) {
            Ok(Endpoint::File(_)) => {
                self.error = Some(tr("Choose a database to save as a snapshot.").into());
                return;
            }
            Ok(e) => e,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        self.error = None;
        self.status = None;
        let (tx, rx) = mpsc::channel();
        let env = self.env.clone();
        let line = trf!(
            "{source} to {file}",
            source = e.label(),
            file = path.display()
        );
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(env.save(&e, &path));
            ctx.request_repaint();
        });
        self.phase = Phase::Saving(rx, line);
    }

    /// O script dos objetos marcados.
    pub fn script(&self) -> Vec<String> {
        self.compared
            .as_ref()
            .map(|c| c.outcome.script(&self.chosen))
            .unwrap_or_default()
    }

    /// O destino pode receber o script (é um banco)?
    pub fn can_apply(&self) -> bool {
        self.compared
            .as_ref()
            .is_some_and(|c| !matches!(c.target, Endpoint::File(_)))
    }

    /// Aplica o script no destino, numa transação.
    pub fn apply(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.compared else {
            return;
        };
        if !self.can_apply() {
            self.error =
                Some(tr("The target is a snapshot file; generate a script instead.").into());
            return;
        }
        let script = c.outcome.script(&self.chosen);
        if script.is_empty() {
            self.error = Some(tr("Select at least one difference.").into());
            return;
        }
        self.error = None;
        self.confirming = false;
        let (tx, rx) = mpsc::channel();
        let env = self.env.clone();
        let target = c.target.clone();
        let stmts = script.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(env.apply(&target, &stmts));
            ctx.request_repaint();
        });
        self.phase = Phase::Applying(rx, script);
    }

    /// O script com o cabeçalho, como vai para a janela ou o arquivo.
    fn script_text(&self) -> String {
        let Some(c) = &self.compared else {
            return String::new();
        };
        // No T-SQL cada comando é um lote (um `CREATE VIEW` tem de ser o
        // primeiro do seu).
        let separator = match c.outcome.dialect {
            Dialect::Postgres => "\n\n",
            Dialect::Tsql => "\nGO\n\n",
        };
        let mut body = self.script().join(separator);
        if c.outcome.dialect == Dialect::Tsql && !body.is_empty() {
            body.push_str("\nGO");
        }
        format!(
            "-- {}\n\n{body}\n",
            trf!(
                "Schema Compare: changes that make {target} like {source}.",
                target = c.target.label(),
                source = c.source
            ),
        )
    }

    /// Recebe o que as threads mandaram.
    pub fn step(&mut self, ctx: &egui::Context) -> Option<Request> {
        if let Ok(d) = self.databases_rx.try_recv() {
            if self.target.database.is_empty() {
                self.target.database = d
                    .iter()
                    .find(|n| **n != self.source.database)
                    .cloned()
                    .unwrap_or_default();
            }
            self.databases = Some(d);
        }
        match &self.phase {
            Phase::Idle => None,
            Phase::Comparing(rx, source, target) => {
                let r = rx.try_recv().ok()?;
                let (source, target) = (source.clone(), target.clone());
                self.phase = Phase::Idle;
                match r {
                    Ok(o) => {
                        self.chosen = o.items.iter().map(|i| i.key.clone()).collect();
                        self.selected = o.items.first().map(|i| i.key.clone());
                        let line = trf!(
                            "Schema Compare: {source} to {target}: {differences} differences, {equal} equal.",
                            source = source,
                            target = target.label(),
                            differences = o.items.len(),
                            equal = o.equal
                        );
                        self.compared = Some(Compared {
                            outcome: o,
                            source,
                            target,
                        });
                        Some(Request::Log(vec![line]))
                    }
                    Err(e) => {
                        self.compared = None;
                        self.error = Some(e.clone());
                        Some(Request::Log(vec![trf!(
                            "Schema Compare: failed ({e}).",
                            e = e
                        )]))
                    }
                }
            }
            Phase::Saving(rx, line) => {
                let r = rx.try_recv().ok()?;
                let line = line.clone();
                self.phase = Phase::Idle;
                match r {
                    Ok(()) => {
                        self.status = Some(trf!("Saved the snapshot of {line}.", line = line));
                        Some(Request::Log(vec![trf!(
                            "Schema Compare: saved the snapshot of {line}.",
                            line = line
                        )]))
                    }
                    Err(e) => {
                        self.error = Some(trf!("The snapshot was not saved: {e}", e = e));
                        None
                    }
                }
            }
            Phase::Applying(rx, script) => {
                let r = rx.try_recv().ok()?;
                let script = script.clone();
                self.phase = Phase::Idle;
                let target = self.compared.as_ref()?.target.clone();
                let label = target.label();
                match r {
                    Ok(()) => {
                        let mut log = vec![trf!(
                            "Schema Compare: applied {count} statements to {label}.",
                            count = script.len(),
                            label = label
                        )];
                        log.extend(script.iter().map(|s| format!("  {}", first_line(s))));
                        // Compara de novo os mesmos lados: o que entrou some
                        // da lista.
                        let source = self.compared.as_ref()?.source.clone();
                        self.recompare(ctx, source, target.clone());
                        self.status =
                            Some(trf!("The changes were applied to {label}.", label = label));
                        Some(match target {
                            Endpoint::Here(db) => Request::Applied(db, log),
                            _ => Request::Log(log),
                        })
                    }
                    Err(e) => {
                        self.error = Some(trf!("Nothing was changed: {e}", e = e));
                        Some(Request::Log(vec![trf!(
                            "Schema Compare: the changes were rolled back ({e}).",
                            e = e
                        )]))
                    }
                }
            }
        }
    }

    /// Compara de novo depois de aplicar, com os lados da última
    /// comparação (mesmo que a janela tenha mudado).
    fn recompare(&mut self, ctx: &egui::Context, source_label: String, target: Endpoint) {
        let source = match self.source.endpoint(trc("side", "source")) {
            Ok(s) if s.label() == source_label => s,
            _ => {
                self.compare(ctx);
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let env = self.env.clone();
        let t2 = target.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = env.load(&source).and_then(|a| outcome(&a, &env.load(&t2)?));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.phase = Phase::Comparing(rx, source_label, target);
    }

    /// O diálogo de arquivo que a janela pediu, para a casca perguntar.
    pub fn take_question(&mut self) -> Option<(Ask, Question)> {
        self.asking.take()
    }

    /// A resposta do diálogo pedido em [`SchemaCompare::take_question`].
    pub fn answered(
        &mut self,
        ask: Ask,
        path: Option<std::path::PathBuf>,
        ctx: &egui::Context,
    ) -> Option<Request> {
        let path = path?;
        match ask {
            Ask::Script => self.save_script(&path),
            Ask::Snapshot(which) => {
                self.save_snapshot(which, path, ctx);
                None
            }
            Ask::Browse(which) => {
                self.side_mut(which).file = path.display().to_string();
                None
            }
        }
    }

    /// Desenha a janela. Os diálogos de arquivo (snapshots e scripts) são
    /// pedidos à casca por [`SchemaCompare::take_question`].
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Request> {
        let mut request = self.step(ctx);
        let modal = crate::theme::modal(egui::Id::new("schema_compare")).show(ctx, |ui| {
            ui.set_width(980.0);
            ui.heading(tr("Schema Compare"));
            ui.separator();
            let idle = !self.is_busy();
            ui.add_enabled_ui(idle, |ui| {
                egui::Grid::new("compare_sides")
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
                if ui
                    .button(format!(
                        "{} {}",
                        egui_phosphor::regular::GIT_DIFF,
                        tr("Compare")
                    ))
                    .clicked()
                {
                    self.compare(ctx);
                }
            });
            ui.separator();
            let body = egui::vec2(ui.available_width(), 360.0);
            ui.allocate_ui_with_layout(body, Layout::top_down(Align::Min), |ui| {
                ui.set_height(360.0);
                self.body(ui);
            });
            ui.separator();
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            } else if let Some(s) = &self.status {
                ui.label(format!("{} {s}", egui_phosphor::regular::CHECK_CIRCLE));
            }
            let bar = egui::vec2(ui.available_width(), 26.0);
            ui.allocate_ui_with_layout(bar, Layout::right_to_left(Align::Center), |ui| {
                let target = self
                    .compared
                    .as_ref()
                    .map(|c| c.target.label())
                    .unwrap_or_default();
                if self.confirming {
                    if ui.button(tr("No")).clicked() {
                        self.confirming = false;
                    }
                    if ui.button(tr("Yes")).clicked() {
                        self.apply(ctx);
                    }
                    ui.label(trf!(
                        "Apply {count} statements to {target} in one transaction?",
                        count = self.script().len(),
                        target = target
                    ));
                    return;
                }
                if ui
                    .add_enabled(idle, egui::Button::new(tr("Close")))
                    .clicked()
                {
                    request = Some(Request::Closed);
                }
                let ready = idle && self.compared.is_some() && !self.chosen.is_empty();
                if ui
                    .add_enabled(ready && self.can_apply(), egui::Button::new(tr("Apply")))
                    .clicked()
                {
                    self.confirming = true;
                }
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
                    request = self.generate_script();
                }
            });
        });
        if modal.should_close() && request.is_none() && !self.is_busy() {
            request = Some(Request::Closed);
        }
        request
    }

    /// "Generate Script": num banco do servidor conectado, abre numa janela
    /// de consulta nele; em outro servidor ou num snapshot, salva num
    /// arquivo `.sql` (uma janela de consulta rodaria no servidor errado).
    fn generate_script(&mut self) -> Option<Request> {
        let text = self.script_text();
        match &self.compared.as_ref()?.target {
            Endpoint::Here(db) => Some(Request::Script(text, db.clone())),
            _ => {
                self.asking = Some((Ask::Script, Question::save("SchemaCompare.sql")));
                None
            }
        }
    }

    fn save_script(&mut self, path: &std::path::Path) -> Option<Request> {
        match std::fs::write(path, self.script_text()) {
            Ok(()) => {
                self.status = Some(trf!(
                    "The script was saved to {path}.",
                    path = path.display()
                ));
                Some(Request::Log(vec![trf!(
                    "Schema Compare: the script was saved to {path}.",
                    path = path.display()
                )]))
            }
            Err(e) => {
                self.error = Some(format!("{}: {e}", path.display()));
                None
            }
        }
    }

    /// Os campos de um lado.
    fn side_ui(&mut self, ui: &mut egui::Ui, which: Which) {
        let dbs: Vec<String> = self.databases.clone().unwrap_or_default();
        let id = match which {
            Which::Source => "source",
            Which::Target => "target",
        };
        let side = self.side_mut(which);
        egui::ComboBox::from_id_salt(format!("compare_{id}_place"))
            .width(120.0)
            .selected_text(side.place.label())
            .show_ui(ui, |ui| {
                for p in Place::ALL {
                    ui.selectable_value(&mut side.place, p, p.label());
                }
            });
        let mut save = false;
        let mut asking = None;
        match side.place {
            Place::Database => {
                egui::ComboBox::from_id_salt(format!("compare_{id}"))
                    .width(220.0)
                    .selected_text(side.database.as_str())
                    .show_ui(ui, |ui| {
                        for d in &dbs {
                            ui.selectable_value(&mut side.database, d.clone(), d.as_str());
                        }
                    });
                save = ui.button(tr("Save Snapshot...")).clicked();
            }
            Place::Server => {
                let field = |ui: &mut egui::Ui, text: &mut String, hint: &str, w: f32| {
                    ui.add(
                        egui::TextEdit::singleline(text)
                            .hint_text(hint)
                            .desired_width(w),
                    );
                };
                field(ui, &mut side.server, tr("Server name"), 150.0);
                field(ui, &mut side.user, tr("Login"), 100.0);
                ui.add(
                    egui::TextEdit::singleline(&mut side.password)
                        .password(true)
                        .hint_text(tr("Password"))
                        .desired_width(100.0),
                );
                field(ui, &mut side.database, tr("Database"), 120.0);
                save = ui.button(tr("Save Snapshot...")).clicked();
            }
            Place::Snapshot => {
                ui.add(
                    egui::TextEdit::singleline(&mut side.file)
                        .hint_text(trf!("File (.{extension})", extension = snapshot::EXTENSION))
                        .desired_width(420.0),
                );
                if ui.button(tr("Browse...")).clicked() {
                    asking = Some((Ask::Browse(which), Question::Open));
                }
            }
        }
        if save {
            let name = format!("{}.{}", side.database.trim(), snapshot::EXTENSION);
            asking = Some((Ask::Snapshot(which), Question::save(name)));
        }
        if asking.is_some() {
            self.asking = asking;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        if matches!(self.phase, Phase::Comparing(..)) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Comparing..."));
            });
            return;
        }
        if matches!(self.phase, Phase::Applying(..)) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Applying..."));
            });
            return;
        }
        if matches!(self.phase, Phase::Saving(..)) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Saving the snapshot..."));
            });
            return;
        }
        let Some(o) = self.compared.as_ref().map(|c| &c.outcome) else {
            ui.label(tr("Choose the source and the target and press Compare."));
            return;
        };
        if o.items.is_empty() {
            ui.label(format!(
                "{} {}",
                egui_phosphor::regular::CHECK_CIRCLE,
                trf!(
                    "The schemas are equal ({count} objects compared).",
                    count = o.equal
                )
            ));
            return;
        }
        ui.horizontal(|ui| {
            ui.strong(trf!(
                "{differences} differences, {selected} selected ({equal} objects equal)",
                differences = o.items.len(),
                selected = o
                    .items
                    .iter()
                    .filter(|i| self.chosen.contains(&i.key))
                    .count(),
                equal = o.equal
            ));
            if ui.small_button(tr("All")).clicked() {
                self.chosen = o.items.iter().map(|i| i.key.clone()).collect();
            }
            if ui.small_button(tr("None")).clicked() {
                self.chosen.clear();
            }
        });
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            let list = egui::vec2(430.0, h);
            ui.allocate_ui_with_layout(list, Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(list);
                egui::ScrollArea::vertical()
                    .id_salt("compare_items")
                    .max_height(h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("compare_grid")
                            .num_columns(3)
                            .striped(true)
                            .show(ui, |ui| {
                                for item in &o.items {
                                    let mut on = self.chosen.contains(&item.key);
                                    if ui.checkbox(&mut on, "").changed() {
                                        if on {
                                            self.chosen.insert(item.key.clone());
                                        } else {
                                            self.chosen.remove(&item.key);
                                        }
                                    }
                                    let (kind, schema, name) = &item.key;
                                    let label = format!("{} {schema}.{name}", kind.label());
                                    let sel = self.selected.as_ref() == Some(&item.key);
                                    if ui.selectable_label(sel, label).clicked() {
                                        self.selected = Some(item.key.clone());
                                    }
                                    let color = match item.status {
                                        Status::OnlyInSource => {
                                            egui::Color32::from_rgb(40, 150, 70)
                                        }
                                        Status::OnlyInTarget => ui.visuals().error_fg_color,
                                        Status::Different => ui.visuals().warn_fg_color,
                                    };
                                    ui.colored_label(color, item.status.label());
                                    ui.end_row();
                                }
                            });
                    });
            });
            let rest = egui::vec2(ui.available_width(), h);
            ui.allocate_ui_with_layout(rest, Layout::top_down(Align::Min), |ui| {
                ui.set_min_size(rest);
                let item = self
                    .selected
                    .as_ref()
                    .and_then(|k| o.items.iter().find(|i| &i.key == k));
                egui::ScrollArea::both()
                    .id_salt("compare_ddl")
                    .max_height(h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match item {
                        Some(i) => {
                            for s in &i.statements {
                                ui.monospace(s);
                            }
                        }
                        None => {
                            ui.label(tr("Select a difference to see its script."));
                        }
                    });
            });
        });
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}
