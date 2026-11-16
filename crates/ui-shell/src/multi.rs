//! A mesma consulta em vários servidores de uma vez.
//!
//! Uma janela de consulta pode ter, no lugar da conexão dela, uma lista de
//! servidores guardados (do workspace ou dos Registered Servers), escolhida
//! em "Run on Multiple Servers…". O F5 dessa janela roda o lote em todos ao
//! mesmo tempo e junta as respostas num resultado só, com o servidor na
//! primeira coluna. Um servidor que falha (sem senha guardada, fora do ar,
//! erro no SQL) não para os outros: o erro dele fica nas mensagens e no
//! resumo por servidor.
//!
//! A junção é uma [`QuerySession`] ([`MultiSession`]) que distribui o lote
//! e devolve os eventos de um resultado comum; por isso a grade, o modo
//! texto, o Results to File e o Save Results As funcionam sem saber que são
//! vários servidores. A escolha fica na sessão do app (veja
//! [`crate::session`]) e volta ao reabrir.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{Connected, ConnectionSettings, Connector, Engine};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_i18n::{tr, trf};
use pgstudio_result_grid::ResultStore;
use serde::{Deserialize, Serialize};

use crate::registered::ServerTag;
use crate::secrets::SecretStore;
use crate::servers::server_key;
use crate::{ShellApp, execution};

/// Conexões abertas ao mesmo tempo, para não disparar centenas de uma vez.
const MAX_PARALLEL: usize = 8;

/// Identifica um servidor escolhido: a chave do servidor
/// (`usuario@host:porta`) e o banco, porque o mesmo servidor pode estar
/// registrado uma vez para cada banco de cliente.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetId {
    pub server: String,
    pub database: String,
}

impl TargetId {
    pub fn of(settings: &ConnectionSettings) -> Self {
        TargetId {
            server: server_key(settings),
            database: settings.database.clone(),
        }
    }
}

/// Um servidor guardado que pode receber a consulta.
#[derive(Clone, Debug)]
pub struct Target {
    /// Nome na coluna do servidor (o do registro; com o banco ou a pasta
    /// quando dois registros têm o mesmo nome).
    pub name: String,
    /// Nome do registro, como está na árvore.
    pub short: String,
    /// Pasta por extenso ("Clientes / Acme"); vazio na raiz.
    pub folder: String,
    pub settings: ConnectionSettings,
    pub tag: Option<ServerTag>,
    /// Veio do workspace aberto (senão, dos Registered Servers).
    pub workspace: bool,
}

impl Target {
    pub fn id(&self) -> TargetId {
        TargetId::of(&self.settings)
    }
}

/// Conexões abertas para uma janela de vários servidores (as dos servidores
/// que não estão no Object Explorer).
pub type Connections = Arc<Mutex<HashMap<TargetId, Connected>>>;

/// Os servidores de uma janela de consulta.
#[derive(Default)]
pub struct Selection {
    pub targets: Vec<TargetId>,
    connections: Connections,
    /// A sessão que distribui o lote, criada na primeira execução e refeita
    /// quando a escolha muda.
    session: Option<Arc<MultiSession>>,
}

impl std::fmt::Debug for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Selection")
            .field("targets", &self.targets)
            .finish_non_exhaustive()
    }
}

impl Selection {
    pub fn new(targets: Vec<TargetId>) -> Self {
        Selection {
            targets,
            connections: Arc::default(),
            session: None,
        }
    }

    /// A sessão da última execução, se houve uma.
    pub fn session(&self) -> Option<Arc<dyn QuerySession>> {
        self.session.clone().map(|s| s as Arc<dyn QuerySession>)
    }
}

/// Um servidor de uma execução.
#[derive(Clone)]
pub struct RunTarget {
    pub name: String,
    pub settings: ConnectionSettings,
    /// O servidor já está conectado no Object Explorer: usa a conexão dele.
    pub live: Option<Arc<dyn QueryService>>,
    /// O registro não existe mais nas conexões guardadas.
    pub missing: bool,
}

/// O que um servidor respondeu.
#[derive(Default)]
struct Report {
    results: Vec<ResultStore>,
    /// Um resultado aberto que ainda não terminou.
    open: bool,
    /// Linhas alteradas pelos comandos sem resultado.
    affected: u64,
    notices: Vec<ServerMessage>,
    error: Option<String>,
    canceled: bool,
    elapsed: Duration,
}

impl Report {
    fn failed(message: String) -> Self {
        Report {
            error: Some(message),
            ..Default::default()
        }
    }

    fn rows(&self) -> u64 {
        self.results.iter().map(ResultStore::len).sum()
    }
}

