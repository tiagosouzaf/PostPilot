//! "Script as CREATE/DROP" e "Select Top N Rows" do menu de contexto.
//!
//! O DDL vem das funções do próprio PostgreSQL (`pg_get_viewdef`,
//! `pg_get_functiondef`, `pg_get_indexdef`, `pg_get_constraintdef`,
//! `pg_get_triggerdef`, `pg_get_expr`); o `CREATE TABLE` e o `CREATE TYPE`,
//! que o PostgreSQL não gera, são montados a partir do catálogo. Os nomes saem do `quote_ident` do
//! servidor, que conhece as palavras reservadas da versão.

use pgstudio_core::metadata::{MetadataError, NodeId, ScriptAction};
use tokio_postgres::types::ToSql;

use crate::CatalogExplorer;
use crate::path::{Path, dec};

/// Objeto com script, como o caminho do nó o identifica.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Object {
    Database(String),
    Schema(String, String),
    Relation(String, u32),
    /// Função ou procedure.
    Function(String, u32),
    Sequence(String, u32),
    Index(String, u32),
    Constraint(String, u32),
    Trigger(String, u32),
    Type(String, u32),
    Extension(String, u32),
}

pub(crate) fn object(id: &NodeId) -> Option<Object> {
    match Path::parse(id) {
        Path::Database(db) => return Some(Object::Database(db)),
        Path::Schema(db, s) => return Some(Object::Schema(db, s)),
        Path::Table(db, oid) | Path::View(db, oid) | Path::MaterializedView(db, oid) => {
            return Some(Object::Relation(db, oid));
        }
        _ => {}
    }
    let parts: Vec<&str> = id.0.split('/').collect();
    let ["db", db, tag, oid] = parts.as_slice() else {
        return None;
    };
    let (db, oid) = (dec(db), oid.parse().ok()?);
    Some(match *tag {
        "fn" | "proc" => Object::Function(db, oid),
        "trg" => Object::Trigger(db, oid),
        "typ" => Object::Type(db, oid),
        "ext" => Object::Extension(db, oid),
        "seq" => Object::Sequence(db, oid),
        "idx" => Object::Index(db, oid),
        "con" => Object::Constraint(db, oid),
        _ => return None,
    })
}

/// Alvo de segurança de um nó: roles, a pasta Roles e objetos com
/// permissões (banco, schema, tabela, view e função).
pub(crate) fn securable(id: &NodeId) -> Option<pgstudio_core::metadata::Securable> {
    use pgstudio_core::metadata::Securable;
    if Path::parse(id) == Path::Roles {
        return Some(Securable::NewRole);
    }
    if let Some(oid) = id.0.strip_prefix("role/") {
        return oid.parse().ok().map(Securable::Role);
    }
    Some(match object(id)? {
        Object::Database(db) => Securable::Database(db),
        Object::Schema(database, name) => Securable::Schema { database, name },
        Object::Relation(database, oid) => Securable::Relation { database, oid },
        Object::Function(database, oid) => Securable::Function { database, oid },
        _ => return None,
    })
}

