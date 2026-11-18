//! Classificador de SQL: diz se um lote só lê ou se pode alterar alguma
//! coisa.
//!
//! O que o Claude manda rodar passa por aqui. Lote só de leitura roda sem
//! perguntar, dentro de uma transação `READ ONLY` (veja
//! [`read_only_batch`]): se o classificador errar, o PostgreSQL ainda recusa
//! a escrita. Qualquer outra coisa (DML, DDL, controle de transação, SQL que
//! o parser não entende) pede confirmação ao usuário.
//!
//! Quando o parser não entende um lote de PostgreSQL (falta um `;` entre
//! dois `SELECT`, por exemplo), o [`reads_by_tokens`] olha as palavras: se
//! todo comando começa como leitura e nenhuma palavra de escrita aparece,
//! o lote só lê, e o erro de sintaxe quem mostra é o servidor.

use std::ops::ControlFlow;

use sqlparser::ast::{Expr, Spanned, Statement, visit_expressions, visit_statements};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

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

/// Onde está, no lote, o que fez o classificador pedir confirmação.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Place {
    /// Linha dentro do lote, a partir de 1.
    pub line: u64,
    /// O texto em que o parser parou, quando ele não entendeu o lote.
    pub near: Option<String>,
}

/// Comandos que só leem: todo comando do lote tem de começar por um deles
/// para o [`reads_by_tokens`] aceitar.
const READ_STARTS: &[&str] = &["SELECT", "WITH", "VALUES", "TABLE", "SHOW", "EXPLAIN"];

/// Palavras que, em qualquer lugar do lote, tiram a certeza de que ele só
/// lê: os comandos de escrita (que cabem dentro de um `WITH`), `INTO`
/// (`SELECT INTO`), `ANALYZE` (`EXPLAIN ANALYZE` roda o comando), `SHARE`
/// e `UPDATE` (`FOR UPDATE`, `FOR SHARE`) e o começo de todo comando que
/// altera schema, sessão ou transação.
const WRITE_WORDS: &[&str] = &[
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "INTO",
    "ANALYZE",
    "ANALYSE",
    "SHARE",
    "CREATE",
    "DROP",
    "ALTER",
    "TRUNCATE",
    "GRANT",
    "REVOKE",
    "CALL",
    "DO",
    "COPY",
    "SET",
    "RESET",
    "LOCK",
    "EXECUTE",
    "PREPARE",
    "DEALLOCATE",
    "VACUUM",
    "REINDEX",
    "CLUSTER",
    "REFRESH",
    "COMMENT",
    "NOTIFY",
    "LISTEN",
    "UNLISTEN",
    "BEGIN",
    "START",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    "ABORT",
    "DISCARD",
    "CHECKPOINT",
    "LOAD",
    "IMPORT",
    "REASSIGN",
    "SECURITY",
];

/// Diz se um lote de PostgreSQL que o parser não entendeu só lê, olhando
/// as palavras fora de strings e comentários.
///
/// Vale para o que o PostgreSQL aceitar: lá todo comando termina em `;`,
/// então cada trecho entre `;` é um comando só, e um comando que começa
/// por uma palavra de [`READ_STARTS`] só altera alguma coisa com uma das
/// [`WRITE_WORDS`] ou com uma função de [`SIDE_EFFECT_FUNCTIONS`]. Na
/// dúvida (o texto nem vira tokens, um comando começa por outra coisa), a
/// resposta é `false` e o lote continua pedindo confirmação.
fn reads_by_tokens(sql: &str) -> bool {
    let Ok(tokens) = Tokenizer::new(&PostgreSqlDialect {}, sql).tokenize() else {
        return false;
    };
    let mut start = true;
    let mut any = false;
    for token in tokens {
        match token {
            // Comentários também chegam como espaço.
            Token::Whitespace(_) | Token::EOF => continue,
            Token::SemiColon => {
                start = true;
                continue;
            }
            Token::Word(word) => {
                let quoted = word.quote_style.is_some();
                let upper = word.value.to_ascii_uppercase();
                if start && (quoted || !READ_STARTS.contains(&upper.as_str())) {
                    return false;
                }
                if !quoted && WRITE_WORDS.contains(&upper.as_str()) {
                    return false;
                }
                if SIDE_EFFECT_FUNCTIONS.contains(&upper.to_ascii_lowercase().as_str()) {
                    return false;
                }
            }
            _ if start => return false,
            _ => {}
        }
        start = false;
        any = true;
    }
    any
}

