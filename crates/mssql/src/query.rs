//! Execução de consultas das janelas de consulta no SQL Server.
//!
//! O script é dividido no `GO` ([`crate::script`]); cada lote vai inteiro
//! ao servidor, e tudo o que ele devolve (resultados, contagens, `PRINT`,
//! erros) vira [`QueryEvent`]s, na ordem. Um erro num lote não impede os
//! seguintes, como no SSMS.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use pgstudio_core::connection::{ConnectionSettings, Engine};
use pgstudio_core::query::{
    ColumnSource, DescribeDone, QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage,
};
use pgstudio_pg_client::DbRuntime;
use tiberius::{BatchItem, BatchMessage, CancelHandle};

use crate::script::{self, Batch};
use crate::{Session, describe, value};

/// Mensagens de troca de contexto que o SSMS não mostra.
const HIDDEN_MESSAGES: [u32; 2] = [
    5701, // Changed database context to ...
    5703, // Changed language setting to ...
];

/// Abre uma [`MssqlQuerySession`] por janela de consulta.
pub struct MssqlQueryService {
    settings: ConnectionSettings,
    password: String,
    runtime: DbRuntime,
}

impl MssqlQueryService {
    pub fn new(settings: ConnectionSettings, password: String, runtime: DbRuntime) -> Self {
        MssqlQueryService {
            settings,
            password,
            runtime,
        }
    }
}

impl QueryService for MssqlQueryService {
    fn engine(&self) -> Engine {
        Engine::SqlServer
    }

    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        let mut settings = self.settings.clone();
        settings.database = database.to_string();
        Arc::new(MssqlQuerySession {
            settings,
            password: self.password.clone(),
            runtime: self.runtime.clone(),
            shared: Arc::default(),
        })
    }
}

/// A conexão de uma janela de consulta. Abre na primeira execução e reabre
/// se cair.
pub struct MssqlQuerySession {
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
    cancel: Mutex<Option<CancelHandle>>,
}

