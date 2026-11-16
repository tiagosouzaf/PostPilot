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

use crate::plan::{self, Plan, Server, Source};

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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_rows: 1_000_000,
        }
    }
}

/// Fontes lidas ao mesmo tempo.
const MAX_PARALLEL: usize = 8;

/// Linhas esperando para entrar no SQLite; cheio, o servidor espera.
const QUEUE: usize = 4096;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A sessão de uma janela de consulta entre servidores.
pub struct FederatedSession {
    servers: Vec<Server>,
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
            remote,
            options,
            running: Arc::default(),
        }
    }
}

impl QuerySession for FederatedSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        self.running.canceled.store(false, Ordering::SeqCst);
        let run = Run {
            servers: self.servers.clone(),
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
    error: Option<String>,
    finished: bool,
    elapsed: Duration,
}

impl Loading {
    fn new() -> Self {
        Loading {
            started: Instant::now(),
            columns: Vec::new(),
            shapes: Vec::new(),
            insert: String::new(),
            rows: 0,
            closed: false,
            extra_results: false,
            truncated: false,
            error: None,
            finished: false,
            elapsed: Duration::ZERO,
        }
    }
}

struct Run {
    servers: Vec<Server>,
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

impl Run {
    fn canceled(&self) -> bool {
        self.running.canceled.load(Ordering::SeqCst)
    }

    fn execute(&self, sql: &str, sink: &mut QuerySink) -> Result<(), Box<ServerMessage>> {
        let plan = plan::plan(sql, &self.servers).map_err(|e| ServerMessage {
            position: Some(e.position),
            ..ServerMessage::error(e.message)
        })?;
        if plan.statements.is_empty() {
            return Ok(());
        }
        let mut plan = plan;
        for source in &mut plan.sources {
            let server = &self.servers[source.server];
            source.query = self
                .remote
                .read_only(server.engine, &source.query)
                .map_err(|reason| {
                    ServerMessage::error(trf!(
                        "A cross-server query only reads, and the query for {source} can change the server ({reason}).",
                        source = source.label,
                        reason = reason
                    ))
                })?;
        }
        let failed = |e: rusqlite::Error| {
            ServerMessage::error(trf!(
                "Could not use the local database for the join: {error}",
                error = local_error(&e)
            ))
        };
        // Um banco temporário em disco, só desta execução.
        let mut db = rusqlite::Connection::open("").map_err(failed)?;
        db.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")
            .map_err(failed)?;
        *lock(&self.running.local) = Some(db.get_interrupt_handle());

        let loaded = self.load(&plan, &db).map_err(failed)?;
        if self.canceled() {
            return Ok(());
        }
        let mut errors = Vec::new();
        for (source, l) in plan.sources.iter().zip(&loaded) {
            let name = format!("{} ({})", source.label, self.servers[source.server].name);
            match &l.error {
                Some(e) => errors.push(format!("{name}: {e}")),
                None if !l.finished => {
                    errors.push(format!("{name}: {}", tr("No answer.")));
                }
                None if l.columns.is_empty() => errors.push(format!(
                    "{name}: {}",
                    tr("The query returned no result to join.")
                )),
                None => {
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
            }
        }
        if !errors.is_empty() {
            return Err(ServerMessage::error(errors.join("\n")).into());
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

    /// Lê todas as fontes para as tabelas locais.
    fn load(&self, plan: &Plan, db: &rusqlite::Connection) -> rusqlite::Result<Vec<Loading>> {
        let n = plan.sources.len();
        let mut loaded: Vec<Loading> = (0..n).map(|_| Loading::new()).collect();
        if n == 0 {
            return Ok(loaded);
        }
        let (tx, rx) = mpsc::sync_channel::<(usize, Message)>(QUEUE);
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sources: Arc<Vec<Source>> = Arc::new(plan.sources.clone());
        for _ in 0..n.min(MAX_PARALLEL) {
            let (tx, next, sources) = (tx.clone(), next.clone(), sources.clone());
            let (remote, running) = (self.remote.clone(), self.running.clone());
            std::thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(source) = sources.get(i) else { break };
                    if running.canceled.load(Ordering::SeqCst) {
                        let _ = tx.send((i, Message::Finished));
                        continue;
                    }
                    let session = match remote.session(source.server, &source.database) {
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
                        source.query.clone(),
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
            let l = &mut loaded[i];
            let table = &plan.sources[i].table;
            match message {
                Message::Columns(columns) => {
                    if !l.columns.is_empty() || l.closed {
                        l.closed = true;
                        l.extra_results = true;
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
                    if l.closed || l.truncated || l.columns.is_empty() || l.error.is_some() {
                        continue;
                    }
                    if l.rows >= self.options.max_rows {
                        l.truncated = true;
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
                }
                Message::Complete => {
                    if !l.columns.is_empty() {
                        l.closed = true;
                    }
                }
                Message::Failed(e) => {
                    // Cortar a fonte cancela a consulta dela: não é erro.
                    if !l.truncated && l.error.is_none() {
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
        for (l, source) in loaded.iter().zip(&plan.sources) {
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
            if !casts.is_empty() && l.rows > 0 && !self.canceled() {
                db.execute_batch(&format!(
                    "UPDATE {} SET {}",
                    quote(&source.table),
                    casts.join(", ")
                ))?;
            }
        }
        db.execute_batch("COMMIT")?;
        Ok(loaded)
    }
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
            Options { max_rows: 20 },
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
            Options { max_rows: 1000 },
        );
        assert_eq!(error(&events), None);
        assert_eq!(rows(&events), vec![vec![s("1000")]]);
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
