//! Object Explorer do SQL Server, com a árvore do SSMS: servidor, bancos
//! (os do sistema numa pasta à parte) e, em cada banco, Tables, Views e
//! Programmability; em cada tabela, Columns, Keys, Constraints, Triggers e
//! Indexes.
//!
//! Uma sessão só serve todos os bancos: as consultas usam nomes de três
//! partes (`[banco].sys.objects`). Os ids dos nós carregam o nome do banco
//! (com `/` e `%` escapados) e os ids numéricos do catálogo:
//!
//! | Nó | Id |
//! | --- | --- |
//! | banco | `db/<banco>` |
//! | pasta do banco | `db/<banco>/f/<pasta>` |
//! | tabela, view | `db/<banco>/t/<object_id>`, `db/<banco>/v/<object_id>` |
//! | pasta da tabela | `db/<banco>/t/<object_id>/f/<pasta>` |
//! | coluna | `db/<banco>/col/<object_id>/<column_id>` |
//! | índice | `db/<banco>/ix/<object_id>/<index_id>` |
//! | procedure, função, constraint, trigger, sequence | `db/<banco>/o/<object_id>` |
//! | tipo | `db/<banco>/type/<user_type_id>` |

use pgstudio_core::metadata::{
    DesignTarget, Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property,
    ScriptAction,
};
use pgstudio_i18n::tr;
use pgstudio_pg_client::DbRuntime;

use crate::Session;

/// Nomes das colunas e linhas de uma consulta.
type Named = (Vec<String>, Vec<Vec<Option<String>>>);

/// Lista dos bancos que o login pode abrir.
pub const DATABASES_SQL: &str = "SELECT name FROM sys.databases \
    WHERE HAS_DBACCESS(name) = 1 AND state = 0 ORDER BY name";

/// Os bancos do sistema, que o SSMS põe em "System Databases".
const SYSTEM_DATABASES: [&str; 4] = ["master", "model", "msdb", "tempdb"];

const SERVER: &str = "server";
const DATABASES: &str = "server/Databases";
const SYSTEM: &str = "server/Databases/System";

/// Lê os metadados com a sessão aberta pelo Connect to Server.
pub struct MssqlExplorer {
    runtime: DbRuntime,
    session: tokio::sync::Mutex<Session>,
    label: String,
}

impl MssqlExplorer {
    pub fn new(session: Session, _password: String, runtime: DbRuntime) -> Self {
        let s = session.settings();
        let label = format!(
            "{} (SQL Server {} - {})",
            s.server_name(),
            session.server_version(),
            s.user
        );
        MssqlExplorer {
            runtime,
            session: tokio::sync::Mutex::new(session),
            label,
        }
    }

    /// Nome do servidor como o SSMS mostra: `host (SQL Server 16.0.4135.4 - sa)`.
    pub fn server_label(&self) -> &str {
        &self.label
    }

    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, MetadataError> {
        self.runtime
            .block_on(async { self.session.lock().await.rows(sql).await })
            .map_err(MetadataError)
    }

    fn table(&self, sql: &str) -> Result<Named, MetadataError> {
        self.runtime
            .block_on(async { self.session.lock().await.table(sql).await })
            .map_err(MetadataError)
    }

    /// Nós `(id, nome)` de uma consulta de duas colunas, com o id prefixado.
    fn listed(
        &self,
        sql: &str,
        prefix: &str,
        kind: NodeKind,
        children: bool,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(self
            .rows(sql)?
            .into_iter()
            .filter_map(|r| {
                let mut r = r.into_iter();
                let id = r.next().flatten()?;
                let name = r.next().flatten()?;
                Some(node(format!("{prefix}{id}"), name, kind, children))
            })
            .collect())
    }

    fn database_children(&self, id: &str, db: &str) -> Result<Vec<ObjectNode>, MetadataError> {
        let _ = db;
        Ok([
            ("tables", Folder::Tables),
            ("views", Folder::Views),
            ("prog", Folder::Programmability),
        ]
        .into_iter()
        .map(|(tag, f)| folder(format!("{id}/f/{tag}"), f))
        .collect())
    }

