//! Classificador de SQL: diz se um lote só lê ou se pode alterar alguma
//! coisa.
//!
//! O que o Claude manda rodar passa por aqui. Lote só de leitura roda sem
//! perguntar, dentro de uma transação `READ ONLY` (veja
//! [`read_only_batch`]): se o classificador errar, o PostgreSQL ainda recusa
//! a escrita. Qualquer outra coisa (DML, DDL, controle de transação, SQL que
//! o parser não entende) pede confirmação ao usuário.

use std::ops::ControlFlow;

use sqlparser::ast::{Expr, Statement, visit_expressions, visit_statements};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

/// O que um lote faz, do ponto de vista de quem confirma.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Safety {
    /// Só lê: `SELECT`, `WITH` só com `SELECT`, `EXPLAIN` sem `ANALYZE`,
    /// `SHOW`.
    ReadOnly,
    /// Altera dados, schema, sessão ou transação. `reason` diz o quê, como
    /// "UPDATE" ou "DROP".
    Writes { reason: String },
    /// O parser não entendeu o lote; trata como escrita.
    Unknown { reason: String },
}

impl Safety {
    /// Se pode rodar sem perguntar.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Safety::ReadOnly)
    }

    /// Frase para o diálogo de confirmação.
    pub fn describe(&self) -> String {
        match self {
            Safety::ReadOnly => "This SQL only reads data.".into(),
            Safety::Writes { reason } => format!("This SQL can change the database ({reason})."),
            Safety::Unknown { reason } => {
                format!("PostPilot could not confirm that this SQL only reads data ({reason}).")
            }
        }
    }
}

/// Funções que fazem alguma coisa mesmo dentro de um `SELECT` numa
/// transação só de leitura: derrubar sessões, gravar arquivos, mudar a
/// configuração, falar com outros servidores.
const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_switch_wal",
    "pg_create_restore_point",
    "pg_promote",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    "pg_log_backend_memory_contexts",
    "pg_create_physical_replication_slot",
    "pg_create_logical_replication_slot",
    "pg_drop_replication_slot",
    "pg_file_write",
    "pg_file_rename",
    "pg_file_unlink",
    "lo_import",
    "lo_export",
    "lo_unlink",
    "set_config",
    "pg_advisory_lock",
    "pg_advisory_xact_lock",
    "pg_advisory_lock_shared",
    "pg_try_advisory_lock",
    "pg_notify",
    "dblink",
    "dblink_exec",
    "dblink_connect",
    "dblink_send_query",
    "pg_stat_reset",
    "pg_stat_statements_reset",
    "nextval",
    "setval",
];

/// Classifica um lote (um ou mais comandos separados por `;`).
pub fn classify(sql: &str) -> Safety {
    classify_with(&PostgreSqlDialect {}, sql)
}

/// Como [`classify`], no dialeto do motor: no SQL Server, T-SQL, com os
/// separadores `GO` tirados antes.
pub fn classify_for(engine: pgstudio_core::connection::Engine, sql: &str) -> Safety {
    match engine {
        // Redis: cada comando do texto é de leitura ou não, pelo nome.
        pgstudio_core::connection::Engine::Redis => {
            match pgstudio_redis::command::first_write(sql) {
                Ok(None) => Safety::ReadOnly,
                Ok(Some(reason)) => Safety::Writes { reason },
                Err(reason) => Safety::Unknown { reason },
            }
        }
        // MongoDB: pelo comando do servidor em que cada consulta vira.
        pgstudio_core::connection::Engine::MongoDb => {
            match pgstudio_mongodb::shell::first_write(sql) {
                None => Safety::ReadOnly,
                Some(reason) if reason == "?" => Safety::Unknown {
                    reason: "the query could not be read".into(),
                },
                Some(reason) => Safety::Writes { reason },
            }
        }
        pgstudio_core::connection::Engine::PostgreSql => classify(sql),
        pgstudio_core::connection::Engine::SqlServer => {
            let without_go: String = sql
                .split_inclusive('\n')
                .map(|line| if is_go_line(line) { "\n" } else { line })
                .collect();
            // Consultas a outros servidores e a arquivos rodam o que
            // vier dentro delas, fora da transação.
            let lower = without_go.to_lowercase();
            for function in ["openquery", "openrowset", "opendatasource"] {
                if lower.contains(function) {
                    return Safety::Writes {
                        reason: format!("calls {}", function.to_uppercase()),
                    };
                }
            }
            // T-SQL não exige `;` entre comandos.
            let options = sqlparser::parser::ParserOptions {
                require_semicolon_stmt_delimiter: false,
                ..Default::default()
            };
            classify_parsed(
                Parser::new(&sqlparser::dialect::MsSqlDialect {})
                    .with_options(options)
                    .try_with_sql(&without_go)
                    .and_then(|mut p| p.parse_statements()),
            )
        }
    }
}

