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
    MaterializedViews,
    Functions,
    Procedures,
    Types,
    Extensions,
    Columns,
    Indexes,
    Sequences,
    Constraints,
    Triggers,
    Security,
    Roles,
    /// System Databases (SQL Server: master, model, msdb, tempdb).
    SystemDatabases,
    /// Programmability (SQL Server: procedures, funções, tipos...).
    Programmability,
    /// Keys de uma tabela (SQL Server: PK, UNIQUE, FK).
    Keys,
    /// Logins do servidor (SQL Server).
    Logins,
    /// Server Roles (SQL Server).
    ServerRoles,
    /// Users de um banco (SQL Server).
    Users,
    /// Database Roles (SQL Server).
    DatabaseRoles,
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
            Folder::MaterializedViews => "Materialized Views",
            Folder::Functions => "Functions",
            Folder::Procedures => "Procedures",
            Folder::Types => "Types",
            Folder::Extensions => "Extensions",
            Folder::Columns => "Columns",
            Folder::Indexes => "Indexes",
            Folder::Sequences => "Sequences",
            Folder::Constraints => "Constraints",
            Folder::Triggers => "Triggers",
            Folder::Security => "Security",
            Folder::Roles => "Roles",
            Folder::SystemDatabases => "System Databases",
            Folder::Programmability => "Programmability",
            Folder::Keys => "Keys",
            Folder::Logins => "Logins",
            Folder::ServerRoles => "Server Roles",
            Folder::Users => "Users",
            Folder::DatabaseRoles => "Database Roles",
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
    MaterializedView,
    Function,
    Procedure,
    Column,
    Index,
    Sequence,
    Constraint,
    Trigger,
    Type,
    Extension,
    Role,
    /// Grupo de chaves com o mesmo prefixo (`user:*` no Redis).
    KeyGroup,
    /// Uma chave de um banco chave-valor (Redis).
    Key,
    /// Linha de aviso do provedor (filtro ativo, lista cortada): não tem
    /// filhos, menu nem propriedades.
    Note,
}

impl NodeKind {
    /// Nome do tipo para a janela Properties, no idioma da interface.
    pub fn label(self) -> &'static str {
        pgstudio_i18n::tr(self.english_label())
    }

    /// Nome do tipo em inglês (a chave da tradução).
    pub fn english_label(self) -> &'static str {
        match self {
            NodeKind::Server => "Server",
            NodeKind::Folder(_) => "Folder",
            NodeKind::Database => "Database",
            NodeKind::Schema => "Schema",
            NodeKind::Table => "Table",
            NodeKind::View => "View",
            NodeKind::MaterializedView => "Materialized View",
            NodeKind::Function => "Function",
            NodeKind::Procedure => "Procedure",
            NodeKind::Column => "Column",
            NodeKind::Index => "Index",
            NodeKind::Sequence => "Sequence",
            NodeKind::Constraint => "Constraint",
            NodeKind::Trigger => "Trigger",
            NodeKind::Type => "Type",
            NodeKind::Extension => "Extension",
            NodeKind::Role => "Role",
            NodeKind::KeyGroup => "Key Group",
            NodeKind::Key => "Key",
            NodeKind::Note => "Note",
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
    /// Select Top N Rows (e Edit Top N Rows de uma view, só leitura).
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
                | NodeKind::MaterializedView
                | NodeKind::Function
                | NodeKind::Procedure
                | NodeKind::Index
                | NodeKind::Sequence
                | NodeKind::Constraint
                | NodeKind::Trigger
                | NodeKind::Type
                | NodeKind::Extension
        )
    }

    /// Se dá para ler linhas (Select Top, Edit Top).
    pub fn has_rows(self) -> bool {
        matches!(
            self,
            NodeKind::Table | NodeKind::View | NodeKind::MaterializedView
        )
    }

    /// Se o nó é um objeto com propriedades (tudo menos o servidor e as
    /// pastas).
    pub fn has_properties(self) -> bool {
        !matches!(
            self,
            NodeKind::Server | NodeKind::Folder(_) | NodeKind::KeyGroup | NodeKind::Note
        )
    }
}

/// Recebe uma parte dos filhos de um nó ([`MetadataProvider::children_in_parts`]).
/// Devolve false quando ninguém mais espera por eles.
pub type PartSink = std::sync::Arc<dyn Fn(Vec<ObjectNode>) -> bool + Send + Sync>;

/// Fonte dos nós do Object Explorer.
///
/// As chamadas podem demorar (vão ao banco); quem usa chama fora da thread da
/// interface.
pub trait MetadataProvider: Send + Sync {
    /// Nós de primeiro nível (os servidores conectados).
    fn roots(&self) -> Result<Vec<ObjectNode>, MetadataError>;

    /// Filhos de um nó.
    fn children(&self, parent: &NodeId) -> Result<Vec<ObjectNode>, MetadataError>;

    /// Filhos de um nó, em partes: cada parte pronta vai para `part` na
    /// ordem em que aparece na árvore, e a última volta como resultado.
    /// Quem lê listas grandes ou lentas (os bancos de um SQL Server com
    /// centenas deles) entrega assim o começo sem esperar o resto. Quando
    /// `part` devolve false ninguém mais espera a lista, e a leitura pode
    /// parar. O padrão lê tudo de uma vez.
    fn children_in_parts(
        &self,
        parent: &NodeId,
        part: &PartSink,
    ) -> Result<Vec<ObjectNode>, MetadataError> {
        let _ = part;
        self.children(parent)
    }