    fn database_folder(
        &self,
        id: &str,
        db: &str,
        tag: &str,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let d = bracket(db);
        let base = format!("db/{}/", enc(db));
        match tag {
            "tables" => self.listed(
                &format!(
                    "SELECT o.object_id, s.name + '.' + o.name FROM {d}.sys.tables o \
                     JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                     WHERE o.is_ms_shipped = 0 ORDER BY s.name, o.name"
                ),
                &format!("{base}t/"),
                NodeKind::Table,
                true,
            ),
            "views" => self.listed(
                &format!(
                    "SELECT o.object_id, s.name + '.' + o.name FROM {d}.sys.views o \
                     JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                     WHERE o.is_ms_shipped = 0 ORDER BY s.name, o.name"
                ),
                &format!("{base}v/"),
                NodeKind::View,
                true,
            ),
            "prog" => Ok([
                ("procs", Folder::Procedures),
                ("funcs", Folder::Functions),
                ("types", Folder::Types),
                ("seqs", Folder::Sequences),
            ]
            .into_iter()
            .map(|(tag, f)| folder(format!("{base}f/{tag}"), f))
            .collect()),
            "procs" => self.objects(db, "o.type IN ('P', 'PC')", NodeKind::Procedure),
            "funcs" => self.objects(
                db,
                "o.type IN ('FN', 'IF', 'TF', 'FS', 'FT')",
                NodeKind::Function,
            ),
            "seqs" => self.objects(db, "o.type = 'SO'", NodeKind::Sequence),
            "types" => self.listed(
                &format!(
                    "SELECT t.user_type_id, s.name + '.' + t.name FROM {d}.sys.types t \
                     JOIN {d}.sys.schemas s ON s.schema_id = t.schema_id \
                     WHERE t.is_user_defined = 1 ORDER BY s.name, t.name"
                ),
                &format!("{base}type/"),
                NodeKind::Type,
                false,
            ),
            _ => {
                let _ = id;
                Ok(Vec::new())
            }
        }
    }

    /// Objetos do banco (fora as tabelas e views) que passam em `filter`.
    fn objects(
        &self,
        db: &str,
        filter: &str,
        kind: NodeKind,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let d = bracket(db);
        self.listed(
            &format!(
                "SELECT o.object_id, s.name + '.' + o.name FROM {d}.sys.objects o \
                 JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                 WHERE o.is_ms_shipped = 0 AND {filter} ORDER BY s.name, o.name"
            ),
            &format!("db/{}/o/", enc(db)),
            kind,
            false,
        )
    }

    fn relation_children(&self, id: &str, view: bool) -> Vec<ObjectNode> {
        let folders: &[(&str, Folder)] = if view {
            &[("cols", Folder::Columns), ("idx", Folder::Indexes)]
        } else {
            &[
                ("cols", Folder::Columns),
                ("keys", Folder::Keys),
                ("cons", Folder::Constraints),
                ("trig", Folder::Triggers),
                ("idx", Folder::Indexes),
            ]
        };
        folders
            .iter()
            .map(|(tag, f)| folder(format!("{id}/f/{tag}"), *f))
            .collect()
    }

