//! Executa uma consulta entre servidores.
//!
//! Cada fonte do plano ([`crate::plan`]) roda no servidor dela, várias ao
//! mesmo tempo, e as linhas vão direto para uma tabela de um SQLite
//! temporário em disco (some ao fechar). Depois o SQL do usuário roda nesse
//! SQLite, que faz os joins, e o resultado sai como o de qualquer consulta.
//!
//! Nada escreve nos servidores: o texto de cada fonte passa por
//! [`Remote::read_only`] antes de rodar, e o SQL local só pode ler (um
//! autorizador do SQLite recusa o resto).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use pgstudio_core::connection::Engine;
use pgstudio_core::query::{QueryEvent, QuerySession, QuerySink, ServerMessage};
use pgstudio_i18n::{tr, trf};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

use crate::plan::{self, Home, Plan, Server, Source};
use crate::pushdown::{self, Analysis, Column, Link, Need, Nulls, Read, SourcePlan};

/// De onde saem as sessões dos servidores.
pub trait Remote: Send + Sync {
    /// Uma sessão no banco `database` do servidor `server` (índice na lista
    /// de servidores). Pode conectar: é chamado fora da thread da interface.
    fn session(&self, server: usize, database: &str) -> Result<Arc<dyn QuerySession>, String>;

    /// O texto a mandar ao servidor para rodar `query` só lendo (o mesmo,
    /// ou embrulhado numa transação só de leitura), ou `Err` com o motivo
    /// quando `query` pode alterar alguma coisa. A consulta entre
    /// servidores só lê.
    fn read_only(&self, engine: Engine, query: &str) -> Result<String, String>;
}

/// Limites de uma execução.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Linhas lidas de cada fonte; passou disso, a fonte é cortada e uma
    /// mensagem avisa.
    pub max_rows: u64,
    /// Linhas de uma tabela inteira que a consulta baixa sem perguntar;
    /// acima disso ela para e avisa (o comentário `-- baixar tudo` libera
    /// até `max_rows`).
    pub guard_rows: u64,
    /// Chaves de uma tabela usadas para ler só as linhas ligadas de outra.
    pub max_keys: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_rows: 1_000_000,
            guard_rows: 200_000,
            max_keys: 50_000,
        }
    }
}

/// Fontes lidas ao mesmo tempo.
const MAX_PARALLEL: usize = 8;

/// Linhas esperando para entrar no SQLite; cheio, o servidor espera.
const QUEUE: usize = 4096;

/// Quanto esperar por uma pergunta pequena a um servidor (o tamanho de
/// uma tabela, os tipos das colunas).
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

/// De quantas em quantas linhas uma fonte avisa o andamento.
const PROGRESS: u64 = 100_000;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A sessão de uma janela de consulta entre servidores.
pub struct FederatedSession {
    servers: Vec<Server>,
    home: Option<Home>,
    remote: Arc<dyn Remote>,
    options: Options,
    running: Arc<Running>,
}

#[derive(Default)]
struct Running {
    canceled: AtomicBool,
    /// Sessões das fontes em andamento, pelo índice da fonte.
    sessions: Mutex<HashMap<usize, Arc<dyn QuerySession>>>,
    /// Para interromper o SQL local.
    local: Mutex<Option<rusqlite::InterruptHandle>>,
}

impl FederatedSession {
    pub fn new(servers: Vec<Server>, remote: Arc<dyn Remote>, options: Options) -> Self {
        FederatedSession {
            servers,
            home: None,
            remote,
            options,
            running: Arc::default(),
        }
    }

    /// O servidor da janela: as tabelas escritas sem servidor são dele.
    pub fn with_home(mut self, home: Option<Home>) -> Self {
        self.home = home;
        self
    }
}

impl QuerySession for FederatedSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        self.running.canceled.store(false, Ordering::SeqCst);
        let run = Run {
            servers: self.servers.clone(),
            home: self.home.clone(),
            remote: self.remote.clone(),
            options: self.options,
            running: self.running.clone(),
        };
        std::thread::spawn(move || {
            if let Err(error) = run.execute(&sql, &mut sink) {
                sink(QueryEvent::Error(*error));
            }
            lock(&run.running.sessions).clear();
            *lock(&run.running.local) = None;
            sink(QueryEvent::Finished {
                canceled: run.running.canceled.load(Ordering::SeqCst),
            });
        });
    }

    fn cancel(&self) {
        self.running.canceled.store(true, Ordering::SeqCst);
        for s in lock(&self.running.sessions).values() {
            s.cancel();
        }
        if let Some(handle) = lock(&self.running.local).as_ref() {
            handle.interrupt();
        }
    }

    fn database(&self) -> String {
        tr("(several servers)").to_string()
    }
}

/// O que uma fonte manda para quem grava no SQLite.
enum Message {
    /// A fonte começou a ser lida.
    Started,
    Columns(Vec<String>),
    Row(Vec<Option<String>>),
    /// Fim de um resultado.
    Complete,
    Failed(String),
    Finished,
}

/// O que cada coluna de uma fonte guardou até agora.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Só NULL.
    Empty,
    Integer,
    Real,
    Text,
}

/// Como uma fonte está sendo lida.
struct Loading {
    started: Instant,
    /// Colunas da tabela local (os nomes do servidor, sem repetir).
    columns: Vec<String>,
    shapes: Vec<Shape>,
    insert: String,
    rows: u64,
    /// O primeiro resultado já fechou; o resto do lote é ignorado.
    closed: bool,
    extra_results: bool,
    truncated: bool,
    /// Passou do que a consulta baixa sem perguntar.
    over: bool,
    /// Os resultados seguintes com as mesmas colunas somam.
    append: bool,
    error: Option<String>,
    finished: bool,
    elapsed: Duration,
}

impl Default for Loading {
    fn default() -> Self {
        Loading::new(false)
    }
}

impl Loading {
    fn new(append: bool) -> Self {
        Loading {
            started: Instant::now(),
            columns: Vec::new(),
            shapes: Vec::new(),
            insert: String::new(),
            rows: 0,
            closed: false,
            extra_results: false,
            truncated: false,
            over: false,
            append,
            error: None,
            finished: false,
            elapsed: Duration::ZERO,
        }
    }
}

struct Run {
    servers: Vec<Server>,
    home: Option<Home>,
    remote: Arc<dyn Remote>,
    options: Options,
    running: Arc<Running>,
}

fn notice(text: String) -> QueryEvent {
    QueryEvent::Notice(ServerMessage {
        message: text,
        ..Default::default()
    })
}

fn describe(m: &ServerMessage) -> String {
    let mut text = m.message.clone();
    for extra in [&m.detail, &m.hint].into_iter().flatten() {
        text.push(' ');
        text.push_str(extra);
    }
    text
}

fn local_error(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        rusqlite::Error::SqlInputError { msg, .. } => msg.clone(),
        other => other.to_string(),
    }
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Os nomes das colunas de uma fonte como colunas de uma tabela: sem
/// vazio e sem repetir (o SQLite não diferencia maiúsculas).
fn column_names(columns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, name) in columns.iter().enumerate() {
        let base = if name.trim().is_empty() {
            format!("col{}", i + 1)
        } else {
            name.clone()
        };
        let mut candidate = base.clone();
        let mut n = 2;
        while out.iter().any(|c| c.eq_ignore_ascii_case(&candidate)) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        out.push(candidate);
    }
    out
}

/// Se `text` é um número inteiro escrito do jeito comum (sem zeros na
/// frente, que são de códigos como CEP e CPF, e cabendo em 64 bits).
fn is_integer(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty()
        && digits.len() <= 18
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
}

/// Se `text` é um número com casas decimais que um `REAL` guarda sem
/// perder dígitos (até 15 significativos).
fn is_real(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let Some((whole, fraction)) = digits.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.len() + fraction.len() <= 15
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && (whole == "0" || !whole.starts_with('0'))
}

fn shape_of(text: &str) -> Shape {
    if is_integer(text) {
        Shape::Integer
    } else if is_real(text) {
        Shape::Real
    } else {
        Shape::Text
    }
}

