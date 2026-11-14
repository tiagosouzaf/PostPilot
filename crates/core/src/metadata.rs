//! Metadados do servidor que o Object Explorer mostra.
//!
//! O Object Explorer só conhece o trait [`MetadataProvider`]. Na Fase 1 ele
//! recebe um provedor falso; na Fase 2, o que lê o `pg_catalog`.

use std::fmt;

/// Tipo de pasta agrupadora, como "Tables" dentro de um schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Folder {
    Databases,
    /// Database Diagrams (diagramas salvos em arquivo `.pgsdiagram`).
    Diagrams,
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
            Folder::Diagrams => "Database Diagrams",
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

/// O que gerar para um objeto pelo menu de contexto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptAction {
    /// Script as > CREATE To.
    Create,
    /// Script as > DROP To.
    Drop,
    /// Select Top N Rows (e Edit Top N Rows, só leitura por enquanto).
    SelectTop(u32),
}

impl NodeKind {
    /// Se "Script as CREATE/DROP" vale para este tipo de nó.
    pub fn scriptable(self) -> bool {
        matches!(
            self,
            NodeKind::Database
                | NodeKind::Schema
                | NodeKind::Table
                | NodeKind::View
                | NodeKind::Function
                | NodeKind::Index
                | NodeKind::Sequence
                | NodeKind::Constraint
        )
    }

    /// Se dá para ler linhas (Select Top, Edit Top).
    pub fn has_rows(self) -> bool {
        matches!(self, NodeKind::Table | NodeKind::View)
    }
}

/// Fonte dos nós do Object Explorer.
///
/// As chamadas podem demorar (vão ao banco); quem usa chama fora da thread da
/// interface.
pub trait MetadataProvider: Send + Sync {
    /// Nós de primeiro nível (os servidores conectados).
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError>;

    /// Filhos de um nó.
    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError>;

    /// Banco a que o nó pertence (None para o servidor e o que fica fora
    /// dos bancos, como roles).
    fn database_of(&self, node: &NodeId) -> Option<String> {
        let _ = node;
        None
    }

    /// Gera o SQL de uma ação do menu de contexto (Script as, Select Top).
    fn script(&self, node: &NodeId, action: ScriptAction) -> Result<String, MetadataError> {
        let _ = (node, action);
        Err(MetadataError(
            "Scripting is not available for this server.".to_string(),
        ))
    }

    /// Catálogo inteiro de um banco, para o IntelliSense.
    fn catalog(&self, database: &str) -> Result<crate::catalog::Catalog, MetadataError> {
        let _ = database;
        Err(MetadataError(
            "IntelliSense is not available for this server.".to_string(),
        ))
    }

    /// O que o Table Designer abre a partir do nó: "New Table" na pasta
    /// Tables de um schema, "Design" numa tabela. None quando o nó não
    /// tem designer.
    fn design_target(&self, node: &NodeId) -> Option<DesignTarget> {
        let _ = node;
        None
    }

    /// O que a janela de propriedades de segurança abre a partir do nó:
    /// uma role (ou a pasta Roles, para uma nova) ou um objeto com
    /// permissões. None quando o nó não tem.
    fn securable(&self, node: &NodeId) -> Option<Securable> {
        let _ = node;
        None
    }

    /// As tabelas de um banco como modelo editável (Fase 3).
    fn load_model(&self, database: &str) -> Result<Model, MetadataError> {
        let _ = database;
        Err(MetadataError(
            "Designers are not available for this server.".to_string(),
        ))
    }

    /// Aplica DDL numa transação: tudo ou nada. O erro diz qual comando
    /// falhou.
    fn apply_ddl(&self, database: &str, statements: &[String]) -> Result<(), MetadataError> {
        let _ = (database, statements);
        Err(MetadataError(
            "Designers are not available for this server.".to_string(),
        ))
    }

    /// Tipos que o designer sugere: os comuns do PostgreSQL mais os do
    /// banco (enums, domínios, tipos compostos), como `format_type` os
    /// escreve.
    fn type_names(&self, database: &str) -> Result<Vec<String>, MetadataError> {
        let _ = database;
        Ok(COMMON_TYPES.iter().map(|t| (*t).to_string()).collect())
    }
}

pub use pgstudio_schema_model::Model;

/// Onde o Table Designer abre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignTarget {
    pub database: String,
    /// Schema da tabela nova (para "Design", o da tabela).
    pub schema: String,
    /// Id da tabela no [`Model`] (o oid), ou None para uma tabela nova.
    pub table: Option<u64>,
}

/// Alvo das propriedades de segurança (roles e permissões).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Securable {
    /// Uma role existente, pelo oid.
    Role(u32),
    /// A pasta Roles: "New Role...".
    NewRole,
    Database(String),
    /// Schema, pelo banco e nome.
    Schema {
        database: String,
        name: String,
    },
    /// Tabela ou view, pelo oid.
    Relation {
        database: String,
        oid: u32,
    },
    /// Função ou procedure, pelo oid.
    Function {
        database: String,
        oid: u32,
    },
}

impl Securable {
    /// Banco onde as permissões são lidas e alteradas (as roles são do
    /// servidor: None).
    pub fn database(&self) -> Option<&str> {
        match self {
            Securable::Role(_) | Securable::NewRole => None,
            Securable::Database(db) => Some(db),
            Securable::Schema { database, .. }
            | Securable::Relation { database, .. }
            | Securable::Function { database, .. } => Some(database),
        }
    }
}

/// Tipos sugeridos em qualquer banco, como `format_type` os escreve.
pub const COMMON_TYPES: &[&str] = &[
    "bigint",
    "boolean",
    "bytea",
    "character varying",
    "character",
    "date",
    "double precision",
    "integer",
    "interval",
    "json",
    "jsonb",
    "numeric",
    "real",
    "smallint",
    "text",
    "time without time zone",
    "timestamp with time zone",
    "timestamp without time zone",
    "uuid",
];
