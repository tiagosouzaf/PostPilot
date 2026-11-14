//! Activity Monitor: o que está rodando no servidor agora.
//!
//! Como o do SSMS: um painel com gráficos (Overview) e as seções
//! Processes, Resource Waits, Data File I/O e Recent Expensive Queries.
//! Tudo vem das views que o PostgreSQL já expõe (`pg_stat_activity`,
//! `pg_stat_database`); `pg_stat_statements` só aparece se estiver
//! instalada. A leitura roda numa sessão própria, fora da thread da
//! interface, a cada N segundos; Cancel e Kill pedem confirmação e usam
//! `pg_cancel_backend` e `pg_terminate_backend`.

use pgstudio_i18n::{tr, tr_noop, trf};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, Sense, Stroke, vec2};
use egui_phosphor::regular as icon;
use pgstudio_core::query::{QueryEvent, QuerySession};

/// Intervalos de atualização oferecidos, em segundos (o SSMS usa 10).
pub const INTERVALS: &[u64] = &[1, 5, 10, 30, 60];
/// Pontos guardados em cada gráfico.
pub const HISTORY: usize = 60;
/// Altura de uma linha das tabelas.
const ROW_HEIGHT: f32 = 20.0;

/// Uma sessão do servidor (linha de `pg_stat_activity`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Process {
    pub pid: i32,
    pub user: String,
    pub database: String,
    pub application: String,
    pub client: String,
    pub state: String,
    pub wait_type: String,
    pub wait_event: String,
    /// Sessões que bloqueiam esta (`pg_blocking_pids`), separadas por vírgula.
    pub blocked_by: String,
    /// Segundos desde o início da transação.
    pub transaction_seconds: Option<f64>,
    /// Segundos desde o início da consulta atual (ou da última).
    pub query_seconds: Option<f64>,
    pub backend_type: String,
    pub query: String,
}

impl Process {
    /// Está esperando por um recurso enquanto roda (lock, I/O...); um
    /// `pg_sleep` (Timeout) não conta.
    pub fn is_waiting(&self) -> bool {
        self.state == "active" && !self.wait_type.is_empty() && self.wait_type != "Timeout"
    }

    /// Sessão de um cliente (e não um processo do próprio servidor, como o
    /// checkpointer).
    pub fn is_client(&self) -> bool {
        self.backend_type == "client backend"
    }
}

/// Contadores de um banco (`pg_stat_database`), acumulados desde o reset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatabaseIo {
    pub name: String,
    pub transactions: f64,
    pub blocks_read: f64,
    pub blocks_hit: f64,
    pub temp_bytes: f64,
}

/// Um lock (linha de `pg_locks`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lock {
    pub pid: i32,
    pub lock_type: String,
    pub mode: String,
    pub granted: bool,
    /// Tabela ou índice travado, quando é um lock de relação.
    pub relation: String,
    /// Transação ou xid virtual, nos locks de transação.
    pub target: String,
    pub database: String,
}

/// Uma consulta de `pg_stat_statements`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Statement {
    pub calls: f64,
    pub total_ms: f64,
    pub mean_ms: f64,
    pub rows: f64,
    pub query: String,
}

/// Uma leitura completa do servidor.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub at: Instant,
    pub processes: Vec<Process>,
    pub databases: Vec<DatabaseIo>,
    /// Locks, os que esperam primeiro.
    pub locks: Vec<Lock>,
    /// `pg_stat_statements` está instalada no banco da sessão.
    pub has_statements: bool,
    /// Consultas mais caras, quando lidas nesta atualização.
    pub statements: Option<Vec<Statement>>,
}

/// Um ponto dos gráficos.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub active: f64,
    pub waiting: f64,
    pub transactions_per_second: f64,
    pub reads_per_second: f64,
}

/// Linhas de um resultado, como texto.
type Rows = Vec<Vec<Option<String>>>;

/// Consultas de cada atualização. A última só vai quando
/// `pg_stat_statements` existe.
pub fn refresh_sql(with_statements: bool) -> String {
    let mut sql = String::from(
        "SELECT pid, usename, datname, application_name, host(client_addr), state, \
         wait_event_type, wait_event, array_to_string(pg_blocking_pids(pid), ', '), \
         extract(epoch FROM now() - xact_start), extract(epoch FROM now() - query_start), \
         backend_type, query \
         FROM pg_stat_activity WHERE pid <> pg_backend_pid() ORDER BY pid;\n\
         SELECT datname, xact_commit + xact_rollback, blks_read, blks_hit, temp_bytes \
         FROM pg_stat_database WHERE datname IS NOT NULL ORDER BY datname;\n\
         SELECT l.pid, l.locktype, l.mode, l.granted, l.relation::regclass::text, \
         coalesce(l.transactionid::text, l.virtualxid), d.datname \
         FROM pg_locks l LEFT JOIN pg_database d ON d.oid = l.database \
         WHERE l.pid IS DISTINCT FROM pg_backend_pid() ORDER BY l.granted, l.pid;\n\
         SELECT count(*) FROM pg_extension WHERE extname = 'pg_stat_statements'",
    );
    if with_statements {
        sql.push_str(
            ";\nSELECT calls, total_exec_time, mean_exec_time, rows, query \
             FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 25",
        );
    }
    sql
}