/// Linha e texto em que o parser parou, tirados da mensagem dele
/// ("Expected: end of statement, found: SELECT at Line: 17, Column: 1").
fn parse_error_place(message: &str) -> Option<Place> {
    let (before, after) = message.rsplit_once(" at Line: ")?;
    let line = after.split(',').next()?.trim().parse().ok()?;
    let near = before
        .rsplit_once("found: ")
        .map(|(_, found)| found.trim().to_string())
        .filter(|found| !found.is_empty() && found != "EOF");
    Some(Place { line, near })
}

/// Classifica um lote (um ou mais comandos separados por `;`).
pub fn classify(sql: &str) -> Safety {
    classify_with(&PostgreSqlDialect {}, sql).0
}

/// Como [`classify`], no dialeto do motor: no SQL Server, T-SQL, com os
/// separadores `GO` tirados antes.
pub fn classify_for(engine: pgstudio_core::connection::Engine, sql: &str) -> Safety {
    match engine {
        // Fora da janela de consulta (o Claude, por exemplo) o lote roda
        // numa sessão que o PostPilot prepara: comandos que mexem nela
        // (`USE`, `SET NOCOUNT`) continuam pedindo confirmação.
        pgstudio_core::connection::Engine::SqlServer => classify_tsql(sql, Mode::Tsql).0,
        _ => classify_located(engine, sql).0,
    }
}

/// Como [`classify_for`], para o lote que o usuário roda na janela de
/// consulta, com o lugar do lote que motivou a resposta (quando ela não é
/// "só lê" e o motor é SQL). No SQL Server, a janela também aceita sem
/// perguntar o que só mexe na própria sessão (`USE`, `SET NOCOUNT ON`).
pub fn classify_located(
    engine: pgstudio_core::connection::Engine,
    sql: &str,
) -> (Safety, Option<Place>) {
    match engine {
        pgstudio_core::connection::Engine::PostgreSql => classify_with(&PostgreSqlDialect {}, sql),
        pgstudio_core::connection::Engine::SqlServer => classify_tsql(sql, Mode::TsqlWindow),
        other => (classify_other(other, sql), None),
    }
}

/// Motores que não são SQL: cada um tem o seu jeito de dizer o que escreve.
fn classify_other(engine: pgstudio_core::connection::Engine, sql: &str) -> Safety {
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
        // Elasticsearch: pelo método e pela API de cada requisição.
        pgstudio_core::connection::Engine::Elasticsearch => {
            match pgstudio_elasticsearch::console::parse(sql) {
                Err(e) => Safety::Unknown { reason: e.message },
                Ok(_) => match pgstudio_elasticsearch::console::first_write(sql) {
                    None => Safety::ReadOnly,
                    Some(reason) => Safety::Writes { reason },
                },
            }
        }
        pgstudio_core::connection::Engine::PostgreSql => classify(sql),
        pgstudio_core::connection::Engine::SqlServer => classify_tsql(sql, Mode::Tsql).0,
    }
}