/// Sessão que roda cada lote em todos os servidores e junta as respostas.
pub struct MultiSession {
    targets: Vec<RunTarget>,
    connector: Option<Arc<dyn Connector>>,
    secrets: Option<Arc<dyn SecretStore>>,
    /// Conexões abertas para a janela, reaproveitadas entre execuções.
    connected: Connections,
    /// Sessões com o lote em andamento, para cancelar.
    running: Arc<Mutex<Vec<Arc<dyn QuerySession>>>>,
    canceled: Arc<AtomicBool>,
}

impl MultiSession {
    pub fn new(
        targets: Vec<RunTarget>,
        connector: Option<Arc<dyn Connector>>,
        secrets: Option<Arc<dyn SecretStore>>,
        connected: Connections,
    ) -> Self {
        MultiSession {
            targets,
            connector,
            secrets,
            connected,
            running: Arc::default(),
            canceled: Arc::default(),
        }
    }

    /// Os servidores, na ordem da coluna.
    pub fn targets(&self) -> &[RunTarget] {
        &self.targets
    }
}

/// O que cada thread de servidor precisa.
#[derive(Clone)]
struct Shared {
    connector: Option<Arc<dyn Connector>>,
    secrets: Option<Arc<dyn SecretStore>>,
    connected: Connections,
    running: Arc<Mutex<Vec<Arc<dyn QuerySession>>>>,
    canceled: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    /// De onde saem as sessões do servidor `i`: a conexão do Object
    /// Explorer, uma já aberta por esta sessão, ou uma nova com a senha
    /// guardada.
    fn service(&self, target: &RunTarget) -> Result<Arc<dyn QueryService>, String> {
        if target.missing {
            return Err(tr("This server is no longer in the saved connections.").to_string());
        }
        if let Some(live) = &target.live {
            return Ok(live.clone());
        }
        let id = TargetId::of(&target.settings);
        if let Some(c) = lock(&self.connected).get(&id) {
            return Ok(c.queries.clone());
        }
        let Some(connector) = &self.connector else {
            return Err(tr("Connect to a server to run queries.").to_string());
        };
        let key = crate::secrets::password_key(&target.settings);
        let password = self
            .secrets
            .as_ref()
            .and_then(|s| s.get(&key).ok().flatten());
        let Some(password) = password else {
            return Err(tr(
                "No saved password. Connect to this server once with \"Remember password\", or save its password in the workspace.",
            )
            .to_string());
        };
        let connected = connector
            .connect(&target.settings, &password)
            .map_err(|e| e.message().to_string())?;
        let queries = connected.queries.clone();
        lock(&self.connected).insert(id, connected);
        Ok(queries)
    }

    /// Roda `sql` num servidor e espera a resposta inteira.
    fn run(&self, target: &RunTarget, sql: &str) -> Report {
        let started = Instant::now();
        if self.canceled.load(Ordering::SeqCst) {
            return Report {
                canceled: true,
                ..Default::default()
            };
        }
        let service = match self.service(target) {
            Ok(s) => s,
            Err(e) => {
                return Report {
                    elapsed: started.elapsed(),
                    ..Report::failed(e)
                };
            }
        };
        let session = service.open(&target.settings.database);
        lock(&self.running).push(session.clone());
        let report = Arc::new(Mutex::new(Report::default()));
        let (done, finished) = mpsc::channel();
        let sink_report = report.clone();
        session.execute(
            sql.to_string(),
            Box::new(move |event| {
                let mut r = lock(&sink_report);
                match event {
                    QueryEvent::Columns(columns) => {
                        r.results.push(ResultStore::new(columns));
                        r.open = true;
                    }
                    QueryEvent::Row(row) => {
                        if r.open
                            && let Some(store) = r.results.last_mut()
                        {
                            store.push(&row);
                        }
                    }
                    QueryEvent::CommandComplete { rows } => {
                        if !std::mem::take(&mut r.open) {
                            r.affected += rows;
                        }
                    }
                    QueryEvent::Notice(n) => r.notices.push(n),
                    QueryEvent::Error(e) => {
                        r.open = false;
                        if r.error.is_none() {
                            r.error = Some(describe(&e));
                        }
                    }
                    QueryEvent::Finished { canceled } => {
                        r.canceled = canceled;
                        let _ = done.send(());
                    }
                }
            }),
        );
        // A sessão sempre termina com `Finished`; se ela sumir sem avisar,
        // o canal fecha e a espera acaba do mesmo jeito.
        let _ = finished.recv();
        lock(&self.running).retain(|s| !Arc::ptr_eq(s, &session));
        // Se o sink ainda tiver outra referência, o relatório fica com o
        // que chegou até aqui.
        let mut report = std::mem::take(&mut *lock(&report));
        report.elapsed = started.elapsed();
        report
    }
}

