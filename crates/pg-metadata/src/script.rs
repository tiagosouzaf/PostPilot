//! "Script as CREATE/DROP" e "Select Top N Rows" do menu de contexto.
//!
//! O DDL vem das funções do próprio PostgreSQL (`pg_get_viewdef`,
//! `pg_get_functiondef`, `pg_get_indexdef`, `pg_get_constraintdef`,
//! `pg_get_expr`); o `CREATE TABLE`, que o PostgreSQL não gera, é montado
//! a partir das colunas e constraints. Os nomes saem do `quote_ident` do
//! servidor, que conhece as palavras reservadas da versão.

use pgstudio_core::metadata::{MetadataError, NodeId, ScriptAction};
use tokio_postgres::types::ToSql;

use crate::CatalogExplorer;
use crate::path::{Path, dec};

/// Objeto com script, como o caminho do nó o identifica.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Object {
    Database(String),
    Schema(String, String),
    Relation(String, u32),
    Function(String, u32),
    Sequence(String, u32),
    Index(String, u32),
    Constraint(String, u32),
}

fn object(id: &NodeId) -> Option<Object> {
    match Path::parse(id) {
        Path::Database(db) => return Some(Object::Database(db)),
        Path::Schema(db, s) => return Some(Object::Schema(db, s)),
        Path::Table(db, oid) | Path::View(db, oid) => return Some(Object::Relation(db, oid)),
        _ => {}
    }
    let parts: Vec<&str> = id.0.split('/').collect();
    let ["db", db, tag, oid] = parts.as_slice() else {
        return None;
    };
    let (db, oid) = (dec(db), oid.parse().ok()?);
    Some(match *tag {
        "fn" => Object::Function(db, oid),
        "seq" => Object::Sequence(db, oid),
        "idx" => Object::Index(db, oid),
        "con" => Object::Constraint(db, oid),
        _ => return None,
    })
}

/// Banco de um nó, pelo caminho.
pub(crate) fn database_of(id: &NodeId) -> Option<String> {
    match Path::parse(id) {
        Path::Database(db)
        | Path::Schemas(db)
        | Path::Schema(db, _)
        | Path::SchemaFolder(db, _, _)
        | Path::Table(db, _)
        | Path::View(db, _)
        | Path::RelationFolder(db, _, _, _) => Some(db),
        _ => match id.0.split('/').collect::<Vec<_>>().as_slice() {
            ["db", db, ..] => Some(dec(db)),
            _ => None,
        },
    }
}

mod sql {
    pub const RELATION: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname), \
        c.relkind::text, c.relpersistence::text \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE c.oid = $1";

    /// Colunas para o CREATE TABLE. `serial` é uma coluna com default de
    /// uma sequência que pertence a ela (dependência automática).
    pub const COLUMNS: &str = "SELECT quote_ident(a.attname), \
        pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull, \
        pg_catalog.pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text, \
        EXISTS (SELECT 1 FROM pg_catalog.pg_depend dep \
            JOIN pg_catalog.pg_class s ON s.oid = dep.objid \
            WHERE dep.refobjid = a.attrelid AND dep.refobjsubid = a.attnum \
            AND dep.classid = 'pg_catalog.pg_class'::regclass \
            AND s.relkind = 'S' AND dep.deptype = 'a') \
        FROM pg_catalog.pg_attribute a \
        LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
        WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum";

    pub const CONSTRAINTS: &str = "SELECT quote_ident(conname), \
        pg_catalog.pg_get_constraintdef(oid, true) \
        FROM pg_catalog.pg_constraint \
        WHERE conrelid = $1 AND contype IN ('p', 'u', 'c', 'x', 'f') AND coninhcount = 0 \
        ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'c' THEN 2 \
            WHEN 'x' THEN 3 ELSE 4 END, conname";

    /// Índices que não são de constraints (essas já criam o índice).
    pub const INDEXES: &str = "SELECT pg_catalog.pg_get_indexdef(i.indexrelid) \
        FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
        WHERE i.indrelid = $1 AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
            WHERE k.conindid = i.indexrelid AND k.conrelid = i.indrelid) \
        ORDER BY c.relname";

    pub const PARTITION_KEY: &str = "SELECT pg_catalog.pg_get_partkeydef($1::oid)";

    pub const VIEW: &str = "SELECT pg_catalog.pg_get_viewdef($1::oid, true)";

    pub const FUNCTION: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(p.proname), \
        pg_catalog.pg_get_function_identity_arguments(p.oid), p.prokind::text, \
        pg_catalog.pg_get_functiondef(p.oid) \
        FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
        WHERE p.oid = $1";

    pub const SEQUENCE: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname), \
        pg_catalog.format_type(s.seqtypid, NULL), s.seqincrement, s.seqmin, s.seqmax, \
        s.seqstart, s.seqcache, s.seqcycle \
        FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid = s.seqrelid \
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE s.seqrelid = $1";

    pub const INDEX: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname), \
        pg_catalog.pg_get_indexdef(c.oid) \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE c.oid = $1";

    pub const CONSTRAINT: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(t.relname), \
        quote_ident(k.conname), pg_catalog.pg_get_constraintdef(k.oid, true) \
        FROM pg_catalog.pg_constraint k JOIN pg_catalog.pg_class t ON t.oid = k.conrelid \
        JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace WHERE k.oid = $1";

    pub const SELECT_COLUMNS: &str = "SELECT quote_ident(attname) FROM pg_catalog.pg_attribute \
        WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped ORDER BY attnum";

    pub const QUOTE: &str = "SELECT quote_ident($1)";

    pub const DATABASE: &str = "SELECT pg_catalog.pg_encoding_to_char(encoding) \
        FROM pg_catalog.pg_database WHERE datname = $1";
}