fn classify_tsql(sql: &str, mode: Mode) -> (Safety, Option<Place>) {
    let without_go: String = sql
        .split_inclusive('\n')
        .map(|line| if is_go_line(line) { "\n" } else { line })
        .collect();
    // Consultas a outros servidores e a arquivos rodam o que
    // vier dentro delas, fora da transação.
    let lower = without_go.to_lowercase();
    for function in ["openquery", "openrowset", "opendatasource"] {
        if let Some(at) = lower.find(function) {
            let line = lower[..at].matches('\n').count() as u64 + 1;
            return (
                Safety::Writes {
                    reason: format!("calls {}", function.to_uppercase()),
                },
                Some(Place { line, near: None }),
            );
        }
    }
    // T-SQL não exige `;` entre comandos.
    let options = sqlparser::parser::ParserOptions {
        require_semicolon_stmt_delimiter: false,
        ..Default::default()
    };
    let dialect = sqlparser::dialect::MsSqlDialect {};
    let classified = classify_parsed(
        Parser::new(&dialect)
            .with_options(options)
            .try_with_sql(&without_go)
            .and_then(|mut p| p.parse_statements()),
        mode,
    );
    placed(&dialect, &without_go, classified)
}

/// Completa o lugar de uma escrita que o parser não localizou (nem todo
/// comando traz a posição): a linha da primeira vez que a palavra do
/// motivo ("DROP", em "DROP TABLE") aparece fora de strings e comentários.
fn placed(
    dialect: &dyn sqlparser::dialect::Dialect,
    sql: &str,
    classified: (Safety, Option<Place>),
) -> (Safety, Option<Place>) {
    let (Safety::Writes { reason }, None) = &classified else {
        return classified;
    };
    let word = reason.split_whitespace().next().unwrap_or_default();
    let line = Tokenizer::new(dialect, sql)
        .tokenize_with_location()
        .ok()
        .and_then(|tokens| {
            tokens.into_iter().find_map(|t| match &t.token {
                Token::Word(w)
                    if w.quote_style.is_none()
                        && (w.value.eq_ignore_ascii_case(word)
                            // O motivo sai por extenso; o usuário escreve EXEC.
                            || (word == "EXECUTE" && w.value.eq_ignore_ascii_case("EXEC"))) =>
                {
                    Some(t.span.start.line)
                }
                _ => None,
            })
        });
    let place = line.map(|line| Place { line, near: None });
    (classified.0, place)
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

fn classify_with(dialect: &dyn sqlparser::dialect::Dialect, sql: &str) -> (Safety, Option<Place>) {
    let classified = classify_parsed(Parser::parse_sql(dialect, sql), Mode::Plain);
    // O parser não entendeu, mas as palavras do lote são só de leitura:
    // roda sem perguntar, e o erro de sintaxe vem do servidor.
    if matches!(classified.0, Safety::Unknown { .. })
        && dialect.is::<PostgreSqlDialect>()
        && reads_by_tokens(sql)
    {
        return (Safety::ReadOnly, None);
    }
    placed(dialect, sql, classified)
}

fn classify_parsed(
    parsed: Result<Vec<Statement>, sqlparser::parser::ParserError>,
    mode: Mode,
) -> (Safety, Option<Place>) {
    let statements = match parsed {
        Ok(s) => s,
        Err(e) => {
            let message = e.to_string();
            let place = parse_error_place(&message);
            return (
                Safety::Unknown {
                    reason: format!("parse error: {message}"),
                },
                place,
            );
        }
    };
    if statements.is_empty() {
        return (
            Safety::Unknown {
                reason: "no statement".into(),
            },
            None,
        );
    }
    let place = |line: u64| (line > 0).then_some(Place { line, near: None });
    for statement in &statements {
        if let Some(reason) = writes(statement, mode) {
            return (
                Safety::Writes { reason },
                place(statement.span().start.line),
            );
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
                ControlFlow::Break((name, f.name.span().start.line))
            } else {
                ControlFlow::Continue(())
            }
        }
        _ => ControlFlow::Continue(()),
    });
    if let ControlFlow::Break((name, line)) = side_effect {
        return (
            Safety::Writes {
                reason: format!("calls {name}()"),
            },
            place(line),
        );
    }
    (Safety::ReadOnly, None)
}

