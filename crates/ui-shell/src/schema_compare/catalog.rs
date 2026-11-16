//! Lê de um banco o que o Schema Compare compara além das tabelas: views,
//! funções, procedures, triggers, sequências e tipos (sem os que pertencem
//! a extensões).

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::connection::Engine;
use pgstudio_core::metadata::MetadataProvider;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_core::schema::objects::{Kind, Object, Sequence};
use pgstudio_core::schema::snapshot::Schema;

/// Uma linha de resultado, em texto.
pub type Row = Vec<Option<String>>;

/// Lê os objetos das linhas de uma consulta.
type Parse = fn(&[Row]) -> Vec<Object>;

/// Schemas de usuário.
macro_rules! user_schema {
    () => {
        "n.nspname !~ '^pg_' AND n.nspname <> 'information_schema'"
    };
}

/// Não pertence a uma extensão nem é parte interna de outro objeto (as
/// funções construtoras de um range). `$class` é o catálogo, `$oid` o
/// objeto.
macro_rules! not_in_extension {
    ($class:literal, $oid:literal) => {
        concat!(
            "NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d WHERE d.classid = '",
            $class,
            "'::regclass AND d.objid = ",
            $oid,
            " AND d.deptype IN ('e', 'i'))"
        )
    };
}

/// Views: schema, nome, `pg_get_viewdef`.
pub const VIEWS_SQL: &str = concat!(
    "SELECT n.nspname, c.relname, pg_catalog.pg_get_viewdef(c.oid) \
    FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    WHERE c.relkind = 'v' AND ",
    user_schema!(),
    " AND ",
    not_in_extension!("pg_catalog.pg_class", "c.oid"),
    " ORDER BY 1, 2"
);

/// Funções e procedures: schema, nome, argumentos, `pg_get_functiondef`,
/// `prokind`.
pub const ROUTINES_SQL: &str = concat!(
    "SELECT n.nspname, p.proname, \
    pg_catalog.pg_get_function_identity_arguments(p.oid), pg_catalog.pg_get_functiondef(p.oid), \
    p.prokind::text \
    FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
    WHERE p.prokind IN ('f', 'p') AND ",
    user_schema!(),
    " AND ",
    not_in_extension!("pg_catalog.pg_proc", "p.oid"),
    " ORDER BY 1, 2, 3"
);

/// Triggers de usuário: schema e nome da tabela, nome, `pg_get_triggerdef`.
pub const TRIGGERS_SQL: &str = concat!(
    "SELECT n.nspname, c.relname, t.tgname, pg_catalog.pg_get_triggerdef(t.oid) \
    FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    WHERE NOT t.tgisinternal AND ",
    user_schema!(),
    " AND ",
    not_in_extension!("pg_catalog.pg_class", "c.oid"),
    " ORDER BY 1, 2, 3"
);

/// Sequências (sem as de colunas identity, que são parte da coluna):
/// schema, nome, tipo, start, increment, min, max, cache, cycle e a coluna
/// dona.
pub const SEQUENCES_SQL: &str = concat!(
    "SELECT n.nspname, c.relname, pg_catalog.format_type(s.seqtypid, NULL), \
    s.seqstart::text, s.seqincrement::text, s.seqmin::text, s.seqmax::text, \
    s.seqcache::text, s.seqcycle::text, \
    (SELECT pg_catalog.quote_ident(tn.nspname) || '.' || pg_catalog.quote_ident(t.relname) \
        || '.' || pg_catalog.quote_ident(a.attname) \
        FROM pg_catalog.pg_depend d JOIN pg_catalog.pg_class t ON t.oid = d.refobjid \
        JOIN pg_catalog.pg_namespace tn ON tn.oid = t.relnamespace \
        JOIN pg_catalog.pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid \
        WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = c.oid \
        AND d.refclassid = 'pg_catalog.pg_class'::regclass AND d.deptype = 'a' LIMIT 1) \
    FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid = s.seqrelid \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
    WHERE ",
    user_schema!(),
    " AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
        WHERE d.classid = 'pg_catalog.pg_class'::regclass AND d.objid = c.oid \
        AND d.deptype IN ('e', 'i')) \
    ORDER BY 1, 2"
);