/// Linha só com `GO` (e um número de repetições, e um comentário).
fn is_go_line(line: &str) -> bool {
    let line = line.trim();
    let line = line.split("--").next().unwrap_or("").trim_end();
    if !line.get(..2).is_some_and(|g| g.eq_ignore_ascii_case("go")) {
        return false;
    }
    let tail = line[2..].trim();
    tail.is_empty() || tail.chars().all(|c| c.is_ascii_digit())
}

fn classify_with(dialect: &dyn sqlparser::dialect::Dialect, sql: &str) -> Safety {
    classify_parsed(Parser::parse_sql(dialect, sql))
}

fn classify_parsed(parsed: Result<Vec<Statement>, sqlparser::parser::ParserError>) -> Safety {
    let statements = match parsed {
        Ok(s) => s,
        Err(e) => {
            return Safety::Unknown {
                reason: format!("parse error: {e}"),
            };
        }
    };
    if statements.is_empty() {
        return Safety::Unknown {
            reason: "no statement".into(),
        };
    }
    for statement in &statements {
        if let Some(reason) = writes(statement) {
            return Safety::Writes { reason };
        }
    }
    let side_effect = visit_expressions(&statements, |expr| match expr {
        Expr::Function(f) => {
            let name = f
                .name
                .0
                .last()
                .and_then(|p| p.as_ident())
                .map(|i| i.value.to_ascii_lowercase())
                .unwrap_or_default();
            if SIDE_EFFECT_FUNCTIONS.contains(&name.as_str()) {
                ControlFlow::Break(name)
            } else {
                ControlFlow::Continue(())
            }
        }
        _ => ControlFlow::Continue(()),
    });
    if let ControlFlow::Break(name) = side_effect {
        return Safety::Writes {
            reason: format!("calls {name}()"),
        };
    }
    Safety::ReadOnly
}

/// O que um comando de topo altera, ou `None` se só lê.
fn writes(statement: &Statement) -> Option<String> {
    match statement {
        Statement::Query(query) => {
            // Comandos dentro do SELECT: WITH x AS (DELETE ...), por exemplo.
            let nested = visit_statements(query, |s| match s {
                Statement::Query(_) => ControlFlow::Continue(()),
                other => ControlFlow::Break(keyword(other)),
            });
            if let ControlFlow::Break(k) = nested {
                return Some(k);
            }
            if !query.locks.is_empty() {
                return Some("SELECT ... FOR UPDATE".into());
            }
            let into = visit_statements(query, |s| match s {
                Statement::Query(q) => match q.body.as_select() {
                    Some(sel) if sel.into.is_some() => ControlFlow::Break(()),
                    _ => ControlFlow::Continue(()),
                },
                _ => ControlFlow::Continue(()),
            });
            if into.is_break() || select_into(query) {
                return Some("SELECT INTO".into());
            }
            None
        }
        Statement::Explain {
            analyze, statement, ..
        } => {
            let inner = writes(statement);
            match (analyze, inner) {
                // EXPLAIN sem ANALYZE não roda o comando.
                (false, _) => None,
                (true, inner) => inner.map(|k| format!("EXPLAIN ANALYZE {k}")),
            }
        }
        Statement::ShowVariable { .. }
        | Statement::ShowVariables { .. }
        | Statement::ShowTables { .. }
        | Statement::ShowColumns { .. } => None,
        other => Some(keyword(other)),
    }
}

