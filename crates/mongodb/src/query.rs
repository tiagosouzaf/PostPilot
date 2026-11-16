//! Execução das consultas das janelas no MongoDB.
//!
//! O texto é lido por [`crate::shell`]; cada comando vai ao servidor e a
//! resposta vira [`QueryEvent`]s: documentos numa grade (um campo do
//! primeiro nível por coluna), escritas como uma mensagem com a contagem.
//! Um erro para a execução, como no SQL.

use std::sync::{Arc, Mutex};

use mongodb::bson::{Bson, Document, doc};
use mongodb::{Client, Cursor};
use pgstudio_core::connection::{
    ConnectionSettings, Engine, REDIS_IN_TRANSACTION, REDIS_LIST_DATABASES,
};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_i18n::{tr, trf};
use pgstudio_pg_client::DbRuntime;
use tokio::sync::Notify;

use crate::render;
use crate::shell::{self, Action, Shape, Statement, Write};

/// Documentos lidos antes de decidir as colunas da grade. Campos que só
/// aparecem depois disso ficam fora da grade (uma mensagem avisa quais).
pub const COLUMN_SAMPLE: usize = 1000;

/// Abre uma [`MongoQuerySession`] por janela de consulta.
pub struct MongoQueryService {
    client: Client,
    settings: ConnectionSettings,
    runtime: DbRuntime,
}

impl MongoQueryService {
    pub fn new(client: Client, settings: ConnectionSettings, runtime: DbRuntime) -> Self {
        MongoQueryService {
            client,
            settings,
            runtime,
        }
    }
}

impl QueryService for MongoQueryService {
    fn engine(&self) -> Engine {
        Engine::MongoDb
    }

    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        let database = match database.trim() {
            "" => self.settings.database.clone(),
            d => d.to_string(),
        };
        Arc::new(MongoQuerySession {
            client: self.client.clone(),
            runtime: self.runtime.clone(),
            shared: Arc::new(Shared {
                database: Mutex::new(database),
                running: tokio::sync::Mutex::new(()),
                cancel: Mutex::new(None),
            }),
        })
    }
}

/// A sessão de uma janela de consulta. As conexões são do `Client`, que
/// as abre e reaproveita sozinho.
pub struct MongoQuerySession {
    client: Client,
    runtime: DbRuntime,
    shared: Arc<Shared>,
}

struct Shared {
    /// O banco da janela; `use nome` troca.
    database: Mutex<String>,
    /// Uma execução por vez.
    running: tokio::sync::Mutex<()>,
    /// O aviso de cancelamento da execução em andamento.
    cancel: Mutex<Option<Arc<Notify>>>,
}

impl Shared {
    fn database(&self) -> String {
        self.database.lock().map(|d| d.clone()).unwrap_or_default()
    }
}

impl QuerySession for MongoQuerySession {
    fn execute(&self, text: String, mut sink: QuerySink) {
        let shared = self.shared.clone();
        let client = self.client.clone();
        let cancel = Arc::new(Notify::new());
        if let Ok(mut c) = shared.cancel.lock() {
            *c = Some(cancel.clone());
        }
        self.runtime.handle().spawn(async move {
            let _running = shared.running.lock().await;
            let canceled = tokio::select! {
                _ = run(&client, &shared, &text, &mut sink) => false,
                // Largar a execução fecha o cursor em andamento.
                _ = cancel.notified() => true,
            };
            if let Ok(mut c) = shared.cancel.lock() {
                *c = None;
            }
            sink(QueryEvent::Finished { canceled });
        });
    }

    fn cancel(&self) {
        if let Some(cancel) = self.shared.cancel.lock().ok().and_then(|c| c.clone()) {
            cancel.notify_one();
        }
    }

    fn database(&self) -> String {
        self.shared.database()
    }

    fn engine(&self) -> Engine {
        Engine::MongoDb
    }
}

/// Posição (em caracteres, a partir de 1) do byte `at` de `text`.
fn position(text: &str, at: usize) -> usize {
    text.char_indices().take_while(|(i, _)| *i < at).count() + 1
}

fn error_at(message: String, code: String, text: &str, at: usize) -> QueryEvent {
    QueryEvent::Error(ServerMessage {
        severity: "ERROR".into(),
        code,
        message,
        position: Some(position(text, at)),
        ..Default::default()
    })
}

fn notice(message: String) -> QueryEvent {
    QueryEvent::Notice(ServerMessage {
        // Sem severidade: a mensagem aparece como está em Messages.
        message,
        ..Default::default()
    })
}