    /// Se o servidor é um banco relacional. Os outros (Redis...) não
    /// mostram no menu o que só vale para tabelas: Tasks, Design, Script
    /// as CREATE, Activity Monitor.
    fn relational(&self) -> bool {
        true
    }

    /// Se o que guarda os documentos se chama índice (Elasticsearch) e
    /// não coleção: muda só os nomes nos menus.
    fn search_indices(&self) -> bool {
        false
    }

    /// O filtro dos filhos de um nó (as chaves de um banco do Redis):
    /// `Some("")` quando o nó aceita filtro e não tem nenhum, None quando
    /// não filtra.
    fn filter(&self, node: &NodeId) -> Option<String> {
        let _ = node;
        None
    }

    /// Troca o filtro dos filhos de um nó (vazio tira o filtro). Vale na
    /// próxima leitura dos filhos.
    fn set_filter(&self, node: &NodeId, filter: &str) {
        let _ = (node, filter);
    }

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
            pgstudio_i18n::tr("Scripting is not available for this server.").to_string(),
        ))
    }

    /// Propriedades do objeto para a janela Properties (F4): pares de nome
    /// e valor, na ordem em que aparecem. Vazio quando o provedor não tem
    /// nada além do nome e do tipo.
    fn properties(&self, node: &NodeId) -> Result<Vec<Property>, MetadataError> {
        let _ = node;
        Ok(Vec::new())
    }

    /// Catálogo inteiro de um banco, para o IntelliSense.
    fn catalog(&self, database: &str) -> Result<crate::catalog::Catalog, MetadataError> {
        let _ = database;
        Err(MetadataError(
            pgstudio_i18n::tr("IntelliSense is not available for this server.").to_string(),
        ))
    }

    /// O que o Table Designer abre a partir do nó: "New Table" na pasta
    /// Tables de um schema, "Design" numa tabela. None quando o nó não
    /// tem designer.
    fn design_target(&self, node: &NodeId) -> Option<DesignTarget> {
        let _ = node;
        None
    }

    /// A pasta do Object Explorer que lista `target` (para atualizar depois
    /// de criar ou renomear um login, usuário ou role).
    fn security_folder(&self, target: &Securable) -> Option<NodeId> {
        let _ = target;
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
            pgstudio_i18n::tr("Designers are not available for this server.").to_string(),
        ))
    }

    /// Aplica DDL numa transação: tudo ou nada. O erro diz qual comando
    /// falhou.
    fn apply_ddl(&self, database: &str, statements: &[String]) -> Result<(), MetadataError> {
        let _ = (database, statements);
        Err(MetadataError(
            pgstudio_i18n::tr("Designers are not available for this server.").to_string(),
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

/// Uma linha da janela Properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub value: String,
}

impl Property {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Property {
            name: name.into(),
            value: value.into(),
        }
    }
}

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
    /// Login, usuário ou role do SQL Server, pelo nome (None: um novo).
    /// `database` é o banco dos usuários e das database roles.
    Principal {
        kind: PrincipalKind,
        database: Option<String>,
        name: Option<String>,
    },
}

/// Os tipos de principal do SQL Server que o app edita.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrincipalKind {
    Login,
    ServerRole,
    User,
    DatabaseRole,
}

impl PrincipalKind {
    /// Se vive no servidor (logins e server roles) e não num banco.
    pub fn server_level(self) -> bool {
        matches!(self, PrincipalKind::Login | PrincipalKind::ServerRole)
    }
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
            Securable::Principal { database, .. } => database.as_deref(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_have_portuguese_translations() {
        use Folder::*;
        use pgstudio_i18n::{Language, has_translation};
        let folders = [
            Databases,
            Diagrams,
            Schemas,
            Tables,
            Views,
            MaterializedViews,
            Functions,
            Procedures,
            Types,
            Extensions,
            Columns,
            Indexes,
            Sequences,
            Constraints,
            Triggers,
            Security,
            Roles,
            SystemDatabases,
            Programmability,
            Keys,
            Logins,
            ServerRoles,
            Users,
            DatabaseRoles,
        ];
        let kinds = [
            NodeKind::Server,
            NodeKind::Database,
            NodeKind::Schema,
            NodeKind::Table,
            NodeKind::View,
            NodeKind::MaterializedView,
            NodeKind::Function,
            NodeKind::Procedure,
            NodeKind::Column,
            NodeKind::Index,
            NodeKind::Sequence,
            NodeKind::Constraint,
            NodeKind::Trigger,
            NodeKind::Type,
            NodeKind::Extension,
            NodeKind::Role,
            NodeKind::Folder(Tables),
        ];
        let texts = folders
            .iter()
            .map(|f| f.label())
            .chain(kinds.iter().map(|k| k.english_label()));
        for text in texts {
            assert!(
                has_translation(Language::PortugueseBrazil, None, text),
                "sem tradução em locales/pt-BR.po: {text:?}"
            );
        }
    }
}