/// Em que regras o lote é lido.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// PostgreSQL: só consultas, `EXPLAIN` e `SHOW` leem.
    Plain,
    /// T-SQL: também não altera nada o que só existe durante o lote
    /// (variáveis, `PRINT`, `IF`, `WHILE`) e as procedures de sistema que
    /// só consultam.
    Tsql,
    /// T-SQL na janela de consulta do usuário: além disso, o que só muda a
    /// sessão dele (`USE`, `SET NOCOUNT ON`).
    TsqlWindow,
}

/// Opções de sessão do SQL Server que não alteram dados nem fazem um
/// comando seguinte alterar mais do que alteraria. Ficam de fora, e pedem
/// confirmação, `IDENTITY_INSERT`, `ROWCOUNT`, `IMPLICIT_TRANSACTIONS`,
/// `NOEXEC`, `PARSEONLY`, `FMTONLY` e `CONTEXT_INFO`.
const TSQL_SESSION_OPTIONS: &[&str] = &[
    "nocount",
    "dateformat",
    "datefirst",
    "language",
    "ansi_nulls",
    "ansi_warnings",
    "ansi_padding",
    "ansi_null_dflt_on",
    "ansi_null_dflt_off",
    "ansi_defaults",
    "arithabort",
    "arithignore",
    "quoted_identifier",
    "concat_null_yields_null",
    "numeric_roundabort",
    "lock_timeout",
    "deadlock_priority",
    "textsize",
    "xact_abort",
    "showplan_all",
    "showplan_text",
    "showplan_xml",
];

/// Procedures de sistema do SQL Server que só consultam o catálogo ou a
/// atividade. A `sp_spaceused` fica de fora: com `@updateusage` ela
/// corrige contadores.
const TSQL_READ_PROCEDURES: &[&str] = &[
    "sp_help",
    "sp_helptext",
    "sp_helpindex",
    "sp_helpconstraint",
    "sp_helptrigger",
    "sp_helpdb",
    "sp_helpfile",
    "sp_helpfilegroup",
    "sp_helpuser",
    "sp_helprole",
    "sp_helprolemember",
    "sp_helpsrvrole",
    "sp_helpsrvrolemember",
    "sp_helplogins",
    "sp_helpserver",
    "sp_helpsort",
    "sp_helplanguage",
    "sp_columns",
    "sp_tables",
    "sp_pkeys",
    "sp_fkeys",
    "sp_statistics",
    "sp_stored_procedures",
    "sp_sproc_columns",
    "sp_databases",
    "sp_datatype_info",
    "sp_server_info",
    "sp_depends",
    "sp_who",
    "sp_who2",
    "sp_lock",
];

/// O que um comando de topo altera, ou `None` se só lê. Olha também os
/// comandos dentro dele (`WITH x AS (DELETE ...)`, o corpo de um `IF`).
fn writes(statement: &Statement, mode: Mode) -> Option<String> {
    if let Statement::Explain {
        analyze, statement, ..
    } = statement
    {
        // EXPLAIN sem ANALYZE não roda o comando.
        return match analyze {
            false => None,
            true => writes(statement, mode).map(|k| format!("EXPLAIN ANALYZE {k}")),
        };
    }
    match visit_statements(statement, |s| match own_write(s, mode) {
        Some(reason) => ControlFlow::Break(reason),
        None => ControlFlow::Continue(()),
    }) {
        ControlFlow::Break(reason) => Some(reason),
        ControlFlow::Continue(()) => None,
    }
}

