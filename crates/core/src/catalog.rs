//! Catálogo de um banco para o IntelliSense: schemas, tabelas, views,
//! colunas e funções, lidos de uma vez e guardados em cache.

/// Tudo que o IntelliSense sabe de um banco.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
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

/// Palavras que o PostgreSQL não aceita como nome sem aspas: as reservadas e
/// as que podem ser nome de função ou de tipo (`RESERVED_KEYWORD` e
/// `TYPE_FUNC_NAME_KEYWORD` do `kwlist.h`), em ordem alfabética.
const RESERVED: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "authorization",
    "binary",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "collation",
    "column",
    "concurrently",
    "constraint",
    "create",
    "cross",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "freeze",
    "from",
    "full",
    "grant",
    "group",
    "having",
    "ilike",
    "in",
    "initially",
    "inner",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "lateral",
    "leading",
    "left",
    "like",
    "limit",
    "localtime",
    "localtimestamp",
    "natural",
    "not",
    "notnull",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "overlaps",
    "placing",
    "primary",
    "references",
    "returning",
    "right",
    "select",
    "session_user",
    "similar",
    "some",
    "symmetric",
    "system_user",
    "table",
    "tablesample",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "verbose",
    "when",
    "where",
    "window",
    "with",
];

/// Como escrever `name` no SQL: entre aspas se precisar (maiúsculas,
/// espaços, símbolos ou palavra reservada, como `"user"`).
pub fn quote_ident(name: &str) -> String {
    let plain = RESERVED.binary_search(&name).is_err()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    if plain {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

impl Catalog {
    /// Schema pelo nome como escrito no SQL.
    pub fn schema(&self, written: &str) -> Option<&Schema> {
        let name = normalize_ident(written);
        self.schemas.iter().find(|s| s.name == name)
    }

    /// Tabela ou view, com ou sem schema. Sem schema, procura no
    /// `search_path` e depois em qualquer schema.
    pub fn relation<'a>(&'a self, schema: Option<&str>, written: &str) -> Option<&'a Relation> {
        let name = normalize_ident(written);
        let name = name.as_str();
        let find = |s: &'a Schema| s.relations.iter().find(move |r| r.name == name);
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
        assert_eq!(quote_ident("cliente"), "cliente");
        assert_eq!(quote_ident("Nome"), "\"Nome\"");
        assert_eq!(quote_ident("dois nomes"), "\"dois nomes\"");
        assert_eq!(quote_ident("user"), "\"user\"");
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("time"), "time");
        assert!(
            RESERVED.windows(2).all(|w| w[0] < w[1]),
            "RESERVED fora de ordem"
        );
        assert_eq!(quote_ident("1a"), "\"1a\"");
    }

    #[test]
    fn unqualified_names_use_the_search_path() {
        let rel = |name: &str| Relation {
            name: name.into(),
            kind: RelationKind::Table,
            columns: Vec::new(),
        };
        let cat = Catalog {
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