/// Monta a leitura a partir dos resultados de [`refresh_sql`].
pub fn parse_snapshot(results: &[Rows], at: Instant) -> Result<Snapshot, String> {
    if results.len() < 4 {
        return Err(tr("The server did not return the activity data.").into());
    }
    let text = |r: &[Option<String>], i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let num = |r: &[Option<String>], i: usize| {
        r.get(i)
            .and_then(|v| v.as_deref())
            .and_then(|v| v.parse::<f64>().ok())
    };
    let processes = results[0]
        .iter()
        .map(|r| Process {
            pid: num(r, 0).unwrap_or_default() as i32,
            user: text(r, 1),
            database: text(r, 2),
            application: text(r, 3),
            client: text(r, 4),
            state: text(r, 5),
            wait_type: text(r, 6),
            wait_event: text(r, 7),
            blocked_by: text(r, 8),
            transaction_seconds: num(r, 9),
            query_seconds: num(r, 10),
            backend_type: text(r, 11),
            query: text(r, 12),
        })
        .collect();
    let databases = results[1]
        .iter()
        .map(|r| DatabaseIo {
            name: text(r, 0),
            transactions: num(r, 1).unwrap_or_default(),
            blocks_read: num(r, 2).unwrap_or_default(),
            blocks_hit: num(r, 3).unwrap_or_default(),
            temp_bytes: num(r, 4).unwrap_or_default(),
        })
        .collect();
    let locks = results[2]
        .iter()
        .map(|r| Lock {
            pid: num(r, 0).unwrap_or_default() as i32,
            lock_type: text(r, 1),
            mode: text(r, 2),
            granted: text(r, 3) == "t",
            relation: text(r, 4),
            target: text(r, 5),
            database: text(r, 6),
        })
        .collect();
    let has_statements = results[3]
        .first()
        .and_then(|r| num(r, 0))
        .is_some_and(|n| n > 0.0);
    let statements = results.get(4).map(|rows| {
        rows.iter()
            .map(|r| Statement {
                calls: num(r, 0).unwrap_or_default(),
                total_ms: num(r, 1).unwrap_or_default(),
                mean_ms: num(r, 2).unwrap_or_default(),
                rows: num(r, 3).unwrap_or_default(),
                query: text(r, 4),
            })
            .collect()
    });
    Ok(Snapshot {
        at,
        processes,
        databases,
        locks,
        has_statements,
        statements,
    })
}

/// Ponto dos gráficos entre duas leituras.
pub fn sample(previous: Option<&Snapshot>, current: &Snapshot) -> Sample {
    let active = current
        .processes
        .iter()
        .filter(|p| p.state == "active")
        .count() as f64;
    let waiting = current.processes.iter().filter(|p| p.is_waiting()).count() as f64;
    let (mut tps, mut reads) = (0.0, 0.0);
    if let Some(prev) = previous {
        let secs = current.at.duration_since(prev.at).as_secs_f64();
        if secs > 0.0 {
            let total = |s: &Snapshot, f: fn(&DatabaseIo) -> f64| -> f64 {
                s.databases.iter().map(f).sum()
            };
            tps = ((total(current, |d| d.transactions) - total(prev, |d| d.transactions)) / secs)
                .max(0.0);
            reads = ((total(current, |d| d.blocks_read) - total(prev, |d| d.blocks_read)) / secs)
                .max(0.0);
        }
    }
    Sample {
        active,
        waiting,
        transactions_per_second: tps,
        reads_per_second: reads,
    }
}

/// Esperas agrupadas por tipo e evento, das sessões que estão rodando.
pub fn resource_waits(processes: &[Process]) -> Vec<(String, String, usize)> {
    let mut groups: BTreeMap<(String, String), usize> = BTreeMap::new();
    for p in processes.iter().filter(|p| p.is_waiting()) {
        *groups
            .entry((p.wait_type.clone(), p.wait_event.clone()))
            .or_default() += 1;
    }
    let mut out: Vec<_> = groups.into_iter().map(|((t, e), n)| (t, e, n)).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.cmp(b)));
    out
}

/// Cadeias de bloqueio: cada sessão que bloqueia outras sem estar
/// bloqueada (a "head blocker" do SSMS), seguida de quem ela bloqueia, com a
/// profundidade na cadeia.
pub fn blocking_chains(processes: &[Process]) -> Vec<(usize, i32)> {
    let blockers = |p: &Process| -> Vec<i32> {
        p.blocked_by
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    };
    let mut blocked_by: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    let mut blocks: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for p in processes {
        for b in blockers(p) {
            blocked_by.entry(p.pid).or_default().push(b);
            blocks.entry(b).or_default().push(p.pid);
        }
    }
    let mut heads: Vec<i32> = blocks
        .keys()
        .copied()
        .filter(|pid| !blocked_by.contains_key(pid))
        .collect();
    // Num ciclo (deadlock ainda não detectado) ninguém é cabeça: começa
    // pelo menor pid.
    if heads.is_empty()
        && let Some(&pid) = blocks.keys().next()
    {
        heads.push(pid);
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<(usize, i32)> = heads.into_iter().rev().map(|p| (0, p)).collect();
    while let Some((depth, pid)) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        out.push((depth, pid));
        if let Some(children) = blocks.get(&pid) {
            for &c in children.iter().rev() {
                stack.push((depth + 1, c));
            }
        }
    }
    out
}

/// O que o usuário pediu para uma sessão.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `pg_cancel_backend`: cancela a consulta, a sessão continua.
    Cancel(i32),
    /// `pg_terminate_backend`: derruba a sessão.
    Kill(i32),
}

