//! Execução dos comandos das janelas de consulta no Redis.
//!
//! Cada comando do texto ([`crate::command`]) vai ao servidor e a resposta
//! vira [`QueryEvent`]s ([`crate::render`]): status como mensagem, o resto
//! numa grade. Um erro num comando não impede os seguintes, como no
//! redis-cli. `SUBSCRIBE`, `PSUBSCRIBE` e `MONITOR` seguem mostrando o que
//! chega até o usuário cancelar.

use std::sync::{Arc, Mutex};

use pgstudio_core::connection::{ConnectionSettings, Engine};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_i18n::{tr, trf};
use pgstudio_pg_client::DbRuntime;
use tokio::sync::Notify;

use crate::Connection;
use crate::command::{self, Command};
use crate::render::{Rendered, render};
use crate::resp::Value;

/// Abre uma [`RedisQuerySession`] por janela de consulta.
pub struct RedisQueryService {
    settings: ConnectionSettings,
    password: String,
    runtime: DbRuntime,
}

impl RedisQueryService {
    pub fn new(settings: ConnectionSettings, password: String, runtime: DbRuntime) -> Self {
        RedisQueryService {
            settings,
            password,
            runtime,
        }
    }
}

impl QueryService for RedisQueryService {
    fn engine(&self) -> Engine {
        Engine::Redis
    }

    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        let mut settings = self.settings.clone();
        settings.database = database.to_string();
        Arc::new(RedisQuerySession {
            settings,
            password: self.password.clone(),
            runtime: self.runtime.clone(),
            shared: Arc::default(),
        })
    }
}

/// A conexão de uma janela de consulta. Abre na primeira execução e reabre
/// se cair.
pub struct RedisQuerySession {
    settings: ConnectionSettings,
    password: String,
    runtime: DbRuntime,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// A conexão; o lock fica com a execução em andamento.
    state: tokio::sync::Mutex<State>,
    /// O aviso de cancelamento da execução em andamento.
    cancel: Mutex<Option<Arc<Notify>>>,
}

#[derive(Default)]
struct State {
    connection: Option<Connection>,
    /// Dentro de um `MULTI`.
    in_transaction: bool,
}