/// O que este comando, sem contar os que estão dentro dele, altera.
fn own_write(statement: &Statement, mode: Mode) -> Option<String> {
    use sqlparser::ast::{Set, SetSessionParamKind, TransactionModifier};
    let tsql = mode != Mode::Plain;
    let window = mode == Mode::TsqlWindow;
    match statement {
        Statement::Query(query) => {
            if !query.locks.is_empty() {
                return Some("SELECT ... FOR UPDATE".into());
            }
            if select_into(query) {
                return Some("SELECT INTO".into());
            }
            None
        }
        Statement::ShowVariable { .. }
        | Statement::ShowVariables { .. }
        | Statement::ShowTables { .. }
        | Statement::ShowColumns { .. } => None,
        // Variáveis, cursores e controle de fluxo só existem durante o
        // lote; o que vai dentro de um IF ou WHILE é visto à parte.
        Statement::Declare { .. }
        | Statement::Print(_)
        | Statement::If(_)
        | Statement::While(_)
        | Statement::Return(_)
        | Statement::RaisError { .. }
        | Statement::Throw(_)
            if tsql =>
        {
            None
        }
        // BEGIN TRY ... END TRY BEGIN CATCH ... END CATCH.
        Statement::StartTransaction {
            modifier: Some(TransactionModifier::Try | TransactionModifier::Catch),
            ..
        }
        | Statement::Commit {
            modifier: Some(TransactionModifier::Try | TransactionModifier::Catch),
            ..
        } if tsql => None,
        // SET @variavel = ...
        Statement::Set(Set::SingleAssignment { variable, .. })
            if tsql
                && variable.0.len() == 1
                && variable.0[0]
                    .as_ident()
                    .is_some_and(|i| i.quote_style.is_none() && i.value.starts_with('@')) =>
        {
            None
        }
        Statement::Set(Set::SetSessionParam(SetSessionParamKind::Statistics(_))) if window => None,
        Statement::Set(Set::SetSessionParam(SetSessionParamKind::Generic(param)))
            if window
                && param.names.len() == 1
                && TSQL_SESSION_OPTIONS.contains(&param.names[0].to_ascii_lowercase().as_str()) =>
        {
            None
        }
        // No T-SQL só existe SET TRANSACTION ISOLATION LEVEL.
        Statement::Set(Set::SetTransaction { .. }) if window => None,
        Statement::Use(_) if window => None,
        Statement::Execute {
            name: Some(name),
            immediate: false,
            ..
        } if tsql && is_read_procedure(name) => None,
        other => Some(keyword(other)),
    }
}

