//! Catálogo de um banco para o IntelliSense: schemas, tabelas, views,
//! colunas e funções, lidos de uma vez e guardados em cache.

/// Tudo que o IntelliSense sabe de um banco.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    /// Motor do servidor: muda as regras dos nomes (maiúsculas, aspas ou
    /// colchetes).
    pub engine: crate::connection::Engine,
    /// Banco lido.
    pub database: String,
    /// Schemas do `search_path`, em ordem: nomes sem schema são procurados
    /// neles.
    pub search_path: Vec<String>,
    pub schemas: Vec<Schema>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schema {
    pub name: String,
    pub relations: Vec<Relation>,
    /// Funções e procedures (só o nome; sobrecargas aparecem uma vez).
    pub functions: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    View,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    pub kind: RelationKind,
    pub columns: Vec<Column>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Tipo como o `format_type` mostra, como `numeric(10,2)`.
    pub ty: String,
}

/// Como o PostgreSQL lê um nome escrito no SQL: entre aspas fica como está,
/// sem aspas vira minúsculas.
pub fn normalize_ident(written: &str) -> String {
    match written.strip_prefix('"') {
        Some(rest) => rest.strip_suffix('"').unwrap_or(rest).replace("\"\"", "\""),
        None => written.to_lowercase(),
    }
}

pub use pgstudio_schema_model::quote_ident;

/// O nome como o SQL Server guarda, para comparar: sem `[...]` nem
/// `"..."`, e em minúsculas (a comparação não diferencia maiúsculas nas
/// collations comuns).
pub fn normalize_tsql_ident(written: &str) -> String {
    let inner = if let Some(rest) = written.strip_prefix('[') {
        rest.strip_suffix(']').unwrap_or(rest).replace("]]", "]")
    } else if let Some(rest) = written.strip_prefix('"') {
        rest.strip_suffix('"').unwrap_or(rest).replace("\"\"", "\"")
    } else {
        written.to_string()
    };
    inner.to_lowercase()
}

impl Catalog {
    /// O nome escrito no SQL, na forma de comparar com os do catálogo.
    pub fn normalize(&self, written: &str) -> String {
        match self.engine {
            crate::connection::Engine::PostgreSql => normalize_ident(written),
            crate::connection::Engine::SqlServer => normalize_tsql_ident(written),
        }
    }

    /// Se o nome `stored` do catálogo é o `normalized` (de
    /// [`Catalog::normalize`]).
    fn same(&self, stored: &str, normalized: &str) -> bool {
        match self.engine {
            crate::connection::Engine::PostgreSql => stored == normalized,
            crate::connection::Engine::SqlServer => stored.to_lowercase() == normalized,
        }
    }

    /// O nome como entra no SQL: com aspas (PostgreSQL) ou colchetes (SQL
    /// Server) só quando precisa.
    pub fn quote(&self, name: &str) -> String {
        self.engine.ident(name)
    }

    /// Schema pelo nome como escrito no SQL.
    pub fn schema(&self, written: &str) -> Option<&Schema> {
        let name = self.normalize(written);
        self.schemas.iter().find(|s| self.same(&s.name, &name))
    }

    /// Tabela ou view, com ou sem schema. Sem schema, procura no
    /// `search_path` e depois em qualquer schema.
    pub fn relation<'a>(&'a self, schema: Option<&str>, written: &str) -> Option<&'a Relation> {
        let name = self.normalize(written);
        let name = name.as_str();
        let find = |s: &'a Schema| s.relations.iter().find(move |r| self.same(&r.name, name));
        match schema {
            Some(s) => self.schema(s).and_then(find),
            None => self
                .search_path
                .iter()
                .filter_map(|p| self.schemas.iter().find(|s| &s.name == p))
                .find_map(find)
                .or_else(|| self.schemas.iter().find_map(find)),
        }
    }

    /// Schemas do `search_path` que existem no banco.
    pub fn visible_schemas(&self) -> impl Iterator<Item = &Schema> {
        self.search_path
            .iter()
            .filter_map(|p| self.schemas.iter().find(|s| &s.name == p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_follow_postgres_rules() {
        assert_eq!(normalize_ident("Cliente"), "cliente");
        assert_eq!(normalize_ident("\"Cliente\""), "Cliente");
        assert_eq!(normalize_ident("\"a\"\"b\""), "a\"b");
    }

    #[test]
    fn unqualified_names_use_the_search_path() {
        let rel = |name: &str| Relation {
            name: name.into(),
            kind: RelationKind::Table,
            columns: Vec::new(),
        };
        let cat = Catalog {
            engine: Default::default(),
            database: "db".into(),
            search_path: vec!["app".into(), "public".into()],
            schemas: vec![
                Schema {
                    name: "public".into(),
                    relations: vec![rel("t"), rel("only_public")],
                    functions: Vec::new(),
                },
                Schema {
                    name: "app".into(),
                    relations: vec![rel("t")],
                    functions: Vec::new(),
                },
                Schema {
                    name: "other".into(),
                    relations: vec![rel("hidden")],
                    functions: Vec::new(),
                },
            ],
        };
        let schema_of = |r: &Relation| {
            cat.schemas
                .iter()
                .find(|s| s.relations.iter().any(|x| std::ptr::eq(x, r)))
                .unwrap()
                .name
                .clone()
        };
        assert_eq!(schema_of(cat.relation(None, "T").unwrap()), "app");
        assert_eq!(
            schema_of(cat.relation(Some("public"), "t").unwrap()),
            "public"
        );
        assert!(cat.relation(None, "only_public").is_some());
        assert!(cat.relation(None, "hidden").is_some());
        assert!(cat.relation(Some("app"), "hidden").is_none());
    }
}