/// Enums, tipos compostos, domínios e ranges: schema, nome, `typtype` e o
/// corpo (os valores do enum em JSON; os atributos; o tipo base com default
/// e checks; o subtipo).
pub const TYPES_SQL: &str = concat!(
    "SELECT n.nspname, t.typname, t.typtype::text, CASE t.typtype \
    WHEN 'e' THEN (SELECT pg_catalog.json_agg(e.enumlabel ORDER BY e.enumsortorder)::text \
        FROM pg_catalog.pg_enum e WHERE e.enumtypid = t.oid) \
    WHEN 'c' THEN (SELECT pg_catalog.string_agg(pg_catalog.quote_ident(a.attname) || ' ' \
        || pg_catalog.format_type(a.atttypid, a.atttypmod), ', ' ORDER BY a.attnum) \
        FROM pg_catalog.pg_attribute a \
        WHERE a.attrelid = t.typrelid AND a.attnum > 0 AND NOT a.attisdropped) \
    WHEN 'd' THEN pg_catalog.format_type(t.typbasetype, t.typtypmod) \
        || COALESCE(' DEFAULT ' || t.typdefault, '') \
        || CASE WHEN t.typnotnull THEN ' NOT NULL' ELSE '' END \
        || COALESCE((SELECT pg_catalog.string_agg(' CONSTRAINT ' \
            || pg_catalog.quote_ident(co.conname) || ' ' || pg_catalog.pg_get_constraintdef(co.oid), \
            '' ORDER BY co.conname) \
            FROM pg_catalog.pg_constraint co WHERE co.contypid = t.oid AND co.contype = 'c'), '') \
    WHEN 'r' THEN (SELECT 'SUBTYPE = ' || pg_catalog.format_type(r.rngsubtype, NULL) \
        FROM pg_catalog.pg_range r WHERE r.rngtypid = t.oid) \
    END \
    FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
    WHERE t.typtype IN ('e', 'c', 'd', 'r') AND ",
    user_schema!(),
    " AND (t.typtype <> 'c' OR EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
        WHERE c.oid = t.typrelid AND c.relkind = 'c')) AND ",
    not_in_extension!("pg_catalog.pg_type", "t.oid"),
    " ORDER BY 1, 2"
);

fn text(r: &Row, i: usize) -> Option<String> {
    r.get(i).cloned().flatten()
}

/// As views, das linhas de [`VIEWS_SQL`].
pub fn views(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| Some(Object::view(&text(r, 0)?, &text(r, 1)?, &text(r, 2)?)))
        .collect()
}

/// As funções e procedures, das linhas de [`ROUTINES_SQL`].
pub fn routines(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            Some(Object::routine(
                &text(r, 0)?,
                &text(r, 1)?,
                &text(r, 2).unwrap_or_default(),
                &text(r, 3)?,
                text(r, 4).as_deref() == Some("p"),
            ))
        })
        .collect()
}

/// Os triggers, das linhas de [`TRIGGERS_SQL`].
pub fn triggers(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            Some(Object::trigger(
                &text(r, 0)?,
                &text(r, 1)?,
                &text(r, 2)?,
                &text(r, 3)?,
            ))
        })
        .collect()
}

/// As sequências, das linhas de [`SEQUENCES_SQL`].
pub fn sequences(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            let seq = Sequence {
                ty: text(r, 2)?,
                start: text(r, 3)?,
                increment: text(r, 4)?,
                min: text(r, 5)?,
                max: text(r, 6)?,
                cache: text(r, 7)?,
                cycle: text(r, 8).as_deref() == Some("true"),
                owned_by: text(r, 9),
            };
            Some(Object::sequence(&text(r, 0)?, &text(r, 1)?, seq))
        })
        .collect()
}

/// Os tipos, das linhas de [`TYPES_SQL`].
pub fn types(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            let (schema, name) = (text(r, 0)?, text(r, 1)?);
            let body = text(r, 3).unwrap_or_default();
            Some(match text(r, 2)?.as_str() {
                "e" => Object::enum_type(
                    &schema,
                    &name,
                    serde_json::from_str(&body).unwrap_or_default(),
                ),
                "c" => Object::composite_type(&schema, &name, &body),
                "d" => Object::domain(&schema, &name, &body),
                "r" => Object::range_type(&schema, &name, &body),
                _ => return None,
            })
        })
        .collect()
}