fn merge(a: Shape, b: Shape) -> Shape {
    match (a, b) {
        (Shape::Empty, x) | (x, Shape::Empty) => x,
        (Shape::Integer, Shape::Integer) => Shape::Integer,
        (Shape::Integer | Shape::Real, Shape::Integer | Shape::Real) => Shape::Real,
        _ => Shape::Text,
    }
}

/// O valor de uma célula do resultado local como texto.
fn cell(value: rusqlite::types::ValueRef<'_>) -> Option<String> {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(n) => Some(n.to_string()),
        ValueRef::Real(x) => Some(x.to_string()),
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Some(b.iter().map(|x| format!("{x:02x}")).collect()),
    }
}

fn seconds(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

fn rows_text(rows: u64) -> String {
    if rows == 1 {
        tr("1 row").to_string()
    } else {
        trf!("{rows} rows", rows = rows)
    }
}

/// Como uma tentativa lê as fontes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Só as primeiras linhas da tabela da ordem; o número é quantas.
    Top(u64),
    /// Colunas, filtros e chaves nos servidores.
    Optimized,
    /// As fontes como estão escritas.
    Plain,
}

/// Por que uma tentativa parou.
enum Stop {
    Canceled,
    /// Viria linha demais: a consulta para antes de baixar.
    Guard(String),
    /// Um servidor recusou ou falhou.
    Failed(String),
    /// A fonte escrita pelo usuário não é só leitura.
    Refused(String),
    Local(rusqlite::Error),
}

impl From<rusqlite::Error> for Stop {
    fn from(e: rusqlite::Error) -> Self {
        Stop::Local(e)
    }
}

/// O fim de uma tentativa.
enum Outcome {
    Done(rusqlite::Connection, Vec<Loading>),
    /// As primeiras linhas não bastaram: tentar com mais.
    More,
    /// "Os N primeiros" não serve para esta consulta.
    NoTop,
}

/// Uma leitura a fazer num servidor.
struct Job {
    /// A fonte (índice em `Plan::sources`).
    index: usize,
    /// O texto mandado ao servidor.
    query: String,
    /// O mesmo, curto, para as mensagens.
    shown: String,
    /// O texto tem vários SELECTs iguais (um por lote de chaves): os
    /// resultados somam.
    append: bool,
    /// Parar a consulta se passar de `Options::guard_rows` linhas.
    guard: bool,
}

/// O que ler de uma tabela.
struct Spec<'a> {
    columns: Option<&'a [Column]>,
    filters: Vec<String>,
    keys: Option<(&'a Link, Vec<String>)>,
    order: &'a [(Column, bool)],
    nulls: Nulls,
    limit: Option<u64>,
}

/// Tipo e "não aceita vazio" de cada coluna (nome minúsculo) de uma tabela.
type Types = HashMap<String, (String, bool)>;

/// O que já se perguntou aos servidores nesta execução.
#[derive(Default)]
struct Probes {
    /// Linhas estimadas, pelo servidor, banco e SELECT.
    estimates: HashMap<(usize, String, String), Option<u64>>,
    /// Tipo e "não aceita vazio" das colunas de uma fonte (nome minúsculo).
    types: HashMap<usize, Option<Types>>,
}

/// O que o tipo de uma coluna permite.
struct Kind {
    integer: bool,
    number: bool,
    /// `Some(true)`: só data; `Some(false)`: data e hora sem fuso.
    date: Option<bool>,
}

fn kind(engine: Engine, ty: &str) -> Kind {
    let ty = ty.to_lowercase();
    let base = ty.split('(').next().unwrap_or("").trim();
    match engine {
        Engine::PostgreSql => {
            let integer = matches!(base, "smallint" | "integer" | "bigint");
            Kind {
                integer,
                number: integer || matches!(base, "numeric" | "real" | "double precision"),
                date: if base == "date" {
                    Some(true)
                } else if base.starts_with("timestamp") && ty.ends_with("without time zone") {
                    Some(false)
                } else {
                    None
                },
            }
        }
        _ => {
            let integer = matches!(base, "tinyint" | "smallint" | "int" | "bigint");
            Kind {
                integer,
                number: integer
                    || matches!(
                        base,
                        "decimal" | "numeric" | "money" | "smallmoney" | "float" | "real"
                    ),
                date: None,
            }
        }
    }
}

fn shorten(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 400 {
        let cut: String = one_line.chars().take(400).collect();
        format!("{cut}…")
    } else {
        one_line
    }
}

/// O usuário pediu para baixar tudo (`-- baixar tudo` na consulta).
fn wants_everything(sql: &str) -> bool {
    sql.lines().any(|line| {
        line.find("--").is_some_and(|at| {
            let comment = line[at..].to_lowercase();
            comment.contains("baixar tudo") || comment.contains("download all")
        })
    })
}

/// Põe a leitura na onda; uma parada por linhas demais fica anotada para
/// sair junto com as das outras fontes.
fn push(stops: &mut Vec<String>, job: Result<Job, Stop>, wave: &mut Vec<Job>) -> Result<(), Stop> {
    match job {
        Ok(job) => wave.push(job),
        Err(Stop::Guard(message)) => stops.push(message),
        Err(other) => return Err(other),
    }
    Ok(())
}

/// Quantas das primeiras linhas ler, a cada tentativa.
fn ladder(need: u64, guard: u64) -> Vec<u64> {
    if need == 0 || need > guard / 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut k = need.saturating_mul(2).max(1000);
    while k < guard {
        out.push(k);
        k = k.saturating_mul(20);
    }
    out.push(guard);
    out
}

impl Run {
    fn canceled(&self) -> bool {
        self.running.canceled.load(Ordering::SeqCst)
    }