/// SELECT INTO no corpo de topo (o visitor não passa por ele como
/// comando).
fn select_into(query: &sqlparser::ast::Query) -> bool {
    use sqlparser::ast::SetExpr;
    fn walk(body: &SetExpr) -> bool {
        match body {
            SetExpr::Select(s) => s.into.is_some(),
            SetExpr::Query(q) => walk(&q.body),
            SetExpr::SetOperation { left, right, .. } => walk(left) || walk(right),
            _ => false,
        }
    }
    walk(&query.body)
}

/// Primeira palavra do comando, como "UPDATE" ou "CREATE".
fn keyword(statement: &Statement) -> String {
    let text = statement.to_string();
    let mut words = text.split_whitespace();
    let first = words.next().unwrap_or("statement").to_ascii_uppercase();
    match first.as_str() {
        "CREATE" | "DROP" | "ALTER" | "TRUNCATE" | "GRANT" | "REVOKE" => match words.next() {
            Some(w) if w.chars().all(|c| c.is_ascii_alphabetic()) => {
                format!("{first} {}", w.to_ascii_uppercase())
            }
            _ => first,
        },
        _ => first,
    }
}

/// Tempo máximo de um comando de leitura do Claude.
pub const READ_ONLY_TIMEOUT: &str = "60s";

/// Embrulha um lote só de leitura numa transação `READ ONLY` com tempo
/// máximo. São 2 comandos antes do lote e 1 depois (veja
/// [`crate::collect::Collector::wrapped`]).
pub fn read_only_batch(sql: &str) -> String {
    format!(
        "BEGIN TRANSACTION READ ONLY;\nSET LOCAL statement_timeout = '{READ_ONLY_TIMEOUT}';\n{sql}\n;\nROLLBACK;"
    )
}

