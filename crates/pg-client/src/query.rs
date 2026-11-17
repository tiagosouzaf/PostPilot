//! Execução de consultas das janelas de consulta, sobre o protocolo simples
//! do PostgreSQL: vários comandos por execução, resultados em texto e
//! avisos do servidor no meio.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use pgstudio_core::connection::ConnectionSettings;
use pgstudio_core::query::{
    ColumnSource, DescribeDone, Pace, QueryEvent, QueryService, QuerySession, QuerySink,
    ServerMessage,
};
use tokio_postgres::SimpleQueryMessage;
use tokio_postgres::error::{DbError, ErrorPosition, SqlState};

use crate::{Canceller, DbRuntime, Session, describe};

/// Abre uma [`PgQuerySession`] por janela de consulta, com as credenciais
/// da conexão do Object Explorer.
pub struct PgQueryService {
    settings: ConnectionSettings,
    password: String,
    runtime: DbRuntime,
}

impl PgQueryService {
    pub fn new(settings: ConnectionSettings, password: String, runtime: DbRuntime) -> Self {
        PgQueryService {
            settings,
            password,
            runtime,
        }
    }
}

impl QueryService for PgQueryService {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        let mut settings = self.settings.clone();
        settings.database = database.to_string();
        Arc::new(PgQuerySession {
            settings,
            password: self.password.clone(),
            runtime: self.runtime.clone(),
            shared: Arc::default(),
        })
    }
}

/// A conexão de uma janela de consulta. Abre na primeira execução e reabre
/// se cair.
pub struct PgQuerySession {
    settings: ConnectionSettings,
    password: String,
    runtime: DbRuntime,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// A conexão; o lock fica com a execução em andamento.
    session: tokio::sync::Mutex<Option<Session>>,
    /// Fora do lock, para cancelar com a sessão ocupada.
    canceller: Mutex<Option<Canceller>>,
    cancel_requested: AtomicBool,
    /// A execução em andamento busca as linhas aos poucos.
    pace: Mutex<Option<Pace>>,
}

impl Shared {
    /// Uma execução parada à espera de mais linhas segura a conexão: quem
    /// precisa dela (outra execução, um cancelamento) faz a parada
    /// desistir do resto. Devolve se havia uma.
    fn give_up_paused(&self) -> bool {
        let pace = self.pace.lock().ok().and_then(|p| p.clone());
        match pace {
            Some(p) if p.is_waiting() => {
                p.abandon();
                true
            }
            _ => false,
        }
    }
}

impl PgQuerySession {
    fn start(&self, sql: String, mut sink: QuerySink, pace: Option<Pace>) {
        self.shared.give_up_paused();
        let shared = self.shared.clone();
        let settings = self.settings.clone();
        let password = self.password.clone();
        self.runtime.handle().spawn(async move {
            let mut guard = shared.session.lock().await;
            shared.cancel_requested.store(false, Ordering::SeqCst);
            if guard.as_ref().is_none_or(Session::is_closed) {
                *guard = None;
                match Session::connect(&settings, &password).await {
                    Ok(session) => {
                        if let Ok(mut c) = shared.canceller.lock() {
                            *c = Some(session.canceller());
                        }
                        *guard = Some(session);
                    }
                    Err(e) => {
                        sink(QueryEvent::Error(ServerMessage::error(e.message())));
                        sink(QueryEvent::Finished { canceled: false });
                        return;
                    }
                }
            }
            let canceled = match guard.as_ref() {
                Some(session) => {
                    // Desistir de uma consulta parada cancela o comando, e
                    // dentro de uma transação isso a deixaria abortada:
                    // ali o resultado vem inteiro, como sempre.
                    let pace = match pace {
                        Some(p) if !in_transaction(session).await => Some(p),
                        _ => None,
                    };
                    if let Ok(mut p) = shared.pace.lock() {
                        *p = pace.clone();
                    }
                    let canceled = run(
                        session,
                        &sql,
                        &mut sink,
                        &shared.cancel_requested,
                        pace.as_ref(),
                    )
                    .await;
                    if let Ok(mut p) = shared.pace.lock() {
                        *p = None;
                    }
                    canceled
                }
                None => false,
            };
            sink(QueryEvent::Finished { canceled });
        });
    }
}

/// Se a sessão está dentro de uma transação aberta por um comando
/// anterior (uma abortada também conta).
async fn in_transaction(session: &Session) -> bool {
    let sql = pgstudio_core::connection::Engine::PostgreSql.in_transaction_sql();
    match session.client().simple_query(sql).await {
        Ok(messages) => messages.iter().any(|m| match m {
            SimpleQueryMessage::Row(row) => row.get(0) != Some("f"),
            _ => false,
        }),
        Err(_) => true,
    }
}

