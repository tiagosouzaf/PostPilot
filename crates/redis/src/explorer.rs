//! Object Explorer do Redis: o servidor e os bancos (`db0`, `db1`...) com
//! a contagem de chaves.
//!
//! | Nó | Id |
//! | --- | --- |
//! | servidor | `server` |
//! | banco | `db/<n>` |

use pgstudio_core::connection::{ConnectionSettings, redis_database_index};
use pgstudio_core::metadata::{
    MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property,
};
use pgstudio_i18n::tr;
use pgstudio_pg_client::DbRuntime;

use crate::Connection;
use crate::resp::{Value, bytes_to_text};

const SERVER: &str = "server";

/// Lê os metadados com a conexão aberta pelo Connect to Server.
pub struct RedisExplorer {
    runtime: DbRuntime,
    connection: tokio::sync::Mutex<Connection>,
    label: String,
}

impl RedisExplorer {
    pub fn new(connection: Connection, settings: &ConnectionSettings, runtime: DbRuntime) -> Self {
        let user = if settings.user.trim().is_empty() {
            "default"
        } else {
            settings.user.trim()
        };
        let label = format!(
            "{} (Redis {} - {})",
            settings.server_name(),
            connection.server_version(),
            user
        );
        RedisExplorer {
            runtime,
            connection: tokio::sync::Mutex::new(connection),
            label,
        }
    }

    /// Nome do servidor na árvore: `host (Redis 7.2.4 - default)`.
    pub fn server_label(&self) -> &str {
        &self.label
    }

    fn query(&self, args: &[&str]) -> Result<Value, MetadataError> {
        self.runtime
            .block_on(async { self.connection.lock().await.query(args).await })
            .map_err(MetadataError)
    }

    /// Chaves por banco, do `INFO keyspace` (só os bancos com chaves
    /// aparecem lá).
    fn key_counts(&self) -> Result<Vec<(u32, u64)>, MetadataError> {
        let info = match self.query(&["INFO", "keyspace"])? {
            Value::Bulk(b) => bytes_to_text(&b),
            _ => String::new(),
        };
        Ok(parse_keyspace(&info))
    }

    fn databases(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        let count = self
            .runtime
            .block_on(async { self.connection.lock().await.database_count().await })
            .map_err(MetadataError)?;
        let keys = self.key_counts()?;
        Ok((0..count)
            .map(|i| {
                let n = keys.iter().find(|(db, _)| *db == i).map_or(0, |(_, n)| *n);
                ObjectNode {
                    id: NodeId(format!("db/{i}")),
                    name: format!("db{i} ({n})"),
                    kind: NodeKind::Database,
                    has_children: false,
                }
            })
            .collect())
    }
}

/// `db0:keys=2,expires=0,avg_ttl=0` de cada banco com chaves.
fn parse_keyspace(info: &str) -> Vec<(u32, u64)> {
    info.lines()
        .filter_map(|line| {
            let (db, rest) = line.trim().split_once(':')?;
            let db = redis_database_index(db).filter(|_| db.starts_with("db"))?;
            let keys = rest
                .split(',')
                .find_map(|p| p.strip_prefix("keys="))?
                .parse()
                .ok()?;
            Some((db, keys))
        })
        .collect()
}

/// O número do banco de um nó `db/<n>...`.
fn database_index(node: &NodeId) -> Option<u32> {
    node.0.strip_prefix("db/")?.split('/').next()?.parse().ok()
}

impl MetadataProvider for RedisExplorer {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![ObjectNode {
            id: NodeId(SERVER.into()),
            name: self.label.clone(),
            kind: NodeKind::Server,
            has_children: true,
        }])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        match parent.0.as_str() {
            SERVER => self.databases(),
            _ => Ok(Vec::new()),
        }
    }

    fn database_of(&self, node: &NodeId) -> Option<String> {
        database_index(node).map(|n| format!("db{n}"))
    }

    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        let Some(db) = database_index(node) else {
            return Ok(Vec::new());
        };
        let keys = self
            .key_counts()?
            .into_iter()
            .find(|(d, _)| *d == db)
            .map_or(0, |(_, n)| n);
        Ok(vec![Property::new(tr("Keys"), keys.to_string())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyspace_lines_are_read() {
        let info = "# Keyspace\r\ndb0:keys=12,expires=1,avg_ttl=0\r\ndb3:keys=1,expires=0\r\n";
        assert_eq!(parse_keyspace(info), vec![(0, 12), (3, 1)]);
        assert_eq!(parse_keyspace(""), vec![]);
        assert_eq!(database_index(&NodeId("db/3".into())), Some(3));
        assert_eq!(database_index(&NodeId("server".into())), None);
    }
}