impl QuerySession for MssqlQuerySession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let shared = self.shared.clone();
        let settings = self.settings.clone();
        let password = self.password.clone();
        self.runtime.handle().spawn(async move {
            let mut guard = shared.session.lock().await;
            if guard.is_none() {
                match Session::connect(&settings, &password).await {
                    Ok(session) => {
                        if let Ok(mut c) = shared.cancel.lock() {
                            *c = Some(session.cancel_handle());
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
            let Some(session) = guard.as_mut() else {
                return;
            };
            session.cancel_handle().reset();
            let (canceled, broken) = run(session, &sql, &mut sink).await;
            if broken {
                *guard = None;
            } else if canceled && turns_plans_on(&sql) {
                // O cancelamento pulou o `SET ... OFF` do fim: sem isto a
                // sessão ficaria devolvendo planos em vez de executar.
                session.cancel_handle().reset();
                let mut quiet: QuerySink = Box::new(|_| {});
                let off = "SET SHOWPLAN_XML OFF\nGO\nSET STATISTICS XML OFF";
                if run(session, off, &mut quiet).await.1 {
                    *guard = None;
                }
            }
            sink(QueryEvent::Finished { canceled });
        });
    }

    fn cancel(&self) {
        if let Some(handle) = self.shared.cancel.lock().ok().and_then(|c| c.clone()) {
            handle.cancel();
        }
    }

    fn database(&self) -> String {
        self.settings.database.clone()
    }

    fn engine(&self) -> Engine {
        Engine::SqlServer
    }

    fn describe(&self, sql: String, done: DescribeDone) {
        let shared = self.shared.clone();
        let settings = self.settings.clone();
        let password = self.password.clone();
        self.runtime.handle().spawn(async move {
            let mut guard = shared.session.lock().await;
            if guard.is_none() {
                match Session::connect(&settings, &password).await {
                    Ok(session) => {
                        if let Ok(mut c) = shared.cancel.lock() {
                            *c = Some(session.cancel_handle());
                        }
                        *guard = Some(session);
                    }
                    Err(e) => return done(Err(e.message().to_string())),
                }
            }
            let Some(session) = guard.as_mut() else {
                return;
            };
            done(
                session
                    .rows(&describe_sql(&sql))
                    .await
                    .map(|rows| sources(&rows)),
            );
        });
    }
}

/// Se o script liga o plano em XML (`SET SHOWPLAN_XML ON` ou `SET
/// STATISTICS XML ON`).
fn turns_plans_on(sql: &str) -> bool {
    script::split(sql).iter().any(|b| {
        let words: Vec<String> = b
            .sql
            .split_whitespace()
            .map(|w| w.trim_end_matches(';').to_ascii_uppercase())
            .collect();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        matches!(
            words.as_slice(),
            ["SET", "SHOWPLAN_XML", "ON"] | ["SET", "STATISTICS", "XML", "ON"]
        )
    })
}

/// De onde vem cada coluna do resultado de `sql`, sem rodar:
/// `sys.dm_exec_describe_first_result_set` com as informações de origem
/// (modo 1) dá a tabela (`object_id`) e o `column_id`. Colunas de outro
/// banco ou servidor ficam sem origem; as escondidas (a chave que o modo 1
/// acrescenta) ficam de fora.
pub fn describe_sql(sql: &str) -> String {
    format!(
        "SELECT o.id, COLUMNPROPERTY(o.id, d.source_column, 'ColumnId') \
           FROM sys.dm_exec_describe_first_result_set(N'{}', NULL, 1) d \
          CROSS APPLY (SELECT CASE WHEN d.source_server IS NULL \
                                    AND d.source_database = DB_NAME() \
                               THEN OBJECT_ID(QUOTENAME(d.source_schema) + '.' \
                                              + QUOTENAME(d.source_table)) END AS id) o \
          WHERE d.is_hidden = 0 \
          ORDER BY d.column_ordinal",
        sql.replace('\'', "''")
    )
}

/// As linhas de [`describe_sql`] como origem das colunas.
fn sources(rows: &[Vec<Option<String>>]) -> Vec<Option<ColumnSource>> {
    rows.iter()
        .map(|r| {
            let number = |i: usize| r.get(i).cloned().flatten()?.parse::<i64>().ok();
            Some(ColumnSource {
                table: u32::try_from(number(0)?).ok()?,
                column: i16::try_from(number(1)?).ok()?,
            })
        })
        .collect()
}

/// Roda o script. Devolve se foi cancelado e se a conexão não serve mais.
async fn run(session: &mut Session, sql: &str, sink: &mut QuerySink) -> (bool, bool) {
    let cancel = session.cancel_handle();
    for batch in script::split(sql) {
        for _ in 0..batch.repeat {
            if cancel.is_requested() {
                return (true, false);
            }
            match run_batch(session, &batch, sink).await {
                Ok(true) => return (true, false),
                Ok(false) => {}
                Err(message) => {
                    sink(QueryEvent::Error(ServerMessage::error(message)));
                    return (cancel.is_requested(), true);
                }
            }
        }
    }
    (cancel.is_requested(), false)
}

/// Roda um lote. `Ok(true)` quando o servidor confirmou um cancelamento;
/// `Err` quando a conexão caiu ou saiu de sincronia.
async fn run_batch(
    session: &mut Session,
    batch: &Batch,
    sink: &mut QuerySink,
) -> Result<bool, String> {
    let cancel = session.cancel_handle();
    let mut canceled = false;
    {
        let mut stream = session
            .client()
            .batch(batch.sql.as_str())
            .await
            .map_err(|e| describe(&e))?;
        // Linhas do resultado aberto, para a contagem quando o servidor não
        // manda (SET NOCOUNT ON).
        let mut open: Option<u64> = None;
        let mut types = Vec::new();
        while let Some(item) = stream.next().await {
            match item.map_err(|e| describe(&e))? {
                BatchItem::Metadata(columns) => {
                    types = columns.iter().map(|c| c.column_type()).collect();
                    open = Some(0);
                    sink(QueryEvent::Columns(
                        columns.iter().map(|c| c.name().to_string()).collect(),
                    ));
                }
                BatchItem::Row(row) => {
                    if let Some(n) = open.as_mut() {
                        *n += 1;
                    }
                    sink(QueryEvent::Row(
                        row.cells()
                            .zip(&types)
                            .map(|((_, data), ty)| value::to_text(data, *ty))
                            .collect(),
                    ));
                }
                BatchItem::Done {
                    attention: true, ..
                } => canceled = true,
                BatchItem::Done { rows, .. } => {
                    let read = open.take();
                    if let Some(rows) = rows.or(read) {
                        sink(QueryEvent::CommandComplete { rows });
                    }
                }
                BatchItem::Message(m) => {
                    if let Some(event) = message_event(&m, batch) {
                        if matches!(event, QueryEvent::Error(_)) {
                            open = None;
                        }
                        sink(event);
                    }
                }
                BatchItem::ReturnStatus(_) => {}
            }
        }
    }
    if cancel.is_pending() {
        session
            .client()
            .finish_cancel()
            .await
            .map_err(|e| describe(&e))?;
        canceled = true;
    }
    Ok(canceled || cancel.is_requested())
}

/// A mensagem do servidor como evento: erro com linha, ou aviso só com o
/// texto (como o `PRINT` aparece no SSMS).
fn message_event(m: &BatchMessage, batch: &Batch) -> Option<QueryEvent> {
    if !m.is_error {
        if HIDDEN_MESSAGES.contains(&m.number) {
            return None;
        }
        return Some(QueryEvent::Notice(ServerMessage {
            message: m.message.clone(),
            ..Default::default()
        }));
    }
    // Erro dentro de procedure: a linha é a da procedure, não do script.
    let position = (m.procedure.is_empty() && m.line > 0).then(|| batch.line_position(m.line));
    Some(QueryEvent::Error(ServerMessage {
        severity: m.class.to_string(),
        code: m.number.to_string(),
        message: m.message.clone(),
        detail: (!m.procedure.is_empty()).then(|| {
            pgstudio_i18n::trf!(
                "Procedure {procedure}, Line {line}",
                procedure = m.procedure,
                line = m.line
            )
        }),
        hint: None,
        position,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(is_error: bool, number: u32, line: u32, procedure: &str) -> BatchMessage {
        BatchMessage {
            number,
            state: 1,
            class: if is_error { 16 } else { 0 },
            message: "texto".into(),
            server: "srv".into(),
            procedure: procedure.into(),
            line,
            is_error,
        }
    }

    #[test]
    fn scripts_that_turn_plans_on_are_noticed() {
        assert!(turns_plans_on(
            "SET SHOWPLAN_XML ON\nGO\nSELECT 1\nGO\nSET SHOWPLAN_XML OFF"
        ));
        assert!(turns_plans_on("set statistics xml on;\nGO\nSELECT 1"));
        assert!(!turns_plans_on("SELECT 'SET SHOWPLAN_XML ON'"));
        assert!(!turns_plans_on("SET STATISTICS IO ON"));
    }

    #[test]
    fn prints_are_plain_and_context_changes_are_hidden() {
        let b = &script::split("SELECT 1")[0];
        match message_event(&msg(false, 0, 1, ""), b) {
            Some(QueryEvent::Notice(n)) => {
                assert_eq!(n.message, "texto");
                assert!(n.severity.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert!(message_event(&msg(false, 5701, 1, ""), b).is_none());
    }

    #[test]
    fn errors_carry_number_level_and_script_line() {
        let s = "SELECT 1\nGO\nSELECT 2\nSELECT * FROM nada";
        let b = &script::split(s)[1];
        match message_event(&msg(true, 208, 2, ""), b) {
            Some(QueryEvent::Error(e)) => {
                assert_eq!((e.code.as_str(), e.severity.as_str()), ("208", "16"));
                assert_eq!(&s[e.position.unwrap() - 1..], "SELECT * FROM nada");
            }
            other => panic!("{other:?}"),
        }
        match message_event(&msg(true, 50000, 7, "dbo.p"), b) {
            Some(QueryEvent::Error(e)) => {
                assert_eq!(e.position, None);
                assert!(e.detail.unwrap().contains("dbo.p"));
            }
            other => panic!("{other:?}"),
        }
    }
}