    fn execute(&self, sql: &str, sink: &mut QuerySink) -> Result<(), Box<ServerMessage>> {
        let plan =
            plan::plan_with(sql, &self.servers, self.home.as_ref()).map_err(|e| ServerMessage {
                position: Some(e.position),
                ..ServerMessage::error(e.message)
            })?;
        if plan.statements.is_empty() {
            return Ok(());
        }
        let failed = |e: rusqlite::Error| {
            ServerMessage::error(trf!(
                "Could not use the local database for the join: {error}",
                error = local_error(&e)
            ))
        };
        let all = wants_everything(sql);
        let analysis = pushdown::analyze(&plan, &self.servers);
        let mut modes = Vec::new();
        if analysis.any() {
            if let Some(top) = &analysis.top {
                modes.extend(
                    ladder(top.need, self.options.guard_rows)
                        .into_iter()
                        .map(Mode::Top),
                );
            }
            modes.push(Mode::Optimized);
        }
        modes.push(Mode::Plain);
        let mut probes = Probes::default();
        let mut done = None;
        let mut at = 0;
        while let Some(mode) = modes.get(at).copied() {
            at += 1;
            match self.attempt(&plan, &analysis, mode, all, &mut probes, sink) {
                Ok(Outcome::Done(db, loaded)) => {
                    done = Some((db, loaded));
                    break;
                }
                Ok(Outcome::More) => {
                    if let (Mode::Top(k), Some(top)) = (mode, &analysis.top) {
                        sink(notice(trf!(
                            "[{source}] The first {rows} rows were not enough for the {need} asked; reading more.",
                            source = plan.sources[top.source].label,
                            rows = thousands(k),
                            need = top.need
                        )));
                    }
                }
                Ok(Outcome::NoTop) => {
                    while matches!(modes.get(at), Some(Mode::Top(_))) {
                        at += 1;
                    }
                }
                Err(Stop::Canceled) => return Ok(()),
                Err(Stop::Guard(message)) | Err(Stop::Refused(message)) => {
                    return Err(ServerMessage::error(message).into());
                }
                Err(Stop::Failed(message)) if mode != Mode::Plain => {
                    // O servidor não aceitou o SELECT montado aqui (um
                    // tipo que não compara, uma coluna que não é dele):
                    // vale a leitura simples.
                    sink(notice(trf!(
                        "Reading only what the query uses did not work ({error}). Trying again with the tables as they are.",
                        error = message
                    )));
                    at = modes.len() - 1;
                }
                Err(Stop::Failed(message)) => return Err(ServerMessage::error(message).into()),
                Err(Stop::Local(e)) => return Err(failed(e).into()),
            }
            if self.canceled() {
                return Ok(());
            }
        }
        let Some((mut db, loaded)) = done else {
            return Ok(());
        };
        for (source, l) in plan.sources.iter().zip(&loaded) {
            sink(notice(format!(
                "[{}] {} ({})",
                source.label,
                rows_text(l.rows),
                seconds(l.elapsed)
            )));
            if l.truncated {
                sink(notice(trf!(
                    "[{source}] WARNING: only the first {rows} rows were read. Filter on the server with server($$ query $$) to read less.",
                    source = source.label,
                    rows = l.rows
                )));
            }
            if l.extra_results {
                sink(notice(trf!(
                    "[{source}] The query returned more than one result; only the first is used.",
                    source = source.label
                )));
            }
        }

        // Daqui em diante o SQL é do usuário: só pode ler as tabelas.
        db.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Select
            | AuthAction::Read { .. }
            | AuthAction::Function { .. }
            | AuthAction::Recursive => Authorization::Allow,
            _ => Authorization::Deny,
        }))
        .map_err(failed)?;
        for statement in &plan.statements {
            if self.canceled() {
                return Ok(());
            }
            let wrong = |e: rusqlite::Error| {
                if self.canceled() {
                    return ServerMessage::error(tr("Canceled.").to_string());
                }
                let position = match &e {
                    rusqlite::Error::SqlInputError { offset, sql, .. } if *offset >= 0 => {
                        position_in_original(sql, *offset as usize, statement.offset)
                    }
                    _ => None,
                };
                let message = match &e {
                    rusqlite::Error::SqliteFailure(f, _)
                        if f.code == rusqlite::ErrorCode::AuthorizationForStatementDenied =>
                    {
                        tr("A cross-server query only reads: write a SELECT.").to_string()
                    }
                    e if local_error(e) == "not authorized" => {
                        tr("A cross-server query only reads: write a SELECT.").to_string()
                    }
                    e => local_error(e),
                };
                ServerMessage {
                    position,
                    ..ServerMessage::error(message)
                }
            };
            let mut stmt = db.prepare(&statement.sql).map_err(wrong)?;
            let width = stmt.column_count();
            if width == 0 {
                return Err(ServerMessage::error(
                    tr("A cross-server query only reads: write a SELECT.").to_string(),
                )
                .into());
            }
            sink(QueryEvent::Columns(
                stmt.column_names().map(|c| c.to_string()).collect(),
            ));
            let mut rows = stmt.query([]).map_err(wrong)?;
            let mut count = 0u64;
            loop {
                match rows.next() {
                    Ok(Some(row)) => {
                        let cells = (0..width)
                            .map(|i| row.get_ref(i).ok().and_then(cell))
                            .collect();
                        sink(QueryEvent::Row(cells));
                        count += 1;
                    }
                    Ok(None) => break,
                    Err(_) if self.canceled() => return Ok(()),
                    Err(e) => return Err(wrong(e).into()),
                }
            }
            sink(QueryEvent::CommandComplete { rows: count });
        }
        Ok(())
    }

    /// O conselho no fim de uma parada por linhas demais.
    fn advice(&self) -> String {
        trf!(
            "Filter the table in the WHERE, join it by a key to a table that is filtered, or write the query for the server with server($$ ... $$). To download it anyway (up to {max} rows per table), add the comment -- baixar tudo to the query.",
            max = thousands(self.options.max_rows)
        )
    }

    /// Uma tentativa: lê as fontes do jeito de `mode`, em ondas (primeiro
    /// as que já vêm pequenas, depois as que são lidas pelas chaves delas).
    fn attempt(
        &self,
        plan: &Plan,
        analysis: &Analysis,
        mode: Mode,
        all: bool,
        probes: &mut Probes,
        sink: &mut QuerySink,
    ) -> Result<Outcome, Stop> {
        // Um banco temporário em disco, só desta tentativa.
        let db = rusqlite::Connection::open("")?;
        db.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")?;
        *lock(&self.running.local) = Some(db.get_interrupt_handle());

        let n = plan.sources.len();
        let mut loaded: Vec<Option<Loading>> = (0..n).map(|_| None).collect();
        let mut wave: Vec<Job> = Vec::new();
        let mut waiting: Vec<usize> = Vec::new();
        let mut stops: Vec<String> = Vec::new();
        // A tabela da ordem foi lida sem as linhas de chave vazia.
        let mut without_nulls = false;
        let mut top_rows = None;
        for i in 0..n {
            let sp = &analysis.sources[i];
            if mode == Mode::Plain || !sp.optimizable {
                push(&mut stops, self.job(plan, i, None, all, probes), &mut wave)?;
                continue;
            }
            let filters = self.filters(plan, i, sp, probes);
            if self.canceled() {
                return Err(Stop::Canceled);
            }
            if let (Mode::Top(k), Some(top)) = (mode, &analysis.top)
                && top.source == i
            {
                let mut spec = Spec {
                    columns: sp.columns.as_deref(),
                    filters,
                    keys: None,
                    order: &top.order,
                    nulls: Nulls::Same,
                    limit: Some(k),
                };
                // Uma linha com a chave do join vazia nunca tem par: sem
                // elas, as primeiras linhas lidas são as que podem servir.
                let engine = self.servers[plan.sources[i].server].engine;
                let mut seen: Vec<&Column> = Vec::new();
                for link in &sp.links {
                    if !seen
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case(&link.column.name))
                    {
                        seen.push(&link.column);
                        spec.filters
                            .push(format!("{} IS NOT NULL", link.column.sql(engine)));
                    }
                }
                if !top.order.is_empty() {
                    let Some(types) = self.types(plan, i, probes) else {
                        return Ok(Outcome::NoTop);
                    };
                    let mut nullable = false;
                    for (c, _) in &top.order {
                        let Some((ty, not_null)) = types.get(&c.name.to_lowercase()) else {
                            return Ok(Outcome::NoTop);
                        };
                        let kind = kind(engine, ty);
                        // Só o que ordena igual aqui e lá.
                        if !kind.integer && kind.date.is_none() {
                            return Ok(Outcome::NoTop);
                        }
                        nullable |= !not_null;
                    }
                    if nullable && engine == Engine::PostgreSql {
                        // Aqui os vazios vêm primeiro na ordem crescente e
                        // por último na decrescente.
                        match top.order.as_slice() {
                            [(c, true)] => {
                                // Sem eles o servidor usa o índice.
                                spec.filters.push(format!("{} IS NOT NULL", c.sql(engine)));
                                without_nulls = true;
                            }
                            _ => spec.nulls = Nulls::Explicit,
                        }
                    }
                }
                top_rows = Some((i, k));
                push(
                    &mut stops,
                    self.job(plan, i, Some(spec), all, probes),
                    &mut wave,
                )?;
                continue;
            }
            if filters.is_empty() && !sp.links.is_empty() {
                waiting.push(i);
                continue;
            }
            let spec = Spec {
                columns: sp.columns.as_deref(),
                filters,
                keys: None,
                order: &[],
                nulls: Nulls::Same,
                limit: None,
            };
            push(
                &mut stops,
                self.job(plan, i, Some(spec), all, probes),
                &mut wave,
            )?;
        }
        loop {
            if self.canceled() {
                return Err(Stop::Canceled);
            }
            if !stops.is_empty() {
                stops.push(self.advice());
                return Err(Stop::Guard(stops.join("\n")));
            }
            self.load(plan, &db, &wave, &mut loaded, sink)?;
            if waiting.is_empty() {
                break;
            }
            wave = Vec::new();
            let mut still = Vec::new();
            let mut alone = Vec::new();
            for i in std::mem::take(&mut waiting) {
                let sp = &analysis.sources[i];
                let mut found = None;
                let mut parent_waiting = false;
                for link in &sp.links {
                    match &loaded[link.parent] {
                        Some(parent) => {
                            if let Some(keys) =
                                self.keys(&db, &plan.sources[link.parent], link, parent)?
                            {
                                found = Some((link, keys));
                                break;
                            }
                        }
                        None => parent_waiting = true,
                    }
                }
                match found {
                    Some((link, keys)) => {
                        let spec = Spec {
                            columns: sp.columns.as_deref(),
                            filters: Vec::new(),
                            keys: Some((link, keys)),
                            order: &[],
                            nulls: Nulls::Same,
                            limit: None,
                        };
                        push(
                            &mut stops,
                            self.job(plan, i, Some(spec), all, probes),
                            &mut wave,
                        )?;
                    }
                    None if parent_waiting => still.push(i),
                    None => alone.push(i),
                }
            }
            let whole = |i: usize| Spec {
                columns: analysis.sources[i].columns.as_deref(),
                filters: Vec::new(),
                keys: None,
                order: &[],
                nulls: Nulls::Same,
                limit: None,
            };
            // Sem chave que sirva: inteira, se for pequena.
            for i in alone {
                push(
                    &mut stops,
                    self.job(plan, i, Some(whole(i)), all, probes),
                    &mut wave,
                )?;
            }
            if wave.is_empty() && stops.is_empty() {
                // Tabelas que só se ligam entre si: as pequenas vão
                // primeiro e as outras são lidas pelas chaves delas.
                let mut big = Vec::new();
                let mut reasons = Vec::new();
                for i in std::mem::take(&mut still) {
                    match self.job(plan, i, Some(whole(i)), all, probes) {
                        Ok(job) => wave.push(job),
                        Err(Stop::Guard(message)) => {
                            big.push(i);
                            reasons.push(message);
                        }
                        Err(other) => return Err(other),
                    }
                }
                if wave.is_empty() {
                    stops.extend(reasons);
                }
                still = big;
            }
            waiting = still;
        }
        let loaded: Vec<Loading> = loaded.into_iter().map(|l| l.unwrap_or_default()).collect();
        if let (Some((source, k)), Some(top)) = (top_rows, &analysis.top) {
            let count: rusqlite::Result<i64> = db.query_row(&top.check, [], |row| row.get(0));
            if self.canceled() {
                return Err(Stop::Canceled);
            }
            let Ok(count) = count else {
                return Ok(Outcome::NoTop);
            };
            let exhausted = loaded[source].rows < k;
            if (count as u64) < top.need {
                if exhausted && without_nulls {
                    return Ok(Outcome::NoTop);
                }
                if !exhausted {
                    return Ok(Outcome::More);
                }
            }
        }
        Ok(Outcome::Done(db, loaded))
    }

    /// Os filtros de uma fonte que o servidor dela pode aplicar.
    fn filters(&self, plan: &Plan, i: usize, sp: &SourcePlan, probes: &mut Probes) -> Vec<String> {
        let engine = self.servers[plan.sources[i].server].engine;
        let mut out = Vec::new();
        for filter in &sp.filters {
            let mut ok = true;
            for (column, need) in &filter.needs {
                let ty = self
                    .types(plan, i, probes)
                    .and_then(|t| t.get(&column.name.to_lowercase()))
                    .map(|(ty, _)| kind(engine, ty));
                ok &= match (ty, need) {
                    (Some(k), Need::Number) => k.number,
                    (Some(k), Need::Date { date_only }) => match k.date {
                        // Uma coluna só de data com um texto de data e
                        // hora compara diferente aqui e lá.
                        Some(true) => *date_only,
                        Some(false) => true,
                        None => false,
                    },
                    (None, _) => false,
                };
            }
            if ok {
                out.push(filter.sql.clone());
            }
        }
        out
    }

    /// A leitura de uma fonte: `spec` diz o que ler de uma tabela; sem
    /// ele, a fonte vai como está escrita. `Err(Stop::Guard)` quando viria
    /// linha demais.
    fn job(
        &self,
        plan: &Plan,
        i: usize,
        spec: Option<Spec<'_>>,
        all: bool,
        probes: &mut Probes,
    ) -> Result<Job, Stop> {
        let source = &plan.sources[i];
        let server = &self.servers[source.server];
        let engine = server.engine;
        let guard = source.whole && !all;
        let (relation, spec) = match (&source.relation, spec) {
            (Some(relation), Some(spec)) => (relation, spec),
            _ => {
                // Do jeito que o usuário escreveu.
                if guard && source.relation.is_some() {
                    self.check_size(plan, i, &source.query, true, probes)?;
                }
                let query = self
                    .remote
                    .read_only(engine, &source.query)
                    .map_err(|reason| {
                        Stop::Refused(trf!(
                            "A cross-server query only reads, and the query for {source} can change the server ({reason}).",
                            source = source.label,
                            reason = reason
                        ))
                    })?;
                return Ok(Job {
                    index: i,
                    query,
                    shown: shorten(&source.query),
                    append: false,
                    guard,
                });
            }
        };
        let read = |filters: Vec<String>| Read {
            engine,
            relation,
            columns: spec.columns,
            filters,
            order: spec.order,
            nulls: spec.nulls,
            limit: spec.limit,
        };
        let (query, shown, append) = match &spec.keys {
            Some((link, keys)) => {
                let chunk = if engine == Engine::SqlServer {
                    1000
                } else {
                    5000
                };
                let mut parts: Vec<String> = keys
                    .chunks(chunk)
                    .map(|keys| {
                        let mut filters = spec.filters.clone();
                        filters.push(pushdown::key_filter(engine, &link.column, keys));
                        read(filters).sql()
                    })
                    .collect();
                if parts.is_empty() {
                    // Nenhuma chave: só as colunas, sem linha.
                    let mut filters = spec.filters.clone();
                    filters.push("1 = 0".to_string());
                    parts.push(read(filters).sql());
                }
                let mut filters = spec.filters.clone();
                filters.push(format!(
                    "{} IN ({})",
                    link.column.sql(engine),
                    trf!(
                        "the {count} keys of {source}",
                        count = keys.len(),
                        source = plan.sources[link.parent].label
                    )
                ));
                (parts.join(";\n"), read(filters).sql(), true)
            }
            None => {
                let sql = read(spec.filters.clone()).sql();
                if guard && spec.limit.is_none() {
                    self.check_size(plan, i, &sql, spec.filters.is_empty(), probes)?;
                }
                (sql.clone(), sql, false)
            }
        };
        let query = self
            .remote
            .read_only(engine, &query)
            .map_err(Stop::Failed)?;
        Ok(Job {
            index: i,
            query,
            shown: shorten(&shown),
            append,
            guard,
        })
    }

    /// Para antes de baixar quando o servidor estima linha demais para
    /// `sql`. `unfiltered`: o SELECT é a tabela inteira.
    fn check_size(
        &self,
        plan: &Plan,
        i: usize,
        sql: &str,
        unfiltered: bool,
        probes: &mut Probes,
    ) -> Result<(), Stop> {
        let source = &plan.sources[i];
        let server = &self.servers[source.server];
        let key = (source.server, source.database.clone(), sql.to_string());
        let estimate = match probes.estimates.get(&key) {
            Some(known) => *known,
            None => {
                let ask = match (server.engine, &source.relation) {
                    (Engine::PostgreSql, _) => Some(format!("EXPLAIN (FORMAT JSON) {sql}")),
                    (Engine::SqlServer, Some(relation)) if unfiltered => Some(format!(
                        "SELECT SUM(p.rows) FROM sys.partitions p WHERE p.object_id = OBJECT_ID({}) AND p.index_id IN (0, 1)",
                        pushdown::text_literal(Engine::SqlServer, relation)
                    )),
                    _ => None,
                };
                let estimate = ask.and_then(|ask| self.ask(source, &ask)).and_then(|rows| {
                    match server.engine {
                        Engine::PostgreSql => {
                            let json: String = rows.into_iter().flatten().flatten().collect();
                            let at = json.find("\"Plan Rows\":")? + "\"Plan Rows\":".len();
                            let digits: String = json[at..]
                                .trim_start()
                                .chars()
                                .take_while(|c| c.is_ascii_digit())
                                .collect();
                            digits.parse::<u64>().ok()
                        }
                        _ => rows.first()?.first()?.as_ref()?.parse::<u64>().ok(),
                    }
                });
                probes.estimates.insert(key, estimate);
                estimate
            }
        };
        if self.canceled() {
            return Err(Stop::Canceled);
        }
        match estimate {
            Some(rows) if rows > self.options.guard_rows => Err(Stop::Guard(trf!(
                "[{source}] Stopped before downloading: about {rows} rows would come from {server}, and the query has nothing that lets the server send less.",
                source = source.label,
                rows = thousands(rows),
                server = server.name
            ))),
            _ => Ok(()),
        }
    }

    /// Tipo e "não aceita vazio" das colunas da tabela de uma fonte.
    fn types<'a>(&self, plan: &Plan, i: usize, probes: &'a mut Probes) -> Option<&'a Types> {
        if let std::collections::hash_map::Entry::Vacant(slot) = probes.types.entry(i) {
            let source = &plan.sources[i];
            let engine = self.servers[source.server].engine;
            let found = source.relation.as_ref().and_then(|relation| {
                let name = pushdown::text_literal(engine, relation);
                let ask = match engine {
                    Engine::PostgreSql => format!(
                        "SELECT a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull FROM pg_catalog.pg_attribute a WHERE a.attrelid = {name}::regclass AND a.attnum > 0 AND NOT a.attisdropped"
                    ),
                    Engine::SqlServer => format!(
                        "SELECT c.name, t.name, CASE WHEN c.is_nullable = 1 THEN 'f' ELSE 't' END FROM sys.columns c JOIN sys.types t ON t.user_type_id = c.user_type_id WHERE c.object_id = OBJECT_ID({name})"
                    ),
                    _ => return None,
                };
                let rows = self.ask(source, &ask)?;
                let mut types = HashMap::new();
                for row in rows {
                    if let [Some(name), Some(ty), not_null, ..] = row.as_slice() {
                        let not_null = matches!(not_null.as_deref(), Some("t" | "true" | "1"));
                        types.insert(name.to_lowercase(), (ty.clone(), not_null));
                    }
                }
                (!types.is_empty()).then_some(types)
            });
            slot.insert(found);
        }
        probes.types.get(&i)?.as_ref()
    }

    /// Roda uma consulta pequena no servidor de `source` e espera as
    /// linhas do primeiro resultado; `None` quando não deu.
    fn ask(&self, source: &Source, sql: &str) -> Option<Vec<Vec<Option<String>>>> {
        if self.canceled() {
            return None;
        }
        let engine = self.servers[source.server].engine;
        let text = self.remote.read_only(engine, sql).ok()?;
        let session = self.remote.session(source.server, &source.database).ok()?;
        lock(&self.running.sessions).insert(usize::MAX, session.clone());
        let (tx, rx) = mpsc::channel();
        session.execute(
            text,
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        );
        let mut rows = Vec::new();
        let (mut open, mut closed, mut failed) = (false, false, false);
        // Uma pergunta pequena: se demorar ou vier coisa demais, fica sem
        // resposta (e a leitura segue com a guarda de linhas).
        let deadline = Instant::now() + ASK_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = match rx.recv_timeout(left) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    session.cancel();
                    failed = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match event {
                QueryEvent::Columns(_) if !open => open = true,
                QueryEvent::Columns(_) => closed = true,
                QueryEvent::Row(row) if open && !closed => {
                    rows.push(row);
                    if rows.len() > 10_000 {
                        session.cancel();
                        failed = true;
                        break;
                    }
                }
                QueryEvent::CommandComplete { .. } if open => closed = true,
                QueryEvent::Error(_) => failed = true,
                QueryEvent::Finished { .. } => break,
                _ => {}
            }
        }
        lock(&self.running.sessions).remove(&usize::MAX);
        (open && !failed).then_some(rows)
    }

    /// As chaves que `parent` trouxe para a ligação `link`; `None` quando
    /// não dá para ler a outra tabela por elas (chaves demais, por exemplo).
    fn keys(
        &self,
        db: &rusqlite::Connection,
        parent_source: &Source,
        link: &Link,
        parent: &Loading,
    ) -> rusqlite::Result<Option<Vec<String>>> {
        let Some(at) = parent
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&link.parent_column))
        else {
            return Ok(None);
        };
        // Um número com casas pode estar escrito de outro jeito lá.
        if parent.shapes[at] == Shape::Real {
            return Ok(None);
        }
        if parent.rows == 0 {
            return Ok(Some(Vec::new()));
        }
        let column = quote(&parent.columns[at]);
        let mut stmt = db.prepare(&format!(
            "SELECT DISTINCT {column} FROM main.{} WHERE {column} IS NOT NULL LIMIT {}",
            quote(&parent_source.table),
            self.options.max_keys + 1
        ))?;
        let mut rows = stmt.query([])?;
        let mut keys = Vec::new();
        while let Some(row) = rows.next()? {
            match row.get_ref(0).ok().and_then(cell) {
                Some(key) if key.len() <= 200 => keys.push(key),
                _ => return Ok(None),
            }
        }
        Ok((keys.len() <= self.options.max_keys).then_some(keys))
    }

    /// Lê as fontes de `jobs` para as tabelas locais, várias ao mesmo
    /// tempo.
    fn load(
        &self,
        plan: &Plan,
        db: &rusqlite::Connection,
        jobs: &[Job],
        loaded: &mut [Option<Loading>],
        sink: &mut QuerySink,
    ) -> Result<(), Stop> {
        if jobs.is_empty() {
            return Ok(());
        }
        for job in jobs {
            loaded[job.index] = Some(Loading::new(job.append));
        }
        let (tx, rx) = mpsc::sync_channel::<(usize, Message)>(QUEUE);
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // (fonte, servidor, banco, texto)
        let work: Arc<Vec<(usize, usize, String, String)>> = Arc::new(
            jobs.iter()
                .map(|j| {
                    let source = &plan.sources[j.index];
                    (
                        j.index,
                        source.server,
                        source.database.clone(),
                        j.query.clone(),
                    )
                })
                .collect(),
        );
        for _ in 0..jobs.len().min(MAX_PARALLEL) {
            let (tx, next, work) = (tx.clone(), next.clone(), work.clone());
            let (remote, running) = (self.remote.clone(), self.running.clone());
            std::thread::spawn(move || {
                loop {
                    let at = next.fetch_add(1, Ordering::SeqCst);
                    let Some((i, server, database, query)) = work.get(at) else {
                        break;
                    };
                    let i = *i;
                    if running.canceled.load(Ordering::SeqCst) {
                        let _ = tx.send((i, Message::Finished));
                        continue;
                    }
                    let _ = tx.send((i, Message::Started));
                    let session = match remote.session(*server, database) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.send((i, Message::Failed(e)));
                            let _ = tx.send((i, Message::Finished));
                            continue;
                        }
                    };
                    lock(&running.sessions).insert(i, session.clone());
                    // A thread só pega a próxima fonte quando esta acabar.
                    let (done, finished) = mpsc::channel::<()>();
                    let out = tx.clone();
                    session.execute(
                        query.clone(),
                        Box::new(move |event| {
                            let message = match event {
                                QueryEvent::Columns(c) => Message::Columns(c),
                                QueryEvent::Row(r) => Message::Row(r),
                                QueryEvent::CommandComplete { .. } => Message::Complete,
                                QueryEvent::Notice(_) => return,
                                QueryEvent::Error(e) => Message::Failed(describe(&e)),
                                QueryEvent::Finished { .. } => {
                                    let _ = out.send((i, Message::Finished));
                                    let _ = done.send(());
                                    return;
                                }
                            };
                            let _ = out.send((i, message));
                        }),
                    );
                    let _ = finished.recv();
                    lock(&running.sessions).remove(&i);
                }
            });
        }
        drop(tx);

        db.execute_batch("BEGIN")?;
        // Quando todas as fontes acabam (ou somem), o canal fecha.
        for (i, message) in rx {
            let Some(l) = loaded[i].as_mut() else {
                continue;
            };
            let source = &plan.sources[i];
            let table = &source.table;
            let job = jobs.iter().find(|j| j.index == i);
            match message {
                Message::Started => {
                    l.started = Instant::now();
                    if let Some(job) = job {
                        sink(notice(format!(
                            "[{}] {} → {}",
                            source.label, self.servers[source.server].name, job.shown
                        )));
                    }
                }
                Message::Columns(columns) => {
                    if !l.columns.is_empty() {
                        // Mais um lote de chaves: as mesmas colunas.
                        if l.append && columns.len() == l.columns.len() {
                            l.closed = false;
                        } else {
                            l.closed = true;
                            l.extra_results = true;
                        }
                        continue;
                    }
                    l.columns = column_names(&columns);
                    l.shapes = vec![Shape::Empty; l.columns.len()];
                    // Sem tipo: o valor fica como chega (texto) até o fim,
                    // quando as colunas só de números viram números.
                    let list: Vec<String> = l.columns.iter().map(|c| quote(c)).collect();
                    db.execute_batch(&format!(
                        "CREATE TABLE {} ({})",
                        quote(table),
                        list.join(", ")
                    ))?;
                    l.insert = format!(
                        "INSERT INTO {} VALUES ({})",
                        quote(table),
                        vec!["?"; l.columns.len()].join(", ")
                    );
                }
                Message::Row(mut row) => {
                    if l.closed
                        || l.truncated
                        || l.over
                        || l.columns.is_empty()
                        || l.error.is_some()
                    {
                        continue;
                    }
                    let guarded = job.is_some_and(|j| j.guard);
                    if guarded && l.rows >= self.options.guard_rows {
                        l.over = true;
                    } else if l.rows >= self.options.max_rows {
                        l.truncated = true;
                    }
                    if l.over || l.truncated {
                        if let Some(s) = lock(&self.running.sessions).get(&i) {
                            s.cancel();
                        }
                        continue;
                    }
                    row.resize(l.columns.len(), None);
                    for (shape, value) in l.shapes.iter_mut().zip(&row) {
                        if let Some(text) = value
                            && *shape != Shape::Text
                        {
                            *shape = merge(*shape, shape_of(text));
                        }
                    }
                    db.prepare_cached(&l.insert)?
                        .execute(rusqlite::params_from_iter(row.iter()))?;
                    l.rows += 1;
                    if l.rows % PROGRESS == 0 {
                        sink(notice(trf!(
                            "[{source}] {rows} rows so far...",
                            source = source.label,
                            rows = thousands(l.rows)
                        )));
                    }
                }
                Message::Complete => {
                    if !l.columns.is_empty() {
                        l.closed = true;
                    }
                }
                Message::Failed(e) => {
                    // Cortar a fonte cancela a consulta dela: não é erro.
                    if !l.truncated && !l.over && l.error.is_none() {
                        l.error = Some(e);
                    }
                }
                Message::Finished => {
                    l.finished = true;
                    l.elapsed = l.started.elapsed();
                }
            }
        }
        // As colunas só de números viram números, para comparar e somar
        // com as de outro servidor.
        for job in jobs {
            let Some(l) = loaded[job.index].as_ref() else {
                continue;
            };
            let casts: Vec<String> = l
                .columns
                .iter()
                .zip(&l.shapes)
                .filter_map(|(c, shape)| {
                    let ty = match shape {
                        Shape::Integer => "INTEGER",
                        Shape::Real => "REAL",
                        _ => return None,
                    };
                    Some(format!("{0} = CAST({0} AS {ty})", quote(c)))
                })
                .collect();
            if !casts.is_empty() && l.rows > 0 && !l.over && !self.canceled() {
                db.execute_batch(&format!(
                    "UPDATE {} SET {}",
                    quote(&plan.sources[job.index].table),
                    casts.join(", ")
                ))?;
            }
        }
        db.execute_batch("COMMIT")?;
        if self.canceled() {
            return Err(Stop::Canceled);
        }
        let mut over = Vec::new();
        let mut errors = Vec::new();
        for job in jobs {
            let source = &plan.sources[job.index];
            let server = &self.servers[source.server];
            let Some(l) = loaded[job.index].as_ref() else {
                continue;
            };
            let name = format!("{} ({})", source.label, server.name);
            if l.over {
                over.push(trf!(
                    "[{source}] Stopped: more than {rows} rows were coming from {server}.",
                    source = source.label,
                    rows = thousands(self.options.guard_rows),
                    server = server.name
                ));
            } else if let Some(e) = &l.error {
                errors.push(format!("{name}: {e}"));
            } else if !l.finished {
                errors.push(format!("{name}: {}", tr("No answer.")));
            } else if l.columns.is_empty() {
                errors.push(format!(
                    "{name}: {}",
                    tr("The query returned no result to join.")
                ));
            }
        }
        if !errors.is_empty() {
            return Err(Stop::Failed(errors.join("\n")));
        }
        if !over.is_empty() {
            over.push(self.advice());
            return Err(Stop::Guard(over.join("\n")));
        }
        Ok(())
    }
}