impl Action {
    fn sql(self) -> String {
        match self {
            Action::Cancel(pid) => format!("SELECT pg_cancel_backend({pid})"),
            Action::Kill(pid) => format!("SELECT pg_terminate_backend({pid})"),
        }
    }

    fn pid(self) -> i32 {
        match self {
            Action::Cancel(pid) | Action::Kill(pid) => pid,
        }
    }
}

/// O que a tela tira de uma leitura: refeito só quando a leitura, o
/// filtro, a ordem ou "System processes" mudam, e não a cada frame (com
/// milhares de sessões, filtrar, ordenar e montar as cadeias pesa).
#[derive(Default)]
struct Derived {
    key: Option<(Instant, String, (Column, bool), bool)>,
    /// Índices em `processes` das linhas da tabela, na ordem.
    visible: Vec<usize>,
    /// Sessões de cliente (ou todas, com "System processes").
    total: usize,
    /// Cadeias de bloqueio: profundidade, pid e o texto da linha.
    chains: Vec<(usize, i32, String)>,
}

/// Coluna de ordenação da tabela Processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Pid,
    User,
    Database,
    Application,
    Client,
    State,
    WaitType,
    WaitEvent,
    BlockedBy,
    Duration,
    Query,
}

const PROCESS_COLUMNS: &[(Column, &str, f32)] = &[
    (Column::Pid, tr_noop("Process ID"), 78.0),
    (Column::User, tr_noop("User"), 70.0),
    (Column::Database, tr_noop("Database"), 70.0),
    (Column::Application, tr_noop("Application"), 90.0),
    (Column::Client, tr_noop("Client"), 80.0),
    (Column::State, tr_noop("State"), 110.0),
    (Column::WaitType, tr_noop("Wait Type"), 70.0),
    (Column::WaitEvent, tr_noop("Wait Resource"), 100.0),
    (Column::BlockedBy, tr_noop("Blocked By"), 70.0),
    (Column::Duration, tr_noop("Duration"), 64.0),
    (Column::Query, tr_noop("Command"), 320.0),
];

/// Resposta de uma atualização ou ação.
enum Reply {
    Snapshot(Result<Snapshot, String>),
    Action(Action, Result<bool, String>),
}

/// O painel do Activity Monitor.
pub struct ActivityMonitor {
    session: Arc<dyn QuerySession>,
    server: String,
    /// Segundos entre as atualizações.
    pub interval: u64,
    pub paused: bool,
    tx: mpsc::Sender<Reply>,
    rx: Receiver<Reply>,
    refreshing: bool,
    last_refresh: Option<Instant>,
    snapshot: Option<Snapshot>,
    statements: Vec<Statement>,
    history: VecDeque<Sample>,
    error: Option<String>,
    /// Última mensagem de Cancel ou Kill.
    message: Option<String>,
    pub filter: String,
    /// Mostra também os processos do servidor (checkpointer, autovacuum...).
    pub show_system: bool,
    pub selected: Option<i32>,
    sort: (Column, bool),
    /// Ação esperando o "Yes".
    confirm: Option<Action>,
    /// Linhas desenhadas no último frame (para testes e medição).
    drawn_rows: usize,
    derived: Derived,
}