/// `sp_help` ou `sys.sp_help`: as procedures de
/// sistema de [`TSQL_READ_PROCEDURES`]. Com outro schema ou banco na
/// frente o nome pode ser de uma procedure do usuário.
fn is_read_procedure(name: &sqlparser::ast::ObjectName) -> bool {
    let parts: Option<Vec<String>> = name
        .0
        .iter()
        .map(|p| p.as_ident().map(|i| i.value.to_ascii_lowercase()))
        .collect();
    match parts.as_deref() {
        Some([proc]) => TSQL_READ_PROCEDURES.contains(&proc.as_str()),
        Some([schema, proc]) => schema == "sys" && TSQL_READ_PROCEDURES.contains(&proc.as_str()),
        _ => false,
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

/// Como começa o lote só de leitura do SQL Server.
pub const MSSQL_READ_ONLY_PREFIX: &str =
    "SET XACT_ABORT ON; SET LOCK_TIMEOUT 60000; BEGIN TRANSACTION;";

/// Quanto uma leitura do assistente pode rodar no SQL Server, que não tem
/// `statement_timeout`: quem conta o tempo e cancela é o PostPilot. Igual
/// ao [`READ_ONLY_TIMEOUT`] do PostgreSQL.
pub const MSSQL_READ_ONLY_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Se `batch` é um lote só de leitura do SQL Server, montado pelo
/// [`read_only_batch_for`].
pub fn is_mssql_read_only_batch(batch: &str) -> bool {
    batch.starts_with(MSSQL_READ_ONLY_PREFIX)
}

/// Como [`read_only_batch`], no motor. O SQL Server não tem transação só
/// de leitura: o lote (que o [`classify_for`] já viu que só lê) roda numa
/// transação que é sempre desfeita, com `XACT_ABORT` e sem esperar mais
/// de um minuto por um bloqueio. O embrulho fica na primeira linha, para
/// as linhas das mensagens de erro baterem com as do lote.
pub fn read_only_batch_for(engine: pgstudio_core::connection::Engine, sql: &str) -> String {
    match engine {
        // Sem transação só de leitura no Redis: vale o classificador.
        pgstudio_core::connection::Engine::Redis
        | pgstudio_core::connection::Engine::MongoDb
        | pgstudio_core::connection::Engine::Elasticsearch => sql.to_string(),
        pgstudio_core::connection::Engine::PostgreSql => read_only_batch(sql),
        pgstudio_core::connection::Engine::SqlServer => {
            format!("{MSSQL_READ_ONLY_PREFIX} {sql}\n;\nIF @@TRANCOUNT > 0 ROLLBACK TRANSACTION;")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elasticsearch_requests_are_read_or_write_by_method_and_api() {
        use pgstudio_core::connection::Engine::Elasticsearch;
        assert_eq!(
            classify_for(
                Elasticsearch,
                "GET /_cat/indices\nPOST /a/_search\n{ \"size\": 1 }"
            ),
            Safety::ReadOnly
        );
        assert_eq!(
            classify_for(Elasticsearch, "GET /a/_count\nDELETE /a"),
            Safety::Writes {
                reason: "DELETE /a".into()
            }
        );
        assert!(matches!(
            classify_for(Elasticsearch, "{ \"size\": 1 }"),
            Safety::Unknown { .. }
        ));
    }

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
        rw("exec dbo.minha_proc");
        rw("update dbo.clientes set nome = N'x'");
        rw("select * from openquery(outro, 'delete from t')");
        rw("drop table dbo.clientes");
        let batch = read_only_batch_for(SqlServer, "select 1");
        assert!(batch.starts_with("SET XACT_ABORT ON;"));
        assert!(batch.ends_with("ROLLBACK TRANSACTION;"));
        assert_eq!(batch.lines().next().unwrap().matches("select 1").count(), 1);
    }

    /// O lote que o Tiago rodava a toda hora em Produção.
    const DECLARE_AND_SELECTS: &str = "DECLARE @lote int = 46521, @seq int = 11;\n\
        \n\
        SELECT l.DataHora, l.CaminhoDaPasta, m.Id AS PreservacaoMatrizId\n\
        FROM LIBERACAO_COPIA_COMPRIMIDA l\n\
        JOIN PRESERVACAO_MATRIZ m ON m.Id = l.Id\n\
        WHERE l.BatchId = @lote AND l.TransactionSequence = @seq;\n\
        \n\
        SELECT Desfecho, COUNT(*) AS Pastas,\n\
               CAST(SUM(TamanhoEmBytes) / 1073741824.0 AS decimal(10,1)) AS GB\n\
        FROM LIBERACAO_COPIA_COMPRIMIDA\n\
        GROUP BY Desfecho;";

    #[test]
    fn tsql_that_only_lives_in_the_batch_does_not_ask() {
        use pgstudio_core::connection::Engine::SqlServer;
        // Vale na janela de consulta e para o Claude.
        for classify in [
            (|sql| classify_for(SqlServer, sql)) as fn(&str) -> Safety,
            |sql| classify_located(SqlServer, sql).0,
        ] {
            let ro = |sql: &str| assert_eq!(classify(sql), Safety::ReadOnly, "{sql}");
            let rw = |sql: &str, reason: &str| {
                assert_eq!(
                    classify(sql),
                    Safety::Writes {
                        reason: reason.into()
                    },
                    "{sql}"
                )
            };
            ro(DECLARE_AND_SELECTS);
            ro("declare @t table (id int)\nselect * from @t");
            ro("declare c cursor for select 1");
            ro("declare @x int\nset @x = 1\nselect @x = count(*) from t\nprint @x");
            ro("if exists (select 1 from t) select 2 else select 3");
            ro("if 1 = 1 begin select 1 end");
            ro("begin try select 1 end try begin catch select 2; throw 50000, 'x', 1 end catch");
            ro("exec sp_who2");
            ro("EXEC sys.sp_helptext 'dbo.p'");
            ro("execute sp_help @objname = 'dbo.t'");
            ro("select 1\nraiserror('x', 16, 1)\nreturn");

            // O que altera continua perguntando, mesmo no meio do resto.
            rw("declare @x int = 1\ndelete from t where id = @x", "DELETE");
            rw("declare @x int = 1; update t set a = @x", "UPDATE");
            rw("if 1 = 1 begin delete from t end", "DELETE");
            rw(
                "if exists (select 1 from t) select 1 else drop table t",
                "DROP TABLE",
            );
            rw(
                "begin try truncate table t end try begin catch select 1 end catch",
                "TRUNCATE TABLE",
            );
            rw(
                "declare @t table (id int)\ninsert into @t select 1",
                "INSERT",
            );
            rw(
                "declare @x int\nselect * into dbo.copia from t",
                "SELECT INTO",
            );
            rw("create table #tmp (id int)", "CREATE TABLE");
            rw("begin tran", "BEGIN");
            rw("commit", "COMMIT");
            rw("exec dbo.sp_help", "EXECUTE");
            rw("exec outro.dbo.sp_who", "EXECUTE");
            ro("exec [sp_who]");
            rw("exec sp_executesql N'delete from t'", "EXECUTE");
            rw("exec sp_spaceused @updateusage = 'TRUE'", "EXECUTE");
            rw("exec ('delete from t')", "EXECUTE");
            rw("set identity_insert dbo.t on", "SET");
            rw("set rowcount 10", "SET");
            rw("set implicit_transactions on", "SET");
            rw("set noexec on", "SET");
            rw("declare @x int = (select nextval('s'))", "calls nextval()");
        }
    }

    #[test]
    fn the_query_window_also_accepts_what_only_changes_its_session() {
        use pgstudio_core::connection::Engine::SqlServer;
        for sql in [
            "use CONTROLE_JUCESP\nselect 1",
            "set nocount on\nselect 1",
            "SET NOCOUNT ON; SET STATISTICS IO ON; select 1",
            "set transaction isolation level read uncommitted\nselect 1",
            "set dateformat dmy\nset language 'Portuguese'\nselect 1",
            "set lock_timeout 5000",
            "set showplan_xml on",
        ] {
            assert_eq!(
                classify_located(SqlServer, sql).0,
                Safety::ReadOnly,
                "{sql}"
            );
            // O Claude roda numa sessão preparada pelo PostPilot.
            assert!(!classify_for(SqlServer, sql).is_read_only(), "{sql}");
        }
        assert!(
            !classify_located(SqlServer, "use outro\ndelete from t")
                .0
                .is_read_only()
        );
        let (_, place) = classify_located(SqlServer, "declare @x int = 1\n\nexec dbo.p @x");
        assert_eq!(place.map(|p| p.line), Some(3));
    }

    #[test]
    fn postgres_keeps_asking_for_the_same_statements() {
        rw("set search_path = x");
        rw("declare c cursor for select 1");
        rw("begin");
        assert!(!classify("use outro").is_read_only());
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

    /// O lote que o Tiago rodou em Produção: só `SELECT`, com um `;`
    /// faltando entre os dois últimos.
    const MISSING_SEMICOLON: &str = "-- 1) Confira primeiro o formato do JSON (nomes das chaves):\n\
        -- select id, valores_campos_complementares from caixa where valores_campos_complementares <> '[]'::jsonb limit 5;\n\
        \n\
        -- 2) Caixas com campo complementar \"departamento\" = 'GCm'\n\
        select c.*\n\
        from caixa c\n\
        where exists (\n\
            select 1\n\
            from jsonb_array_elements(c.valores_campos_complementares) e\n\
            where lower(e->>'nome') = 'Departamento'\n\
              and e->>'valor' Ilike 'GCM%'\n\
        );\n\
        \n\
        select * from campo_complementar\n\
        \n\
        \n\
        SELECT *\n\
        FROM caixa\n\
        ORDER BY id DESC\n\
        LIMIT 10\n";

    #[test]
    fn selects_with_a_missing_semicolon_only_read() {
        ro(MISSING_SEMICOLON);
        ro("select * from a\nselect * from b");
        ro("select 1 from t where\n;\nselect 2");
        ro("with x as (select 1) select * from x select 2");
        ro("select case when a then 1 else 2 end from t select 3");
        ro("select 'delete from t' from a select \"update\" from b");
        ro("select 1 -- drop table t\nselect /* delete */ 2");
        ro("explain select 1 select 2");
    }

    #[test]
    fn broken_sql_with_a_write_word_still_asks() {
        let asks = |sql: &str| {
            assert!(
                matches!(classify(sql), Safety::Unknown { .. }),
                "{sql}: {:?}",
                classify(sql)
            )
        };
        asks("select * from a\ndelete from b");
        asks("select * from a\ndrop table b");
        asks("select * from a update b set c = 1");
        asks("select 1 select * into novo from t");
        asks("select 1 select * from t for update");
        asks("select 1 select * from t for share");
        asks("with d as (delete from t returning *) select * from d select 2");
        asks("select 1 select pg_terminate_backend(1)");
        asks("select 1 select \"nextval\"('s')");
        asks("explain analyze select 1 select 2");
        asks("select 1 select 2; commit");
        asks("select 1 select 2; frobnicate t");
        asks("select 1 select 2; (select 3");
        asks("select 1 select 'aberta");
        asks("select 1 select 2; do $$ begin delete from t; end $$");
        asks("select 1 select 2 set search_path = x");
        asks("select 1 select 2 truncate t");
        asks("select 1 select 2 call p()");
    }

    #[test]
    fn t_sql_keeps_asking_when_it_is_not_understood() {
        use pgstudio_core::connection::Engine::SqlServer;
        assert!(matches!(
            classify_for(SqlServer, "select from from where"),
            Safety::Unknown { .. }
        ));
    }

    #[test]
    fn the_place_says_where_the_doubt_is() {
        use pgstudio_core::connection::Engine::{PostgreSql, SqlServer};
        assert_eq!(
            classify_located(PostgreSql, MISSING_SEMICOLON),
            (Safety::ReadOnly, None)
        );
        let (safety, place) = classify_located(
            PostgreSql,
            "select 1;\n\nselect * from a\nupdate b set c = 1",
        );
        assert!(matches!(safety, Safety::Unknown { .. }));
        assert_eq!(
            place,
            Some(Place {
                line: 4,
                near: Some("b".into())
            })
        );
        let (safety, place) =
            classify_located(PostgreSql, "select 1;\nselect 2;\n\nupdate t set a = 1");
        assert_eq!(
            safety,
            Safety::Writes {
                reason: "UPDATE".into()
            }
        );
        assert_eq!(
            place,
            Some(Place {
                line: 4,
                near: None
            })
        );
        let (_, place) = classify_located(PostgreSql, "select 1;\nselect\n  nextval('s')");
        assert_eq!(place.map(|p| p.line), Some(3));
        let (_, place) = classify_located(SqlServer, "select 1\nGO\ndrop table dbo.t");
        assert_eq!(place.map(|p| p.line), Some(3));
        let (_, place) = classify_located(PostgreSql, "select 'drop';\n-- drop\ndelete from t");
        assert_eq!(place.map(|p| p.line), Some(3));
        assert_eq!(classify_located(PostgreSql, "").1, None);
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