/// `1234567` como `1,234,567` (`1.234.567` em português).
fn thousands(n: u64) -> String {
    let separator = if pgstudio_i18n::language().code().starts_with("pt") {
        '.'
    } else {
        ','
    };
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(c);
    }
    out
}

/// A posição de um erro do SQL local no texto do usuário, quando ela cai
/// antes de qualquer fonte trocada (depois disso os textos não batem).
fn position_in_original(local: &str, offset: usize, statement_offset: usize) -> Option<usize> {
    let before = local.get(..offset)?;
    if before.contains("main.\"pp_") {
        return None;
    }
    Some(statement_offset + before.chars().count() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Servidores de mentira: cada consulta devolve uma grade fixa.
    struct Fake {
        writes: bool,
    }

    struct FakeSession(usize);

    /// Manda linhas sem parar até ser cancelada.
    struct Endless(Arc<AtomicBool>);

    impl QuerySession for Endless {
        fn execute(&self, _sql: String, mut sink: QuerySink) {
            let stop = self.0.clone();
            std::thread::spawn(move || {
                sink(QueryEvent::Columns(vec!["n".into()]));
                while !stop.load(Ordering::SeqCst) {
                    sink(QueryEvent::Row(vec![Some("1".into())]));
                }
                sink(QueryEvent::Error(ServerMessage::error("canceled")));
                sink(QueryEvent::Finished { canceled: true });
            });
        }

        fn cancel(&self) {
            self.0.store(true, Ordering::SeqCst);
        }

        fn database(&self) -> String {
            String::new()
        }
    }

    type Grid = (Vec<&'static str>, Vec<Vec<Option<&'static str>>>);

    fn grid(server: usize, query: &str) -> Result<Vec<Grid>, String> {
        let s = Some;
        match (server, query) {
            (0, "SELECT * FROM public.clientes") => Ok(vec![(
                vec!["id", "nome", "cep"],
                vec![
                    vec![s("1"), s("Ana"), s("01310")],
                    vec![s("2"), s("Bruno"), s("20040")],
                    vec![s("3"), s("Carla"), None],
                ],
            )]),
            (1, "SELECT * FROM dbo.pedidos") => Ok(vec![(
                vec!["cliente_id", "total"],
                vec![
                    vec![s("1"), s("10.50")],
                    vec![s("1"), s("4.5")],
                    vec![s("2"), s("100")],
                ],
            )]),
            (2, "db.getCollection(\"sessoes\").find({})") => Ok(vec![(
                vec!["_id", "usuario", "dados"],
                vec![
                    vec![s("a1"), s("1"), s("{\"plano\":\"pro\"}")],
                    vec![s("a2"), s("3"), s("{\"plano\":\"free\"}")],
                ],
            )]),
            (0, "select 1 as id, 2 as id") => Ok(vec![
                (vec!["id", "id"], vec![vec![s("1"), s("2")]]),
                (vec!["outro"], vec![vec![s("x")]]),
            ]),
            (0, "select muitos") => Ok(vec![(vec!["n"], (0..50).map(|_| vec![s("7")]).collect())]),
            (0, "update") => Ok(vec![]),
            (_, "falha") => Err("boom".into()),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    impl QuerySession for FakeSession {
        fn execute(&self, sql: String, mut sink: QuerySink) {
            let server = self.0;
            std::thread::spawn(move || {
                match grid(server, &sql) {
                    Ok(grids) => {
                        for (columns, rows) in grids {
                            sink(QueryEvent::Columns(
                                columns.iter().map(|c| c.to_string()).collect(),
                            ));
                            let n = rows.len() as u64;
                            for row in rows {
                                sink(QueryEvent::Row(
                                    row.into_iter().map(|c| c.map(str::to_string)).collect(),
                                ));
                            }
                            sink(QueryEvent::CommandComplete { rows: n });
                        }
                    }
                    Err(e) => sink(QueryEvent::Error(ServerMessage::error(e))),
                }
                sink(QueryEvent::Finished { canceled: false });
            });
        }

        fn cancel(&self) {}

        fn database(&self) -> String {
            String::new()
        }
    }

    impl Remote for Fake {
        fn session(&self, server: usize, _database: &str) -> Result<Arc<dyn QuerySession>, String> {
            if server == 3 {
                return Err("No saved password.".into());
            }
            if server == 4 {
                return Ok(Arc::new(Endless(Arc::default())));
            }
            Ok(Arc::new(FakeSession(server)))
        }

        fn read_only(&self, _engine: Engine, query: &str) -> Result<String, String> {
            if self.writes || query == "update" {
                Err("UPDATE".into())
            } else {
                Ok(query.to_string())
            }
        }
    }

    fn servers() -> Vec<Server> {
        let s = |name: &str, engine| Server {
            name: name.into(),
            engine,
            database: "db".into(),
        };
        vec![
            s("pg", Engine::PostgreSql),
            s("mssql", Engine::SqlServer),
            s("mongo", Engine::MongoDb),
            s("fora", Engine::PostgreSql),
            s("semfim", Engine::PostgreSql),
        ]
    }

    fn run_with(sql: &str, options: Options) -> Vec<QueryEvent> {
        let session = FederatedSession::new(servers(), Arc::new(Fake { writes: false }), options);
        let (tx, rx) = mpsc::channel();
        session.execute(
            sql.into(),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(10)) {
            let end = matches!(e, QueryEvent::Finished { .. });
            events.push(e);
            if end {
                break;
            }
        }
        events
    }

    fn run(sql: &str) -> Vec<QueryEvent> {
        run_with(sql, Options::default())
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

    fn error(events: &[QueryEvent]) -> Option<String> {
        events.iter().find_map(|e| match e {
            QueryEvent::Error(m) => Some(m.message.clone()),
            _ => None,
        })
    }

    fn notices(events: &[QueryEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                QueryEvent::Notice(n) => Some(n.message.clone()),
                _ => None,
            })
            .collect()
    }

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn joins_tables_of_two_servers() {
        let events = run("select c.nome, sum(p.total) as total, count(*) as pedidos \
             from pg.public.clientes c join mssql.dbo.pedidos p on p.cliente_id = c.id \
             group by c.nome order by c.nome");
        assert_eq!(error(&events), None);
        assert!(events.contains(&QueryEvent::Columns(vec![
            "nome".into(),
            "total".into(),
            "pedidos".into()
        ])));
        assert_eq!(
            rows(&events),
            vec![
                vec![s("Ana"), s("15"), s("2")],
                vec![s("Bruno"), s("100"), s("1")],
            ]
        );
        assert_eq!(
            events.last(),
            Some(&QueryEvent::Finished { canceled: false })
        );
        let n = notices(&events);
        assert!(
            n.iter()
                .any(|m| m.starts_with("[pg.public.clientes] 3 rows")),
            "{n:?}"
        );
    }

    #[test]
    fn sql_joins_with_documents_and_reads_inside_the_json() {
        let events = run("select c.nome, json_extract(s.dados, '$.plano') as plano \
             from pg.public.clientes c left join mongo.sessoes s on s.usuario = c.id \
             order by c.id");
        assert_eq!(error(&events), None);
        assert_eq!(
            rows(&events),
            vec![
                vec![s("Ana"), s("pro")],
                vec![s("Bruno"), None],
                vec![s("Carla"), s("free")],
            ]
        );
    }

    #[test]
    fn codes_with_leading_zeros_stay_text() {
        let events =
            run("select cep, typeof(cep), typeof(id) from pg.public.clientes where id = 1");
        assert_eq!(
            rows(&events),
            vec![vec![s("01310"), s("text"), s("integer")]]
        );
    }

    #[test]
    fn a_source_that_fails_stops_the_query_and_says_which() {
        let events = run(
            "select * from pg.public.clientes c join mssql($$falha$$) f on 1 = 1 join fora.public.t on 1 = 1",
        );
        let e = error(&events).unwrap();
        assert!(e.contains("mssql($$falha$$) (mssql): boom"), "{e}");
        assert!(
            e.contains("fora.public.t (fora): No saved password."),
            "{e}"
        );
        assert!(rows(&events).is_empty());
    }

    #[test]
    fn nothing_writes_on_the_servers_or_locally() {
        let events = run("select * from pg($$update$$) x");
        let e = error(&events).unwrap();
        assert!(e.contains("only reads"), "{e}");
        assert!(e.contains("UPDATE"), "{e}");

        for sql in [
            "create table t (a)",
            "delete from pg.public.clientes",
            "attach 'x.db' as outro",
            "select 1; drop table if exists t",
        ] {
            let events = run(sql);
            let e = error(&events).unwrap_or_else(|| panic!("{sql} ran"));
            assert!(e.contains("only reads"), "{sql}: {e}");
        }
    }

    #[test]
    fn a_big_source_is_cut_with_a_visible_warning() {
        let events = run_with(
            "select count(*) from pg($$select muitos$$) m",
            Options {
                max_rows: 20,
                ..Options::default()
            },
        );
        assert_eq!(error(&events), None);
        assert_eq!(rows(&events), vec![vec![s("20")]]);
        let n = notices(&events);
        assert!(
            n.iter().any(|m| m.contains("only the first 20 rows")),
            "{n:?}"
        );
    }

    #[test]
    fn repeated_column_names_and_extra_results() {
        let events = run("select id, id_2 from pg($$select 1 as id, 2 as id$$) x");
        assert_eq!(error(&events), None);
        assert_eq!(rows(&events), vec![vec![s("1"), s("2")]]);
        assert!(
            notices(&events)
                .iter()
                .any(|m| m.contains("more than one result"))
        );
    }

    #[test]
    fn several_statements_and_local_errors() {
        let events = run("select 1 as a; select nome from pg.public.clientes where id = 2");
        assert_eq!(rows(&events), vec![vec![s("1")], vec![s("Bruno")]]);

        let events = run("select nada from pg.public.clientes");
        let e = error(&events).unwrap();
        assert!(e.contains("nada"), "{e}");
    }

    #[test]
    fn cancel_stops_a_source_that_never_ends() {
        let session = FederatedSession::new(
            servers(),
            Arc::new(Fake { writes: false }),
            Options::default(),
        );
        let (tx, rx) = mpsc::channel();
        session.execute(
            "select count(*) from semfim.public.t".into(),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        std::thread::sleep(Duration::from_millis(100));
        session.cancel();
        let mut last = None;
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(10)) {
            assert!(!matches!(e, QueryEvent::Row(_)), "a result came out");
            last = Some(e);
        }
        assert_eq!(last, Some(QueryEvent::Finished { canceled: true }));
    }

    #[test]
    fn a_cut_source_stops_reading_the_server() {
        let events = run_with(
            "select count(*) from semfim.public.t",
            Options {
                max_rows: 1000,
                ..Options::default()
            },
        );
        assert_eq!(error(&events), None);
        assert_eq!(rows(&events), vec![vec![s("1000")]]);
    }

    /// Um servidor que responde o que o roteiro manda e anota o que
    /// recebeu.
    struct Scripted {
        answers: Vec<(&'static str, Grid)>,
        log: Arc<Mutex<Vec<String>>>,
    }

    struct ScriptedSession(Vec<(&'static str, Grid)>, Arc<Mutex<Vec<String>>>);

    impl QuerySession for ScriptedSession {
        fn execute(&self, sql: String, mut sink: QuerySink) {
            lock(&self.1).push(sql.clone());
            let answer = self
                .0
                .iter()
                .find(|(q, _)| *q == sql)
                .map(|(_, g)| g.clone());
            std::thread::spawn(move || {
                match answer {
                    Some((columns, rows)) => {
                        sink(QueryEvent::Columns(
                            columns.iter().map(|c| c.to_string()).collect(),
                        ));
                        let n = rows.len() as u64;
                        for row in rows {
                            sink(QueryEvent::Row(
                                row.into_iter().map(|c| c.map(str::to_string)).collect(),
                            ));
                        }
                        sink(QueryEvent::CommandComplete { rows: n });
                    }
                    None => sink(QueryEvent::Error(ServerMessage::error("unexpected"))),
                }
                sink(QueryEvent::Finished { canceled: false });
            });
        }

        fn cancel(&self) {}

        fn database(&self) -> String {
            String::new()
        }
    }

    impl Remote for Scripted {
        fn session(&self, server: usize, _database: &str) -> Result<Arc<dyn QuerySession>, String> {
            if server == 4 {
                return Ok(Arc::new(Endless(Arc::default())));
            }
            Ok(Arc::new(ScriptedSession(
                self.answers.clone(),
                self.log.clone(),
            )))
        }

        fn read_only(&self, _engine: Engine, query: &str) -> Result<String, String> {
            Ok(query.to_string())
        }
    }

    /// Roda `sql` com a janela no servidor `pg`, banco `loja`.
    fn scripted(sql: &str, answers: Vec<(&'static str, Grid)>) -> (Vec<QueryEvent>, Vec<String>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let session = FederatedSession::new(
            servers(),
            Arc::new(Scripted {
                answers,
                log: log.clone(),
            }),
            Options {
                guard_rows: 5000,
                ..Options::default()
            },
        )
        .with_home(Some(Home {
            server: 0,
            database: "loja".into(),
        }));
        let (tx, rx) = mpsc::channel();
        session.execute(
            sql.into(),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(10)) {
            let done = matches!(e, QueryEvent::Finished { .. });
            events.push(e);
            if done {
                break;
            }
        }
        let log = lock(&log).clone();
        (events, log)
    }

    const TYPES: &str = "SELECT a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull FROM pg_catalog.pg_attribute a WHERE a.attrelid = 'documento'::regclass AND a.attnum > 0 AND NOT a.attisdropped";

    fn types() -> Grid {
        let s = Some;
        (
            vec!["attname", "format_type", "attnotnull"],
            vec![
                vec![s("id"), s("bigint"), s("t")],
                vec![s("chave"), s("text"), s("f")],
                vec![s("criado"), s("timestamp without time zone"), s("f")],
            ],
        )
    }

    #[test]
    fn the_latest_rows_come_with_order_and_limit_and_the_other_table_by_their_keys() {
        let s = Some;
        let (events, log) = scripted(
            "select d.id, l.nome from documento d \
             join mssql.dbo.lotes l on l.chave = d.chave order by d.id desc limit 1",
            vec![
                (TYPES, types()),
                (
                    "SELECT id, chave FROM documento WHERE chave IS NOT NULL ORDER BY id DESC LIMIT 1000",
                    (
                        vec!["id", "chave"],
                        vec![
                            vec![s("9"), s("c'9")],
                            vec![s("8"), s("c8")],
                            vec![s("7"), None],
                        ],
                    ),
                ),
                (
                    "SELECT [nome], [chave] FROM dbo.lotes WHERE [chave] IN ('c''9', 'c8')",
                    (vec!["nome", "chave"], vec![vec![s("lote oito"), s("c8")]]),
                ),
            ],
        );
        assert_eq!(error(&events), None, "{log:?}");
        assert_eq!(
            rows(&events),
            vec![vec![super::tests::s("8"), super::tests::s("lote oito")]]
        );
        assert_eq!(log.len(), 3, "{log:?}");
        let n = notices(&events);
        assert!(
            n.iter().any(|m| m
                == "[documento] pg → SELECT id, chave FROM documento WHERE chave IS NOT NULL ORDER BY id DESC LIMIT 1000"),
            "{n:?}"
        );
        assert!(
            n.iter().any(|m| m.contains("IN (the 2 keys of documento)")),
            "{n:?}"
        );
    }

    #[test]
    fn a_table_too_big_stops_before_any_row_is_read() {
        let s = Some;
        let (events, log) = scripted(
            "select count(*) from documento",
            vec![(
                "EXPLAIN (FORMAT JSON) SELECT 1 AS pp_one FROM documento",
                (
                    vec!["QUERY PLAN"],
                    vec![vec![s(
                        "[{\"Plan\": {\"Node Type\": \"Seq Scan\", \"Plan Rows\": 10000000, \"Plan Width\": 0}}]",
                    )]],
                ),
            )],
        );
        let message = error(&events).expect("stopped");
        assert!(
            message.contains(
                "[documento] Stopped before downloading: about 10,000,000 rows would come from pg"
            ),
            "{message}"
        );
        assert!(message.contains("-- baixar tudo"), "{message}");
        assert!(rows(&events).is_empty());
        // só a pergunta do tamanho chegou ao servidor
        assert_eq!(log.len(), 1, "{log:?}");
    }

    #[test]
    fn a_filter_goes_to_the_server_only_when_the_column_type_allows() {
        let s = Some;
        let (events, log) = scripted(
            "select d.id from documento d where d.id > 5 and d.chave > 7 and d.criado >= '2024-01-01' and d.chave = 'x'",
            vec![
                (TYPES, types()),
                (
                    "SELECT id, chave, criado FROM documento WHERE id > 5 AND criado >= '2024-01-01' AND chave = 'x'",
                    (
                        vec!["id", "chave", "criado"],
                        vec![vec![s("6"), s("x"), s("2024-05-05 10:00:00")]],
                    ),
                ),
            ],
        );
        assert_eq!(error(&events), None, "{log:?}");
        // `chave > 7` ficou só aqui: texto é sempre maior que número no SQLite
        assert_eq!(rows(&events), vec![vec![super::tests::s("6")]]);
    }

    #[test]
    fn a_table_that_keeps_coming_is_stopped_at_the_guard() {
        let (events, _) = scripted("select count(*) from semfim.public.t", Vec::new());
        let message = error(&events).expect("stopped");
        assert!(
            message.contains(
                "[semfim.public.t] Stopped: more than 5,000 rows were coming from semfim"
            ),
            "{message}"
        );
        // com o comentário, vem até o limite de sempre
        let events = run_with(
            "select count(*) from semfim.public.t -- baixar tudo",
            Options {
                max_rows: 7000,
                guard_rows: 5000,
                ..Options::default()
            },
        );
        assert_eq!(error(&events), None);
        assert_eq!(rows(&events), vec![vec![Some("7000".to_string())]]);
    }

    #[test]
    fn when_the_server_refuses_the_short_read_the_table_comes_as_it_is() {
        // O servidor do roteiro antigo só conhece `SELECT * FROM ...`.
        let events = run("select c.nome from pg.public.clientes c where c.id = 2");
        assert_eq!(error(&events), None);
        assert_eq!(rows(&events), vec![vec![s("Bruno")]]);
        let n = notices(&events);
        assert!(
            n.iter()
                .any(|m| m.starts_with("Reading only what the query uses did not work")),
            "{n:?}"
        );
    }

    #[test]
    fn the_first_rows_grow_until_the_limit_is_met() {
        assert_eq!(ladder(1, 200_000), [1000, 20_000, 200_000]);
        assert_eq!(ladder(5000, 200_000), [10_000, 200_000]);
        assert!(ladder(150_000, 200_000).is_empty());
        assert!(ladder(0, 200_000).is_empty());
        assert!(wants_everything("select 1 -- Baixar Tudo"));
        assert!(wants_everything("-- download all\nselect 1"));
        assert!(!wants_everything("select 'baixar tudo'"));
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn number_shapes() {
        assert!(is_integer("0") && is_integer("-12") && !is_integer("007") && !is_integer(""));
        assert!(!is_integer("1234567890123456789"));
        assert!(is_real("10.50") && is_real("-0.5") && !is_real("1.") && !is_real("01.5"));
        assert!(!is_real("1234567890.1234567"));
        assert_eq!(merge(Shape::Integer, Shape::Real), Shape::Real);
        assert_eq!(merge(Shape::Empty, Shape::Integer), Shape::Integer);
        assert_eq!(merge(Shape::Real, Shape::Text), Shape::Text);
    }
}
