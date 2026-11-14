//! Provedor de metadados falso, usado até a Fase 2 trazer o PostgreSQL.
//!
//! Gera um servidor com bancos, schemas, tabelas, views, funções, colunas e
//! índices sintéticos, com um atraso por chamada para imitar a rede. O banco
//! `stress` tem um schema com 10 mil tabelas, para testar a árvore.

use std::thread;
use std::time::Duration;

use pgstudio_core::catalog::{Catalog, Column, Relation, RelationKind, Schema};
use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode,
};

/// Bancos comuns do servidor falso.
const DATABASES: usize = 6;
/// Schemas por banco comum.
const SCHEMAS: usize = 4;
/// Tabelas por schema comum.
const TABLES: usize = 50;
/// Tabelas do schema `stress.public`.
pub const STRESS_TABLES: usize = 10_000;

/// Provedor falso com atraso configurável.
#[derive(Clone, Debug)]
pub struct FakeProvider {
    delay: Duration,
}

impl FakeProvider {
    /// Cria o provedor; `delay` é o tempo de cada chamada.
    pub fn new(delay: Duration) -> Self {
        FakeProvider { delay }
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        FakeProvider::new(Duration::from_millis(200))
    }
}

/// Nome exibido do servidor falso.
pub const SERVER_NAME: &str = "localhost (PostgreSQL 17, demo)";

fn node(id: String, name: impl Into<String>, kind: NodeKind, has_children: bool) -> ObjectNode {
    ObjectNode {
        id: NodeId(id),
        name: name.into(),
        kind,
        has_children,
    }
}

fn folder(parent: &str, f: Folder) -> ObjectNode {
    node(
        format!("{parent}/{}", f.label()),
        f.label(),
        NodeKind::Folder(f),
        true,
    )
}

fn database_names() -> Vec<String> {
    let mut names: Vec<String> = ["postgres".to_string(), "stress".to_string()].into();
    names.extend((1..DATABASES - 1).map(|i| format!("app_{i:02}")));
    names
}