/// Mensagem do servidor com detalhe e dica na mesma linha.
fn describe(m: &ServerMessage) -> String {
    let mut text = m.message.clone();
    for extra in [&m.detail, &m.hint].into_iter().flatten() {
        text.push(' ');
        text.push_str(extra);
    }
    text
}

impl QuerySession for MultiSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        self.canceled.store(false, Ordering::SeqCst);
        let shared = Shared {
            connector: self.connector.clone(),
            secrets: self.secrets.clone(),
            connected: self.connected.clone(),
            running: self.running.clone(),
            canceled: self.canceled.clone(),
        };
        let targets = self.targets.clone();
        std::thread::spawn(move || {
            let n = targets.len();
            let targets = Arc::new(targets);
            let next = Arc::new(AtomicUsize::new(0));
            let sql = Arc::new(sql);
            let (tx, rx) = mpsc::channel();
            for _ in 0..n.min(MAX_PARALLEL) {
                let (shared, targets, next, sql, tx) = (
                    shared.clone(),
                    targets.clone(),
                    next.clone(),
                    sql.clone(),
                    tx.clone(),
                );
                std::thread::spawn(move || {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(target) = targets.get(i) else { break };
                        let report = shared.run(target, &sql);
                        if tx.send((i, report)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            let mut reports: Vec<Option<Report>> = (0..n).map(|_| None).collect();
            for (i, report) in rx {
                reports[i] = Some(report);
            }
            let reports: Vec<Report> = reports
                .into_iter()
                .map(|r| r.unwrap_or_else(|| Report::failed(tr("No answer.").to_string())))
                .collect();
            let canceled = shared.canceled.load(Ordering::SeqCst);
            emit(&targets, reports, canceled, &mut sink);
        });
    }

    fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
        for s in lock(&self.running).iter() {
            s.cancel();
        }
    }

    fn database(&self) -> String {
        let mut names = self.targets.iter().map(|t| t.settings.database.as_str());
        match names.next() {
            Some(first) if names.all(|n| n == first) => first.to_string(),
            _ => tr("(multiple databases)").to_string(),
        }
    }

    fn engine(&self) -> Engine {
        let mut engines = self.targets.iter().map(|t| t.settings.engine);
        match engines.next() {
            Some(first) if engines.all(|e| e == first) => first,
            _ => Engine::PostgreSql,
        }
    }
}

fn notice(text: String) -> QueryEvent {
    QueryEvent::Notice(ServerMessage {
        message: text,
        ..Default::default()
    })
}

fn seconds(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// Manda para a janela o resultado comum: os resultados de mesma posição e
/// mesmas colunas juntos, com o servidor na frente; depois o que cada
/// servidor disse e, se algum falhou ou não houve resultado, um resumo por
/// servidor.
fn emit(targets: &[RunTarget], mut reports: Vec<Report>, canceled: bool, sink: &mut QuerySink) {
    let server_column = tr("Server").to_string();
    let rows: Vec<u64> = reports.iter().map(Report::rows).collect();
    let depth = reports.iter().map(|r| r.results.len()).max().unwrap_or(0);
    let mut emitted = 0;
    for position in 0..depth {
        // Servidores cujo resultado nesta posição tem as mesmas colunas.
        let mut groups: Vec<(Vec<String>, Vec<usize>)> = Vec::new();
        for (i, r) in reports.iter().enumerate() {
            let Some(store) = r.results.get(position) else {
                continue;
            };
            let columns = store.columns().to_vec();
            match groups.iter_mut().find(|(c, _)| *c == columns) {
                Some((_, members)) => members.push(i),
                None => groups.push((columns, vec![i])),
            }
        }
        for (columns, members) in groups {
            let mut header = vec![server_column.clone()];
            header.extend(columns);
            sink(QueryEvent::Columns(header));
            let mut total = 0;
            for i in members {
                let store = &mut reports[i].results[position];
                for row in 0..store.len() {
                    let mut cells = vec![Some(targets[i].name.clone())];
                    cells.extend(store.row(row));
                    sink(QueryEvent::Row(cells));
                }
                total += store.len();
            }
            sink(QueryEvent::CommandComplete { rows: total });
            emitted += 1;
        }
    }

    let failed = reports.iter().filter(|r| r.error.is_some()).count();
    for (i, r) in reports.iter().enumerate() {
        let name = &targets[i].name;
        for n in &r.notices {
            let text = if n.severity.is_empty() {
                describe(n)
            } else {
                format!("{}: {}", n.severity, describe(n))
            };
            sink(notice(format!("[{name}] {text}")));
        }
        match &r.error {
            Some(e) => sink(notice(format!("[{name}] {}: {e}", tr("ERROR")))),
            None if r.canceled => sink(notice(format!("[{name}] {}", tr("Canceled.")))),
            None => sink(notice(format!(
                "[{name}] {} ({})",
                status_rows(r, rows[i]),
                seconds(r.elapsed)
            ))),
        }
    }

    if failed > 0 || emitted == 0 {
        sink(QueryEvent::Columns(vec![
            server_column,
            tr("Status").to_string(),
            tr("Rows").to_string(),
            tr("Time").to_string(),
            tr("Message").to_string(),
        ]));
        for (i, r) in reports.iter().enumerate() {
            let status = match (&r.error, r.canceled) {
                (Some(_), _) => tr("Failed"),
                (None, true) => tr("Canceled"),
                (None, false) => tr("OK"),
            };
            sink(QueryEvent::Row(vec![
                Some(targets[i].name.clone()),
                Some(status.to_string()),
                Some((rows[i] + r.affected).to_string()),
                Some(seconds(r.elapsed)),
                r.error.clone(),
            ]));
        }
        sink(QueryEvent::CommandComplete {
            rows: reports.len() as u64,
        });
    }

    let n = reports.len();
    if failed == n && n > 0 {
        sink(QueryEvent::Error(ServerMessage::error(tr(
            "The query failed on every server.",
        ))));
    } else if failed > 0 {
        sink(notice(trf!(
            "{ok} of {n} servers answered; {failed} failed.",
            ok = n - failed,
            n = n,
            failed = failed
        )));
    } else {
        sink(notice(trf!("{n} servers answered.", n = n)));
    }
    sink(QueryEvent::Finished { canceled });
}

fn status_rows(r: &Report, rows: u64) -> String {
    if r.results.is_empty() {
        execution::rows_affected(r.affected)
    } else if rows == 1 {
        tr("1 row").to_string()
    } else {
        trf!("{rows} rows", rows = rows)
    }
}

/// O que o usuário fez no diálogo neste frame.
#[derive(Debug, PartialEq, Eq)]
pub enum DialogAction {
    None,
    Cancel,
    /// Usar estes servidores (nenhum volta a janela para um servidor só).
    Use(Vec<TargetId>),
}

/// "Run on Multiple Servers": a lista das conexões guardadas, por pasta,
/// com uma caixa em cada servidor e em cada pasta.
pub struct Dialog {
    pub doc: u32,
    items: Vec<Target>,
    checked: HashSet<TargetId>,
    filter: String,
    focus: bool,
}

impl Dialog {
    pub fn new(doc: u32, items: Vec<Target>, checked: &[TargetId]) -> Self {
        let known: HashSet<TargetId> = items.iter().map(Target::id).collect();
        Dialog {
            doc,
            items,
            checked: checked
                .iter()
                .filter(|t| known.contains(t))
                .cloned()
                .collect(),
            filter: String::new(),
            focus: true,
        }
    }

    /// Os servidores marcados, na ordem da lista.
    pub fn chosen(&self) -> Vec<TargetId> {
        self.items
            .iter()
            .map(Target::id)
            .filter(|id| self.checked.contains(id))
            .collect()
    }

    /// Marca ou desmarca um servidor pelo nome (testes e screenshots).
    pub fn set(&mut self, name: &str, on: bool) {
        for t in self.items.iter().filter(|t| t.short == name) {
            if on {
                self.checked.insert(t.id());
            } else {
                self.checked.remove(&t.id());
            }
        }
    }

    /// Os servidores que passam no filtro: todas as palavras em algum
    /// lugar do nome, pasta, ambiente, servidor, banco ou usuário.
    fn visible(&self) -> Vec<usize> {
        let terms = crate::repo::search::terms(&self.filter);
        self.items
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                let hay = crate::repo::search::fold(&format!(
                    "{} {} {} {} {} {} {}",
                    t.name,
                    t.folder,
                    t.tag
                        .as_ref()
                        .and_then(|g| g.environment.as_deref())
                        .unwrap_or(""),
                    t.settings.server_name(),
                    t.settings.database,
                    t.settings.user,
                    t.settings.engine.label(),
                ));
                terms.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn show(&mut self, ctx: &egui::Context) -> DialogAction {
        let mut action = DialogAction::None;
        let modal = crate::theme::modal(egui::Id::new("multi_server")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(tr("Run on Multiple Servers"));
            ui.add_space(2.0);
            ui.label(tr(
                "Choose the servers where this query window runs. The results come together, with the server in the first column.",
            ));
            ui.add_space(6.0);
            if self.items.is_empty() {
                ui.label(tr(
                    "There are no saved connections. Register servers in the workspace or in Registered Servers.",
                ));
            } else {
                self.list(ui);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let n = self.checked.len();
                ui.label(if n == 1 {
                    tr("1 server selected").to_string()
                } else {
                    trf!("{n} servers selected", n = n)
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        action = DialogAction::Cancel;
                    }
                    if ui.button(tr("OK")).clicked() {
                        action = DialogAction::Use(self.chosen());
                    }
                });
            });
        });
        if modal.should_close() && action == DialogAction::None {
            action = DialogAction::Cancel;
        }
        action
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let visible = self.visible();
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(tr("Search servers"))
                    .desired_width(260.0),
            );
            if std::mem::take(&mut self.focus) {
                field.request_focus();
            }
            if ui.button(tr("Select All")).clicked() {
                for &i in &visible {
                    self.checked.insert(self.items[i].id());
                }
            }
            if ui.button(tr("Clear")).clicked() {
                for &i in &visible {
                    self.checked.remove(&self.items[i].id());
                }
            }
        });
        ui.add_space(4.0);
        egui::Frame::new()
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(4.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("multi_server_list")
                    .max_height(340.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.weak(tr("No server matches the search."));
                        }
                        let mut section: Option<(bool, String)> = None;
                        for &i in &visible {
                            let here = (self.items[i].workspace, self.items[i].folder.clone());
                            if section.as_ref() != Some(&here) {
                                self.folder_row(ui, &here, &visible);
                                section = Some(here);
                            }
                            self.server_row(ui, i);
                        }
                    });
            });
    }

    /// A linha de uma pasta: a caixa marca tudo o que está nela e nas
    /// subpastas (entre o que o filtro mostra).
    fn folder_row(&mut self, ui: &mut egui::Ui, folder: &(bool, String), visible: &[usize]) {
        let (workspace, name) = folder;
        let inside: Vec<TargetId> = visible
            .iter()
            .map(|&i| &self.items[i])
            .filter(|t| {
                t.workspace == *workspace
                    && (name.is_empty()
                        || t.folder == *name
                        || t.folder.starts_with(&format!("{name} / ")))
            })
            .map(Target::id)
            .collect();
        let marked = inside.iter().filter(|id| self.checked.contains(id)).count();
        let mut all = marked == inside.len();
        let source = if *workspace {
            tr("Workspace")
        } else {
            tr("Registered Servers")
        };
        let label = if name.is_empty() {
            source.to_string()
        } else {
            format!("{source} / {name}")
        };
        let label = egui::RichText::new(label).strong();
        let partly = marked > 0 && !all;
        if ui
            .add(egui::Checkbox::new(&mut all, label).indeterminate(partly))
            .changed()
        {
            for id in inside {
                if all {
                    self.checked.insert(id);
                } else {
                    self.checked.remove(&id);
                }
            }
        }
    }

    fn server_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let t = &self.items[i];
        let id = t.id();
        let mut on = self.checked.contains(&id);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            let changed = ui.checkbox(&mut on, t.short.as_str()).changed();
            if let Some(ServerTag {
                environment: Some(name),
                color,
                ..
            }) = &t.tag
            {
                crate::registered::environment_badge(ui, name, *color);
            }
            let mut detail = format!("{} / {}", t.settings.server_name(), t.settings.database);
            if t.settings.engine == Engine::SqlServer {
                detail = format!("{detail} ({})", t.settings.engine.label());
            }
            ui.weak(detail);
            if changed {
                if on {
                    self.checked.insert(id);
                } else {
                    self.checked.remove(&id);
                }
            }
        });
    }
}