impl ActivityMonitor {
    /// Abre o monitor sobre uma sessão própria; a primeira leitura sai no
    /// primeiro frame.
    pub fn new(session: Arc<dyn QuerySession>, server: String) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            session,
            server,
            interval: 10,
            paused: false,
            tx,
            rx,
            refreshing: false,
            last_refresh: None,
            snapshot: None,
            statements: Vec::new(),
            history: VecDeque::new(),
            error: None,
            message: None,
            filter: String::new(),
            show_system: false,
            selected: None,
            sort: (Column::Pid, true),
            confirm: None,
            drawn_rows: 0,
            derived: Derived::default(),
        }
    }

    pub fn title(&self) -> String {
        format!("{} - {}", tr("Activity Monitor"), self.server)
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn history(&self) -> &VecDeque<Sample> {
        &self.history
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    /// Há uma confirmação de Cancel ou Kill aberta.
    pub fn is_confirming(&self) -> bool {
        self.confirm.is_some()
    }

    pub fn drawn_rows(&self) -> usize {
        self.drawn_rows
    }

    /// Pede uma leitura agora (se não houver outra em andamento).
    pub fn refresh(&mut self, ctx: &egui::Context) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.last_refresh = Some(Instant::now());
        let with_statements = self.snapshot.as_ref().is_some_and(|s| s.has_statements);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let mut results: Vec<Rows> = Vec::new();
        let mut current: Option<Rows> = None;
        let mut error: Option<String> = None;
        self.session.execute(
            refresh_sql(with_statements),
            Box::new(move |event| match event {
                QueryEvent::Columns(_) => current = Some(Vec::new()),
                QueryEvent::Row(r) => {
                    if let Some(rows) = &mut current {
                        rows.push(r);
                    }
                }
                QueryEvent::CommandComplete { .. } => {
                    results.push(current.take().unwrap_or_default());
                }
                QueryEvent::Error(e) => error = Some(e.message),
                QueryEvent::Notice(_) => {}
                QueryEvent::Finished { .. } => {
                    // A leitura de pg_stat_statements é a última; se ela
                    // falhar (biblioteca não carregada), o resto vale.
                    let reply = match (&error, results.len()) {
                        (Some(e), n) if n < 4 => Err(e.clone()),
                        _ => parse_snapshot(&results, Instant::now()),
                    };
                    let _ = tx.send(Reply::Snapshot(reply));
                    ctx.request_repaint();
                }
            }),
        );
    }

    /// Pede Cancel ou Kill; só roda depois do "Yes".
    pub fn request(&mut self, action: Action) {
        self.confirm = Some(action);
    }

    /// Responde a confirmação aberta.
    pub fn answer(&mut self, yes: bool, ctx: &egui::Context) {
        let Some(action) = self.confirm.take() else {
            return;
        };
        if !yes {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let mut value: Option<bool> = None;
        let mut error: Option<String> = None;
        self.session.execute(
            action.sql(),
            Box::new(move |event| match event {
                QueryEvent::Row(r) => {
                    value = Some(r.first().cloned().flatten().as_deref() == Some("t"))
                }
                QueryEvent::Error(e) => error = Some(e.message),
                QueryEvent::Finished { .. } => {
                    let reply = match error.take() {
                        Some(e) => Err(e),
                        None => Ok(value.unwrap_or(false)),
                    };
                    let _ = tx.send(Reply::Action(action, reply));
                    ctx.request_repaint();
                }
                _ => {}
            }),
        );
    }

    /// Recebe as respostas e agenda a próxima leitura. Devolve as linhas
    /// para o Output.
    pub fn step(&mut self, ctx: &egui::Context) -> Vec<String> {
        let mut log = Vec::new();
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Snapshot(Ok(snap)) => {
                    self.refreshing = false;
                    self.error = None;
                    let point = sample(self.snapshot.as_ref(), &snap);
                    if self.history.len() == HISTORY {
                        self.history.pop_front();
                    }
                    self.history.push_back(point);
                    if let Some(s) = &snap.statements {
                        self.statements = s.clone();
                    }
                    self.snapshot = Some(snap);
                }
                Reply::Snapshot(Err(e)) => {
                    self.refreshing = false;
                    self.error = Some(e);
                }
                Reply::Action(action, result) => {
                    let pid = action.pid();
                    let text = match (action, result) {
                        (Action::Cancel(_), Ok(true)) => {
                            trf!("The query of process {pid} was canceled.", pid = pid)
                        }
                        (Action::Kill(_), Ok(true)) => {
                            trf!("Process {pid} was killed.", pid = pid)
                        }
                        (_, Ok(false)) => trf!("Process {pid} was not found.", pid = pid),
                        (_, Err(e)) => trf!("Process {pid}: {error}", pid = pid, error = e),
                    };
                    log.push(trf!("Activity Monitor: {text}", text = text));
                    self.message = Some(text);
                    // Mostra logo o efeito.
                    self.last_refresh = None;
                }
            }
        }
        if !self.paused && !self.refreshing {
            let interval = Duration::from_secs(self.interval);
            match self.last_refresh {
                Some(t) if t.elapsed() < interval => {
                    ctx.request_repaint_after(interval - t.elapsed());
                }
                _ => self.refresh(ctx),
            }
        }
        log
    }

    /// Sessões que passam pelo filtro, já ordenadas.
    pub fn visible_processes(&self) -> Vec<&Process> {
        let Some(snap) = &self.snapshot else {
            return Vec::new();
        };
        self.visible_indices()
            .into_iter()
            .map(|i| &snap.processes[i])
            .collect()
    }

    /// Índices das sessões que passam pelo filtro, na ordem da tabela.
    fn visible_indices(&self) -> Vec<usize> {
        let Some(snap) = &self.snapshot else {
            return Vec::new();
        };
        let needle = self.filter.to_lowercase();
        let mut rows: Vec<(usize, &Process)> = snap
            .processes
            .iter()
            .enumerate()
            .filter(|(_, p)| self.show_system || p.is_client())
            .filter(|(_, p)| {
                needle.is_empty()
                    || [
                        &p.pid.to_string(),
                        &p.user,
                        &p.database,
                        &p.application,
                        &p.client,
                        &p.state,
                        &p.wait_event,
                        &p.query,
                    ]
                    .iter()
                    .any(|f| f.to_lowercase().contains(&needle))
            })
            .collect();
        let (column, ascending) = self.sort;
        rows.sort_by(|(_, a), (_, b)| {
            let ord = match column {
                Column::Pid => a.pid.cmp(&b.pid),
                Column::User => a.user.cmp(&b.user),
                Column::Database => a.database.cmp(&b.database),
                Column::Application => a.application.cmp(&b.application),
                Column::Client => a.client.cmp(&b.client),
                Column::State => a.state.cmp(&b.state),
                Column::WaitType => a.wait_type.cmp(&b.wait_type),
                Column::WaitEvent => a.wait_event.cmp(&b.wait_event),
                Column::BlockedBy => a.blocked_by.cmp(&b.blocked_by),
                Column::Duration => a
                    .query_seconds
                    .unwrap_or(-1.0)
                    .total_cmp(&b.query_seconds.unwrap_or(-1.0)),
                Column::Query => a.query.cmp(&b.query),
            };
            if ascending { ord } else { ord.reverse() }
        });
        rows.into_iter().map(|(i, _)| i).collect()
    }

    /// Refaz [`Derived`] se algo mudou desde o último frame.
    fn derive(&mut self) {
        let Some(snap) = &self.snapshot else {
            return;
        };
        let key = (snap.at, self.filter.clone(), self.sort, self.show_system);
        if self.derived.key.as_ref() == Some(&key) {
            return;
        }
        let total = snap
            .processes
            .iter()
            .filter(|p| self.show_system || p.is_client())
            .count();
        let by_pid: std::collections::HashMap<i32, &Process> =
            snap.processes.iter().map(|p| (p.pid, p)).collect();
        let chains = blocking_chains(&snap.processes)
            .into_iter()
            .map(|(depth, pid)| {
                let what = by_pid
                    .get(&pid)
                    .map(|p| {
                        format!(
                            "{} {} ({}{}) {}",
                            p.application,
                            p.user,
                            p.state,
                            if p.wait_event.is_empty() {
                                String::new()
                            } else {
                                format!(", {}", p.wait_event)
                            },
                            one_line(&p.query)
                        )
                    })
                    .unwrap_or_default();
                let text = if depth == 0 {
                    format!(
                        "{} {}",
                        icon::LOCK,
                        trf!("Head blocker {pid}: {what}", pid = pid, what = what)
                    )
                } else {
                    format!(
                        "{} {}",
                        icon::HOURGLASS,
                        trf!("{pid} waits: {what}", pid = pid, what = what)
                    )
                };
                (depth, pid, text)
            })
            .collect();
        self.derived = Derived {
            key: Some(key),
            visible: self.visible_indices(),
            total,
            chains,
        };
    }

    /// Desenha o painel.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);
        ui.separator();
        if self.snapshot.is_none() {
            // As seções abrem conforme a primeira leitura (Locks, por
            // exemplo, já aberta se houver bloqueio).
            if self.error.is_none() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Reading the server activity..."));
                });
            }
            self.confirm_window(ui.ctx());
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("activity_monitor")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::CollapsingHeader::new(tr("Overview"))
                    .default_open(true)
                    .show(ui, |ui| self.overview(ui));
                egui::CollapsingHeader::new(tr("Processes"))
                    .default_open(true)
                    .show(ui, |ui| self.processes(ui));
                // Começa aberta quando já há alguém bloqueado.
                let blocked = self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.processes.iter().any(|p| !p.blocked_by.is_empty()));
                egui::CollapsingHeader::new(tr("Locks"))
                    .default_open(blocked)
                    .show(ui, |ui| self.locks(ui));
                egui::CollapsingHeader::new(tr("Resource Waits"))
                    .default_open(false)
                    .show(ui, |ui| self.waits(ui));
                egui::CollapsingHeader::new(tr("Data File I/O"))
                    .default_open(false)
                    .show(ui, |ui| self.data_io(ui));
                if self.snapshot.as_ref().is_some_and(|s| s.has_statements) {
                    egui::CollapsingHeader::new(tr("Recent Expensive Queries"))
                        .default_open(false)
                        .show(ui, |ui| self.expensive(ui));
                }
            });
        self.confirm_window(ui.ctx());
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(format!("{} {}", icon::ARROWS_CLOCKWISE, tr("Refresh")))
                .clicked()
            {
                self.refresh(ui.ctx());
            }
            ui.checkbox(&mut self.paused, tr("Pause"));
            ui.label(tr("Refresh interval:"));
            egui::ComboBox::from_id_salt("activity_interval")
                .selected_text(format!("{} s", self.interval))
                .show_ui(ui, |ui| {
                    for &s in INTERVALS {
                        ui.selectable_value(&mut self.interval, s, format!("{s} s"));
                    }
                });
            if self.refreshing {
                ui.spinner();
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            } else if let Some(m) = &self.message {
                ui.label(m);
            }
        });
    }

    fn overview(&self, ui: &mut egui::Ui) {
        let last = self.history.back().copied().unwrap_or_default();
        type Chart = (&'static str, String, fn(&Sample) -> f64);
        let charts: [Chart; 4] = [
            (
                tr_noop("Active Sessions"),
                format!("{:.0}", last.active),
                |s| s.active,
            ),
            (
                tr_noop("Waiting Sessions"),
                format!("{:.0}", last.waiting),
                |s| s.waiting,
            ),
            (
                tr_noop("Transactions/sec"),
                format!("{:.1}", last.transactions_per_second),
                |s| s.transactions_per_second,
            ),
            (
                tr_noop("Database I/O (reads/sec)"),
                format!("{:.1}", last.reads_per_second),
                |s| s.reads_per_second,
            ),
        ];
        let width = ((ui.available_width() - 3.0 * 8.0) / 4.0).max(140.0);
        ui.horizontal_wrapped(|ui| {
            for (title, value, get) in charts {
                let values: Vec<f64> = self.history.iter().map(get).collect();
                chart(ui, title, &value, &values, width);
            }
        });
    }

    fn processes(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(tr("Filter:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(tr("pid, user, database, query..."))
                    .desired_width(220.0),
            );
            ui.checkbox(&mut self.show_system, tr("System processes"));
            let selected = self.selected;
            if ui
                .add_enabled(selected.is_some(), egui::Button::new(tr("Cancel Query")))
                .clicked()
                && let Some(pid) = selected
            {
                self.request(Action::Cancel(pid));
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new(tr("Kill Process")))
                .clicked()
                && let Some(pid) = selected
            {
                self.request(Action::Kill(pid));
            }
        });
        self.derive();
        let Some(snap) = &self.snapshot else {
            return;
        };
        let processes = &snap.processes;
        let visible = &self.derived.visible;
        let mut clicked_header = None;
        let mut select = None;
        let mut action = None;
        let sort = self.sort;
        let selected = self.selected;
        let drawn = table(
            ui,
            "activity_processes",
            PROCESS_COLUMNS
                .iter()
                .map(|&(c, t, w)| {
                    let arrow = match sort {
                        (s, true) if s == c => format!(" {}", icon::CARET_UP),
                        (s, false) if s == c => format!(" {}", icon::CARET_DOWN),
                        _ => String::new(),
                    };
                    (format!("{}{arrow}", tr(t)), w)
                })
                .collect(),
            visible.len(),
            320.0,
            &mut |i| clicked_header = Some(i),
            &mut |ui, i| {
                let p = &processes[visible[i]];
                let response = cell(
                    ui,
                    PROCESS_COLUMNS[0].2,
                    egui::Button::selectable(selected == Some(p.pid), p.pid.to_string())
                        .frame_when_inactive(false),
                );
                if response.clicked() {
                    select = Some(p.pid);
                }
                response.context_menu(|ui| {
                    select = Some(p.pid);
                    if ui.button(tr("Cancel Query")).clicked() {
                        action = Some(Action::Cancel(p.pid));
                        ui.close();
                    }
                    if ui.button(tr("Kill Process")).clicked() {
                        action = Some(Action::Kill(p.pid));
                        ui.close();
                    }
                    if ui.button(tr("Copy Command")).clicked() {
                        ui.ctx().copy_text(p.query.clone());
                        ui.close();
                    }
                });
                let duration = p.query_seconds.map(format_seconds).unwrap_or_default();
                let texts = [
                    &p.user,
                    &p.database,
                    &p.application,
                    &p.client,
                    &p.state,
                    &p.wait_type,
                    &p.wait_event,
                    &p.blocked_by,
                    &duration,
                ];
                for (k, text) in texts.into_iter().enumerate() {
                    let label = egui::Label::new(text.as_str()).truncate();
                    cell(ui, PROCESS_COLUMNS[k + 1].2, label);
                }
                let query = one_line(&p.query);
                cell(
                    ui,
                    PROCESS_COLUMNS[10].2,
                    egui::Label::new(egui::RichText::new(query).monospace()).truncate(),
                )
                .on_hover_text(&p.query);
            },
        );
        self.drawn_rows = drawn;
        if let Some(i) = clicked_header {
            let column = PROCESS_COLUMNS[i].0;
            self.sort = if self.sort.0 == column {
                (column, !self.sort.1)
            } else {
                (column, true)
            };
        }
        if let Some(pid) = select {
            self.selected = Some(pid);
        }
        if let Some(a) = action {
            self.request(a);
        }
        let total = self.derived.total;
        ui.weak(trf!(
            "{visible} of {total} sessions",
            visible = self.derived.visible.len(),
            total = total
        ));
    }

    fn locks(&mut self, ui: &mut egui::Ui) {
        self.derive();
        let Some(snap) = &self.snapshot else {
            return;
        };
        let chains = &self.derived.chains;
        if chains.is_empty() {
            ui.weak(tr("No session is blocked."));
        } else {
            ui.strong(tr("Blocking chains"));
            let mut select = None;
            let row_h = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical()
                .id_salt("activity_chains")
                .max_height(160.0)
                .show_rows(ui, row_h, chains.len(), |ui, range| {
                    for (depth, pid, text) in &chains[range] {
                        ui.horizontal(|ui| {
                            ui.add_space(*depth as f32 * 18.0);
                            let r = ui.add(
                                egui::Button::selectable(
                                    self.selected == Some(*pid),
                                    text.as_str(),
                                )
                                .truncate()
                                .frame_when_inactive(false),
                            );
                            if r.clicked() {
                                select = Some(*pid);
                            }
                        });
                    }
                });
            if let Some(pid) = select {
                self.selected = Some(pid);
            }
            ui.add_space(4.0);
        }
        let locks = &snap.locks;
        table(
            ui,
            "activity_locks",
            vec![
                (tr("Process ID").into(), 78.0),
                (tr("Lock Type").into(), 100.0),
                (tr("Mode").into(), 150.0),
                (tr("Status").into(), 80.0),
                (tr("Object").into(), 180.0),
                (tr("Transaction").into(), 100.0),
                (tr("Database").into(), 90.0),
            ],
            locks.len(),
            220.0,
            &mut |_| {},
            &mut |ui, i| {
                let l = &locks[i];
                cell(ui, 78.0, egui::Label::new(l.pid.to_string()));
                cell(ui, 100.0, egui::Label::new(l.lock_type.as_str()));
                cell(ui, 150.0, egui::Label::new(l.mode.as_str()).truncate());
                let status = if l.granted {
                    egui::RichText::new(tr("Granted"))
                } else {
                    egui::RichText::new(tr("Waiting")).color(ui.visuals().warn_fg_color)
                };
                cell(ui, 80.0, egui::Label::new(status));
                cell(ui, 180.0, egui::Label::new(l.relation.as_str()).truncate());
                cell(ui, 100.0, egui::Label::new(l.target.as_str()).truncate());
                cell(ui, 90.0, egui::Label::new(l.database.as_str()).truncate());
            },
        );
    }

    fn waits(&self, ui: &mut egui::Ui) {
        let waits = self
            .snapshot
            .as_ref()
            .map(|s| resource_waits(&s.processes))
            .unwrap_or_default();
        if waits.is_empty() {
            ui.weak(tr("No session is waiting."));
            return;
        }
        table(
            ui,
            "activity_waits",
            vec![
                (tr("Wait Category").into(), 140.0),
                (tr("Wait Event").into(), 200.0),
                (tr("Waiting Sessions").into(), 120.0),
            ],
            waits.len(),
            200.0,
            &mut |_| {},
            &mut |ui, i| {
                let (t, e, n) = &waits[i];
                cell(ui, 140.0, egui::Label::new(t.as_str()).truncate());
                cell(ui, 200.0, egui::Label::new(e.as_str()).truncate());
                cell(ui, 120.0, egui::Label::new(n.to_string()));
            },
        );
    }

    fn data_io(&self, ui: &mut egui::Ui) {
        let Some(snap) = &self.snapshot else {
            return;
        };
        table(
            ui,
            "activity_io",
            vec![
                (tr("Database").into(), 160.0),
                (tr("Blocks Read").into(), 110.0),
                (tr("Blocks Hit").into(), 110.0),
                (tr("Cache Hit %").into(), 90.0),
                (tr("Temp Files (MB)").into(), 110.0),
                (tr("Transactions").into(), 110.0),
            ],
            snap.databases.len(),
            200.0,
            &mut |_| {},
            &mut |ui, i| {
                let d = &snap.databases[i];
                let total = d.blocks_read + d.blocks_hit;
                let hit = if total > 0.0 {
                    format!("{:.1}", d.blocks_hit / total * 100.0)
                } else {
                    String::new()
                };
                cell(ui, 160.0, egui::Label::new(d.name.as_str()).truncate());
                cell(ui, 110.0, egui::Label::new(format!("{:.0}", d.blocks_read)));
                cell(ui, 110.0, egui::Label::new(format!("{:.0}", d.blocks_hit)));
                cell(ui, 90.0, egui::Label::new(hit));
                cell(
                    ui,
                    110.0,
                    egui::Label::new(format!("{:.1}", d.temp_bytes / 1_048_576.0)),
                );
                cell(
                    ui,
                    110.0,
                    egui::Label::new(format!("{:.0}", d.transactions)),
                );
            },
        );
    }

    fn expensive(&self, ui: &mut egui::Ui) {
        let rows = &self.statements;
        table(
            ui,
            "activity_statements",
            vec![
                (tr("Executions").into(), 90.0),
                (tr("Total Time (ms)").into(), 120.0),
                (tr("Average Time (ms)").into(), 120.0),
                (tr("Rows").into(), 90.0),
                (tr("Query").into(), 420.0),
            ],
            rows.len(),
            240.0,
            &mut |_| {},
            &mut |ui, i| {
                let s = &rows[i];
                cell(ui, 90.0, egui::Label::new(format!("{:.0}", s.calls)));
                cell(ui, 120.0, egui::Label::new(format!("{:.1}", s.total_ms)));
                cell(ui, 120.0, egui::Label::new(format!("{:.2}", s.mean_ms)));
                cell(ui, 90.0, egui::Label::new(format!("{:.0}", s.rows)));
                cell(
                    ui,
                    420.0,
                    egui::Label::new(egui::RichText::new(one_line(&s.query)).monospace())
                        .truncate(),
                )
                .on_hover_text(&s.query);
            },
        );
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(action) = self.confirm else {
            return;
        };
        let (title, text) = match action {
            Action::Cancel(pid) => (
                tr("Cancel Query"),
                trf!(
                    "Are you sure you want to cancel the query running in process {pid}?",
                    pid = pid
                ),
            ),
            Action::Kill(pid) => (
                tr("Kill Process"),
                trf!(
                    "Are you sure you want to kill process {pid}? \
                     Its open transaction, if any, will be rolled back.",
                    pid = pid
                ),
            ),
        };
        let mut answer = None;
        let modal = crate::theme::modal(egui::Id::new("activity_confirm")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(title);
            ui.label(text);
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
        if let Some(yes) = answer {
            self.answer(yes, ctx);
        }
    }
}