impl QuerySession for RedisQuerySession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let shared = self.shared.clone();
        let settings = self.settings.clone();
        let password = self.password.clone();
        let cancel = Arc::new(Notify::new());
        if let Ok(mut c) = shared.cancel.lock() {
            *c = Some(cancel.clone());
        }
        self.runtime.handle().spawn(async move {
            let commands = match command::split(&sql) {
                Ok(c) => c,
                Err(e) => {
                    sink(QueryEvent::Error(ServerMessage {
                        severity: "ERROR".into(),
                        message: e.message,
                        position: Some(e.position + 1),
                        ..Default::default()
                    }));
                    sink(QueryEvent::Finished { canceled: false });
                    return;
                }
            };
            let mut state = shared.state.lock().await;
            let canceled = tokio::select! {
                _ = run(&mut state, &settings, &password, &commands, &mut sink) => false,
                _ = cancel.notified() => {
                    // A resposta do comando em andamento ficou pela metade:
                    // a conexão não serve mais.
                    state.connection = None;
                    state.in_transaction = false;
                    true
                }
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
        self.settings.database.clone()
    }

    fn engine(&self) -> Engine {
        Engine::Redis
    }
}

fn error_event(message: String, command: &Command) -> QueryEvent {
    // `WRONGTYPE Operation against...`: a primeira palavra é o código.
    let code = message
        .split_whitespace()
        .next()
        .filter(|w| w.chars().all(|c| c.is_ascii_uppercase()))
        .unwrap_or("ERR")
        .to_string();
    QueryEvent::Error(ServerMessage {
        severity: "ERROR".into(),
        code,
        message,
        position: Some(command.start + 1),
        ..Default::default()
    })
}

fn notice(message: String) -> QueryEvent {
    QueryEvent::Notice(ServerMessage {
        message,
        ..Default::default()
    })
}

/// Roda os comandos, um a um.
async fn run(
    state: &mut State,
    settings: &ConnectionSettings,
    password: &str,
    commands: &[Command],
    sink: &mut QuerySink,
) {
    for command in commands {
        if command.is_client() {
            client_command(state, settings, password, command, sink).await;
            continue;
        }
        let connection = match connect(state, settings, password).await {
            Ok(c) => c,
            Err(e) => {
                sink(QueryEvent::Error(ServerMessage::error(e)));
                return;
            }
        };
        let value = match connection.call(&command.args).await {
            Ok(v) => v,
            Err(e) => {
                state.connection = None;
                state.in_transaction = false;
                sink(error_event(
                    trf!("The connection to the server was lost: {error}", error = e),
                    command,
                ));
                return;
            }
        };
        let failed = matches!(value, Value::Error(_));
        if !failed {
            match command.name().as_str() {
                "MULTI" => state.in_transaction = true,
                "EXEC" | "DISCARD" => state.in_transaction = false,
                _ => {}
            }
        }
        if command.is_stream() && !failed {
            stream(state, command, value, sink).await;
            return;
        }
        // O número que um comando de escrita devolve (campos criados,
        // chaves apagadas) é mensagem, como o `OK`: uma grade para cada um
        // esconderia as grades das leituras.
        let rendered = match value {
            Value::Int(n) if !command.is_read_only() => Rendered::Status(format!("(integer) {n}")),
            value => render(command, value),
        };
        emit(rendered, command, sink);
    }
}

fn emit(rendered: Rendered, command: &Command, sink: &mut QuerySink) {
    match rendered {
        Rendered::Status(s) => sink(notice(s)),
        Rendered::Error(e) => sink(error_event(e, command)),
        Rendered::Grid {
            columns,
            rows,
            note,
        } => {
            let count = rows.len() as u64;
            sink(QueryEvent::Columns(columns));
            for row in rows {
                sink(QueryEvent::Row(row));
            }
            sink(QueryEvent::CommandComplete { rows: count });
            if let Some(note) = note {
                sink(notice(note));
            }
        }
    }
}

/// `SUBSCRIBE`, `PSUBSCRIBE`, `MONITOR`: mostra o que chega, uma linha por
/// mensagem, até a execução ser cancelada (o que fecha a conexão).
async fn stream(state: &mut State, command: &Command, first: Value, sink: &mut QuerySink) {
    let monitor = command.name() == "MONITOR";
    let columns: &[&str] = if monitor {
        &["event"]
    } else {
        &["kind", "channel", "message"]
    };
    sink(QueryEvent::Columns(
        columns.iter().map(|c| (*c).to_string()).collect(),
    ));
    sink(notice(
        tr("Listening. Cancel the query to stop.").to_string(),
    ));
    let row = |value: Value| -> Option<Vec<Option<String>>> {
        if monitor {
            return match value {
                // O `OK` do começo.
                Value::Simple(s) if s == "OK" => None,
                other => Some(vec![other.to_cell()]),
            };
        }
        let mut items: Vec<Option<String>> =
            value.into_items().iter().map(Value::to_cell).collect();
        // `pmessage` traz o padrão antes do canal.
        if items.len() == 4 {
            items.remove(1);
        }
        items.resize(3, None);
        Some(items)
    };
    if let Some(r) = row(first) {
        sink(QueryEvent::Row(r));
    }
    // Um SUBSCRIBE com vários canais confirma um a um; depois vêm as
    // mensagens. Tudo é linha.
    loop {
        let Some(connection) = state.connection.as_mut() else {
            return;
        };
        match connection.read().await {
            Ok(value) => {
                if let Some(r) = row(value) {
                    sink(QueryEvent::Row(r));
                }
            }
            Err(e) => {
                state.connection = None;
                sink(error_event(
                    trf!("The connection to the server was lost: {error}", error = e),
                    command,
                ));
                return;
            }
        }
    }
}

async fn connect<'a>(
    state: &'a mut State,
    settings: &ConnectionSettings,
    password: &str,
) -> Result<&'a mut Connection, String> {
    if state.connection.is_none() {
        state.in_transaction = false;
        state.connection = Some(
            Connection::connect(settings, password)
                .await
                .map_err(|e| e.message().to_string())?,
        );
    }
    state
        .connection
        .as_mut()
        .ok_or_else(|| "no connection".to_string())
}

