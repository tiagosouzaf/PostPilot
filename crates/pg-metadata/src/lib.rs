//! Introspecção do catálogo do PostgreSQL para o Object Explorer.
//!
//! [`CatalogExplorer`] implementa o [`MetadataProvider`] lendo o
//! `pg_catalog` com consultas em tempo de execução (nada de SQLx em tempo
//! de compilação), compatíveis com o PostgreSQL 13 em diante. A árvore segue
//! o SSMS:
//!
//! ```text
//! servidor
//! ├── Databases
//! │   └── banco
//! │       └── Schemas
//! │           └── schema
//! │               ├── Tables > tabela > Columns, Constraints, Indexes
//! │               ├── Views > view > Columns
//! │               ├── Functions
//! │               └── Sequences
//! └── Security
//!     └── Roles
//! ```
//!
//! O PostgreSQL só mostra o catálogo do banco da conexão, então cada banco
//! expandido ganha uma conexão própria, aberta na primeira vez e reusada.
//! Schemas do sistema (`pg_*`, `information_schema`) ficam de fora.

mod model;
mod path;
mod script;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pgstudio_core::catalog::{self, Catalog, Column, RelationKind, Schema};
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, ScriptAction,
};
use pgstudio_pg_client::{DbRuntime, PgQueryService, Session, describe};
use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

pub use path::{Path, Relation};
pub use pgstudio_pg_client::DbRuntime as Runtime;

/// Consultas ao catálogo. `$1` é o nome do schema ou o OID da tabela.
mod sql {
    pub const DATABASES: &str = "SELECT datname FROM pg_catalog.pg_database \
        WHERE NOT datistemplate AND datallowconn ORDER BY datname";

    pub const ROLES: &str = "SELECT oid, rolname::text, rolcanlogin FROM pg_catalog.pg_roles \
        WHERE rolname !~ '^pg_' ORDER BY rolname";

    pub const SCHEMAS: &str = "SELECT nspname::text FROM pg_catalog.pg_namespace \
        WHERE nspname !~ '^pg_' AND nspname <> 'information_schema' ORDER BY nspname";

    /// Tabelas comuns, particionadas e estrangeiras; as partições ficam
    /// embaixo da tabela-mãe no SSMS, aqui ficam de fora.
    pub const TABLES: &str = "SELECT c.oid, c.relname::text, c.relkind::text \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'f') AND NOT c.relispartition \
        ORDER BY c.relname";

    pub const VIEWS: &str = "SELECT c.oid, c.relname::text, c.relkind::text \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE n.nspname = $1 AND c.relkind IN ('v', 'm') ORDER BY c.relname";

    pub const SEQUENCES: &str = "SELECT c.oid, c.relname::text \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE n.nspname = $1 AND c.relkind = 'S' ORDER BY c.relname";