/// O erro do driver como texto: a mensagem do servidor quando há.
fn error_text(e: &mongodb::error::Error) -> (String, String) {
    use mongodb::error::{ErrorKind, WriteFailure};
    match &*e.kind {
        ErrorKind::Command(c) => (c.message.clone(), c.code_name.clone()),
        ErrorKind::Write(WriteFailure::WriteError(w)) => {
            (w.message.clone(), w.code_name.clone().unwrap_or_default())
        }
        _ => (e.to_string(), String::new()),
    }
}

async fn run(client: &Client, shared: &Shared, text: &str, sink: &mut QuerySink) {
    // O que a interface pergunta por SQL nos outros motores.
    match text.trim() {
        REDIS_LIST_DATABASES => {
            let mut names = database_names(client).await.unwrap_or_default();
            let current = shared.database();
            if !names.contains(&current) {
                names.push(current);
                names.sort();
            }
            sink(QueryEvent::Columns(vec!["name".into()]));
            let rows = names.len() as u64;
            for name in names {
                sink(QueryEvent::Row(vec![Some(name)]));
            }
            sink(QueryEvent::CommandComplete { rows });
            return;
        }
        REDIS_IN_TRANSACTION => {
            sink(QueryEvent::Columns(vec!["in_transaction".into()]));
            sink(QueryEvent::Row(vec![Some("0".into())]));
            sink(QueryEvent::CommandComplete { rows: 1 });
            return;
        }
        _ => {}
    }
    let statements = match shell::parse(text) {
        Ok(s) => s,
        Err(e) => {
            sink(error_at(e.message, String::new(), text, e.at));
            return;
        }
    };
    for statement in &statements {
        if let Err((message, code)) = run_statement(client, shared, statement, sink).await {
            sink(error_at(message, code, text, statement.start));
            return;
        }
    }
}