/// Como [`read_only_batch`], no motor. O SQL Server não tem transação só
/// de leitura: o lote (que o [`classify_for`] já viu que só lê) roda numa
/// transação que é sempre desfeita, com `XACT_ABORT` e sem esperar mais
/// de um minuto por um bloqueio. O embrulho fica na primeira linha, para
/// as linhas das mensagens de erro baterem com as do lote.
pub fn read_only_batch_for(engine: pgstudio_core::connection::Engine, sql: &str) -> String {
    match engine {
        // Sem transação só de leitura no Redis: vale o classificador.
        pgstudio_core::connection::Engine::Redis | pgstudio_core::connection::Engine::MongoDb => {
            sql.to_string()
        }
        pgstudio_core::connection::Engine::PostgreSql => read_only_batch(sql),
        pgstudio_core::connection::Engine::SqlServer => format!(
            "SET XACT_ABORT ON; SET LOCK_TIMEOUT 60000; BEGIN TRANSACTION; {sql}\n;\nIF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_commands_are_read_or_write_by_name() {
        use pgstudio_core::connection::Engine::Redis;
        assert_eq!(
            classify_for(Redis, "GET a\nHGETALL h\nSCAN 0"),
            Safety::ReadOnly
        );
        assert_eq!(
            classify_for(Redis, "GET a\nDEL a"),
            Safety::Writes {
                reason: "DEL".into()
            }
        );
        assert_eq!(
            classify_for(Redis, "flushdb"),
            Safety::Writes {
                reason: "FLUSHDB".into()
            }
        );
        assert!(matches!(
            classify_for(Redis, "GET \"a"),
            Safety::Unknown { .. }
        ));
    }

    fn ro(sql: &str) {
        assert_eq!(classify(sql), Safety::ReadOnly, "{sql}");
    }

    fn rw(sql: &str) {
        assert!(
            matches!(classify(sql), Safety::Writes { .. }),
            "{sql}: {:?}",
            classify(sql)
        );
    }

    #[test]
    fn tsql_reads_and_writes() {
        use pgstudio_core::connection::Engine::SqlServer;
        let ro = |sql: &str| assert_eq!(classify_for(SqlServer, sql), Safety::ReadOnly, "{sql}");
        let rw = |sql: &str| assert!(!classify_for(SqlServer, sql).is_read_only(), "{sql}");
        ro("select top (10) [nome] from dbo.clientes order by id");
        ro("with x as (select 1 as a) select * from x");
        ro("select 1\nGO\nselect 2");
        rw("select * into dbo.copia from dbo.clientes");
        rw("exec sp_who2");
        rw("update dbo.clientes set nome = N'x'");
        rw("select * from openquery(outro, 'delete from t')");
        rw("drop table dbo.clientes");
        let batch = read_only_batch_for(SqlServer, "select 1");
        assert!(batch.starts_with("SET XACT_ABORT ON;"));
        assert!(batch.ends_with("ROLLBACK TRANSACTION;"));
        assert_eq!(batch.lines().next().unwrap().matches("select 1").count(), 1);
    }

    #[test]
    fn reads() {
        ro("select 1");
        ro("SELECT * FROM pedidos WHERE id = 1;");
        ro("with x as (select 1) select * from x");
        ro("select a from t union all select b from u order by 1 limit 10");
        ro("explain select * from t");
        ro("explain (format json) select * from t join u using (id)");
        ro("explain update t set a = 1");
        ro("explain analyze select * from t");
        ro("show search_path");
        ro("select data->>'nome' from clientes where data @> '{\"ativo\": true}'");
        ro("select 1; select 2;");
        ro("select count(*) from pg_stat_activity");
        ro("values (1), (2)");
    }

    #[test]
    fn writes_need_confirmation() {
        rw("insert into t values (1)");
        rw("update t set a = 1");
        rw("delete from t");
        rw("merge into t using u on t.id = u.id when matched then delete");
        rw("create table t (id int)");
        rw("drop table t");
        rw("alter table t add column b int");
        rw("truncate t");
        rw("grant select on t to bob");
        rw("select 1; delete from t");
        rw("with d as (delete from t returning *) select * from d");
        rw("select * into novo from t");
        rw("select * from t for update");
        rw("explain analyze delete from t");
        rw("commit");
        rw("begin");
        rw("set search_path = x");
        rw("vacuum t");
        rw("select pg_terminate_backend(123)");
        rw("select nextval('seq')");
        rw("select * from t where pg_catalog.set_config('a', 'b', false) is not null");
        rw("copy t from '/tmp/x'");
    }

    #[test]
    fn reasons() {
        assert_eq!(
            classify("drop table t"),
            Safety::Writes {
                reason: "DROP TABLE".into()
            }
        );
        assert_eq!(
            classify("update t set a = 1"),
            Safety::Writes {
                reason: "UPDATE".into()
            }
        );
    }

    #[test]
    fn unknown_is_not_read_only() {
        assert!(matches!(classify("selec 1"), Safety::Unknown { .. }));
        assert!(matches!(classify(""), Safety::Unknown { .. }));
        assert!(!classify("do $$ begin end $$").is_read_only());
    }

    #[test]
    fn wrapper() {
        let b = read_only_batch("select 1");
        assert!(b.starts_with("BEGIN TRANSACTION READ ONLY;"));
        assert!(b.ends_with("ROLLBACK;"));
    }

    #[test]
    fn t_sql_is_classified_in_its_dialect() {
        use pgstudio_core::connection::Engine::SqlServer;
        assert_eq!(
            classify_for(
                SqlServer,
                "SELECT TOP 10 [name] FROM [dbo].[t]\nGO\nSELECT 1"
            ),
            Safety::ReadOnly
        );
        assert!(matches!(
            classify_for(SqlServer, "UPDATE [dbo].[t] SET a = 1\ngo 2"),
            Safety::Writes { .. }
        ));
        assert!(matches!(
            classify_for(SqlServer, "SELECT 1 AS x INTO #tmp"),
            Safety::Writes { .. }
        ));
        assert!(is_go_line("  GO  -- fim\n"));
        assert!(!is_go_line("GOTO x"));
    }
}