    /// Funções e procedures, com os argumentos como o `\df` mostra. As que
    /// vêm de extensões ficam de fora.
    pub const FUNCTIONS: &str = "SELECT p.oid, p.proname::text, \
        pg_catalog.pg_get_function_identity_arguments(p.oid), p.prokind::text \
        FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
        WHERE n.nspname = $1 AND p.prokind IN ('f', 'p') \
        AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_depend d \
            WHERE d.classid = 'pg_catalog.pg_proc'::regclass AND d.objid = p.oid \
            AND d.deptype = 'e') \
        ORDER BY p.proname, 3";

    pub const COLUMNS: &str = "SELECT a.attnum, a.attname::text, \
        pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull, \
        EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k WHERE k.conrelid = a.attrelid \
            AND k.contype = 'p' AND a.attnum = ANY (k.conkey)), \
        EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k WHERE k.conrelid = a.attrelid \
            AND k.contype = 'f' AND a.attnum = ANY (k.conkey)) \
        FROM pg_catalog.pg_attribute a \
        WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum";

    pub const CONSTRAINTS: &str = "SELECT oid, conname::text, contype::text \
        FROM pg_catalog.pg_constraint WHERE conrelid = $1 ORDER BY contype, conname";

    /// Schemas onde nomes sem schema são procurados, com o `pg_catalog`
    /// (funções como `now()` e `count()`).
    pub const SEARCH_PATH: &str = "SELECT unnest(pg_catalog.current_schemas(true))::text";

    /// Tabelas e views de todos os schemas do usuário com as colunas, para o
    /// IntelliSense: uma consulta só, em ordem de schema, nome e coluna.
    pub const CATALOG_RELATIONS: &str = "SELECT n.nspname::text, c.relname::text, \
        c.relkind::text, a.attname::text, pg_catalog.format_type(a.atttypid, a.atttypmod) \
        FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 \
            AND NOT a.attisdropped \
        WHERE c.relkind IN ('r', 'p', 'f', 'v', 'm') AND NOT c.relispartition \
        AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' \
        ORDER BY 1, 2, a.attnum";

    /// Nomes de funções e procedures, incluindo as do `pg_catalog`.
    pub const CATALOG_FUNCTIONS: &str = "SELECT DISTINCT n.nspname::text, p.proname::text \
        FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
        WHERE p.prokind IN ('f', 'p', 'a', 'w') \
        AND (n.nspname = 'pg_catalog' OR (n.nspname !~ '^pg_' \
            AND n.nspname <> 'information_schema')) \
        ORDER BY 1, 2";

    pub const INDEXES: &str = "SELECT i.indexrelid, c.relname::text, i.indisunique, i.indisprimary \
        FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
        WHERE i.indrelid = $1 ORDER BY c.relname";
}

