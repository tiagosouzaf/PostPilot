//! Execução de consultas das janelas de consulta, sobre o protocolo simples
//! do PostgreSQL: vários comandos por execução, resultados em texto e
//! avisos do servidor no meio.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use pgstudio_core::connection::ConnectionSettings;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
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
}

impl QuerySession for PgQuerySession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
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
                Some(session) => run(session, &sql, &mut sink, &shared.cancel_requested).await,
                None => false,
            };
            sink(QueryEvent::Finished { canceled });
        });
    }

    fn cancel(&self) {
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
}

/// Roda `sql` e manda os eventos. Devolve se a execução foi cancelada.
async fn run(session: &Session, sql: &str, sink: &mut QuerySink, cancel: &AtomicBool) -> bool {
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
            Ok(SimpleQueryMessage::Row(row)) => {
                let values = (0..row.len())
                    .map(|i| row.get(i).map(str::to_string))
                    .collect();
                sink(QueryEvent::Row(values));
            }
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
    false
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
