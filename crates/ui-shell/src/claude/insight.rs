//! O plano de execução e a atividade do servidor para o Claude: as
//! ferramentas `explain_query` e `get_activity`.
//!
//! O plano estimado não roda nada. O real (`analyze`) executa o comando,
//! então segue a regra do `run_query` (só leitura roda na hora, o resto
//! pede confirmação) e roda sempre numa transação desfeita no fim. A
//! atividade usa as mesmas consultas do Activity Monitor.

use std::time::Instant;

use pgstudio_core::connection::Engine;
use pgstudio_core::query::QueryEvent;
use pgstudio_execution_plan::{Plan, SHOWPLAN_COLUMN, parse_showplan};
use serde_json::{Value, json};

use crate::activity::{self, Process, Snapshot};

/// Teto de texto de planos numa resposta.
const MAX_PLAN_BYTES: usize = 60 * 1024;
/// Teto do SQL de uma sessão ou de uma consulta cara.
const MAX_QUERY_CHARS: usize = 600;
/// Tetos das listas da atividade.
const MAX_SESSIONS: usize = 150;
const MAX_LOCKS: usize = 200;
/// Consultas mais caras devolvidas, e o teto do texto de cada uma.
const MAX_STATEMENTS: usize = 15;
const MAX_STATEMENT_CHARS: usize = 300;
/// Linhas guardadas por resultado (os locks de um servidor ocupado
/// passam fácil dos milhares).
const MAX_KEPT_ROWS: usize = 20_000;

/// Coluna do `EXPLAIN` em texto do PostgreSQL.
const PG_PLAN_COLUMN: &str = "QUERY PLAN";

/// Um resultado guardado inteiro.
#[derive(Debug, Default)]
pub(super) struct Set {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub row_count: u64,
}

/// Junta os eventos de uma execução sem cortar as células (um plano em
/// XML passa de 2000 caracteres), para o plano e a atividade.
#[derive(Debug, Default)]
pub(super) struct Gatherer {
    /// Só guarda as linhas dos resultados que são planos; dos outros,
    /// conta.
    plans_only: bool,
    pub sets: Vec<Set>,
    open: bool,
    pub notices: Vec<String>,
    pub error: Option<String>,
    pub canceled: bool,
    finished: bool,
}

impl Gatherer {
    /// Guarda todos os resultados.
    pub fn all() -> Self {
        Gatherer::default()
    }

