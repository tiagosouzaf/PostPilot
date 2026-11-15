//! Object Explorer do SQL Server. Neste marco, o servidor e os bancos; as
//! pastas de cada banco chegam no marco S2.1.

use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode,
};
use pgstudio_pg_client::DbRuntime;

use crate::Session;

/// Lista dos bancos que o login pode abrir.
pub const DATABASES_SQL: &str = "SELECT name FROM sys.databases \
    WHERE HAS_DBACCESS(name) = 1 AND state = 0 ORDER BY name";

const SERVER: &str = "server";
const DATABASES: &str = "server/Databases";
const DATABASE_PREFIX: &str = "db/";

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
        match parent.0.as_str() {
            SERVER => Ok(vec![node(
                DATABASES,
                Folder::Databases.label(),
                NodeKind::Folder(Folder::Databases),
                true,
            )]),
            DATABASES => Ok(self
                .rows(DATABASES_SQL)?
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .map(|name| {
                    node(
                        format!("{DATABASE_PREFIX}{name}"),
                        name,
                        NodeKind::Database,
                        false,
                    )
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn database_of(&self, node: &NodeId) -> Option<String> {
        node.0.strip_prefix(DATABASE_PREFIX).map(str::to_string)
    }
}
