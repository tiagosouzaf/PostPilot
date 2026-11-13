//! Metadados do servidor que o Object Explorer mostra.
//!
//! O Object Explorer só conhece o trait [`MetadataProvider`]. Na Fase 1 ele
//! recebe um provedor falso; na Fase 2, o que lê o `pg_catalog`.

use std::fmt;

/// Tipo de pasta agrupadora, como "Tables" dentro de um schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Folder {
    Databases,
    Schemas,
    Tables,
    Views,
    Functions,
    Columns,
    Indexes,
    Sequences,
    Constraints,
    Security,
    Roles,
}

impl Folder {
    /// Nome exibido, como no SSMS.
    pub fn label(self) -> &'static str {
        match self {
            Folder::Databases => "Databases",
            Folder::Schemas => "Schemas",
            Folder::Tables => "Tables",
            Folder::Views => "Views",
            Folder::Functions => "Functions",
            Folder::Columns => "Columns",
            Folder::Indexes => "Indexes",
            Folder::Sequences => "Sequences",
            Folder::Constraints => "Constraints",
            Folder::Security => "Security",
            Folder::Roles => "Roles",
        }
    }
}

/// Tipo de um nó da árvore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Server,
    Folder(Folder),
    Database,
    Schema,
    Table,
    View,
    Function,
    Column,
    Index,
    Sequence,
    Constraint,
    Role,
}

impl NodeKind {
    /// Nome do tipo para a janela Properties.
    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Server => "Server",
            NodeKind::Folder(_) => "Folder",
            NodeKind::Database => "Database",
            NodeKind::Schema => "Schema",
            NodeKind::Table => "Table",
            NodeKind::View => "View",
            NodeKind::Function => "Function",
            NodeKind::Column => "Column",
            NodeKind::Index => "Index",
            NodeKind::Sequence => "Sequence",
            NodeKind::Constraint => "Constraint",
            NodeKind::Role => "Role",
        }
    }
}

/// Identificador estável de um nó, único na árvore (ex.: um caminho).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub String);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Um objeto do servidor como aparece na árvore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectNode {
    pub id: NodeId,
    pub name: String,
    pub kind: NodeKind,
    /// Se vale mostrar a seta de expandir. Os filhos só são lidos ao expandir.
    pub has_children: bool,
}

/// Erro ao ler metadados.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataError(pub String);

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MetadataError {}

/// Fonte dos nós do Object Explorer.
///
/// As chamadas podem demorar (vão ao banco); quem usa chama fora da thread da
/// interface.
pub trait MetadataProvider: Send + Sync {
    /// Nós de primeiro nível (os servidores conectados).
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError>;

    /// Filhos de um nó.
    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError>;
}