/// Banco de um nó, pelo caminho.
pub(crate) fn database_of(id: &NodeId) -> Option<String> {
    match Path::parse(id) {
        Path::Database(db)
        | Path::Schemas(db)
        | Path::Diagrams(db)
        | Path::Extensions(db)
        | Path::MaterializedView(db, _)
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

    pub const TRIGGER: &str = "SELECT quote_ident(t.tgname), \
        quote_ident(n.nspname) || '.' || quote_ident(c.relname), \
        pg_catalog.pg_get_triggerdef(t.oid, true), t.tgenabled::text \
        FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE t.oid = $1";

    /// Tipo, com o que o domínio precisa (tipo base, NOT NULL, default e
    /// collation quando difere da do tipo base).
    pub const TYPE: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(t.typname), \
        t.typtype::text, t.typrelid, pg_catalog.format_type(t.typbasetype, t.typtypmod), \
        t.typnotnull, t.typdefault, \
        CASE WHEN t.typcollation <> 0 AND t.typcollation <> b.typcollation \
            THEN quote_ident(cn.nspname) || '.' || quote_ident(co.collname) END \
        FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
        LEFT JOIN pg_catalog.pg_type b ON b.oid = t.typbasetype \
        LEFT JOIN pg_catalog.pg_collation co ON co.oid = t.typcollation \
        LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid = co.collnamespace \
        WHERE t.oid = $1";

    pub const ENUM_LABELS: &str = "SELECT quote_literal(enumlabel) FROM pg_catalog.pg_enum \
        WHERE enumtypid = $1 ORDER BY enumsortorder";

    /// CHECKs do domínio (o NOT NULL vem do `typnotnull`).
    pub const DOMAIN_CHECKS: &str = "SELECT quote_ident(conname), \
        pg_catalog.pg_get_constraintdef(oid, true) FROM pg_catalog.pg_constraint \
        WHERE contypid = $1 AND contype = 'c' ORDER BY conname";

    pub const COMPOSITE_ATTRIBUTES: &str = "SELECT quote_ident(a.attname), \
        pg_catalog.format_type(a.atttypid, a.atttypmod) FROM pg_catalog.pg_attribute a \
        WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum";

    /// Subtipo e função de diferença do range (a padrão do tipo fica de fora).
    pub const RANGE: &str = "SELECT pg_catalog.format_type(r.rngsubtype, NULL), \
        CASE WHEN r.rngsubdiff <> 0 THEN r.rngsubdiff::regproc::text END \
        FROM pg_catalog.pg_range r WHERE r.rngtypid = $1";

    pub const EXTENSION: &str = "SELECT quote_ident(e.extname), quote_ident(n.nspname), \
        quote_literal(e.extversion) \
        FROM pg_catalog.pg_extension e JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
        WHERE e.oid = $1";

    pub const SELECT_COLUMNS: &str = "SELECT quote_ident(attname) FROM pg_catalog.pg_attribute \
        WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped ORDER BY attnum";

    pub const QUOTE: &str = "SELECT quote_ident($1)";

    pub const DATABASE: &str = "SELECT pg_catalog.pg_encoding_to_char(encoding) \
        FROM pg_catalog.pg_database WHERE datname = $1";
}

fn not_found(what: &str) -> MetadataError {
    MetadataError(pgstudio_i18n::trf!(
        "The {what} no longer exists. Refresh the Object Explorer.",
        what = pgstudio_i18n::tr(what)
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
        Ok(self
            .one(db, sql::QUOTE, &[&name], pgstudio_i18n::tr_noop("object"))?
            .get(0))
    }

    pub(crate) fn script_for(
        &self,
        node: &NodeId,
        action: ScriptAction,
    ) -> Result<String, MetadataError> {
        let object = object(node).ok_or_else(|| {
            MetadataError(pgstudio_i18n::tr("This object cannot be scripted.").to_string())
        })?;
        match (object, action) {
            (Object::Relation(db, oid), ScriptAction::SelectTop(n)) => self.select_top(&db, oid, n),
            (_, ScriptAction::SelectTop(_)) => Err(MetadataError(
                pgstudio_i18n::tr("Only tables and views have rows to select.").to_string(),
            )),
            (Object::Database(db), action) => {
                let home = self.settings.database.clone();
                let name = self.quote(&home, &db)?;
                Ok(match action {
                    ScriptAction::Drop => format!("DROP DATABASE {name};\n"),
                    _ => {
                        let enc: String = self
                            .one(
                                &home,
                                sql::DATABASE,
                                &[&db],
                                pgstudio_i18n::tr_noop("database"),
                            )?
                            .get(0);
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
                let r = self.one(
                    &db,
                    sql::FUNCTION,
                    &[&oid],
                    pgstudio_i18n::tr_noop("function"),
                )?;
                let (name, args, kind, def): (String, String, String, String) =
                    (r.get(0), r.get(1), r.get(2), r.get(3));
                let what = if kind == "p" { "PROCEDURE" } else { "FUNCTION" };
                Ok(match action {
                    ScriptAction::Drop => format!("DROP {what} {name}({args});\n"),
                    _ => format!("{};\n", def.trim_end()),
                })
            }
            (Object::Sequence(db, oid), action) => {
                let r = self.one(
                    &db,
                    sql::SEQUENCE,
                    &[&oid],
                    pgstudio_i18n::tr_noop("sequence"),
                )?;
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
                let r = self.one(&db, sql::INDEX, &[&oid], pgstudio_i18n::tr_noop("index"))?;
                let (name, def): (String, String) = (r.get(0), r.get(1));
                Ok(match action {
                    ScriptAction::Drop => format!("DROP INDEX {name};\n"),
                    _ => format!("{def};\n"),
                })
            }
            (Object::Constraint(db, oid), action) => {
                let r = self.one(
                    &db,
                    sql::CONSTRAINT,
                    &[&oid],
                    pgstudio_i18n::tr_noop("constraint"),
                )?;
                let (table, name, def): (String, String, String) = (r.get(0), r.get(1), r.get(2));
                Ok(match action {
                    ScriptAction::Drop => {
                        format!("ALTER TABLE {table}\n    DROP CONSTRAINT {name};\n")
                    }
                    _ => format!("ALTER TABLE {table}\n    ADD CONSTRAINT {name} {def};\n"),
                })
            }
            (Object::Trigger(db, oid), action) => {
                let r = self.one(
                    &db,
                    sql::TRIGGER,
                    &[&oid],
                    pgstudio_i18n::tr_noop("trigger"),
                )?;
                let (name, table, def, enabled): (String, String, String, String) =
                    (r.get(0), r.get(1), r.get(2), r.get(3));
                Ok(match action {
                    ScriptAction::Drop => format!("DROP TRIGGER {name} ON {table};\n"),
                    _ => {
                        let mut out = format!("{def};\n");
                        if enabled == "D" {
                            out.push_str(&format!(
                                "\nALTER TABLE {table}\n    DISABLE TRIGGER {name};\n"
                            ));
                        }
                        out
                    }
                })
            }
            (Object::Type(db, oid), action) => self.type_script(&db, oid, action),
            (Object::Extension(db, oid), action) => {
                let r = self.one(
                    &db,
                    sql::EXTENSION,
                    &[&oid],
                    pgstudio_i18n::tr_noop("extension"),
                )?;
                let (name, schema, version): (String, String, String) =
                    (r.get(0), r.get(1), r.get(2));
                Ok(match action {
                    ScriptAction::Drop => format!("DROP EXTENSION {name};\n"),
                    _ => format!(
                        "CREATE EXTENSION IF NOT EXISTS {name}\n    WITH SCHEMA {schema}\n    \
                         VERSION {version};\n"
                    ),
                })
            }
        }
    }

    fn type_script(
        &self,
        db: &str,
        oid: u32,
        action: ScriptAction,
    ) -> Result<String, MetadataError> {
        let r = self.one(db, sql::TYPE, &[&oid], pgstudio_i18n::tr_noop("type"))?;
        let (name, kind, relid): (String, String, u32) = (r.get(0), r.get(1), r.get(2));
        if action == ScriptAction::Drop {
            let what = if kind == "d" { "DOMAIN" } else { "TYPE" };
            return Ok(format!("DROP {what} {name};\n"));
        }
        let column = |sql: &str, param: u32| -> Result<Vec<(String, String)>, MetadataError> {
            Ok(self
                .query(db, sql, &[&param])?
                .iter()
                .map(|r| (r.get(0), r.try_get(1).unwrap_or_default()))
                .collect())
        };
        Ok(match kind.as_str() {
            "e" => {
                let labels: Vec<String> = self
                    .query(db, sql::ENUM_LABELS, &[&oid])?
                    .iter()
                    .map(|r| r.get(0))
                    .collect();
                format!(
                    "CREATE TYPE {name} AS ENUM (\n    {}\n);\n",
                    labels.join(",\n    ")
                )
            }
            "d" => {
                let (base, not_null, default, collation): (
                    String,
                    bool,
                    Option<String>,
                    Option<String>,
                ) = (r.get(3), r.get(4), r.get(5), r.get(6));
                let mut out = format!("CREATE DOMAIN {name}\n    AS {base}");
                if let Some(c) = collation {
                    out.push_str(&format!("\n    COLLATE {c}"));
                }
                if let Some(d) = default {
                    out.push_str(&format!("\n    DEFAULT {d}"));
                }
                if not_null {
                    out.push_str("\n    NOT NULL");
                }
                for (con, def) in column(sql::DOMAIN_CHECKS, oid)? {
                    out.push_str(&format!("\n    CONSTRAINT {con} {def}"));
                }
                out.push_str(";\n");
                out
            }
            "c" => {
                let attrs: Vec<String> = column(sql::COMPOSITE_ATTRIBUTES, relid)?
                    .into_iter()
                    .map(|(a, ty)| format!("{a} {ty}"))
                    .collect();
                format!(
                    "CREATE TYPE {name} AS (\n    {}\n);\n",
                    attrs.join(",\n    ")
                )
            }
            "r" => {
                let r = self.one(db, sql::RANGE, &[&oid], pgstudio_i18n::tr_noop("type"))?;
                let (subtype, diff): (String, Option<String>) = (r.get(0), r.get(1));
                let mut out = format!("CREATE TYPE {name} AS RANGE (\n    SUBTYPE = {subtype}");
                if let Some(diff) = diff {
                    out.push_str(&format!(",\n    SUBTYPE_DIFF = {diff}"));
                }
                out.push_str("\n);\n");
                out
            }
            _ => {
                return Err(MetadataError(
                    pgstudio_i18n::tr("Scripting this kind of type is not supported.").to_string(),
                ));
            }
        })
    }

    fn select_top(&self, db: &str, oid: u32, n: u32) -> Result<String, MetadataError> {
        let r = self.one(db, sql::RELATION, &[&oid], pgstudio_i18n::tr_noop("table"))?;
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
        let r = self.one(db, sql::RELATION, &[&oid], pgstudio_i18n::tr_noop("table"))?;
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
                let def: String = self
                    .one(db, sql::VIEW, &[&oid], pgstudio_i18n::tr_noop("view"))?
                    .get(0);
                let def = def.trim_end().trim_end_matches(';');
                Ok(if kind == "v" {
                    format!("CREATE VIEW {name} AS\n{def};\n")
                } else {
                    format!("CREATE MATERIALIZED VIEW {name} AS\n{def}\nWITH DATA;\n")
                })
            }
            "r" | "p" => self.create_table(db, oid, &name, &kind, &persistence),
            _ => Err(MetadataError(
                pgstudio_i18n::tr("Scripting foreign tables is not supported yet.").to_string(),
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
            if generated == "s"
                && let Some(expr) = default.take()
            {
                line.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
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
            let key: String = self
                .one(
                    db,
                    sql::PARTITION_KEY,
                    &[&oid],
                    pgstudio_i18n::tr_noop("table"),
                )?
                .get(0);
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
        assert_eq!(
            object(&id("db/loja/proc/8")),
            Some(Object::Function("loja".into(), 8))
        );
        assert_eq!(
            object(&id("db/loja/mv/9")),
            Some(Object::Relation("loja".into(), 9))
        );
        assert_eq!(
            object(&id("db/loja/trg/10")),
            Some(Object::Trigger("loja".into(), 10))
        );
        assert_eq!(
            object(&id("db/loja/typ/11")),
            Some(Object::Type("loja".into(), 11))
        );
        assert_eq!(
            object(&id("db/loja/ext/12")),
            Some(Object::Extension("loja".into(), 12))
        );
        assert_eq!(
            database_of(&id("db/loja/Extensions")).as_deref(),
            Some("loja")
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

    #[test]
    fn securables_come_from_the_node_paths() {
        use pgstudio_core::metadata::Securable;
        let id = |s: &str| NodeId(s.into());
        assert_eq!(
            securable(&id("server/Security/Roles")),
            Some(Securable::NewRole)
        );
        assert_eq!(securable(&id("role/10")), Some(Securable::Role(10)));
        assert_eq!(
            securable(&id("db/loja")),
            Some(Securable::Database("loja".into()))
        );
        assert_eq!(
            securable(&id("db/loja/s/public")),
            Some(Securable::Schema {
                database: "loja".into(),
                name: "public".into()
            })
        );
        assert_eq!(
            securable(&id("db/loja/t/42")),
            Some(Securable::Relation {
                database: "loja".into(),
                oid: 42
            })
        );
        assert_eq!(
            securable(&id("db/loja/fn/7")),
            Some(Securable::Function {
                database: "loja".into(),
                oid: 7
            })
        );
        assert_eq!(
            securable(&id("db/loja/proc/8")),
            Some(Securable::Function {
                database: "loja".into(),
                oid: 8
            })
        );
        assert_eq!(
            securable(&id("db/loja/mv/9")),
            Some(Securable::Relation {
                database: "loja".into(),
                oid: 9
            })
        );
        assert_eq!(securable(&id("db/loja/idx/7")), None);
        assert_eq!(securable(&id("db/loja/typ/7")), None);
        assert_eq!(securable(&id("server")), None);
    }
}