/// [`MetadataProvider`] de um servidor conectado.
pub struct CatalogExplorer {
    runtime: DbRuntime,
    settings: ConnectionSettings,
    password: String,
    server_version: String,
    /// Uma sessão por banco; a primeira é a do diálogo de conexão.
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl CatalogExplorer {
    /// Usa `session` (já conectada) para o banco dela e abre as dos outros
    /// bancos com as mesmas credenciais, quando forem expandidos.
    pub fn new(session: Session, password: String, runtime: DbRuntime) -> Self {
        let settings = session.settings().clone();
        let server_version = session.server_version().to_string();
        let mut sessions = HashMap::new();
        sessions.insert(settings.database.clone(), Arc::new(session));
        CatalogExplorer {
            runtime,
            settings,
            password,
            server_version,
            sessions: Mutex::new(sessions),
        }
    }

    /// Nome do servidor como o SSMS mostra: `host (PostgreSQL 16.4 - user)`.
    pub fn server_label(&self) -> String {
        format!(
            "{} (PostgreSQL {} - {})",
            self.settings.server_name(),
            self.server_version,
            self.settings.user
        )
    }

    /// Bancos com sessão aberta.
    pub fn open_databases(&self) -> Vec<String> {
        let mut dbs: Vec<_> = self.lock().keys().cloned().collect();
        dbs.sort();
        dbs
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sessão do banco, abrindo se preciso. Uma sessão que caiu é reaberta.
    fn session(&self, db: &str) -> Result<Arc<Session>, MetadataError> {
        if let Some(s) = self.lock().get(db).filter(|s| !s.is_closed()) {
            return Ok(s.clone());
        }
        let settings = ConnectionSettings {
            database: db.to_string(),
            ..self.settings.clone()
        };
        let session = self
            .runtime
            .block_on(Session::connect(&settings, &self.password))
            .map_err(|e| MetadataError(e.message().to_string()))?;
        let session = Arc::new(session);
        self.lock().insert(db.to_string(), session.clone());
        Ok(session)
    }

    fn query(
        &self,
        db: &str,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, MetadataError> {
        let session = self.session(db)?;
        self.runtime
            .block_on(session.client().query(sql, params))
            .map_err(|e| MetadataError(describe(&e)))
    }

    fn read_catalog(&self, db: &str) -> Result<Catalog, MetadataError> {
        let search_path = self
            .query(db, sql::SEARCH_PATH, &[])?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let mut schemas: Vec<Schema> = self
            .query(db, sql::SCHEMAS, &[])?
            .iter()
            .map(|r| Schema {
                name: r.get(0),
                ..Schema::default()
            })
            .collect();
        schemas.push(Schema {
            name: "pg_catalog".into(),
            ..Schema::default()
        });
        let index = |schemas: &[Schema], name: &str| schemas.iter().position(|s| s.name == name);
        for r in self.query(db, sql::CATALOG_RELATIONS, &[])? {
            let (schema, name, kind): (String, String, String) = (r.get(0), r.get(1), r.get(2));
            let Some(i) = index(&schemas, &schema) else {
                continue;
            };
            let rels = &mut schemas[i].relations;
            if rels.last().is_none_or(|l| l.name != name) {
                rels.push(catalog::Relation {
                    name,
                    kind: if matches!(kind.as_str(), "v" | "m") {
                        RelationKind::View
                    } else {
                        RelationKind::Table
                    },
                    columns: Vec::new(),
                });
            }
            let column: Option<String> = r.get(3);
            if let (Some(name), Some(rel)) = (column, rels.last_mut()) {
                rel.columns.push(Column { name, ty: r.get(4) });
            }
        }
        for r in self.query(db, sql::CATALOG_FUNCTIONS, &[])? {
            let schema: String = r.get(0);
            if let Some(i) = index(&schemas, &schema) {
                schemas[i].functions.push(r.get(1));
            }
        }
        Ok(Catalog {
            database: db.to_string(),
            search_path,
            schemas,
        })
    }

    fn databases(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        let rows = self.query(&self.settings.database, sql::DATABASES, &[])?;
        Ok(rows
            .iter()
            .map(|r| {
                let name: String = r.get(0);
                node(
                    Path::Database(name.clone()).id(),
                    name,
                    NodeKind::Database,
                    true,
                )
            })
            .collect())
    }

    fn roles(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        let rows = self.query(&self.settings.database, sql::ROLES, &[])?;
        Ok(rows
            .iter()
            .map(|r| {
                let oid: u32 = r.get(0);
                let name: String = r.get(1);
                let login: bool = r.get(2);
                let name = if login {
                    name
                } else {
                    format!("{name} (group)")
                };
                node(NodeId(format!("role/{oid}")), name, NodeKind::Role, false)
            })
            .collect())
    }

    fn schemas(&self, db: &str) -> Result<Vec<ObjectNode>, MetadataError> {
        let rows = self.query(db, sql::SCHEMAS, &[])?;
        Ok(rows
            .iter()
            .map(|r| {
                let name: String = r.get(0);
                let id = Path::Schema(db.into(), name.clone()).id();
                node(id, name, NodeKind::Schema, true)
            })
            .collect())
    }

    fn schema_folder(
        &self,
        db: &str,
        schema: &str,
        folder: Folder,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let params: [&(dyn ToSql + Sync); 1] = [&schema];
        match folder {
            Folder::Tables | Folder::Views => {
                let sql = if folder == Folder::Tables {
                    sql::TABLES
                } else {
                    sql::VIEWS
                };
                let rows = self.query(db, sql, &params)?;
                Ok(rows
                    .iter()
                    .map(|r| {
                        let oid: u32 = r.get(0);
                        let name: String = r.get(1);
                        let kind: String = r.get(2);
                        let label = match kind.as_str() {
                            "p" => format!("{name} (partitioned)"),
                            "f" => format!("{name} (foreign)"),
                            "m" => format!("{name} (materialized)"),
                            _ => name,
                        };
                        if folder == Folder::Tables {
                            node(
                                Path::Table(db.into(), oid).id(),
                                label,
                                NodeKind::Table,
                                true,
                            )
                        } else {
                            node(Path::View(db.into(), oid).id(), label, NodeKind::View, true)
                        }
                    })
                    .collect())
            }
            Folder::Functions => {
                let rows = self.query(db, sql::FUNCTIONS, &params)?;
                Ok(rows
                    .iter()
                    .map(|r| {
                        let oid: u32 = r.get(0);
                        let name: String = r.get(1);
                        let args: String = r.get(2);
                        let kind: String = r.get(3);
                        let label = if kind == "p" {
                            format!("{name}({args}) (procedure)")
                        } else {
                            format!("{name}({args})")
                        };
                        node(
                            path::leaf(db, "fn", &[&oid]),
                            label,
                            NodeKind::Function,
                            false,
                        )
                    })
                    .collect())
            }
            Folder::Sequences => {
                let rows = self.query(db, sql::SEQUENCES, &params)?;
                Ok(rows
                    .iter()
                    .map(|r| {
                        let oid: u32 = r.get(0);
                        let name: String = r.get(1);
                        node(
                            path::leaf(db, "seq", &[&oid]),
                            name,
                            NodeKind::Sequence,
                            false,
                        )
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn relation_folder(
        &self,
        db: &str,
        oid: u32,
        folder: Folder,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let params: [&(dyn ToSql + Sync); 1] = [&oid];
        match folder {
            Folder::Columns => {
                let rows = self.query(db, sql::COLUMNS, &params)?;
                Ok(rows
                    .iter()
                    .map(|r| {
                        let attnum: i16 = r.get(0);
                        let name: String = r.get(1);
                        let ty: String = r.get(2);
                        let not_null: bool = r.get(3);
                        let pk: bool = r.get(4);
                        let fk: bool = r.get(5);
                        let label = column_label(&name, &ty, not_null, pk, fk);
                        node(
                            path::leaf(db, "col", &[&oid, &attnum]),
                            label,
                            NodeKind::Column,
                            false,
                        )
                    })
                    .collect())
            }
            Folder::Constraints => {
                let rows = self.query(db, sql::CONSTRAINTS, &params)?;
                Ok(rows
                    .iter()
                    .map(|r| {
                        let con: u32 = r.get(0);
                        let name: String = r.get(1);
                        let kind: String = r.get(2);
                        let label = match kind.as_str() {
                            "p" => format!("{name} (primary key)"),
                            "f" => format!("{name} (foreign key)"),
                            "u" => format!("{name} (unique)"),
                            "c" => format!("{name} (check)"),
                            "x" => format!("{name} (exclusion)"),
                            _ => name,
                        };
                        node(
                            path::leaf(db, "con", &[&con]),
                            label,
                            NodeKind::Constraint,
                            false,
                        )
                    })
                    .collect())
            }
            Folder::Indexes => {
                let rows = self.query(db, sql::INDEXES, &params)?;
                Ok(rows
                    .iter()
                    .map(|r| {
                        let idx: u32 = r.get(0);
                        let name: String = r.get(1);
                        let unique: bool = r.get(2);
                        let primary: bool = r.get(3);
                        let label = if primary {
                            format!("{name} (primary key)")
                        } else if unique {
                            format!("{name} (unique)")
                        } else {
                            name
                        };
                        node(
                            path::leaf(db, "idx", &[&idx]),
                            label,
                            NodeKind::Index,
                            false,
                        )
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }
}

/// Coluna como o SSMS mostra: `id (PK, integer, not null)`.
pub fn column_label(name: &str, ty: &str, not_null: bool, pk: bool, fk: bool) -> String {
    let mut parts = Vec::new();
    if pk {
        parts.push("PK".to_string());
    }
    if fk {
        parts.push("FK".to_string());
    }
    parts.push(ty.to_string());
    parts.push(if not_null { "not null" } else { "null" }.to_string());
    format!("{name} ({})", parts.join(", "))
}

fn node(id: NodeId, name: impl Into<String>, kind: NodeKind, has_children: bool) -> ObjectNode {
    ObjectNode {
        id,
        name: name.into(),
        kind,
        has_children,
    }
}

fn folder_node(path: Path, folder: Folder) -> ObjectNode {
    node(path.id(), folder.label(), NodeKind::Folder(folder), true)
}

impl MetadataProvider for CatalogExplorer {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![node(
            Path::Server.id(),
            self.server_label(),
            NodeKind::Server,
            true,
        )])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        match Path::parse(parent) {
            Path::Server => Ok(vec![
                folder_node(Path::ServerFolder(Folder::Databases), Folder::Databases),
                folder_node(Path::ServerFolder(Folder::Security), Folder::Security),
            ]),
            Path::ServerFolder(Folder::Databases) => self.databases(),
            Path::ServerFolder(Folder::Security) => {
                Ok(vec![folder_node(Path::Roles, Folder::Roles)])
            }
            Path::Roles => self.roles(),
            Path::Database(db) => Ok(vec![folder_node(Path::Schemas(db), Folder::Schemas)]),
            Path::Schemas(db) => self.schemas(&db),
            Path::Schema(db, s) => Ok([
                Folder::Tables,
                Folder::Views,
                Folder::Functions,
                Folder::Sequences,
            ]
            .into_iter()
            .map(|f| folder_node(Path::SchemaFolder(db.clone(), s.clone(), f), f))
            .collect()),
            Path::SchemaFolder(db, s, f) => self.schema_folder(&db, &s, f),
            Path::Table(db, oid) => Ok([Folder::Columns, Folder::Constraints, Folder::Indexes]
                .into_iter()
                .map(|f| folder_node(Path::RelationFolder(db.clone(), Relation::Table, oid, f), f))
                .collect()),
            Path::View(db, oid) => Ok(vec![folder_node(
                Path::RelationFolder(db, Relation::View, oid, Folder::Columns),
                Folder::Columns,
            )]),
            Path::RelationFolder(db, _, oid, f) => self.relation_folder(&db, oid, f),
            Path::ServerFolder(_) | Path::Leaf => Ok(Vec::new()),
        }
    }

    fn catalog(&self, database: &str) -> Result<Catalog, MetadataError> {
        self.read_catalog(database)
    }

    fn database_of(&self, node: &NodeId) -> Option<String> {
        script::database_of(node)
    }

    fn script(&self, node: &NodeId, action: ScriptAction) -> Result<String, MetadataError> {
        self.script_for(node, action)
    }
}

/// O [`Connector`] de verdade: conecta ao PostgreSQL e devolve o Object
/// Explorer do catálogo.
#[derive(Clone)]
pub struct PgConnector {
    runtime: DbRuntime,
}

impl PgConnector {
    pub fn new(runtime: DbRuntime) -> Self {
        PgConnector { runtime }
    }
}

impl Connector for PgConnector {
    fn connect(
        &self,
        settings: &ConnectionSettings,
        password: &str,
    ) -> Result<Connected, ConnectError> {
        let session = self
            .runtime
            .block_on(Session::connect(settings, password))?;
        let server_version = session.server_version().to_string();
        let explorer = CatalogExplorer::new(session, password.to_string(), self.runtime.clone());
        Ok(Connected {
            settings: settings.clone(),
            server_version,
            metadata: Arc::new(explorer),
            queries: Arc::new(PgQueryService::new(
                settings.clone(),
                password.to_string(),
                self.runtime.clone(),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_labels_follow_ssms() {
        assert_eq!(
            column_label("id", "integer", true, true, false),
            "id (PK, integer, not null)"
        );
        assert_eq!(
            column_label("customer_id", "bigint", false, false, true),
            "customer_id (FK, bigint, null)"
        );
    }
}