fn not_found(what: &str) -> MetadataError {
    MetadataError(format!(
        "The {what} no longer exists. Refresh the Object Explorer."
    ))
}

impl CatalogExplorer {
    fn one(
        &self,
        db: &str,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
        what: &str,
    ) -> Result<tokio_postgres::Row, MetadataError> {
        self.query(db, sql, params)?
            .into_iter()
            .next()
            .ok_or_else(|| not_found(what))
    }

    fn quote(&self, db: &str, name: &str) -> Result<String, MetadataError> {
        Ok(self.one(db, sql::QUOTE, &[&name], "object")?.get(0))
    }

    pub(crate) fn script_for(
        &self,
        node: &NodeId,
        action: ScriptAction,
    ) -> Result<String, MetadataError> {
        let object = object(node)
            .ok_or_else(|| MetadataError("This object cannot be scripted.".to_string()))?;
        match (object, action) {
            (Object::Relation(db, oid), ScriptAction::SelectTop(n)) => self.select_top(&db, oid, n),
            (_, ScriptAction::SelectTop(_)) => Err(MetadataError(
                "Only tables and views have rows to select.".to_string(),
            )),
            (Object::Database(db), action) => {
                let home = self.settings.database.clone();
                let name = self.quote(&home, &db)?;
                Ok(match action {
                    ScriptAction::Drop => format!("DROP DATABASE {name};\n"),
                    _ => {
                        let enc: String =
                            self.one(&home, sql::DATABASE, &[&db], "database")?.get(0);
                        format!("CREATE DATABASE {name}\n    WITH ENCODING '{enc}';\n")
                    }
                })
            }
            (Object::Schema(db, s), action) => {
                let name = self.quote(&db, &s)?;
                Ok(match action {
                    ScriptAction::Drop => format!("DROP SCHEMA {name};\n"),
                    _ => format!("CREATE SCHEMA {name};\n"),
                })
            }
            (Object::Relation(db, oid), action) => self.relation(&db, oid, action),
            (Object::Function(db, oid), action) => {
                let r = self.one(&db, sql::FUNCTION, &[&oid], "function")?;
                let (name, args, kind, def): (String, String, String, String) =
                    (r.get(0), r.get(1), r.get(2), r.get(3));
                let what = if kind == "p" { "PROCEDURE" } else { "FUNCTION" };
                Ok(match action {
                    ScriptAction::Drop => format!("DROP {what} {name}({args});\n"),
                    _ => format!("{};\n", def.trim_end()),
                })
            }
            (Object::Sequence(db, oid), action) => {
                let r = self.one(&db, sql::SEQUENCE, &[&oid], "sequence")?;
                let name: String = r.get(0);
                Ok(match action {
                    ScriptAction::Drop => format!("DROP SEQUENCE {name};\n"),
                    _ => {
                        let (ty, inc, min, max, start, cache, cycle): (
                            String,
                            i64,
                            i64,
                            i64,
                            i64,
                            i64,
                            bool,
                        ) = (
                            r.get(1),
                            r.get(2),
                            r.get(3),
                            r.get(4),
                            r.get(5),
                            r.get(6),
                            r.get(7),
                        );
                        format!(
                            "CREATE SEQUENCE {name}\n    AS {ty}\n    INCREMENT BY {inc}\n    \
                             MINVALUE {min}\n    MAXVALUE {max}\n    START WITH {start}\n    \
                             CACHE {cache}{};\n",
                            if cycle { "\n    CYCLE" } else { "" }
                        )
                    }
                })
            }
            (Object::Index(db, oid), action) => {
                let r = self.one(&db, sql::INDEX, &[&oid], "index")?;
                let (name, def): (String, String) = (r.get(0), r.get(1));
                Ok(match action {
                    ScriptAction::Drop => format!("DROP INDEX {name};\n"),
                    _ => format!("{def};\n"),
                })
            }
            (Object::Constraint(db, oid), action) => {
                let r = self.one(&db, sql::CONSTRAINT, &[&oid], "constraint")?;
                let (table, name, def): (String, String, String) = (r.get(0), r.get(1), r.get(2));
                Ok(match action {
                    ScriptAction::Drop => {
                        format!("ALTER TABLE {table}\n    DROP CONSTRAINT {name};\n")
                    }
                    _ => format!("ALTER TABLE {table}\n    ADD CONSTRAINT {name} {def};\n"),
                })
            }
        }
    }