/// Faixa de status de uma janela de vários servidores que ainda não
/// executou.
pub fn strip(ui: &mut egui::Ui, label: &str, tag: Option<&ServerTag>) {
    ui.horizontal(|ui| {
        ui.label(egui_phosphor::regular::HARD_DRIVES);
        ui.label(tr("Runs on multiple servers."));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(label);
            if let Some(ServerTag {
                environment: Some(name),
                color,
                ..
            }) = tag
            {
                crate::registered::environment_badge(ui, name, *color);
            }
        });
    });
}

impl ShellApp {
    /// As conexões guardadas que podem receber uma consulta: as do
    /// workspace aberto primeiro, depois as dos Registered Servers que não
    /// estão lá (mesmo servidor e mesmo banco).
    pub fn multi_targets(&self) -> Vec<Target> {
        let mut out: Vec<Target> = Vec::new();
        let mut seen: HashSet<TargetId> = HashSet::new();
        let workspace = self.workspace_panel.servers().map(|p| p.servers());
        let sources = workspace
            .into_iter()
            .map(|r| (r, true))
            .chain([(self.registered.servers(), false)]);
        for (servers, workspace) in sources {
            for (path, folder, s) in servers.servers_with_folders() {
                if !seen.insert(TargetId::of(&s.settings)) {
                    continue;
                }
                out.push(Target {
                    name: s.name.clone(),
                    short: s.name.clone(),
                    folder,
                    settings: s.settings.clone(),
                    tag: servers.node_tag(&path),
                    workspace,
                });
            }
        }
        // Dois registros com o mesmo nome: a pasta (e o banco, se preciso)
        // entram no nome, para a coluna do servidor dizer qual é qual.
        let count = |out: &[Target]| {
            let mut count: HashMap<String, usize> = HashMap::new();
            for t in out {
                *count.entry(t.name.clone()).or_default() += 1;
            }
            count
        };
        let same = count(&out);
        for t in out.iter_mut() {
            if same[&t.name] > 1 && !t.folder.is_empty() {
                t.name = format!("{} / {}", t.folder, t.name);
            }
        }
        let same = count(&out);
        for t in out.iter_mut() {
            if same[&t.name] > 1 {
                t.name = format!("{} ({})", t.name, t.settings.database);
            }
        }
        out
    }