/// SQL Server: views, funções, procedures e triggers de tabela, com o
/// texto do `CREATE` (os cifrados, sem texto, ficam de fora).
const TSQL_MODULES_SQL: &str = "\
SELECT SCHEMA_NAME(o.schema_id), o.name, RTRIM(o.type), m.definition, \
       OBJECT_NAME(o.parent_object_id) \
  FROM sys.sql_modules m JOIN sys.objects o ON o.object_id = m.object_id \
 WHERE o.is_ms_shipped = 0 AND o.type IN ('V', 'P', 'FN', 'IF', 'TF', 'TR') \
   AND m.definition IS NOT NULL \
 ORDER BY 1, 2";

const TSQL_SEQUENCES_SQL: &str = "\
SELECT SCHEMA_NAME(s.schema_id), s.name, TYPE_NAME(s.user_type_id), \
       CAST(s.start_value AS nvarchar(40)), CAST(s.increment AS nvarchar(40)), \
       CAST(s.minimum_value AS nvarchar(40)), CAST(s.maximum_value AS nvarchar(40)), \
       CASE WHEN s.is_cached = 0 THEN '0' ELSE ISNULL(CAST(s.cache_size AS varchar(20)), '') END, \
       s.is_cycling \
  FROM sys.sequences s WHERE s.is_ms_shipped = 0 ORDER BY 1, 2";

/// Tipos de alias, com o tipo base como o `CREATE TYPE` escreve.
const TSQL_TYPES_SQL: &str = "\
SELECT SCHEMA_NAME(t.schema_id), t.name, \
       b.name + CASE \
         WHEN b.name IN ('varchar', 'char', 'varbinary', 'binary') \
           THEN '(' + IIF(t.max_length = -1, 'max', CAST(t.max_length AS varchar(10))) + ')' \
         WHEN b.name IN ('nvarchar', 'nchar') \
           THEN '(' + IIF(t.max_length = -1, 'max', CAST(t.max_length / 2 AS varchar(10))) + ')' \
         WHEN b.name IN ('decimal', 'numeric') \
           THEN '(' + CAST(t.precision AS varchar(10)) + ',' + CAST(t.scale AS varchar(10)) + ')' \
         WHEN b.name IN ('datetime2', 'time', 'datetimeoffset') \
           THEN '(' + CAST(t.scale AS varchar(10)) + ')' \
         ELSE '' END, \
       t.is_nullable \
  FROM sys.types t \
  JOIN sys.types b ON b.user_type_id = t.system_type_id AND b.is_user_defined = 0 \
 WHERE t.is_user_defined = 1 AND t.is_table_type = 0 AND t.is_assembly_type = 0 \
 ORDER BY 1, 2";

/// `1`, `true` ou `True`, como os drivers escrevem um `bit`.
fn flag(r: &Row, i: usize) -> bool {
    text(r, i).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub fn tsql_modules(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            let (schema, name, def) = (text(r, 0)?, text(r, 1)?, text(r, 3)?);
            let kind = match text(r, 2)?.as_str() {
                "V" => Kind::View,
                "P" => Kind::Procedure,
                "TR" => Kind::Trigger,
                _ => Kind::Function,
            };
            let table = text(r, 4).filter(|_| kind == Kind::Trigger);
            Some(Object::tsql_module(
                kind,
                &schema,
                &name,
                table.as_deref(),
                &def,
            ))
        })
        .collect()
}

pub fn tsql_sequences(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            let seq = Sequence {
                ty: text(r, 2)?,
                start: text(r, 3)?,
                increment: text(r, 4)?,
                min: text(r, 5)?,
                max: text(r, 6)?,
                cache: text(r, 7).unwrap_or_default(),
                cycle: flag(r, 8),
                owned_by: None,
            };
            Some(Object::tsql_sequence(&text(r, 0)?, &text(r, 1)?, seq))
        })
        .collect()
}