    fn relation_folder(
        &self,
        db: &str,
        obj: i64,
        tag: &str,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let d = bracket(db);
        let base = format!("db/{}/", enc(db));
        match tag {
            "cols" => Ok(self
                .rows(&format!(
                    "SELECT c.column_id, c.name, t.name, c.max_length, c.precision, c.scale, \
                            c.is_nullable, c.is_computed, \
                            CASE WHEN EXISTS (SELECT 1 FROM {d}.sys.index_columns ic \
                                 JOIN {d}.sys.indexes i ON i.object_id = ic.object_id \
                                  AND i.index_id = ic.index_id AND i.is_primary_key = 1 \
                                 WHERE ic.object_id = c.object_id \
                                   AND ic.column_id = c.column_id) THEN 1 ELSE 0 END, \
                            CASE WHEN EXISTS (SELECT 1 FROM {d}.sys.foreign_key_columns f \
                                 WHERE f.parent_object_id = c.object_id \
                                   AND f.parent_column_id = c.column_id) THEN 1 ELSE 0 END \
                       FROM {d}.sys.columns c \
                       JOIN {d}.sys.types t ON t.user_type_id = c.user_type_id \
                      WHERE c.object_id = {obj} ORDER BY c.column_id"
                ))?
                .into_iter()
                .filter_map(|r| {
                    let id = r.first().cloned().flatten()?;
                    Some(node(
                        format!("{base}col/{obj}/{id}"),
                        column_label(&r),
                        NodeKind::Column,
                        false,
                    ))
                })
                .collect()),
            "keys" => self.children_objects(
                db,
                obj,
                "o.type IN ('PK', 'UQ', 'F')",
                "CASE o.type WHEN 'PK' THEN 0 WHEN 'UQ' THEN 1 ELSE 2 END, o.name",
                NodeKind::Constraint,
            ),
            "cons" => self.children_objects(
                db,
                obj,
                "o.type IN ('C', 'D')",
                "o.name",
                NodeKind::Constraint,
            ),
            "trig" => self.children_objects(db, obj, "o.type = 'TR'", "o.name", NodeKind::Trigger),
            "idx" => Ok(self
                .rows(&format!(
                    "SELECT i.index_id, i.name, i.type, i.is_unique FROM {d}.sys.indexes i \
                      WHERE i.object_id = {obj} AND i.type > 0 AND i.is_hypothetical = 0 \
                      ORDER BY i.name"
                ))?
                .into_iter()
                .filter_map(|r| {
                    let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                    let id = r.first().cloned().flatten()?;
                    let kind = match text(2).as_str() {
                        "1" | "5" => tr("Clustered"),
                        _ => tr("Non-Clustered"),
                    };
                    let label = if text(3) == "1" {
                        format!("{} ({}, {})", text(1), tr("Unique"), kind)
                    } else {
                        format!("{} ({kind})", text(1))
                    };
                    Some(node(
                        format!("{base}ix/{obj}/{id}"),
                        label,
                        NodeKind::Index,
                        false,
                    ))
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn children_objects(
        &self,
        db: &str,
        parent: i64,
        filter: &str,
        order: &str,
        kind: NodeKind,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let d = bracket(db);
        self.listed(
            &format!(
                "SELECT o.object_id, o.name FROM {d}.sys.objects o \
                 WHERE o.parent_object_id = {parent} AND {filter} ORDER BY {order}"
            ),
            &format!("db/{}/o/", enc(db)),
            kind,
            false,
        )
    }

    /// `[schema].[nome]` e o tipo (`sys.objects.type`) de um objeto.
    fn object_name(&self, db: &str, obj: i64) -> Result<(String, String, String), MetadataError> {
        let d = bracket(db);
        let r = self
            .rows(&format!(
                "SELECT QUOTENAME(s.name) + '.' + QUOTENAME(o.name), RTRIM(o.type), \
                        QUOTENAME(ps.name) + '.' + QUOTENAME(p.name) \
                   FROM {d}.sys.objects o \
                   JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                   LEFT JOIN {d}.sys.objects p ON p.object_id = o.parent_object_id \
                   LEFT JOIN {d}.sys.schemas ps ON ps.schema_id = p.schema_id \
                  WHERE o.object_id = {obj}"
            ))?
            .into_iter()
            .next()
            .ok_or_else(gone)?;
        let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        Ok((text(0), text(1), text(2)))
    }

    fn select_top(&self, db: &str, obj: i64, n: u32) -> Result<String, MetadataError> {
        let (name, _, _) = self.object_name(db, obj)?;
        let cols: Vec<String> = self
            .rows(&format!(
                "SELECT QUOTENAME(name) FROM {}.sys.columns WHERE object_id = {obj} \
                 ORDER BY column_id",
                bracket(db)
            ))?
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        let list = if cols.is_empty() {
            "*".to_string()
        } else {
            cols.join("\n      ,")
        };
        Ok(format!("SELECT TOP ({n}) {list}\n  FROM {name}\n"))
    }

    fn script_object(
        &self,
        db: &str,
        obj: i64,
        action: ScriptAction,
    ) -> Result<String, MetadataError> {
        let (name, ty, parent) = self.object_name(db, obj)?;
        if action == ScriptAction::Drop {
            let what = match ty.as_str() {
                "U" => "TABLE",
                "V" => "VIEW",
                "P" | "PC" => "PROCEDURE",
                "FN" | "IF" | "TF" | "FS" | "FT" => "FUNCTION",
                "TR" => "TRIGGER",
                "SO" => "SEQUENCE",
                "PK" | "UQ" | "F" | "C" | "D" => {
                    // A constraint é do nome sem schema, dentro da tabela.
                    let short = name.split_once("].").map_or(name.as_str(), |(_, n)| n);
                    return Ok(format!(
                        "ALTER TABLE {parent} DROP CONSTRAINT {short}\nGO\n"
                    ));
                }
                _ => return Err(not_yet()),
            };
            return Ok(format!("DROP {what} {name}\nGO\n"));
        }
        match ty.as_str() {
            "V" | "P" | "FN" | "IF" | "TF" | "TR" => {
                let def = self
                    .rows(&format!(
                        "SELECT definition FROM {}.sys.sql_modules WHERE object_id = {obj}",
                        bracket(db)
                    ))?
                    .into_iter()
                    .next()
                    .and_then(|r| r.into_iter().next().flatten())
                    .ok_or_else(|| {
                        MetadataError(
                            tr("The definition is encrypted or not visible to this login.")
                                .to_string(),
                        )
                    })?;
                Ok(format!(
                    "SET ANSI_NULLS ON\nGO\nSET QUOTED_IDENTIFIER ON\nGO\n{}\nGO\n",
                    def.trim()
                ))
            }
            _ => Err(not_yet()),
        }
    }

    fn properties_of(&self, sql: &str) -> Result<Vec<Property>, MetadataError> {
        let (names, rows) = self.table(sql)?;
        let row = rows.into_iter().next().ok_or_else(gone)?;
        Ok(names
            .into_iter()
            .zip(row)
            .filter_map(|(n, v)| Some(Property::new(n, v?)))
            .collect())
    }
}

fn node(
    id: impl Into<String>,
    name: impl Into<String>,
    kind: NodeKind,
    children: bool,
) -> ObjectNode {
    ObjectNode {
        id: NodeId(id.into()),
        name: name.into(),
        kind,
        has_children: children,
    }
}

fn folder(id: String, f: Folder) -> ObjectNode {
    node(id, f.label(), NodeKind::Folder(f), true)
}

fn gone() -> MetadataError {
    MetadataError(tr("The object no longer exists. Refresh the Object Explorer.").to_string())
}

fn not_yet() -> MetadataError {
    MetadataError(tr("This script is not available for SQL Server yet.").to_string())
}

/// `[nome]` com `]` dobrado.
pub fn bracket(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// Nome do banco num id de nó.
fn enc(db: &str) -> String {
    db.replace('%', "%25").replace('/', "%2F")
}

fn dec(text: &str) -> String {
    text.replace("%2F", "/").replace("%25", "%")
}

/// Rótulo da coluna como no SSMS: `id (PK, int, not null)`.
fn column_label(r: &[Option<String>]) -> String {
    let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let number = |i: usize| text(i).parse::<i64>().unwrap_or(0);
    let ty = text(2);
    let length = number(3);
    let ty = match ty.as_str() {
        "varchar" | "char" | "varbinary" | "binary" if length == -1 => format!("{ty}(max)"),
        "varchar" | "char" | "varbinary" | "binary" => format!("{ty}({length})"),
        "nvarchar" | "nchar" if length == -1 => format!("{ty}(max)"),
        "nvarchar" | "nchar" => format!("{ty}({})", length / 2),
        "decimal" | "numeric" => format!("{ty}({},{})", number(4), number(5)),
        "datetime2" | "time" | "datetimeoffset" => format!("{ty}({})", number(5)),
        _ => ty,
    };
    let mut parts = Vec::new();
    if text(8) == "1" {
        parts.push("PK".to_string());
    }
    if text(9) == "1" {
        parts.push("FK".to_string());
    }
    if text(7) == "1" {
        parts.push(tr("Computed").to_string());
    }
    parts.push(ty);
    parts.push(if text(6) == "1" { "null" } else { "not null" }.to_string());
    format!("{} ({})", text(1), parts.join(", "))
}

impl MetadataProvider for MssqlExplorer {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![node(
            SERVER,
            self.label.clone(),
            NodeKind::Server,
            true,
        )])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        let id = parent.0.as_str();
        match id {
            SERVER => return Ok(vec![folder(DATABASES.into(), Folder::Databases)]),
            DATABASES | SYSTEM => {
                let system = id == SYSTEM;
                let mut nodes: Vec<ObjectNode> = self
                    .rows(DATABASES_SQL)?
                    .into_iter()
                    .filter_map(|r| r.into_iter().next().flatten())
                    .filter(|name| SYSTEM_DATABASES.contains(&name.as_str()) == system)
                    .map(|name| node(format!("db/{}", enc(&name)), name, NodeKind::Database, true))
                    .collect();
                if !system {
                    nodes.insert(0, folder(SYSTEM.into(), Folder::SystemDatabases));
                }
                return Ok(nodes);
            }
            _ => {}
        }
        let parts: Vec<&str> = id.split('/').collect();
        match parts.as_slice() {
            ["db", db] => self.database_children(id, &dec(db)),
            ["db", db, "f", tag] => self.database_folder(id, &dec(db), tag),
            ["db", _, "t", _] => Ok(self.relation_children(id, false)),
            ["db", _, "v", _] => Ok(self.relation_children(id, true)),
            ["db", db, "t" | "v", obj, "f", tag] => {
                let obj = obj.parse().map_err(|_| gone())?;
                self.relation_folder(&dec(db), obj, tag)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Só a tabela (para o Edit Top 200 Rows); o Table Designer do SQL
    /// Server ainda não existe.
    fn design_target(&self, node: &NodeId) -> Option<DesignTarget> {
        match node.0.split('/').collect::<Vec<_>>().as_slice() {
            ["db", db, "t", obj] => Some(DesignTarget {
                database: dec(db),
                schema: String::new(),
                table: Some(obj.parse().ok()?),
            }),
            _ => None,
        }
    }

    fn database_of(&self, node: &NodeId) -> Option<String> {
        match node.0.split('/').collect::<Vec<_>>().as_slice() {
            ["db", db, ..] => Some(dec(db)),
            _ => None,
        }
    }

    fn script(&self, node: &NodeId, action: ScriptAction) -> Result<String, MetadataError> {
        let parts: Vec<&str> = node.0.split('/').collect();
        let number = |s: &str| s.parse::<i64>().map_err(|_| gone());
        match (parts.as_slice(), action) {
            (["db", db, "t" | "v", obj], ScriptAction::SelectTop(n)) => {
                self.select_top(&dec(db), number(obj)?, n)
            }
            (_, ScriptAction::SelectTop(_)) => Err(MetadataError(
                tr("Only tables and views have rows to select.").to_string(),
            )),
            (["db", db], ScriptAction::Create) => {
                Ok(format!("CREATE DATABASE {}\nGO\n", bracket(&dec(db))))
            }
            (["db", db], ScriptAction::Drop) => {
                Ok(format!("DROP DATABASE {}\nGO\n", bracket(&dec(db))))
            }
            (["db", db, "t" | "v" | "o", obj], action) => {
                self.script_object(&dec(db), number(obj)?, action)
            }
            (["db", db, "ix", obj, index], ScriptAction::Drop) => {
                let db = dec(db);
                let (table, _, _) = self.object_name(&db, number(obj)?)?;
                let name = self
                    .rows(&format!(
                        "SELECT QUOTENAME(name) FROM {}.sys.indexes \
                         WHERE object_id = {} AND index_id = {}",
                        bracket(&db),
                        number(obj)?,
                        number(index)?
                    ))?
                    .into_iter()
                    .next()
                    .and_then(|r| r.into_iter().next().flatten())
                    .ok_or_else(gone)?;
                Ok(format!("DROP INDEX {name} ON {table}\nGO\n"))
            }
            _ => Err(not_yet()),
        }
    }

    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        let parts: Vec<&str> = node.0.split('/').collect();
        let number = |s: &str| s.parse::<i64>().map_err(|_| gone());
        match parts.as_slice() {
            ["db", db] => self.properties_of(&format!(
                "SELECT SUSER_SNAME(d.owner_sid) AS [Owner], d.state_desc AS [State], \
                        d.recovery_model_desc AS [Recovery model], \
                        CAST(d.compatibility_level AS varchar(10)) AS [Compatibility level], \
                        d.collation_name AS [Collation], \
                        CONVERT(varchar(19), d.create_date, 120) AS [Created] \
                   FROM sys.databases d WHERE d.name = N'{}'",
                dec(db).replace('\'', "''")
            )),
            ["db", db, "t" | "v" | "o", obj] => {
                let d = bracket(&dec(db));
                let obj = number(obj)?;
                self.properties_of(&format!(
                    "SELECT s.name AS [Schema], \
                            CAST((SELECT SUM(p.rows) FROM {d}.sys.partitions p \
                                   WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) \
                                 AS varchar(20)) AS [Rows (estimate)], \
                            CONVERT(varchar(19), o.create_date, 120) AS [Created], \
                            CONVERT(varchar(19), o.modify_date, 120) AS [Last modified] \
                       FROM {d}.sys.objects o \
                       JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
                      WHERE o.object_id = {obj}"
                ))
            }
            ["db", db, "col", obj, col] => {
                let d = bracket(&dec(db));
                self.properties_of(&format!(
                    "SELECT c.name AS [Name], t.name AS [Data type], \
                            CASE c.is_nullable WHEN 1 THEN 'yes' ELSE 'no' END AS [Nullable], \
                            CASE c.is_identity WHEN 1 THEN 'yes' ELSE 'no' END AS [Identity], \
                            CASE c.is_computed WHEN 1 THEN 'yes' ELSE 'no' END AS [Computed], \
                            OBJECT_DEFINITION(c.default_object_id) AS [Default], \
                            c.collation_name AS [Collation] \
                       FROM {d}.sys.columns c \
                       JOIN {d}.sys.types t ON t.user_type_id = c.user_type_id \
                      WHERE c.object_id = {} AND c.column_id = {}",
                    number(obj)?,
                    number(col)?
                ))
            }
            _ => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(values: &[&str]) -> Vec<Option<String>> {
        values.iter().map(|v| Some(v.to_string())).collect()
    }

    #[test]
    fn columns_are_labelled_like_ssms() {
        // column_id, name, type, max_length, precision, scale, nullable,
        // computed, pk, fk
        let c = |v: &[&str]| column_label(&row(v));
        assert_eq!(
            c(&["1", "id", "int", "4", "10", "0", "0", "0", "1", "0"]),
            "id (PK, int, not null)"
        );
        assert_eq!(
            c(&["2", "nome", "nvarchar", "120", "0", "0", "1", "0", "0", "0"]),
            "nome (nvarchar(60), null)"
        );
        assert_eq!(
            c(&["3", "obs", "varchar", "-1", "0", "0", "1", "0", "0", "0"]),
            "obs (varchar(max), null)"
        );
        assert_eq!(
            c(&["4", "valor", "decimal", "9", "10", "2", "0", "0", "0", "1"]),
            "valor (FK, decimal(10,2), not null)"
        );
    }

    #[test]
    fn database_names_survive_node_ids() {
        let name = "a/b%c";
        assert_eq!(dec(&enc(name)), name);
        assert!(!enc(name).contains('/'));
        assert_eq!(bracket("x]y"), "[x]]y]");
    }
}