impl QuerySession for PgQuerySession {
    fn execute(&self, sql: String, sink: QuerySink) {
        self.start(sql, sink, None);
    }

    fn execute_paced(&self, sql: String, sink: QuerySink, pace: Pace) {
        self.start(sql, sink, Some(pace));
    }

    fn cancel(&self) {
        if self.shared.give_up_paused() {
            return;
        }
        let canceller = self.shared.canceller.lock().ok().and_then(|c| c.clone());
        if let Some(canceller) = canceller {
            self.shared.cancel_requested.store(true, Ordering::SeqCst);
            self.runtime.handle().spawn(async move {
                // Se falhar, a consulta só continua; não há o que avisar.
                let _ = canceller.cancel().await;
            });
        }
    }

    fn database(&self) -> String {
        self.settings.database.clone()
    }

    fn describe(&self, sql: String, done: DescribeDone) {
        self.shared.give_up_paused();
        let shared = self.shared.clone();
        self.runtime.handle().spawn(async move {
            let guard = shared.session.lock().await;
            let result = match guard.as_ref() {
                Some(session) if !session.is_closed() => session
                    .client()
                    .prepare(&sql)
                    .await
                    .map(|statement| {
                        statement
                            .columns()
                            .iter()
                            .map(|c| match (c.table_oid(), c.column_id()) {
                                (Some(table), Some(column)) if table != 0 && column > 0 => {
                                    Some(ColumnSource { table, column })
                                }
                                _ => None,
                            })
                            .collect()
                    })
                    .map_err(|e| error_message(&e).message),
                _ => Err("The connection is closed.".to_string()),
            };
            done(result);
        });
    }
}

/// Roda `sql` e manda os eventos. Devolve se a execução foi cancelada.
async fn run(
    session: &Session,
    sql: &str,
    sink: &mut QuerySink,
    cancel: &AtomicBool,
    pace: Option<&Pace>,
) -> bool {
    // A interface desistiu do resto do resultado: o comando foi cancelado
    // e o que ainda chegar é jogado fora.
    let mut dropped = false;
    let notices = |sink: &mut QuerySink| {
        for n in session.take_notices() {
            sink(QueryEvent::Notice(db_message(&n)));
        }
    };
    notices(sink);
    let failed = |e: tokio_postgres::Error, sink: &mut QuerySink| {
        if cancel.load(Ordering::SeqCst) && e.code() == Some(&SqlState::QUERY_CANCELED) {
            return true;
        }
        sink(QueryEvent::Error(error_message(&e)));
        false
    };
    let stream = match session.client().simple_query_raw(sql).await {
        Ok(s) => s,
        Err(e) => return failed(e, sink),
    };
    let mut stream = pin!(stream);
    while let Some(item) = stream.next().await {
        notices(sink);
        match item {
            Ok(SimpleQueryMessage::RowDescription(columns)) => {
                sink(QueryEvent::Columns(
                    columns.iter().map(|c| c.name().to_string()).collect(),
                ));
            }
            Ok(SimpleQueryMessage::Row(_)) if dropped => {}
            Ok(SimpleQueryMessage::Row(row)) => {
                if let Some(pace) = pace
                    && !pace.row().await
                {
                    dropped = true;
                    cancel.store(true, Ordering::SeqCst);
                    // Se falhar, o resto do resultado só é lido e jogado
                    // fora.
                    let _ = session.canceller().cancel().await;
                    continue;
                }
                let values = (0..row.len())
                    .map(|i| row.get(i).map(str::to_string))
                    .collect();
                sink(QueryEvent::Row(values));
            }
            Ok(SimpleQueryMessage::CommandComplete(_)) if dropped => {}
            Ok(SimpleQueryMessage::CommandComplete(rows)) => {
                sink(QueryEvent::CommandComplete { rows });
            }
            Ok(_) => {}
            Err(e) => {
                notices(sink);
                return failed(e, sink);
            }
        }
    }
    notices(sink);
    dropped
}

/// Erro do driver ou do servidor como mensagem para a interface.
pub fn error_message(e: &tokio_postgres::Error) -> ServerMessage {
    match e.as_db_error() {
        Some(db) => db_message(db),
        None => ServerMessage::error(describe(e)),
    }
}

fn db_message(db: &DbError) -> ServerMessage {
    ServerMessage {
        severity: db.severity().to_string(),
        code: db.code().code().to_string(),
        message: db.message().to_string(),
        detail: db.detail().map(str::to_string),
        hint: db.hint().map(str::to_string),
        position: match db.position() {
            Some(ErrorPosition::Original(p)) => Some(*p as usize),
            _ => None,
        },
    }
}