pub fn tsql_types(rows: &[Row]) -> Vec<Object> {
    rows.iter()
        .filter_map(|r| {
            Some(Object::tsql_alias_type(
                &text(r, 0)?,
                &text(r, 1)?,
                &text(r, 2)?,
                flag(r, 3),
            ))
        })
        .collect()
}

/// As linhas de uma consulta, esperando nesta thread.
pub fn rows(session: &Arc<dyn QuerySession>, sql: &str) -> Result<Vec<Row>, String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut out = Vec::new();
    let mut error = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(120)) {
            Ok(QueryEvent::Row(r)) => out.push(r),
            Ok(QueryEvent::Error(e)) => error = Some(e.message),
            Ok(QueryEvent::Finished { .. }) => break,
            Ok(_) => {}
            Err(_) => return Err("The server did not answer.".into()),
        }
    }
    error.map_or(Ok(out), Err)
}

/// O schema inteiro de `database` (roda numa thread).
pub fn load(
    provider: &dyn MetadataProvider,
    queries: &dyn QueryService,
    database: &str,
) -> Result<Schema, String> {
    let model = provider.load_model(database).map_err(|e| e.0)?;
    let session = queries.open(database);
    let mut objects = Vec::new();
    let parts: &[(&str, Parse)] = match session.engine() {
        Engine::PostgreSql | Engine::Redis | Engine::MongoDb => &[
            (VIEWS_SQL, views),
            (ROUTINES_SQL, routines),
            (TRIGGERS_SQL, triggers),
            (SEQUENCES_SQL, sequences),
            (TYPES_SQL, types),
        ],
        Engine::SqlServer => &[
            (TSQL_MODULES_SQL, tsql_modules),
            (TSQL_SEQUENCES_SQL, tsql_sequences),
            (TSQL_TYPES_SQL, tsql_types),
        ],
    };
    for &(sql, parse) in parts {
        objects.extend(parse(&rows(&session, sql)?));
    }
    Ok(Schema { model, objects })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::schema::objects::Detail;

    fn row(v: &[Option<&str>]) -> Row {
        v.iter().map(|s| s.map(str::to_string)).collect()
    }

    #[test]
    fn rows_become_objects() {
        let v = views(&[row(&[Some("public"), Some("v"), Some(" SELECT 1;")])]);
        assert_eq!(v[0].definition, "SELECT 1");
        let r = routines(&[row(&[
            Some("public"),
            Some("p"),
            None,
            Some("CREATE OR REPLACE PROCEDURE public.p()\n"),
            Some("p"),
        ])]);
        assert_eq!(r[0].key(), (Kind::Procedure, "public".into(), "p()".into()));
        let t = triggers(&[row(&[
            Some("public"),
            Some("pedido"),
            Some("audita"),
            Some("CREATE TRIGGER audita ..."),
        ])]);
        assert_eq!(t[0].name, "pedido.audita");
        let s = sequences(&[row(&[
            Some("public"),
            Some("pedido_id_seq"),
            Some("integer"),
            Some("1"),
            Some("1"),
            Some("1"),
            Some("2147483647"),
            Some("1"),
            Some("false"),
            Some("public.pedido.id"),
        ])]);
        match &s[0].detail {
            Detail::Sequence(seq) => {
                assert!(!seq.cycle);
                assert_eq!(seq.owned_by.as_deref(), Some("public.pedido.id"));
            }
            d => panic!("{d:?}"),
        }
        let ty = types(&[
            row(&[
                Some("public"),
                Some("status"),
                Some("e"),
                Some("[\"novo\", \"pago\"]"),
            ]),
            row(&[
                Some("public"),
                Some("cep"),
                Some("d"),
                Some("text NOT NULL"),
            ]),
            row(&[Some("public"), Some("x"), Some("b"), None]),
        ]);
        assert_eq!(ty.len(), 2);
        assert_eq!(
            ty[0].detail,
            Detail::Enum(vec!["novo".into(), "pago".into()])
        );
        assert_eq!(
            ty[1].definition,
            "CREATE DOMAIN public.cep AS text NOT NULL;"
        );
    }
}