    fn select_top(&self, db: &str, oid: u32, n: u32) -> Result<String, MetadataError> {
        let r = self.one(db, sql::RELATION, &[&oid], "table")?;
        let name: String = r.get(0);
        let cols: Vec<String> = self
            .query(db, sql::SELECT_COLUMNS, &[&oid])?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let list = if cols.is_empty() {
            "*".to_string()
        } else {
            cols.join(",\n       ")
        };
        Ok(format!("SELECT {list}\n  FROM {name}\n LIMIT {n};\n"))
    }

    fn relation(&self, db: &str, oid: u32, action: ScriptAction) -> Result<String, MetadataError> {
        let r = self.one(db, sql::RELATION, &[&oid], "table")?;
        let (name, kind, persistence): (String, String, String) = (r.get(0), r.get(1), r.get(2));
        let what = match kind.as_str() {
            "v" => "VIEW",
            "m" => "MATERIALIZED VIEW",
            "f" => "FOREIGN TABLE",
            _ => "TABLE",
        };
        if action == ScriptAction::Drop {
            return Ok(format!("DROP {what} {name};\n"));
        }
        match kind.as_str() {
            "v" | "m" => {
                let def: String = self.one(db, sql::VIEW, &[&oid], "view")?.get(0);
                let def = def.trim_end().trim_end_matches(';');
                Ok(if kind == "v" {
                    format!("CREATE VIEW {name} AS\n{def};\n")
                } else {
                    format!("CREATE MATERIALIZED VIEW {name} AS\n{def}\nWITH DATA;\n")
                })
            }
            "r" | "p" => self.create_table(db, oid, &name, &kind, &persistence),
            _ => Err(MetadataError(
                "Scripting foreign tables is not supported yet.".to_string(),
            )),
        }
    }

    fn create_table(
        &self,
        db: &str,
        oid: u32,
        name: &str,
        kind: &str,
        persistence: &str,
    ) -> Result<String, MetadataError> {
        let mut lines = Vec::new();
        for r in self.query(db, sql::COLUMNS, &[&oid])? {
            let (col, mut ty, not_null): (String, String, bool) = (r.get(0), r.get(1), r.get(2));
            let default: Option<String> = r.get(3);
            let (identity, generated, serial): (String, String, bool) =
                (r.get(4), r.get(5), r.get(6));
            let mut line = String::new();
            let mut default = default;
            if serial {
                // A sequência é criada pelo próprio serial.
                ty = match ty.as_str() {
                    "smallint" => "smallserial".into(),
                    "bigint" => "bigserial".into(),
                    _ => "serial".into(),
                };
                default = None;
            }
            line.push_str(&format!("{col} {ty}"));
            match identity.as_str() {
                "a" => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
                "d" => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                _ => {}
            }
            if generated == "s" {
                if let Some(expr) = default.take() {
                    line.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
                }
            }
            if not_null && !serial && identity.is_empty() {
                line.push_str(" NOT NULL");
            }
            if let Some(d) = default {
                line.push_str(&format!(" DEFAULT {d}"));
            }
            lines.push(line);
        }
        for r in self.query(db, sql::CONSTRAINTS, &[&oid])? {
            let (con, def): (String, String) = (r.get(0), r.get(1));
            lines.push(format!("CONSTRAINT {con} {def}"));
        }
        let unlogged = if persistence == "u" { "UNLOGGED " } else { "" };
        let mut out = format!(
            "CREATE {unlogged}TABLE {name} (\n    {}\n)",
            lines.join(",\n    ")
        );
        if kind == "p" {
            let key: String = self.one(db, sql::PARTITION_KEY, &[&oid], "table")?.get(0);
            out.push_str(&format!(" PARTITION BY {key}"));
        }
        out.push_str(";\n");
        for r in self.query(db, sql::INDEXES, &[&oid])? {
            let def: String = r.get(0);
            out.push_str(&format!("\n{def};\n"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_map_to_objects() {
        let id = |s: &str| NodeId(s.into());
        assert_eq!(
            object(&id("db/loja")),
            Some(Object::Database("loja".into()))
        );
        assert_eq!(
            object(&id("db/loja/s/sales%2F2024")),
            Some(Object::Schema("loja".into(), "sales/2024".into()))
        );
        assert_eq!(
            object(&id("db/loja/t/42")),
            Some(Object::Relation("loja".into(), 42))
        );
        assert_eq!(
            object(&id("db/loja/fn/7")),
            Some(Object::Function("loja".into(), 7))
        );
        assert_eq!(object(&id("db/loja/col/7/1")), None);
        assert_eq!(object(&id("server")), None);
        assert_eq!(database_of(&id("db/a%2Fb/idx/9")).as_deref(), Some("a/b"));
        assert_eq!(
            database_of(&id("db/loja/t/42/Columns")).as_deref(),
            Some("loja")
        );
        assert_eq!(database_of(&id("server/Security/Roles")), None);
    }
}