/// Uma célula de largura fixa.
fn cell(ui: &mut egui::Ui, width: f32, widget: impl egui::Widget) -> egui::Response {
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            ui.add(widget)
        },
    )
    .inner
}

/// Tabela com cabeçalho clicável e só as linhas visíveis desenhadas.
/// Devolve quantas linhas foram desenhadas.
#[allow(clippy::too_many_arguments)]
fn table(
    ui: &mut egui::Ui,
    id: &str,
    headers: Vec<(String, f32)>,
    rows: usize,
    height: f32,
    on_header: &mut dyn FnMut(usize),
    row: &mut dyn FnMut(&mut egui::Ui, usize),
) -> usize {
    let mut drawn = 0;
    egui::ScrollArea::horizontal()
        .id_salt((id, "h"))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    for (i, (text, width)) in headers.iter().enumerate() {
                        let b = egui::Button::new(egui::RichText::new(text).strong())
                            .frame_when_inactive(false);
                        if cell(ui, *width, b).clicked() {
                            on_header(i);
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt((id, "v"))
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
                        for i in range {
                            drawn += 1;
                            ui.horizontal(|ui| row(ui, i));
                        }
                    });
            });
        });
    drawn
}

/// Um gráfico de linha pequeno, como os do Overview do SSMS.
fn chart(ui: &mut egui::Ui, title: &str, value: &str, values: &[f64], width: f32) {
    let title = tr(title);
    let height = 110.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let label = format!("{title}: {value}");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &label));
    let v = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, v.extreme_bg_color);
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let text = v.text_color();
    painter.text(
        rect.left_top() + vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(12.0),
        text,
    );
    painter.text(
        rect.right_top() + vec2(-6.0, 4.0),
        egui::Align2::RIGHT_TOP,
        value,
        egui::FontId::proportional(12.0),
        text,
    );
    let plot = egui::Rect::from_min_max(
        rect.left_top() + vec2(6.0, 22.0),
        rect.right_bottom() - vec2(6.0, 6.0),
    );
    for k in 1..4 {
        let y = plot.top() + plot.height() * k as f32 / 4.0;
        painter.hline(
            plot.x_range(),
            y,
            Stroke::new(
                1.0,
                v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.5),
            ),
        );
    }
    if values.is_empty() {
        return;
    }
    let max = values.iter().copied().fold(1.0_f64, f64::max) * 1.1;
    let step = plot.width() / (HISTORY - 1) as f32;
    let start = HISTORY - values.len();
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, &y)| {
            egui::pos2(
                plot.left() + (start + i) as f32 * step,
                plot.bottom() - (y / max) as f32 * plot.height(),
            )
        })
        .collect();
    let color = Color32::from_rgb(0x2e, 0x86, 0xde);
    if points.len() == 1 {
        painter.circle_filled(points[0], 2.0, color);
    } else {
        painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
    }
}