/// Os bancos do servidor, em ordem.
pub async fn database_names(client: &Client) -> Result<Vec<String>, String> {
    let reply = client
        .database("admin")
        .run_command(doc! { "listDatabases": 1, "nameOnly": true })
        .await
        .map_err(|e| error_text(&e).0)?;
    let mut names: Vec<String> = reply
        .get_array("databases")
        .map(|list| {
            list.iter()
                .filter_map(|d| d.as_document()?.get_str("name").ok().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    Ok(names)
}

type Failure = (String, String);

async fn run_statement(
    client: &Client,
    shared: &Shared,
    statement: &Statement,
    sink: &mut QuerySink,
) -> Result<(), Failure> {
    let database = |admin: bool| {
        if admin {
            client.database("admin")
        } else {
            client.database(&shared.database())
        }
    };
    match &statement.action {
        Action::Use(name) => {
            if let Ok(mut d) = shared.database.lock() {
                *d = name.clone();
            }
            sink(notice(trf!(
                "Switched to database {database}.",
                database = name.as_str()
            )));
            Ok(())
        }
        Action::Cursor { command, admin } => {
            let cursor = database(*admin)
                .run_cursor_command(command.clone())
                .await
                .map_err(|e| error_text(&e))?;
            stream(cursor, sink).await
        }
        Action::Single {
            command,
            admin,
            shape,
        } => {
            let reply = database(*admin)
                .run_command(command.clone())
                .await
                .map_err(|e| error_text(&e))?;
            show(reply, shape, sink)
        }
    }
}

/// Os documentos de um cursor como uma grade. As colunas saem dos
/// primeiros [`COLUMN_SAMPLE`] documentos.
async fn stream(mut cursor: Cursor<Document>, sink: &mut QuerySink) -> Result<(), Failure> {
    let mut sample: Vec<Document> = Vec::new();
    let mut more = false;
    while cursor.advance().await.map_err(|e| error_text(&e))? {
        sample.push(cursor.deserialize_current().map_err(|e| error_text(&e))?);
        if sample.len() >= COLUMN_SAMPLE {
            more = true;
            break;
        }
    }
    let mut columns = render::columns(&sample);
    if columns.is_empty() {
        // Nenhum documento: a grade vazia ainda mostra que rodou.
        columns.push("_id".into());
    }
    sink(QueryEvent::Columns(columns.clone()));
    let mut rows = 0u64;
    for doc in &sample {
        sink(QueryEvent::Row(render::row(doc, &columns).0));
        rows += 1;
    }
    drop(sample);
    let mut missing: Vec<String> = Vec::new();
    while more && cursor.advance().await.map_err(|e| error_text(&e))? {
        let doc = cursor.deserialize_current().map_err(|e| error_text(&e))?;
        let (cells, extra) = render::row(&doc, &columns);
        for field in extra {
            if missing.len() < 20 && !missing.iter().any(|m| m == field) {
                missing.push(field.to_string());
            }
        }
        sink(QueryEvent::Row(cells));
        rows += 1;
    }
    if !missing.is_empty() {
        sink(notice(trf!(
            "Fields that only appear after the first {n} documents are not in the grid: {fields}. Add a projection to see them.",
            n = COLUMN_SAMPLE,
            fields = missing.join(", ")
        )));
    }
    sink(QueryEvent::CommandComplete { rows });
    Ok(())
}

/// Os erros de escrita que vêm dentro de uma resposta `ok: 1`.
fn write_errors(reply: &Document) -> Option<Failure> {
    let first = reply
        .get_array("writeErrors")
        .ok()
        .and_then(|errors| errors.first())
        .and_then(Bson::as_document)
        .or_else(|| reply.get_document("writeConcernError").ok())?;
    Some((
        first.get_str("errmsg").unwrap_or("write error").to_string(),
        first
            .get_str("codeName")
            .map(str::to_string)
            .unwrap_or_else(|_| first.get("code").map(|c| c.to_string()).unwrap_or_default()),
    ))
}

fn count(reply: &Document, field: &str) -> u64 {
    match reply.get(field) {
        Some(Bson::Int32(n)) => u64::try_from(*n).unwrap_or(0),
        Some(Bson::Int64(n)) => u64::try_from(*n).unwrap_or(0),
        Some(Bson::Double(n)) => *n as u64,
        _ => 0,
    }
}

/// A resposta de um comando de resposta única, conforme `shape`.
fn show(reply: Document, shape: &Shape, sink: &mut QuerySink) -> Result<(), Failure> {
    match shape {
        Shape::Reply => {
            sink(QueryEvent::Columns(vec!["field".into(), "value".into()]));
            let mut rows = 0;
            for (field, value) in &reply {
                // O que o servidor acrescenta em toda resposta de um
                // replica set não é da pergunta.
                if matches!(field.as_str(), "$clusterTime" | "operationTime") {
                    continue;
                }
                sink(QueryEvent::Row(vec![
                    Some(field.clone()),
                    render::cell(value),
                ]));
                rows += 1;
            }
            sink(QueryEvent::CommandComplete { rows });
        }
        Shape::List(field) => {
            let docs: Vec<&Document> = reply
                .get_array(field)
                .map(|list| list.iter().filter_map(Bson::as_document).collect())
                .unwrap_or_default();
            let columns = render::columns(docs.iter().copied());
            sink(QueryEvent::Columns(columns.clone()));
            for doc in &docs {
                sink(QueryEvent::Row(render::row(doc, &columns).0));
            }
            sink(QueryEvent::CommandComplete {
                rows: docs.len() as u64,
            });
        }
        Shape::Value { field, column } => {
            sink(QueryEvent::Columns(vec![(*column).to_string()]));
            sink(QueryEvent::Row(vec![
                reply.get(field).and_then(render::cell),
            ]));
            sink(QueryEvent::CommandComplete { rows: 1 });
        }
        Shape::Values(field) => {
            let values = reply.get_array(field).cloned().unwrap_or_default();
            sink(QueryEvent::Columns(vec!["value".into()]));
            for value in &values {
                sink(QueryEvent::Row(vec![render::cell(value)]));
            }
            sink(QueryEvent::CommandComplete {
                rows: values.len() as u64,
            });
        }
        Shape::Write(kind) => {
            if let Some(failure) = write_errors(&reply) {
                return Err(failure);
            }
            let n = count(&reply, "n");
            // A mensagem já diz quantos documentos mudaram.
            let message = match kind {
                Write::Insert => trf!("{n} document(s) inserted.", n = n),
                Write::Delete => trf!("{n} document(s) deleted.", n = n),
                Write::Update => {
                    let modified = count(&reply, "nModified");
                    let upserted = reply
                        .get_array("upserted")
                        .ok()
                        .and_then(|u| u.first())
                        .and_then(Bson::as_document)
                        .and_then(|u| u.get("_id"))
                        .and_then(render::cell);
                    match upserted {
                        Some(id) => {
                            trf!("1 document inserted by the upsert (_id {id}).", id = id)
                        }
                        None => trf!(
                            "{matched} document(s) matched, {modified} modified.",
                            matched = n,
                            modified = modified
                        ),
                    }
                }
            };
            sink(notice(message));
        }
        Shape::Done => sink(notice(tr("Command completed.").to_string())),
    }
    Ok(())
}