    /// Guarda só os planos; os resultados do comando (que o plano real do
    /// SQL Server traz junto) ficam só na contagem.
    pub fn plans() -> Self {
        Gatherer {
            plans_only: true,
            ..Default::default()
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn push(&mut self, event: QueryEvent) {
        match event {
            QueryEvent::Columns(columns) => {
                self.sets.push(Set {
                    columns,
                    ..Default::default()
                });
                self.open = true;
            }
            QueryEvent::Row(row) => {
                let Some(set) = self.sets.last_mut().filter(|_| self.open) else {
                    return;
                };
                set.row_count += 1;
                let keep = !self.plans_only || is_plan_set(&set.columns);
                if keep && set.rows.len() < MAX_KEPT_ROWS {
                    set.rows.push(row);
                }
            }
            QueryEvent::CommandComplete { .. } => self.open = false,
            QueryEvent::Notice(m) => self.notices.push(m.message),
            QueryEvent::Error(m) => {
                let mut text = m.message;
                if let Some(d) = m.detail {
                    text.push_str(&format!(" ({d})"));
                }
                if let Some(h) = m.hint {
                    text.push_str(&format!(" Hint: {h}"));
                }
                // O primeiro erro é a causa; os seguintes são consequência.
                self.error.get_or_insert(text);
            }
            QueryEvent::Finished { canceled } => {
                self.canceled = canceled;
                self.finished = true;
            }
        }
    }
}

fn is_plan_set(columns: &[String]) -> bool {
    matches!(columns, [c] if c == PG_PLAN_COLUMN || c == SHOWPLAN_COLUMN)
}

/// O lote que pede o plano de `sql`. `None` quando não há comando.
///
/// PostgreSQL: `EXPLAIN` antes de cada comando; o estimado e o real de
/// leitura numa transação `READ ONLY`, o real de escrita (já confirmado)
/// entre `BEGIN` e `ROLLBACK`. SQL Server: `SHOWPLAN_XML` para o
/// estimado e `STATISTICS XML`, numa transação sempre desfeita, para o
/// real.
pub(super) fn plan_batch(
    engine: Engine,
    sql: &str,
    analyze: bool,
    read_only: bool,
) -> Option<String> {
    let ranges = pgstudio_sql_editor::lexer::statements(sql);
    if ranges.is_empty() {
        return None;
    }
    Some(match engine {
        // Sem plano de execução no Redis.
        Engine::Redis | Engine::MongoDb | Engine::Elasticsearch => return None,
        Engine::PostgreSql => {
            let options = if analyze {
                "EXPLAIN (ANALYZE, BUFFERS) "
            } else {
                "EXPLAIN "
            };
            let body = ranges
                .into_iter()
                .map(|r| format!("{options}{}", &sql[r]))
                .collect::<Vec<_>>()
                .join(";\n");
            if !analyze || read_only {
                pgstudio_automation::safety::read_only_batch(&body)
            } else {
                format!("BEGIN;\n{body}\n;\nROLLBACK;")
            }
        }
        Engine::SqlServer if !analyze => {
            format!("SET SHOWPLAN_XML ON\nGO\n{sql}\nGO\nSET SHOWPLAN_XML OFF")
        }
        Engine::SqlServer => format!(
            "SET STATISTICS XML ON\nGO\n\
             SET XACT_ABORT ON; SET LOCK_TIMEOUT 60000; BEGIN TRANSACTION\nGO\n\
             {sql}\nGO\n\
             IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION\nGO\n\
             SET STATISTICS XML OFF"
        ),
    })
}

/// Os comandos de `sql`, na ordem, para o cabeçalho de cada plano.
pub(super) fn statements(sql: &str) -> Vec<String> {
    pgstudio_sql_editor::lexer::statements(sql)
        .into_iter()
        .map(|r| sql[r].trim().to_string())
        .collect()
}

/// A resposta do `explain_query` e se ela é um erro.
pub(super) fn plan_reply(
    engine: Engine,
    statements: &[String],
    analyze: bool,
    g: &Gatherer,
) -> (Value, bool) {
    let mut plans: Vec<Value> = Vec::new();
    let mut budget = MAX_PLAN_BYTES;
    let mut result_rows: Vec<u64> = Vec::new();
    let mut unread = 0;
    let mut push = |statement: &str, text: String, plans: &mut Vec<Value>| {
        let (text, cut) = cut_bytes(&text, budget);
        budget -= text.len();
        let mut v = json!({ "statement": one_line(statement, MAX_QUERY_CHARS), "plan": text });
        if cut {
            v["truncated"] = json!(true);
        }
        plans.push(v);
    };
    for set in &g.sets {
        match set.columns.as_slice() {
            [c] if c == PG_PLAN_COLUMN => {
                let text = set
                    .rows
                    .iter()
                    .map(|r| r.first().cloned().flatten().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n");
                let statement = statements.get(plans.len()).map_or("", String::as_str);
                push(statement, text, &mut plans);
            }
            [c] if c == SHOWPLAN_COLUMN => {
                for row in &set.rows {
                    let Some(xml) = row.first().and_then(|v| v.as_deref()) else {
                        continue;
                    };
                    match parse_showplan(xml) {
                        Ok(parsed) => {
                            for plan in &parsed {
                                push(&plan.statement, render_plan(plan), &mut plans);
                            }
                        }
                        Err(_) => unread += 1,
                    }
                }
            }
            _ => result_rows.push(set.row_count),
        }
    }
    let mut v = json!({
        "engine": match engine {
            Engine::PostgreSql => "postgresql",
            Engine::SqlServer => "sqlserver",
            Engine::Redis => "redis",
            Engine::MongoDb => "mongodb",
                        Engine::Elasticsearch => "elasticsearch",
        },
        "kind": if analyze { "actual" } else { "estimated" },
        "plans": plans,
    });
    if analyze {
        v["executed"] = json!(true);
        v["rolled_back"] = json!(true);
        if !result_rows.is_empty() {
            v["result_rows"] = json!(result_rows);
        }
    }
    if !g.notices.is_empty() {
        v["messages"] = json!(g.notices);
    }
    if unread > 0 {
        v["unreadable_plans"] = json!(unread);
    }
    let mut failed = false;
    if let Some(e) = &g.error {
        v["error"] = json!(e);
        failed = true;
    } else if g.canceled {
        v["error"] = json!("The execution was canceled.");
        failed = true;
    } else if v["plans"].as_array().is_some_and(Vec::is_empty) {
        v["error"] = json!("The server returned no plan for this SQL.");
        failed = true;
    }
    (v, failed)
}

/// O plano do SQL Server como texto, um operador por linha, no jeito do
/// `EXPLAIN` do PostgreSQL: custo e linhas estimadas, os números reais
/// quando há, predicados e avisos; no fim, o que o servidor disse do
/// comando (memória, paralelismo, índices que faltam).
pub(super) fn render_plan(plan: &Plan) -> String {
    const NODE_PROPERTIES: &[&str] = &[
        "Predicate",
        "Seek Predicates",
        "Hash Keys Probe",
        "Order By",
        "Group By",
        "Warnings",
        "Actual Logical Reads",
    ];
    let depths = plan.depths();
    let mut out = String::new();
    for (id, node) in plan.nodes.iter().enumerate() {
        let indent = "  ".repeat(depths[id]);
        let arrow = if id == 0 { "" } else { "-> " };
        out.push_str(&format!("{indent}{arrow}{}", node.title()));
        if let Some(s) = node.subtitle() {
            out.push_str(&format!(" on {s}"));
        }
        out.push_str(&format!(
            "  (cost={} {:.0}% est_rows={}",
            number(node.total_cost),
            plan.cost_share(id) * 100.0,
            number(node.plan_rows)
        ));
        if let Some(a) = node.actual {
            out.push_str(&format!(
                "; actual rows={} loops={} time={}ms",
                number(a.rows * a.loops),
                number(a.loops),
                number(a.total_ms * a.loops.max(1.0))
            ));
        }
        out.push(')');
        if let Some(times) = plan.misestimate(id) {
            out.push_str(&format!("  [row estimate off by {times:.0}x]"));
        }
        out.push('\n');
        for (key, value) in &node.properties {
            if NODE_PROPERTIES.contains(&key.as_str()) && !value.is_empty() {
                out.push_str(&format!("{indent}     {key}: {}\n", one_line(value, 400)));
            }
        }
    }
    let statement: Vec<String> = plan.nodes[0]
        .properties
        .iter()
        .filter(|(k, _)| {
            k.starts_with("Missing Index")
                || matches!(
                    k.as_str(),
                    "Degree Of Parallelism"
                        | "Memory Grant"
                        | "Statement Optm Early Abort Reason"
                        | "Statement Optm Level"
                )
        })
        .map(|(k, v)| format!("{k}: {v}"))
        .collect();
    if !statement.is_empty() {
        out.push_str(&statement.join("\n"));
        out.push('\n');
    }
    if let Some(ms) = plan.execution_ms {
        out.push_str(&format!("Execution Time: {} ms\n", number(ms)));
    }
    out.trim_end().to_string()
}

/// Número sem casas inúteis.
fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else if v.abs() >= 100.0 {
        format!("{v:.1}")
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

/// `text` com no máximo `max` bytes, e se foi cortado.
fn cut_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", &text[..end]), true)
}

/// A resposta do `get_activity`, a partir dos resultados de
/// [`activity::refresh_sql`].
pub(super) fn activity_reply(
    g: &Gatherer,
    include_idle: bool,
    top_statements: bool,
) -> Result<Value, String> {
    // As consultas mais caras vêm por último: sem `pg_stat_statements`,
    // só elas falham e o resto da leitura vale.
    if let Some(e) = &g.error
        && g.sets.len() < 4
    {
        return Err(e.clone());
    }
    let results: Vec<Vec<Vec<Option<String>>>> = g.sets.iter().map(|s| s.rows.clone()).collect();
    let snapshot = activity::parse_snapshot(&results, Instant::now())?;
    Ok(activity_json(&snapshot, include_idle, top_statements))
}

fn seconds(v: Option<f64>) -> Value {
    v.map_or(Value::Null, |s| json!((s * 10.0).round() / 10.0))
}

fn process_json(p: &Process) -> Value {
    let mut v = json!({
        "pid": p.pid,
        "user": p.user,
        "database": p.database,
        "application": p.application,
        "client": p.client,
        "state": p.state,
        "query_seconds": seconds(p.query_seconds),
        "transaction_seconds": seconds(p.transaction_seconds),
        "query": one_line(&p.query, MAX_QUERY_CHARS),
    });
    if !p.wait_type.is_empty() {
        v["wait"] = json!(format!("{}: {}", p.wait_type, p.wait_event));
    }
    if !p.blocked_by.is_empty() {
        v["blocked_by"] = json!(blockers(p));
    }
    v
}

fn blockers(p: &Process) -> Vec<i32> {
    p.blocked_by
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

/// A leitura do servidor como o Claude recebe.
pub(super) fn activity_json(s: &Snapshot, include_idle: bool, top_statements: bool) -> Value {
    let chains = activity::blocking_chains(&s.processes);
    let in_chain: std::collections::HashSet<i32> = chains.iter().map(|(_, pid)| *pid).collect();
    let by_pid = |pid: i32| s.processes.iter().find(|p| p.pid == pid);

    // Cadeias de bloqueio: a cabeça primeiro, com quem ela segura.
    let blocking: Vec<Value> = chains
        .iter()
        .map(|&(depth, pid)| {
            let mut v = match by_pid(pid) {
                Some(p) => process_json(p),
                None => json!({ "pid": pid, "state": "unknown (session not listed)" }),
            };
            v["depth"] = json!(depth);
            v["role"] = json!(if depth == 0 {
                "head blocker"
            } else {
                "blocked"
            });
            v
        })
        .collect();

    let clients: Vec<&Process> = s.processes.iter().filter(|p| p.is_client()).collect();
    let shown: Vec<&Process> = clients
        .iter()
        .copied()
        .filter(|p| include_idle || p.state != "idle" || in_chain.contains(&p.pid))
        .collect();
    let mut sessions: Vec<Value> = shown
        .iter()
        .take(MAX_SESSIONS)
        .map(|p| process_json(p))
        .collect();
    sessions.shrink_to_fit();

    // Locks: os que esperam e os de quem bloqueia.
    let heads: std::collections::HashSet<i32> = chains
        .iter()
        .filter(|(depth, _)| *depth == 0)
        .map(|(_, pid)| *pid)
        .collect();
    let relevant: Vec<&activity::Lock> = s
        .locks
        .iter()
        .filter(|l| !l.granted || heads.contains(&l.pid))
        .collect();
    let locks: Vec<Value> = relevant
        .iter()
        .take(MAX_LOCKS)
        .map(|l| {
            json!({
                "pid": l.pid,
                "type": l.lock_type,
                "mode": l.mode,
                "granted": l.granted,
                "object": if l.relation.is_empty() { &l.target } else { &l.relation },
                "database": l.database,
            })
        })
        .collect();

    let count = |f: &dyn Fn(&Process) -> bool| clients.iter().filter(|p| f(p)).count();
    let mut v = json!({
        "summary": {
            "sessions": clients.len(),
            "active": count(&|p| p.state == "active"),
            "idle_in_transaction": count(&|p| p.state.starts_with("idle in transaction")),
            "waiting": count(&|p| p.is_waiting()),
            "blocked": count(&|p| !p.blocked_by.is_empty()),
            "locks": s.locks.len(),
            "locks_waiting": s.locks.iter().filter(|l| !l.granted).count(),
        },
        "blocking": blocking,
        "sessions": sessions,
        "locks": locks,
        "waits": activity::resource_waits(&s.processes)
            .into_iter()
            .map(|(kind, event, n)| json!({ "type": kind, "event": event, "sessions": n }))
            .collect::<Vec<_>>(),
    });
    if shown.len() > MAX_SESSIONS {
        v["sessions_truncated"] = json!(shown.len() - MAX_SESSIONS);
    }
    if !include_idle {
        v["idle_sessions_not_listed"] = json!(clients.len() - shown.len());
    }
    if relevant.len() > MAX_LOCKS {
        v["locks_truncated"] = json!(relevant.len() - MAX_LOCKS);
    }
    if top_statements {
        match &s.statements {
            Some(list) => {
                v["top_statements"] = list
                    .iter()
                    .take(MAX_STATEMENTS)
                    .map(|t| {
                        json!({
                            "calls": t.calls,
                            "total_ms": t.total_ms.round(),
                            "mean_ms": (t.mean_ms * 100.0).round() / 100.0,
                            "rows": t.rows,
                            "query": one_line(&t.query, MAX_STATEMENT_CHARS),
                        })
                    })
                    .collect();
            }
            None => {
                v["top_statements_unavailable"] = json!(
                    "pg_stat_statements is not installed in this database (CREATE EXTENSION pg_stat_statements, with the library in shared_preload_libraries)."
                );
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: i32, state: &str, blocked_by: &str, query: &str) -> Process {
        Process {
            pid,
            user: "app".into(),
            database: "loja".into(),
            state: state.into(),
            wait_type: if blocked_by.is_empty() { "" } else { "Lock" }.into(),
            wait_event: if blocked_by.is_empty() {
                ""
            } else {
                "transactionid"
            }
            .into(),
            blocked_by: blocked_by.into(),
            query_seconds: Some(12.34),
            backend_type: "client backend".into(),
            query: query.into(),
            ..Default::default()
        }
    }

    #[test]
    fn activity_says_who_blocks_whom() {
        let s = Snapshot {
            at: Instant::now(),
            processes: vec![
                process(10, "idle in transaction", "", "UPDATE pedido SET total = 1"),
                process(11, "active", "10", "DELETE FROM pedido"),
                process(12, "active", "11", "SELECT * FROM pedido FOR UPDATE"),
                process(13, "idle", "", "SELECT 1"),
                process(14, "active", "", "SELECT pg_sleep(10)"),
            ],
            databases: vec![],
            locks: vec![
                activity::Lock {
                    pid: 11,
                    lock_type: "transactionid".into(),
                    mode: "ShareLock".into(),
                    granted: false,
                    target: "777".into(),
                    ..Default::default()
                },
                activity::Lock {
                    pid: 10,
                    lock_type: "relation".into(),
                    mode: "RowExclusiveLock".into(),
                    granted: true,
                    relation: "pedido".into(),
                    ..Default::default()
                },
                activity::Lock {
                    pid: 14,
                    lock_type: "relation".into(),
                    mode: "AccessShareLock".into(),
                    granted: true,
                    relation: "cliente".into(),
                    ..Default::default()
                },
            ],
            has_statements: false,
            statements: None,
        };
        let v = activity_json(&s, false, true);
        assert_eq!(v["summary"]["blocked"], 2);
        assert_eq!(v["summary"]["idle_in_transaction"], 1);
        let chain: Vec<(i64, i64, &str)> = v["blocking"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["pid"].as_i64().unwrap(),
                    b["depth"].as_i64().unwrap(),
                    b["role"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            chain,
            [
                (10, 0, "head blocker"),
                (11, 1, "blocked"),
                (12, 2, "blocked")
            ]
        );
        assert_eq!(v["blocking"][1]["blocked_by"], json!([10]));
        assert_eq!(v["blocking"][1]["wait"], "Lock: transactionid");
        // A sessão ociosa fica de fora; a cabeça do bloqueio, não.
        let pids: Vec<i64> = v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["pid"].as_i64().unwrap())
            .collect();
        assert_eq!(pids, [10, 11, 12, 14]);
        assert_eq!(v["idle_sessions_not_listed"], 1);
        // Locks esperando e os da cabeça; o da sessão 14 não interessa.
        let locks: Vec<i64> = v["locks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["pid"].as_i64().unwrap())
            .collect();
        assert_eq!(locks, [11, 10]);
        assert!(v["top_statements_unavailable"].is_string());
        assert_eq!(
            activity_json(&s, true, false)["sessions"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn plan_batches() {
        let pg = plan_batch(Engine::PostgreSql, "select 1; select 2", false, true).unwrap();
        assert!(pg.starts_with("BEGIN TRANSACTION READ ONLY;"), "{pg}");
        assert!(pg.contains("EXPLAIN select 1;\nEXPLAIN select 2"), "{pg}");
        let actual = plan_batch(Engine::PostgreSql, "select 1", true, true).unwrap();
        assert!(actual.contains("READ ONLY"), "{actual}");
        assert!(
            actual.contains("EXPLAIN (ANALYZE, BUFFERS) select 1"),
            "{actual}"
        );
        let write = plan_batch(Engine::PostgreSql, "delete from t", true, false).unwrap();
        assert!(write.starts_with("BEGIN;\nEXPLAIN (ANALYZE, BUFFERS) delete from t"));
        assert!(write.ends_with("ROLLBACK;"));
        let ms = plan_batch(Engine::SqlServer, "select 1", false, true).unwrap();
        assert!(ms.starts_with("SET SHOWPLAN_XML ON\nGO\n"));
        let ms = plan_batch(Engine::SqlServer, "delete from t", true, false).unwrap();
        assert!(
            ms.contains("BEGIN TRANSACTION\nGO\ndelete from t\nGO\nIF @@TRANCOUNT > 0 ROLLBACK")
        );
        assert!(ms.ends_with("SET STATISTICS XML OFF"));
        assert_eq!(
            plan_batch(Engine::PostgreSql, " -- nada\n", false, true),
            None
        );
    }

    #[test]
    fn postgres_plans_are_joined_per_statement() {
        let mut g = Gatherer::plans();
        for plan in [
            ["Seq Scan on t", "  Filter: (a = 1)"],
            ["Result", "  One-Time"],
        ] {
            g.push(QueryEvent::Columns(vec![PG_PLAN_COLUMN.into()]));
            for line in plan {
                g.push(QueryEvent::Row(vec![Some(line.into())]));
            }
            g.push(QueryEvent::CommandComplete { rows: 2 });
        }
        g.push(QueryEvent::Finished { canceled: false });
        assert!(g.is_finished());
        let statements = statements("select * from t where a = 1; select 1");
        let (v, failed) = plan_reply(Engine::PostgreSql, &statements, false, &g);
        assert!(!failed);
        assert_eq!(v["kind"], "estimated");
        assert_eq!(v["plans"][0]["statement"], "select * from t where a = 1");
        assert_eq!(v["plans"][0]["plan"], "Seq Scan on t\n  Filter: (a = 1)");
        assert_eq!(v["plans"][1]["statement"], "select 1");
        assert!(v.get("executed").is_none());
    }

    #[test]
    fn no_plan_is_an_error() {
        let mut g = Gatherer::plans();
        g.push(QueryEvent::Error(
            pgstudio_core::query::ServerMessage::error("relation \"x\" does not exist"),
        ));
        g.push(QueryEvent::Finished { canceled: false });
        let (v, failed) = plan_reply(Engine::PostgreSql, &[], true, &g);
        assert!(failed);
        assert_eq!(v["error"], "relation \"x\" does not exist");
    }
}