/// A primeira linha (sem quebras) de um texto.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `hh:mm:ss`.
pub fn format_seconds(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(values: &[&str]) -> Vec<Option<String>> {
        values
            .iter()
            .map(|v| (!v.is_empty()).then(|| v.to_string()))
            .collect()
    }

    fn process(pid: &str, state: &str, wait: &str, event: &str) -> Vec<Option<String>> {
        row(&[
            pid,
            "app",
            "loja",
            "psql",
            "127.0.0.1",
            state,
            wait,
            event,
            "",
            "1.5",
            "0.5",
            "client backend",
            "select 1",
        ])
    }

    #[test]
    fn snapshots_are_read_from_the_results() {
        let results = vec![
            vec![
                process("10", "active", "Lock", "transactionid"),
                process("11", "idle", "Client", "ClientRead"),
            ],
            vec![row(&["loja", "100", "20", "80", "0"])],
            vec![row(&[
                "10",
                "relation",
                "AccessShareLock",
                "f",
                "t",
                "",
                "loja",
            ])],
            vec![row(&["0"])],
        ];
        let s = parse_snapshot(&results, Instant::now()).unwrap();
        assert_eq!(s.processes.len(), 2);
        assert_eq!(s.processes[0].pid, 10);
        assert_eq!(s.processes[0].query_seconds, Some(0.5));
        assert!(s.processes[0].is_waiting());
        assert!(!s.processes[1].is_waiting());
        assert_eq!(s.databases[0].blocks_hit, 80.0);
        assert!(!s.has_statements);
        assert!(s.statements.is_none());
        assert!(!s.locks[0].granted);
        assert_eq!(s.locks[0].relation, "t");
        assert!(parse_snapshot(&results[..3], Instant::now()).is_err());
    }

    #[test]
    fn samples_are_rates_between_snapshots() {
        let at = Instant::now();
        let snap = |at, xacts: &str, reads: &str| {
            parse_snapshot(
                &[
                    vec![process("1", "active", "", "")],
                    vec![row(&["a", xacts, reads, "0", "0"])],
                    vec![],
                    vec![row(&["1"])],
                ],
                at,
            )
            .unwrap()
        };
        let a = snap(at, "100", "10");
        let b = snap(at + Duration::from_secs(2), "140", "30");
        let first = sample(None, &a);
        assert_eq!(first.active, 1.0);
        assert_eq!(first.transactions_per_second, 0.0);
        let s = sample(Some(&a), &b);
        assert_eq!(s.transactions_per_second, 20.0);
        assert_eq!(s.reads_per_second, 10.0);
        assert!(b.has_statements);
        assert!(refresh_sql(true).contains("pg_stat_statements ORDER BY"));
        assert!(!refresh_sql(false).contains("pg_stat_statements ORDER BY"));
    }

    #[test]
    fn waits_are_grouped() {
        let p = |t: &str, e: &str| Process {
            state: "active".into(),
            wait_type: t.into(),
            wait_event: e.into(),
            ..Default::default()
        };
        let waits = resource_waits(&[
            p("Lock", "relation"),
            p("Lock", "relation"),
            p("IO", "DataFileRead"),
            p("", ""),
        ]);
        assert_eq!(
            waits,
            vec![
                ("Lock".into(), "relation".into(), 2),
                ("IO".into(), "DataFileRead".into(), 1)
            ]
        );
    }

    #[test]
    fn blocking_chains_start_at_the_head_blocker() {
        let p = |pid: i32, by: &str| Process {
            pid,
            blocked_by: by.into(),
            ..Default::default()
        };
        // 1 bloqueia 2 e 3; 3 bloqueia 4; 5 está livre.
        let chains = blocking_chains(&[p(1, ""), p(2, "1"), p(3, "1"), p(4, "3"), p(5, "")]);
        assert_eq!(chains, vec![(0, 1), (1, 2), (1, 3), (2, 4)]);
        assert!(blocking_chains(&[p(1, ""), p(2, "")]).is_empty());
        // Ciclo: ninguém é cabeça, mas a cadeia aparece sem repetir.
        assert_eq!(
            blocking_chains(&[p(7, "8"), p(8, "7")]),
            vec![(0, 7), (1, 8)]
        );
    }

    #[test]
    fn seconds_are_formatted() {
        assert_eq!(format_seconds(3725.4), "01:02:05");
        assert_eq!(format_seconds(-1.0), "00:00:00");
    }
}
