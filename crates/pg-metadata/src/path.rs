//! Identificadores dos nós do Object Explorer.
//!
//! Cada nó tem um caminho como `db/vendas/s/public/Tables`. Nomes vão
//! codificados (`%` e `/` viram `%25` e `%2F`), e objetos que têm OID usam o
//! OID, que não muda quando o objeto é renomeado durante a sessão.

use pgstudio_core::metadata::{Folder, NodeId};

/// Onde um nó está na árvore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Path {
    Server,
    /// Pasta de primeiro nível do servidor (Databases, Security).
    ServerFolder(Folder),
    /// Security > Roles.
    Roles,
    Database(String),
    /// Database > Database Diagrams.
    Diagrams(String),
    /// Database > Schemas.
    Schemas(String),
    /// Database > Extensions.
    Extensions(String),
    Schema(String, String),
    /// Schema > Tables, Views, Materialized Views, Functions, Procedures,
    /// Sequences, Types.
    SchemaFolder(String, String, Folder),
    Table(String, u32),
    View(String, u32),
    MaterializedView(String, u32),
    /// Table, View ou Materialized View > Columns, Constraints, Indexes,
    /// Triggers.
    RelationFolder(String, Relation, u32, Folder),
    /// Coluna, índice, constraint, trigger, função, procedure, sequência,
    /// tipo, extensão, role: sem filhos.
    Leaf,
}

/// Tabela, view ou view materializada, para as pastas embaixo delas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Table,
    View,
    MaterializedView,
}

impl Relation {
    fn tag(self) -> &'static str {
        match self {
            Relation::Table => "t",
            Relation::View => "v",
            Relation::MaterializedView => "mv",
        }
    }

    fn from_tag(tag: &str) -> Option<Relation> {
        [Relation::Table, Relation::View, Relation::MaterializedView]
            .into_iter()
            .find(|r| r.tag() == tag)
    }
}

/// Codifica um nome para caber num segmento do caminho.
pub fn enc(name: &str) -> String {
    name.replace('%', "%25").replace('/', "%2F")
}

pub(crate) fn dec(segment: &str) -> String {
    segment.replace("%2F", "/").replace("%25", "%")
}

fn folder(label: &str) -> Option<Folder> {
    use Folder::*;
    [
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
    ]
    .into_iter()
    .find(|f| f.label() == label)
}

impl Path {
    /// Lê um identificador. Desconhecido vira [`Path::Leaf`].
    pub fn parse(id: &NodeId) -> Path {
        let parts: Vec<&str> = id.0.split('/').collect();
        let oid = |s: &str| s.parse::<u32>().ok();
        match parts.as_slice() {
            ["server"] => Path::Server,
            ["server", "Security", "Roles"] => Path::Roles,
            ["server", f] => folder(f).map_or(Path::Leaf, Path::ServerFolder),
            ["db", db] => Path::Database(dec(db)),
            ["db", db, "Schemas"] => Path::Schemas(dec(db)),
            ["db", db, "Database Diagrams"] => Path::Diagrams(dec(db)),
            ["db", db, "Extensions"] => Path::Extensions(dec(db)),
            ["db", db, "s", s] => Path::Schema(dec(db), dec(s)),
            ["db", db, "s", s, f] => {
                folder(f).map_or(Path::Leaf, |f| Path::SchemaFolder(dec(db), dec(s), f))
            }
            ["db", db, "t", o] => oid(o).map_or(Path::Leaf, |o| Path::Table(dec(db), o)),
            ["db", db, "v", o] => oid(o).map_or(Path::Leaf, |o| Path::View(dec(db), o)),
            ["db", db, "mv", o] => {
                oid(o).map_or(Path::Leaf, |o| Path::MaterializedView(dec(db), o))
            }
            ["db", db, tag, o, f] => match (Relation::from_tag(tag), oid(o), folder(f)) {
                (Some(rel), Some(o), Some(f)) => Path::RelationFolder(dec(db), rel, o, f),
                _ => Path::Leaf,
            },
            _ => Path::Leaf,
        }
    }

    /// O identificador deste caminho.
    pub fn id(&self) -> NodeId {
        NodeId(match self {
            Path::Server => "server".into(),
            Path::ServerFolder(f) => format!("server/{}", f.label()),
            Path::Roles => "server/Security/Roles".into(),
            Path::Database(db) => format!("db/{}", enc(db)),
            Path::Schemas(db) => format!("db/{}/Schemas", enc(db)),
            Path::Diagrams(db) => format!("db/{}/Database Diagrams", enc(db)),
            Path::Extensions(db) => format!("db/{}/Extensions", enc(db)),
            Path::Schema(db, s) => format!("db/{}/s/{}", enc(db), enc(s)),
            Path::SchemaFolder(db, s, f) => format!("db/{}/s/{}/{}", enc(db), enc(s), f.label()),
            Path::Table(db, o) => format!("db/{}/t/{o}", enc(db)),
            Path::View(db, o) => format!("db/{}/v/{o}", enc(db)),
            Path::MaterializedView(db, o) => format!("db/{}/mv/{o}", enc(db)),
            Path::RelationFolder(db, rel, o, f) => {
                format!("db/{}/{}/{o}/{}", enc(db), rel.tag(), f.label())
            }
            Path::Leaf => "leaf".into(),
        })
    }
}

/// Identificador de um nó folha (`db/<db>/<tipo>/<partes>`).
pub fn leaf(db: &str, tag: &str, parts: &[&dyn std::fmt::Display]) -> NodeId {
    let mut id = format!("db/{}/{tag}", enc(db));
    for p in parts {
        id.push('/');
        id.push_str(&enc(&p.to_string()));
    }
    NodeId(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip_with_odd_names() {
        let db = "vendas/2024 %".to_string();
        let s = "my/schema".to_string();
        for p in [
            Path::Server,
            Path::ServerFolder(Folder::Databases),
            Path::ServerFolder(Folder::Security),
            Path::Roles,
            Path::Database(db.clone()),
            Path::Schemas(db.clone()),
            Path::Diagrams(db.clone()),
            Path::Extensions(db.clone()),
            Path::Schema(db.clone(), s.clone()),
            Path::SchemaFolder(db.clone(), s.clone(), Folder::Tables),
            Path::SchemaFolder(db.clone(), s.clone(), Folder::Sequences),
            Path::Table(db.clone(), 16384),
            Path::View(db.clone(), 1),
            Path::MaterializedView(db.clone(), 2),
            Path::SchemaFolder(db.clone(), s.clone(), Folder::MaterializedViews),
            Path::SchemaFolder(db.clone(), s.clone(), Folder::Procedures),
            Path::SchemaFolder(db.clone(), s.clone(), Folder::Types),
            Path::RelationFolder(db.clone(), Relation::Table, 16384, Folder::Triggers),
            Path::RelationFolder(db.clone(), Relation::MaterializedView, 9, Folder::Indexes),
            Path::RelationFolder(db.clone(), Relation::Table, 16384, Folder::Columns),
            Path::RelationFolder(db.clone(), Relation::View, 7, Folder::Columns),
        ] {
            assert_eq!(Path::parse(&p.id()), p, "{}", p.id());
        }
    }

    #[test]
    fn leaves_and_garbage_have_no_children() {
        for id in [
            leaf("db", "col", &[&1u32, &2i16]).0,
            "".into(),
            "db".into(),
            "db/x/t/notanoid".into(),
            "db/x/s/y/Nope".into(),
            "server/Nope".into(),
        ] {
            assert_eq!(Path::parse(&NodeId(id.clone())), Path::Leaf, "{id}");
        }
    }
}