    /// Os servidores escolhidos para a janela `id` (vazio numa janela de
    /// um servidor só).
    pub fn multi_selection(&self, id: u32) -> &[TargetId] {
        self.documents
            .get(id)
            .and_then(|d| d.multi.as_ref())
            .map(|m| m.targets.as_slice())
            .unwrap_or_default()
    }

    /// Faz a janela `id` rodar nestes servidores; sem nenhum, ela volta a
    /// usar a conexão dela.
    pub fn set_multi_selection(&mut self, id: u32, targets: Vec<TargetId>) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = tr("A query is already executing in this window.").to_string();
            return;
        }
        let n = targets.len();
        doc.multi = (n > 0).then(|| Selection::new(targets));
        self.status = match n {
            0 => tr("This query window runs on its own connection.").to_string(),
            1 => tr("This query window runs on 1 server.").to_string(),
            n => trf!("This query window runs on {n} servers.", n = n),
        };
    }

    /// "Run on Multiple Servers…": abre a escolha para a janela ativa.
    pub(crate) fn open_multi_dialog(&mut self) {
        let Some(id) = self.active_doc else {
            self.status = tr("No query window is open.").to_string();
            return;
        };
        let checked = self.multi_selection(id).to_vec();
        self.multi_dialog = Some(Dialog::new(id, self.multi_targets(), &checked));
    }

    /// O diálogo aberto (testes e screenshots).
    pub fn multi_dialog_mut(&mut self) -> Option<&mut Dialog> {
        self.multi_dialog.as_mut()
    }

    /// Fecha o diálogo sem mudar a escolha (screenshots).
    pub fn close_multi_dialog(&mut self) {
        self.multi_dialog = None;
    }

    pub(crate) fn multi_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.multi_dialog else {
            return;
        };
        match dialog.show(ctx) {
            DialogAction::None => {}
            DialogAction::Cancel => self.multi_dialog = None,
            DialogAction::Use(targets) => {
                let id = dialog.doc;
                self.multi_dialog = None;
                self.set_multi_selection(id, targets);
            }
        }
    }

    /// Os servidores da janela `id` prontos para executar: o registro de
    /// cada um e, se já está conectado no Object Explorer, a conexão dele.
    fn multi_run_targets(&self, id: u32) -> Vec<(RunTarget, Option<ServerTag>)> {
        let saved = self.multi_targets();
        let live: Vec<&Connected> = self
            .connection
            .iter()
            .chain(
                self.servers
                    .parked
                    .iter()
                    .filter_map(|p| p.connection.as_ref()),
            )
            .collect();
        self.multi_selection(id)
            .iter()
            .map(|id| match saved.iter().find(|t| t.id() == *id) {
                Some(t) => (
                    RunTarget {
                        name: t.name.clone(),
                        settings: t.settings.clone(),
                        live: live
                            .iter()
                            .find(|c| server_key(&c.settings) == id.server)
                            .map(|c| c.queries.clone()),
                        missing: false,
                    },
                    t.tag.clone(),
                ),
                None => (
                    RunTarget {
                        name: format!("{} / {}", id.server, id.database),
                        settings: ConnectionSettings {
                            database: id.database.clone(),
                            ..Default::default()
                        },
                        live: None,
                        missing: true,
                    },
                    None,
                ),
            })
            .collect()
    }

    /// Como a janela de vários servidores `id` aparece na faixa de status:
    /// "N servers" e a etiqueta do ambiente que pede confirmação, se algum
    /// dos servidores está num.
    pub(crate) fn multi_labels(&self) -> Vec<(u32, String, Option<ServerTag>)> {
        let ids: Vec<u32> = self
            .documents
            .ids()
            .into_iter()
            .filter(|id| !self.multi_selection(*id).is_empty())
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let saved = self.multi_targets();
        ids.into_iter()
            .map(|id| {
                let chosen = self.multi_selection(id);
                let label = if chosen.len() == 1 {
                    tr("1 server").to_string()
                } else {
                    trf!("{n} servers", n = chosen.len())
                };
                let tag = saved
                    .iter()
                    .filter(|t| chosen.contains(&t.id()))
                    .filter_map(|t| t.tag.clone())
                    .find(|t| t.confirm_changes);
                (id, label, tag)
            })
            .collect()
    }

    /// F5 numa janela de vários servidores. Devolve `false` quando a janela
    /// é de um servidor só (o F5 comum segue).
    pub(crate) fn execute_multi(&mut self, ctx: &egui::Context, id: u32, plain: bool) -> bool {
        if self.multi_selection(id).is_empty() {
            return false;
        }
        if !plain {
            self.status = tr("Plans and Object Information are not available on multiple servers.")
                .to_string();
            return true;
        }
        let Some(doc) = self.documents.get(id) else {
            return true;
        };
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = tr("A query is already executing in this window.").to_string();
            return true;
        }
        let (sql, offset) = match pgstudio_sql_editor::selection(ctx, crate::panels::editor_id(id))
        {
            Some((a, b)) if a != b => {
                let (a, b) = (a.min(b), a.max(b));
                (doc.text.chars().skip(a).take(b - a).collect(), a)
            }
            _ => (doc.text.clone(), 0),
        };
        self.dispatch_multi(ctx, id, sql, offset, false);
        true
    }

    /// Roda o lote nos servidores da janela `id`. Se o lote pode alterar o
    /// banco e algum servidor está num ambiente que pede confirmação
    /// (Produção, por padrão), espera o "Execute" do usuário; `confirmed`
    /// diz que ele já respondeu.
    pub(crate) fn dispatch_multi(
        &mut self,
        ctx: &egui::Context,
        id: u32,
        sql: String,
        offset: usize,
        confirmed: bool,
    ) {
        let targets = self.multi_run_targets(id);
        if !confirmed {
            use pgstudio_automation::safety::{Safety, classify_for};
            let mut reason = None;
            let mut first: Option<ServerTag> = None;
            let mut names = Vec::new();
            for (t, tag) in &targets {
                let Some(tag) = tag.as_ref().filter(|g| g.confirm_changes) else {
                    continue;
                };
                let why = match classify_for(t.settings.engine, &sql) {
                    Safety::ReadOnly => continue,
                    Safety::Writes { reason } => reason,
                    Safety::Unknown { .. } => tr("not only reading").to_string(),
                };
                reason.get_or_insert(why);
                first.get_or_insert_with(|| tag.clone());
                names.push(t.name.clone());
            }
            if let (Some(reason), Some(tag)) = (reason, first) {
                self.environment_confirm = Some(crate::PendingEnvironment {
                    run: crate::PendingActual {
                        doc: id,
                        sql,
                        offset,
                    },
                    environment: tag.environment.unwrap_or_default(),
                    color: tag.color,
                    reason,
                    servers: names,
                });
                return;
            }
        }
        let (connector, secrets) = (self.connector.clone(), self.secrets.clone());
        let Some(multi) = self.documents.get_mut(id).and_then(|d| d.multi.as_mut()) else {
            return;
        };
        // A sessão é refeita a cada execução, para os registros e as
        // conexões do Object Explorer valerem como estão agora; as conexões
        // que a janela abriu continuam.
        multi.session = Some(Arc::new(MultiSession::new(
            targets.into_iter().map(|(t, _)| t).collect(),
            connector,
            secrets,
            multi.connections.clone(),
        )));
        self.launch(ctx, id, sql, offset, crate::Run::Results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serviço que responde conforme o servidor: `fails` dá erro no SQL,
    /// os outros devolvem uma linha com o nome.
    struct Fake(String);

    impl QueryService for Fake {
        fn open(&self, database: &str) -> Arc<dyn QuerySession> {
            Arc::new(FakeSession(self.0.clone(), database.to_string()))
        }
    }

    struct FakeSession(String, String);

    impl QuerySession for FakeSession {
        fn execute(&self, sql: String, mut sink: QuerySink) {
            let (name, db) = (self.0.clone(), self.1.clone());
            std::thread::spawn(move || {
                if name == "fails" {
                    sink(QueryEvent::Error(ServerMessage::error("boom")));
                } else if sql.starts_with("update") {
                    sink(QueryEvent::CommandComplete { rows: 3 });
                } else {
                    let columns = if name == "other" {
                        vec!["x".to_string()]
                    } else {
                        vec!["version".to_string(), "db".to_string()]
                    };
                    let n = columns.len();
                    sink(QueryEvent::Columns(columns));
                    let mut row = vec![Some(format!("v-{name}")), Some(db)];
                    row.truncate(n);
                    sink(QueryEvent::Row(row));
                    sink(QueryEvent::CommandComplete { rows: 1 });
                }
                sink(QueryEvent::Finished { canceled: false });
            });
        }

        fn cancel(&self) {}

        fn database(&self) -> String {
            self.1.clone()
        }
    }

    fn target(name: &str) -> RunTarget {
        RunTarget {
            name: name.into(),
            settings: ConnectionSettings {
                host: name.into(),
                database: format!("db_{name}"),
                ..Default::default()
            },
            live: Some(Arc::new(Fake(name.into()))),
            missing: false,
        }
    }

    fn run(targets: Vec<RunTarget>, sql: &str) -> Vec<QueryEvent> {
        let session = MultiSession::new(targets, None, None, Arc::default());
        let (tx, rx) = mpsc::channel();
        session.execute(
            sql.into(),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(5)) {
            let end = matches!(e, QueryEvent::Finished { .. });
            events.push(e);
            if end {
                break;
            }
        }
        events
    }

    fn rows(events: &[QueryEvent]) -> Vec<Vec<Option<String>>> {
        events
            .iter()
            .filter_map(|e| match e {
                QueryEvent::Row(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn answers_come_together_with_the_server_in_front() {
        let events = run(vec![target("a"), target("b")], "select version()");
        assert_eq!(
            events[0],
            QueryEvent::Columns(vec!["Server".into(), "version".into(), "db".into()])
        );
        assert_eq!(
            rows(&events),
            vec![
                vec![s("a"), s("v-a"), s("db_a")],
                vec![s("b"), s("v-b"), s("db_b")],
            ]
        );
        assert!(events.contains(&QueryEvent::CommandComplete { rows: 2 }));
        assert!(!events.iter().any(|e| matches!(e, QueryEvent::Error(_))));
        assert_eq!(
            events.last(),
            Some(&QueryEvent::Finished { canceled: false })
        );
    }

    #[test]
    fn a_server_that_fails_does_not_stop_the_others() {
        let mut gone = target("gone");
        gone.live = None;
        let events = run(
            vec![target("a"), target("fails"), gone, target("b")],
            "select version()",
        );
        let r = rows(&events);
        // O resultado comum e depois o resumo por servidor.
        assert_eq!(r[0], vec![s("a"), s("v-a"), s("db_a")]);
        assert_eq!(r[1], vec![s("b"), s("v-b"), s("db_b")]);
        assert_eq!(r[3][..2], [s("fails"), s("Failed")]);
        assert_eq!(r[3][4], s("boom"));
        assert_eq!(r[4][..2], [s("gone"), s("Failed")]);
        assert_eq!(r[5][..3], [s("b"), s("OK"), s("1")]);
        assert!(!events.iter().any(|e| matches!(e, QueryEvent::Error(_))));
        let notices: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                QueryEvent::Notice(n) => Some(n.message.as_str()),
                _ => None,
            })
            .collect();
        assert!(notices.contains(&"[fails] ERROR: boom"), "{notices:?}");
        assert!(notices.contains(&"2 of 4 servers answered; 2 failed."));
    }

    #[test]
    fn when_every_server_fails_the_execution_fails() {
        let events = run(vec![target("fails")], "select 1");
        assert!(events.iter().any(|e| matches!(e, QueryEvent::Error(_))));
    }

    #[test]
    fn different_columns_stay_in_separate_results() {
        let events = run(vec![target("a"), target("other"), target("b")], "select 1");
        let columns: Vec<&QueryEvent> = events
            .iter()
            .filter(|e| matches!(e, QueryEvent::Columns(_)))
            .collect();
        assert_eq!(columns.len(), 2);
        assert_eq!(
            rows(&events),
            vec![
                vec![s("a"), s("v-a"), s("db_a")],
                vec![s("b"), s("v-b"), s("db_b")],
                vec![s("other"), s("v-other")],
            ]
        );
    }

    #[test]
    fn statements_without_results_get_a_summary_per_server() {
        let events = run(vec![target("a"), target("b")], "update t set x = 1");
        assert_eq!(
            events[2],
            QueryEvent::Columns(vec![
                "Server".into(),
                "Status".into(),
                "Rows".into(),
                "Time".into(),
                "Message".into()
            ])
        );
        let r = rows(&events);
        assert_eq!(r[0][..3], [s("a"), s("OK"), s("3")]);
        assert_eq!(r[1][..3], [s("b"), s("OK"), s("3")]);
    }
}