impl MetadataProvider for FakeProvider {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        thread::sleep(self.delay);
        Ok(vec![node(
            "srv".into(),
            SERVER_NAME,
            NodeKind::Server,
            true,
        )])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        thread::sleep(self.delay);
        let id = parent.0.as_str();
        let parts: Vec<&str> = id.split('/').collect();
        let out = match parts.as_slice() {
            ["srv"] => vec![folder(id, Folder::Databases)],
            ["srv", "Databases"] => database_names()
                .into_iter()
                .map(|db| node(format!("{id}/{db}"), db, NodeKind::Database, true))
                .collect(),
            ["srv", "Databases", _db] => vec![folder(id, Folder::Schemas)],
            ["srv", "Databases", db, "Schemas"] => {
                let names: Vec<String> = if *db == "stress" {
                    vec!["public".into()]
                } else {
                    let mut v = vec!["public".to_string()];
                    v.extend((1..SCHEMAS).map(|i| format!("schema_{i}")));
                    v
                };
                names
                    .into_iter()
                    .map(|s| node(format!("{id}/{s}"), s, NodeKind::Schema, true))
                    .collect()
            }
            ["srv", "Databases", _, "Schemas", _] => {
                [Folder::Tables, Folder::Views, Folder::Functions]
                    .into_iter()
                    .map(|f| folder(id, f))
                    .collect()
            }
            ["srv", "Databases", db, "Schemas", schema, "Tables"] => {
                let n = if *db == "stress" {
                    STRESS_TABLES
                } else {
                    TABLES
                };
                (0..n)
                    .map(|i| {
                        let name = format!("{schema}.table_{i:05}");
                        node(format!("{id}/{i}"), name, NodeKind::Table, true)
                    })
                    .collect()
            }
            ["srv", "Databases", _, "Schemas", schema, "Views"] => (0..5)
                .map(|i| {
                    node(
                        format!("{id}/{i}"),
                        format!("{schema}.view_{i}"),
                        NodeKind::View,
                        false,
                    )
                })
                .collect(),
            ["srv", "Databases", _, "Schemas", schema, "Functions"] => (0..5)
                .map(|i| {
                    let name = format!("{schema}.fn_{i}()");
                    node(format!("{id}/{i}"), name, NodeKind::Function, false)
                })
                .collect(),
            ["srv", "Databases", _, "Schemas", _, "Tables", _] => {
                vec![folder(id, Folder::Columns), folder(id, Folder::Indexes)]
            }
            ["srv", "Databases", _, "Schemas", _, "Tables", _, "Columns"] => {
                const COLUMNS: [&str; 6] = [
                    "id (bigint, not null)",
                    "name (text, null)",
                    "email (text, null)",
                    "created_at (timestamptz, not null)",
                    "balance (numeric(12,2), null)",
                    "active (boolean, not null)",
                ];
                COLUMNS
                    .iter()
                    .enumerate()
                    .map(|(i, c)| node(format!("{id}/{i}"), *c, NodeKind::Column, false))
                    .collect()
            }
            ["srv", "Databases", _, "Schemas", _, "Tables", t, "Indexes"] => vec![
                node(
                    format!("{id}/pk"),
                    format!("table_{t}_pkey"),
                    NodeKind::Index,
                    false,
                ),
                node(
                    format!("{id}/email"),
                    format!("table_{t}_email_idx"),
                    NodeKind::Index,
                    false,
                ),
            ],
            _ => return Err(MetadataError(format!("unknown node: {id}"))),
        };
        Ok(out)
    }

    /// O mesmo catálogo da árvore: tabelas `table_NNNNN` com as colunas da
    /// pasta Columns. Como a árvore, aceita qualquer banco.
    fn catalog(&self, database: &str) -> Result<Catalog, MetadataError> {
        thread::sleep(self.delay);
        let (schemas, tables) = if database == "stress" {
            (vec!["public".to_string()], STRESS_TABLES)
        } else {
            let mut v = vec!["public".to_string()];
            v.extend((1..SCHEMAS).map(|i| format!("schema_{i}")));
            (v, TABLES)
        };
        let columns: Vec<Column> = [
            ("id", "bigint"),
            ("name", "text"),
            ("email", "text"),
            ("created_at", "timestamptz"),
            ("balance", "numeric(12,2)"),
            ("active", "boolean"),
        ]
        .iter()
        .map(|(n, t)| Column {
            name: (*n).into(),
            ty: (*t).into(),
        })
        .collect();
        let mut schemas: Vec<Schema> = schemas
            .into_iter()
            .map(|name| Schema {
                relations: (0..tables)
                    .map(|i| Relation {
                        name: format!("table_{i:05}"),
                        kind: RelationKind::Table,
                        columns: columns.clone(),
                    })
                    .chain((0..5).map(|i| Relation {
                        name: format!("view_{i}"),
                        kind: RelationKind::View,
                        columns: columns[..2].to_vec(),
                    }))
                    .collect(),
                functions: (0..5).map(|i| format!("fn_{i}")).collect(),
                name,
            })
            .collect();
        schemas.push(Schema {
            name: "pg_catalog".into(),
            relations: Vec::new(),
            functions: ["count", "max", "min", "now", "sum"]
                .map(String::from)
                .into(),
        });
        Ok(Catalog {
            database: database.to_string(),
            search_path: vec!["public".into(), "pg_catalog".into()],
            schemas,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn children(p: &FakeProvider, id: &str) -> Vec<ObjectNode> {
        p.children(&NodeId(id.into())).unwrap()
    }

    #[test]
    fn walks_from_server_to_columns() {
        let p = FakeProvider::new(Duration::ZERO);
        let srv = &p.roots().unwrap()[0];
        let folders = children(&p, &srv.id.0);
        let dbs = children(&p, &folders[0].id.0);
        assert_eq!(dbs.len(), DATABASES);
        let schemas_folder = &children(&p, &dbs[0].id.0)[0];
        let schema = &children(&p, &schemas_folder.id.0)[0];
        let tables_folder = &children(&p, &schema.id.0)[0];
        let tables = children(&p, &tables_folder.id.0);
        assert_eq!(tables.len(), TABLES);
        let columns_folder = &children(&p, &tables[0].id.0)[0];
        let columns = children(&p, &columns_folder.id.0);
        assert!(
            columns
                .iter()
                .all(|c| c.kind == NodeKind::Column && !c.has_children)
        );
    }

    #[test]
    fn stress_schema_has_ten_thousand_tables() {
        let p = FakeProvider::new(Duration::ZERO);
        let tables = children(&p, "srv/Databases/stress/Schemas/public/Tables");
        assert_eq!(tables.len(), STRESS_TABLES);
    }

    #[test]
    fn unknown_node_is_an_error() {
        let p = FakeProvider::new(Duration::ZERO);
        assert!(p.children(&NodeId("nope".into())).is_err());
    }
}
