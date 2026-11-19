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
//! | login, server role | `login/<nome>`, `srvrole/<nome>` |
//! | usuário, database role | `db/<banco>/user/<nome>`, `db/<banco>/dbrole/<nome>` |

use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_core::connection::Engine;
use pgstudio_core::metadata::{
    DesignTarget, Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode,
    PrincipalKind, Property, ScriptAction, Securable,
};
use pgstudio_i18n::tr;
use pgstudio_pg_client::DbRuntime;

use crate::{Broken, Session};

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
const SECURITY: &str = "server/Security";
const LOGINS: &str = "server/Security/Logins";
const SERVER_ROLES: &str = "server/Security/ServerRoles";

/// `sys.server_principals.type` e `sys.database_principals.type` de quem
/// entra (SQL, Windows, grupo, Entra), sem roles nem certificados.
const LOGIN_TYPES: &str = "'S', 'U', 'G', 'E', 'X'";

/// Lê os metadados com a sessão aberta pelo Connect to Server.
pub struct MssqlExplorer {
    pub(crate) runtime: DbRuntime,
    /// Só por [`MssqlExplorer::healthy`], que reabre a conexão que caiu.
    session: tokio::sync::Mutex<Session>,
    password: String,
    label: String,
}

impl MssqlExplorer {
    pub fn new(session: Session, password: String, runtime: DbRuntime) -> Self {
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
            password,
            label,
        }
    }

    /// Nome do servidor como o SSMS mostra: `host (SQL Server 16.0.4135.4 - sa)`.
    pub fn server_label(&self) -> &str {
        &self.label
    }

    /// A sessão, trocada por uma nova se a anterior não serve mais (um
    /// tempo esgotado deixa a conexão fora de sincronia; o servidor pode
    /// ter derrubado uma conexão parada).
    pub(crate) async fn healthy(&self) -> Result<tokio::sync::MutexGuard<'_, Session>, String> {
        let mut session = self.session.lock().await;
        if session.is_broken() {
            session.reopen(&self.password).await?;
        }
        Ok(session)
    }

    /// Uma leitura. Se a conexão caiu, abre outra e lê de novo, uma vez
    /// (só leituras passam por aqui: repetir não altera nada). Depois de
    /// um tempo esgotado não repete na hora, para não esperar em dobro: a
    /// próxima leitura já vai numa conexão nova.
    async fn read(&self, sql: &str) -> Result<Named, String> {
        let mut session = self.healthy().await?;
        match session.table(sql).await {
            Err(_) if session.broken() == Some(Broken::Lost) => {
                session.reopen(&self.password).await?;
                session.table(sql).await
            }
            result => result,
        }
    }

    pub(crate) fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, MetadataError> {
        self.table(sql).map(|(_, rows)| rows)
    }

    fn table(&self, sql: &str) -> Result<Named, MetadataError> {
        self.runtime.block_on(self.read(sql)).map_err(MetadataError)
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

    /// Logins, usuários e roles de uma consulta de uma coluna (o nome, que
    /// também é o id).
    fn principals(&self, sql: &str, prefix: &str) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(self
            .rows(sql)?
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .map(|name| {
                node(
                    format!("{prefix}{}", enc(&name)),
                    name,
                    NodeKind::Role,
                    false,
                )
            })
            .collect())
    }

    fn database_children(&self, id: &str, db: &str) -> Result<Vec<ObjectNode>, MetadataError> {
        let _ = db;
        Ok([
            ("diagrams", Folder::Diagrams),
            ("tables", Folder::Tables),
            ("views", Folder::Views),
            ("prog", Folder::Programmability),
            ("security", Folder::Security),
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
            // Os diagramas ficam em arquivos `.pgsdiagram`, não no banco.
            "diagrams" => Ok(Vec::new()),
            "security" => Ok(vec![
                folder(format!("{base}f/users"), Folder::Users),
                folder(format!("{base}f/dbroles"), Folder::DatabaseRoles),
            ]),
            "users" => self.principals(
                &format!(
                    "SELECT name FROM {d}.sys.database_principals \
                     WHERE type IN ({LOGIN_TYPES}) ORDER BY name"
                ),
                &format!("{base}user/"),
            ),
            "dbroles" => self.principals(
                &format!(
                    "SELECT name FROM {d}.sys.database_principals \
                     WHERE type = 'R' ORDER BY name"
                ),
                &format!("{base}dbrole/"),
            ),
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
            "U" => self.create_table(db, obj, &name),
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

    /// `CREATE TABLE` como o SSMS gera: colunas, PRIMARY KEY e UNIQUE no
    /// corpo; DEFAULT, FOREIGN KEY e CHECK em `ALTER TABLE`; depois os
    /// índices.
    fn create_table(&self, db: &str, obj: i64, name: &str) -> Result<String, MetadataError> {
        let d = bracket(db);
        let text = |r: &[Option<String>], i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let columns = self.rows(&format!(
            "SELECT c.name, t.name, t.is_user_defined, SCHEMA_NAME(t.schema_id), \
                    c.max_length, c.precision, c.scale, c.is_nullable, \
                    CAST(ic.seed_value AS varchar(40)), CAST(ic.increment_value AS varchar(40)), \
                    cc.definition, cc.is_persisted, \
                    dc.name, dc.definition, dc.is_system_named \
               FROM {d}.sys.columns c \
               JOIN {d}.sys.types t ON t.user_type_id = c.user_type_id \
               LEFT JOIN {d}.sys.identity_columns ic ON ic.object_id = c.object_id \
                                                   AND ic.column_id = c.column_id \
               LEFT JOIN {d}.sys.computed_columns cc ON cc.object_id = c.object_id \
                                                    AND cc.column_id = c.column_id \
               LEFT JOIN {d}.sys.default_constraints dc ON dc.object_id = c.default_object_id \
              WHERE c.object_id = {obj} ORDER BY c.column_id"
        ))?;
        let mut body: Vec<String> = columns
            .iter()
            .map(|r| {
                let column = bracket(&text(r, 0));
                if let Some(def) = r.get(10).cloned().flatten() {
                    let persisted = if text(r, 11) == "1" { " PERSISTED" } else { "" };
                    return format!("\t{column}  AS {def}{persisted}");
                }
                let ty = sql_type(
                    &text(r, 1),
                    text(r, 2) == "1",
                    &text(r, 3),
                    text(r, 4).parse().unwrap_or(0),
                    text(r, 5).parse().unwrap_or(0),
                    text(r, 6).parse().unwrap_or(0),
                );
                let identity = match (r.get(8).cloned().flatten(), r.get(9).cloned().flatten()) {
                    (Some(seed), Some(step)) => format!(" IDENTITY({seed},{step})"),
                    _ => String::new(),
                };
                let null = if text(r, 7) == "1" {
                    "NULL"
                } else {
                    "NOT NULL"
                };
                format!("\t{column} {ty}{identity} {null}")
            })
            .collect();
        // PRIMARY KEY e UNIQUE.
        let keys = self.rows(&format!(
            "SELECT k.name, k.type, i.type_desc, \
                    STRING_AGG(QUOTENAME(c.name) + CASE ic.is_descending_key \
                               WHEN 1 THEN ' DESC' ELSE ' ASC' END, ',') \
                      WITHIN GROUP (ORDER BY ic.key_ordinal) \
               FROM {d}.sys.key_constraints k \
               JOIN {d}.sys.indexes i ON i.object_id = k.parent_object_id \
                                     AND i.index_id = k.unique_index_id \
               JOIN {d}.sys.index_columns ic ON ic.object_id = i.object_id \
                                            AND ic.index_id = i.index_id AND ic.key_ordinal > 0 \
               JOIN {d}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
              WHERE k.parent_object_id = {obj} \
              GROUP BY k.name, k.type, i.type_desc \
              ORDER BY CASE k.type WHEN 'PK' THEN 0 ELSE 1 END, k.name"
        ))?;
        for k in &keys {
            let what = if text(k, 1) == "PK" {
                "PRIMARY KEY"
            } else {
                "UNIQUE"
            };
            let cols = text(k, 3).replace(',', ",\n\t");
            body.push(format!(
                " CONSTRAINT {} {what} {}\n(\n\t{cols}\n)",
                bracket(&text(k, 0)),
                text(k, 2)
            ));
        }
        let mut out = format!(
            "SET ANSI_NULLS ON\nGO\nSET QUOTED_IDENTIFIER ON\nGO\nCREATE TABLE {name}(\n{}\n)\nGO\n",
            body.join(",\n")
        );
        // Índices que não são de constraint.
        for r in self.rows(&format!(
            "SELECT i.name, i.is_unique, i.type_desc, i.filter_definition, \
                    STRING_AGG(CASE WHEN ic.is_included_column = 0 THEN QUOTENAME(c.name) \
                               + CASE ic.is_descending_key WHEN 1 THEN ' DESC' ELSE ' ASC' END \
                               END, ', ') WITHIN GROUP (ORDER BY ic.key_ordinal), \
                    STRING_AGG(CASE WHEN ic.is_included_column = 1 THEN QUOTENAME(c.name) END, ', ') \
               FROM {d}.sys.indexes i \
               JOIN {d}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
               JOIN {d}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
              WHERE i.object_id = {obj} AND i.type IN (1, 2) AND i.is_primary_key = 0 \
                AND i.is_unique_constraint = 0 AND i.is_hypothetical = 0 \
              GROUP BY i.name, i.is_unique, i.type_desc, i.filter_definition \
              ORDER BY i.name"
        ))? {
            let unique = if text(&r, 1) == "1" { "UNIQUE " } else { "" };
            let include = r
                .get(5)
                .cloned()
                .flatten()
                .map(|c| format!("\nINCLUDE({c})"))
                .unwrap_or_default();
            let filter = r
                .get(3)
                .cloned()
                .flatten()
                .map(|f| format!("\nWHERE {f}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "CREATE {unique}{} INDEX {} ON {name}\n(\n\t{}\n){include}{filter}\nGO\n",
                text(&r, 2),
                bracket(&text(&r, 0)),
                text(&r, 4).replace(", ", ",\n\t")
            ));
        }
        // DEFAULT.
        for r in &columns {
            if let Some(def) = r.get(13).cloned().flatten() {
                let constraint = if text(r, 14) == "1" {
                    String::new()
                } else {
                    format!(" CONSTRAINT {}", bracket(&text(r, 12)))
                };
                out.push_str(&format!(
                    "ALTER TABLE {name} ADD{constraint} DEFAULT {def} FOR {}\nGO\n",
                    bracket(&text(r, 0))
                ));
            }
        }
        // FOREIGN KEY.
        for r in self.rows(&format!(
            "SELECT f.name, f.is_system_named, \
                    STRING_AGG(QUOTENAME(pc.name), ', ') WITHIN GROUP (ORDER BY fc.constraint_column_id), \
                    QUOTENAME(SCHEMA_NAME(rt.schema_id)) + '.' + QUOTENAME(rt.name), \
                    STRING_AGG(QUOTENAME(rc.name), ', ') WITHIN GROUP (ORDER BY fc.constraint_column_id), \
                    f.delete_referential_action_desc, f.update_referential_action_desc, \
                    f.is_not_trusted, f.is_disabled \
               FROM {d}.sys.foreign_keys f \
               JOIN {d}.sys.foreign_key_columns fc ON fc.constraint_object_id = f.object_id \
               JOIN {d}.sys.columns pc ON pc.object_id = fc.parent_object_id \
                                      AND pc.column_id = fc.parent_column_id \
               JOIN {d}.sys.columns rc ON rc.object_id = fc.referenced_object_id \
                                      AND rc.column_id = fc.referenced_column_id \
               JOIN {d}.sys.tables rt ON rt.object_id = f.referenced_object_id \
              WHERE f.parent_object_id = {obj} \
              GROUP BY f.name, f.is_system_named, rt.schema_id, rt.name, \
                       f.delete_referential_action_desc, f.update_referential_action_desc, \
                       f.is_not_trusted, f.is_disabled \
              ORDER BY f.name"
        ))? {
            let constraint = if text(&r, 1) == "1" {
                String::new()
            } else {
                format!(" CONSTRAINT {}", bracket(&text(&r, 0)))
            };
            let check = if text(&r, 7) == "1" { "NOCHECK" } else { "CHECK" };
            let action = |i: usize, verb: &str| match text(&r, i).as_str() {
                "NO_ACTION" | "" => String::new(),
                a => format!("\nON {verb} {}", a.replace('_', " ")),
            };
            out.push_str(&format!(
                "ALTER TABLE {name} WITH {check} ADD{constraint} FOREIGN KEY({})\n\
                 REFERENCES {} ({}){}{}\nGO\n",
                text(&r, 2),
                text(&r, 3),
                text(&r, 4),
                action(5, "DELETE"),
                action(6, "UPDATE")
            ));
        }
        // CHECK.
        for r in self.rows(&format!(
            "SELECT name, is_system_named, definition, is_not_trusted FROM {d}.sys.check_constraints \
              WHERE parent_object_id = {obj} ORDER BY name"
        ))? {
            let constraint = if text(&r, 1) == "1" {
                String::new()
            } else {
                format!(" CONSTRAINT {}", bracket(&text(&r, 0)))
            };
            let check = if text(&r, 3) == "1" { "NOCHECK" } else { "CHECK" };
            out.push_str(&format!(
                "ALTER TABLE {name} WITH {check} ADD{constraint} CHECK ({})\nGO\n",
                text(&r, 2)
            ));
        }
        Ok(out)
    }

    /// Catálogo do banco para o IntelliSense: tabelas e views com as
    /// colunas, procedures e funções por schema, e as funções internas mais
    /// usadas em `sys`.
    fn read_catalog(&self, db: &str) -> Result<Catalog, MetadataError> {
        let d = bracket(db);
        let text = |r: &[Option<String>], i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let mut schemas: Vec<Schema> = self
            .rows(&format!(
                "SELECT s.name FROM {d}.sys.schemas s \
                  WHERE s.schema_id < 16384 AND s.name NOT IN ('guest', 'INFORMATION_SCHEMA', 'sys') \
                  ORDER BY s.name"
            ))?
            .iter()
            .map(|r| Schema {
                name: text(r, 0),
                ..Default::default()
            })
            .collect();
        let schema = |schemas: &mut Vec<Schema>, name: String| -> usize {
            match schemas.iter().position(|s| s.name == name) {
                Some(i) => i,
                None => {
                    schemas.push(Schema {
                        name,
                        ..Default::default()
                    });
                    schemas.len() - 1
                }
            }
        };
        for r in self.rows(&format!(
            "SELECT s.name, o.name, RTRIM(o.type), c.name, t.name, c.max_length, c.precision, c.scale \
               FROM {d}.sys.objects o \
               JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
               JOIN {d}.sys.columns c ON c.object_id = o.object_id \
               JOIN {d}.sys.types t ON t.user_type_id = c.user_type_id \
              WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 \
              ORDER BY s.name, o.name, c.column_id"
        ))? {
            let i = schema(&mut schemas, text(&r, 0));
            let name = text(&r, 1);
            if schemas[i].relations.last().is_none_or(|rel| rel.name != name) {
                schemas[i].relations.push(Relation {
                    name,
                    kind: if text(&r, 2) == "V" {
                        RelationKind::View
                    } else {
                        RelationKind::Table
                    },
                    columns: Vec::new(),
                });
            }
            let number = |n: usize| text(&r, n).parse::<i64>().unwrap_or(0);
            let ty = plain_type(&text(&r, 4), number(5), number(6), number(7));
            if let Some(rel) = schemas[i].relations.last_mut() {
                rel.columns.push(Column {
                    name: text(&r, 3),
                    ty,
                });
            }
        }
        for r in self.rows(&format!(
            "SELECT s.name, o.name FROM {d}.sys.objects o \
               JOIN {d}.sys.schemas s ON s.schema_id = o.schema_id \
              WHERE o.type IN ('P', 'FN', 'IF', 'TF') AND o.is_ms_shipped = 0 \
              ORDER BY s.name, o.name"
        ))? {
            let i = schema(&mut schemas, text(&r, 0));
            schemas[i].functions.push(text(&r, 1));
        }
        schemas.push(Schema {
            name: "sys".into(),
            relations: Vec::new(),
            functions: BUILTIN_FUNCTIONS.iter().map(|f| f.to_string()).collect(),
        });
        Ok(Catalog {
            engine: Engine::SqlServer,
            database: db.to_string(),
            search_path: vec!["dbo".into(), "sys".into()],
            schemas,
        })
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

/// Funções internas do T-SQL sugeridas pelo IntelliSense.
const BUILTIN_FUNCTIONS: &[&str] = &[
    "ABS",
    "AVG",
    "CAST",
    "CEILING",
    "CHARINDEX",
    "COALESCE",
    "CONCAT",
    "CONVERT",
    "COUNT",
    "DATEADD",
    "DATEDIFF",
    "DATENAME",
    "DATEPART",
    "DAY",
    "DB_NAME",
    "EOMONTH",
    "FLOOR",
    "FORMAT",
    "GETDATE",
    "GETUTCDATE",
    "IIF",
    "ISNULL",
    "ISNUMERIC",
    "LEFT",
    "LEN",
    "LOWER",
    "LTRIM",
    "MAX",
    "MIN",
    "MONTH",
    "NEWID",
    "NULLIF",
    "OBJECT_ID",
    "PATINDEX",
    "REPLACE",
    "RIGHT",
    "ROUND",
    "ROW_NUMBER",
    "RTRIM",
    "SCOPE_IDENTITY",
    "STRING_AGG",
    "STUFF",
    "SUBSTRING",
    "SUM",
    "SYSDATETIME",
    "TRIM",
    "TRY_CAST",
    "TRY_CONVERT",
    "UPPER",
    "YEAR",
];

/// Tipo de uma coluna como o IntelliSense mostra: `nvarchar(60)`.
pub(crate) fn plain_type(name: &str, length: i64, precision: i64, scale: i64) -> String {
    match name {
        "varchar" | "char" | "varbinary" | "binary" if length == -1 => format!("{name}(max)"),
        "varchar" | "char" | "varbinary" | "binary" => format!("{name}({length})"),
        "nvarchar" | "nchar" if length == -1 => format!("{name}(max)"),
        "nvarchar" | "nchar" => format!("{name}({})", length / 2),
        "decimal" | "numeric" => format!("{name}({precision},{scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{name}({scale})"),
        _ => name.to_string(),
    }
}

/// Tipo de uma coluna no `CREATE TABLE`, como o SSMS escreve:
/// `[nvarchar](60)`, `[decimal](10, 2)`, `[dbo].[Cpf]` (tipo do usuário).
fn sql_type(
    name: &str,
    user_defined: bool,
    schema: &str,
    length: i64,
    precision: i64,
    scale: i64,
) -> String {
    if user_defined {
        return format!("{}.{}", bracket(schema), bracket(name));
    }
    let base = bracket(name);
    match name {
        "varchar" | "char" | "varbinary" | "binary" if length == -1 => format!("{base}(max)"),
        "varchar" | "char" | "varbinary" | "binary" => format!("{base}({length})"),
        "nvarchar" | "nchar" if length == -1 => format!("{base}(max)"),
        "nvarchar" | "nchar" => format!("{base}({})", length / 2),
        "decimal" | "numeric" => format!("{base}({precision}, {scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{base}({scale})"),
        _ => base,
    }
}

/// Rótulo da coluna como no SSMS: `id (PK, int, not null)`.
fn column_label(r: &[Option<String>]) -> String {
    let text = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let number = |i: usize| text(i).parse::<i64>().unwrap_or(0);
    let ty = plain_type(&text(2), number(3), number(4), number(5));
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
            SERVER => {
                return Ok(vec![
                    folder(DATABASES.into(), Folder::Databases),
                    folder(SECURITY.into(), Folder::Security),
                ]);
            }
            SECURITY => {
                return Ok(vec![
                    folder(LOGINS.into(), Folder::Logins),
                    folder(SERVER_ROLES.into(), Folder::ServerRoles),
                ]);
            }
            LOGINS => {
                return self.principals(
                    &format!(
                        "SELECT name FROM sys.server_principals \
                         WHERE type IN ({LOGIN_TYPES}) AND name NOT LIKE '##%' ORDER BY name"
                    ),
                    "login/",
                );
            }
            SERVER_ROLES => {
                return self.principals(
                    "SELECT name FROM sys.server_principals WHERE type = 'R' ORDER BY name",
                    "srvrole/",
                );
            }
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

    /// A tabela (para o Table Designer e o Edit Top 200 Rows) ou a pasta
    /// Tables, onde uma tabela nova nasce no `dbo`.
    fn design_target(&self, node: &NodeId) -> Option<DesignTarget> {
        match node.0.split('/').collect::<Vec<_>>().as_slice() {
            ["db", db, "t", obj] => Some(DesignTarget {
                database: dec(db),
                schema: "dbo".into(),
                table: Some(obj.parse().ok()?),
            }),
            ["db", db, "f", "tables"] => Some(DesignTarget {
                database: dec(db),
                schema: "dbo".into(),
                table: None,
            }),
            _ => None,
        }
    }

    fn apply_ddl(&self, database: &str, statements: &[String]) -> Result<(), MetadataError> {
        self.run_ddl(database, statements)
    }

    fn type_names(&self, database: &str) -> Result<Vec<String>, MetadataError> {
        self.read_type_names(database)
    }

    fn catalog(&self, database: &str) -> Result<Catalog, MetadataError> {
        self.read_catalog(database)
    }

    fn load_model(&self, database: &str) -> Result<pgstudio_core::schema::Model, MetadataError> {
        self.read_model(database)
    }

    fn securable(&self, node: &NodeId) -> Option<Securable> {
        let principal = |kind, database: Option<&str>, name: Option<&str>| {
            Some(Securable::Principal {
                kind,
                database: database.map(dec),
                name: name.map(dec),
            })
        };
        match node.0.as_str() {
            LOGINS => return principal(PrincipalKind::Login, None, None),
            SERVER_ROLES => return principal(PrincipalKind::ServerRole, None, None),
            _ => {}
        }
        match node.0.split('/').collect::<Vec<_>>().as_slice() {
            ["login", name] => principal(PrincipalKind::Login, None, Some(name)),
            ["srvrole", name] => principal(PrincipalKind::ServerRole, None, Some(name)),
            ["db", db, "f", "users"] => principal(PrincipalKind::User, Some(db), None),
            ["db", db, "f", "dbroles"] => principal(PrincipalKind::DatabaseRole, Some(db), None),
            ["db", db] => Some(Securable::Database(dec(db))),
            ["db", db, "t" | "v", obj] => Some(Securable::Relation {
                database: dec(db),
                oid: obj.parse().ok()?,
            }),
            // Procedures e funções; os outros objetos deste id (constraints,
            // triggers, sequences) a janela recusa ao ler.
            ["db", db, "o", obj] => Some(Securable::Function {
                database: dec(db),
                oid: obj.parse().ok()?,
            }),
            ["db", db, "user", name] => principal(PrincipalKind::User, Some(db), Some(name)),
            ["db", db, "dbrole", name] => {
                principal(PrincipalKind::DatabaseRole, Some(db), Some(name))
            }
            _ => None,
        }
    }

    fn security_folder(&self, target: &Securable) -> Option<NodeId> {
        let Securable::Principal { kind, database, .. } = target else {
            return None;
        };
        let db = || database.as_deref().map(enc);
        Some(NodeId(match kind {
            PrincipalKind::Login => LOGINS.to_string(),
            PrincipalKind::ServerRole => SERVER_ROLES.to_string(),
            PrincipalKind::User => format!("db/{}/f/users", db()?),
            PrincipalKind::DatabaseRole => format!("db/{}/f/dbroles", db()?),
        }))
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
            ["db", db, "ix", obj, index] => {
                let d = bracket(&dec(db));
                // As colunas na ordem da chave (FOR XML: STRING_AGG só
                // existe do 2017 em diante).
                let columns = |included: u8, order: &str| {
                    format!(
                        "STUFF((SELECT ', ' + c.name + CASE WHEN ic.is_descending_key = 1 \
                                        THEN ' DESC' ELSE '' END \
                                  FROM {d}.sys.index_columns ic \
                                  JOIN {d}.sys.columns c ON c.object_id = ic.object_id \
                                   AND c.column_id = ic.column_id \
                                 WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id \
                                   AND ic.is_included_column = {included} \
                                 ORDER BY ic.{order} \
                                   FOR XML PATH(''), TYPE).value('.', 'nvarchar(max)'), 1, 2, '')"
                    )
                };
                self.properties_of(&format!(
                    "SELECT o.name AS [Table], {keys} AS [Columns], {included} AS [Include], \
                            i.filter_definition AS [Condition], \
                            LOWER(REPLACE(i.type_desc, '_', ' ')) AS [Kind], \
                            CASE i.is_primary_key WHEN 1 THEN 'yes' ELSE 'no' END AS [Primary key], \
                            CASE i.is_unique WHEN 1 THEN 'yes' ELSE 'no' END AS [Unique], \
                            (SELECT CASE WHEN SUM(a.used_pages) >= 1280 \
                                         THEN CAST(SUM(a.used_pages) / 128 AS varchar(20)) + ' MB' \
                                         ELSE CAST(SUM(a.used_pages) * 8 AS varchar(20)) + ' kB' END \
                               FROM {d}.sys.partitions p \
                               JOIN {d}.sys.allocation_units a ON a.container_id = p.partition_id \
                              WHERE p.object_id = i.object_id AND p.index_id = i.index_id) AS [Size] \
                       FROM {d}.sys.indexes i \
                       JOIN {d}.sys.objects o ON o.object_id = i.object_id \
                      WHERE i.object_id = {obj} AND i.index_id = {index}",
                    keys = columns(0, "key_ordinal"),
                    included = columns(1, "index_column_id"),
                    obj = number(obj)?,
                    index = number(index)?
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
            ["login" | "srvrole", name] => self.properties_of(&format!(
                "SELECT p.type_desc AS [Principal type], p.default_database_name AS [Default database], \
                        CASE p.is_disabled WHEN 1 THEN 'yes' ELSE 'no' END AS [Disabled], \
                        CONVERT(varchar(19), p.create_date, 120) AS [Created], \
                        CONVERT(varchar(19), p.modify_date, 120) AS [Last modified] \
                   FROM sys.server_principals p WHERE p.name = N'{}'",
                dec(name).replace('\'', "''")
            )),
            ["db", db, "user" | "dbrole", name] => {
                let d = bracket(&dec(db));
                self.properties_of(&format!(
                    "SELECT p.type_desc AS [Principal type], SUSER_SNAME(p.sid) AS [Login], \
                            p.default_schema_name AS [Default schema], \
                            CONVERT(varchar(19), p.create_date, 120) AS [Created], \
                            CONVERT(varchar(19), p.modify_date, 120) AS [Last modified] \
                       FROM {d}.sys.database_principals p WHERE p.name = N'{}'",
                    dec(name).replace('\'', "''")
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
    fn create_table_types_are_written_like_ssms() {
        assert_eq!(
            sql_type("nvarchar", false, "sys", 120, 0, 0),
            "[nvarchar](60)"
        );
        assert_eq!(
            sql_type("varbinary", false, "sys", -1, 0, 0),
            "[varbinary](max)"
        );
        assert_eq!(
            sql_type("decimal", false, "sys", 9, 10, 2),
            "[decimal](10, 2)"
        );
        assert_eq!(
            sql_type("datetime2", false, "sys", 8, 27, 0),
            "[datetime2](0)"
        );
        assert_eq!(sql_type("int", false, "sys", 4, 10, 0), "[int]");
        assert_eq!(sql_type("Cpf", true, "dbo", 11, 0, 0), "[dbo].[Cpf]");
    }

    #[test]
    fn database_names_survive_node_ids() {
        let name = "a/b%c";
        assert_eq!(dec(&enc(name)), name);
        assert!(!enc(name).contains('/'));
        assert_eq!(bracket("x]y"), "[x]]y]");
    }
}