/// Comandos `POSTPILOT ...`, que a interface usa e não vão ao servidor
/// com este nome.
async fn client_command(
    state: &mut State,
    settings: &ConnectionSettings,
    password: &str,
    command: &Command,
    sink: &mut QuerySink,
) {
    match command.sub().as_str() {
        "DATABASES" => {
            let count = match connect(state, settings, password).await {
                Ok(c) => c.database_count().await,
                Err(e) => Err(e),
            };
            match count {
                Ok(n) => emit(
                    Rendered::Grid {
                        columns: vec!["database".into()],
                        rows: (0..n).map(|i| vec![Some(format!("db{i}"))]).collect(),
                        note: None,
                    },
                    command,
                    sink,
                ),
                Err(e) => {
                    state.connection = None;
                    sink(QueryEvent::Error(ServerMessage::error(e)));
                }
            }
        }
        "KEYS" => {
            let pattern = command.text(2).unwrap_or_else(|| "*".into());
            if let Err(e) = keys(state, settings, password, &pattern, sink).await {
                state.connection = None;
                sink(QueryEvent::Error(ServerMessage::error(e)));
            }
        }
        "INTRANSACTION" => emit(
            Rendered::Grid {
                columns: vec!["in_transaction".into()],
                rows: vec![vec![Some(u8::from(state.in_transaction).to_string())]],
                note: None,
            },
            command,
            sink,
        ),
        other => sink(error_event(
            format!("ERR unknown PostPilot command '{other}'"),
            command,
        )),
    }
}

/// `POSTPILOT KEYS padrão`: as chaves que casam com o padrão, uma linha
/// por chave, com o tipo e o valor (`key`, `type`, `value`). É a "tabela"
/// do Redis para a consulta entre servidores. Texto sai como está; hash,
/// lista, set e sorted set saem em JSON (o hash como objeto). Percorre com
/// `SCAN`, que não trava o servidor como o `KEYS`.
async fn keys(
    state: &mut State,
    settings: &ConnectionSettings,
    password: &str,
    pattern: &str,
    sink: &mut QuerySink,
) -> Result<(), String> {
    let connection = connect(state, settings, password).await?;
    sink(QueryEvent::Columns(vec![
        "key".into(),
        "type".into(),
        "value".into(),
    ]));
    let mut seen = std::collections::HashSet::new();
    let mut cursor = "0".to_string();
    let mut rows = 0u64;
    loop {
        let reply = connection
            .query(&["SCAN", &cursor, "MATCH", pattern, "COUNT", "1000"])
            .await?;
        let mut parts = reply.into_items();
        let found = parts.pop().unwrap_or(Value::Null).into_items();
        cursor = parts
            .pop()
            .and_then(|c| c.to_cell())
            .unwrap_or_else(|| "0".into());
        // O `SCAN` pode repetir chaves.
        let batch: Vec<String> = found
            .into_iter()
            .filter_map(|k| k.to_cell())
            .filter(|k| seen.insert(k.clone()))
            .collect();
        // Os tipos de todas de uma vez, depois os valores de todas.
        for key in &batch {
            connection
                .send(&[b"TYPE".to_vec(), key.clone().into_bytes()])
                .await
                .map_err(|e| e.to_string())?;
        }
        let mut types = Vec::with_capacity(batch.len());
        for _ in &batch {
            let ty = connection.read().await.map_err(|e| e.to_string())?;
            types.push(ty.to_cell().unwrap_or_default());
        }
        for (key, ty) in batch.iter().zip(&types) {
            let args: Vec<&str> = match ty.as_str() {
                "string" => vec!["GET", key],
                "hash" => vec!["HGETALL", key],
                "list" => vec!["LRANGE", key, "0", "-1"],
                "set" => vec!["SMEMBERS", key],
                "zset" => vec!["ZRANGE", key, "0", "-1", "WITHSCORES"],
                // Streams e tipos de módulos: só a chave e o tipo. O
                // `EXISTS` mantém uma resposta por chave.
                _ => vec!["EXISTS", key],
            };
            let args: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
            connection.send(&args).await.map_err(|e| e.to_string())?;
        }
        for (key, ty) in batch.iter().zip(&types) {
            let value = connection.read().await.map_err(|e| e.to_string())?;
            let cell = match (ty.as_str(), value) {
                (_, Value::Error(_) | Value::Null) => None,
                ("string", v) => v.to_cell(),
                // No RESP2 o hash vem como lista de campo e valor.
                ("hash", v) => Value::Map(v.into_pairs()).to_cell(),
                ("list" | "set" | "zset", v) => v.to_cell(),
                _ => None,
            };
            sink(QueryEvent::Row(vec![
                Some(key.clone()),
                Some(ty.clone()),
                cell,
            ]));
            rows += 1;
        }
        if cursor == "0" {
            break;
        }
    }
    sink(QueryEvent::CommandComplete { rows });
    Ok(())
}
