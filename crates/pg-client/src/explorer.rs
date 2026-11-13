//! O servidor conectado no Object Explorer.
//!
//! Por enquanto mostra o servidor e a lista de bancos; schemas, tabelas e o
//! resto do catálogo chegam com a introspecção (marco 2.2).

use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode,
};
use tokio_postgres::SimpleQueryMessage;

use crate::{DbRuntime, Session, error};

const SERVER: &str = "server";
const DATABASES: &str = "server/Databases";
const DATABASE_PREFIX: &str = "db/";

/// Bancos que aceitam conexão, sem os templates.
const LIST_DATABASES: &str = "SELECT datname FROM pg_catalog.pg_database \
     WHERE NOT datistemplate AND datallowconn ORDER BY datname";

/// [`MetadataProvider`] de um servidor conectado.
pub struct ServerExplorer {
    session: Session,
    runtime: DbRuntime,
}

impl ServerExplorer {
    pub fn new(session: Session, runtime: DbRuntime) -> Self {
        ServerExplorer { session, runtime }
    }

    /// Nome do servidor como o SSMS mostra: `host (PostgreSQL 16.4 - user)`.
    pub fn server_label(&self) -> String {
        let s = self.session.settings();
        format!(
            "{} (PostgreSQL {} - {})",
            s.server_name(),
            self.session.server_version(),
            s.user
        )
    }

    /// A sessão aberta.
    pub fn session(&self) -> &Session {
        &self.session
    }

    fn databases(&self) -> Result<Vec<String>, MetadataError> {
        let rows = self
            .runtime
            .block_on(self.session.simple_query(LIST_DATABASES))
            .map_err(|e| MetadataError(error::describe(&e)))?;
        Ok(rows
            .iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
                _ => None,
            })
            .collect())
    }
}

impl MetadataProvider for ServerExplorer {
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError> {
        Ok(vec![ObjectNode {
            id: NodeId(SERVER.into()),
            name: self.server_label(),
            kind: NodeKind::Server,
            has_children: true,
        }])
    }

    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError> {
        match parent.0.as_str() {
            SERVER => Ok(vec![ObjectNode {
                id: NodeId(DATABASES.into()),
                name: Folder::Databases.label().into(),
                kind: NodeKind::Folder(Folder::Databases),
                has_children: true,
            }]),
            DATABASES => Ok(self
                .databases()?
                .into_iter()
                .map(|name| ObjectNode {
                    id: NodeId(format!("{DATABASE_PREFIX}{name}")),
                    name,
                    kind: NodeKind::Database,
                    has_children: false,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }
}
