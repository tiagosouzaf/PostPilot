//! A explicação de cada sugestão do IntelliSense: o que o comando, a
//! palavra-chave, a função ou o operador faz, em uma ou duas frases,
//! mostrada embaixo da lista para a sugestão escolhida.
//!
//! Os textos fixos ficam em tabelas com o inglês e o português lado a
//! lado (são muitos e só servem aqui); as frases montadas com nomes do
//! banco (tabelas, colunas, chaves) passam pelo catálogo de traduções.

use pgstudio_core::catalog::{Catalog, Relation};
use pgstudio_i18n::{Language, tr, trf};

use crate::complete::{Kind, Suggestion};
use crate::lexer::Dialect;

mod elasticsearch;
mod mongodb;
mod redis;
mod sql;

/// Nome, explicação em inglês, explicação em português.
pub type Doc = (&'static str, &'static str, &'static str);

/// Quantas colunas (ou campos) citar na explicação de uma tabela.
const MAX_COLUMNS: usize = 8;

fn pick(en: &'static str, pt: &'static str) -> &'static str {
    match pgstudio_i18n::language() {
        Language::English => en,
        Language::PortugueseBrazil => pt,
    }
}

fn find(docs: &'static [Doc], name: &str) -> Option<&'static str> {
    docs.iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, en, pt)| pick(en, pt))
}

/// Procura primeiro por `"detalhe:nome"` (o nome naquele contexto) e
/// depois só pelo nome.
fn find_in_context(docs: &'static [Doc], item: &Suggestion) -> Option<&'static str> {
    if !item.detail.is_empty()
        && let Some(doc) = find(docs, &format!("{}:{}", item.detail, item.label))
    {
        return Some(doc);
    }
    find(docs, &item.label)
}

/// `"Colunas: a, b, c..."` de uma tabela, ou nada se ela não tem colunas
/// em cache.
fn columns(relation: Option<&Relation>, label: &str) -> String {
    let Some(relation) = relation.filter(|r| !r.columns.is_empty()) else {
        return String::new();
    };
    let mut names: Vec<&str> = relation
        .columns
        .iter()
        .take(MAX_COLUMNS)
        .map(|c| c.name.as_str())
        .collect();
    if relation.columns.len() > MAX_COLUMNS {
        names.push("...");
    }
    format!(" {label}: {}.", names.join(", "))
}

/// A tabela (coleção, índice) com este nome em qualquer schema.
fn any_relation<'a>(catalog: Option<&'a Catalog>, name: &str) -> Option<&'a Relation> {
    catalog?
        .schemas
        .iter()
        .flat_map(|s| &s.relations)
        .find(|r| r.name == name)
}

fn redis_doc(item: &Suggestion) -> Option<String> {
    match item.kind {
        Kind::Table | Kind::View => Some(if item.detail.is_empty() {
            tr("Key of the database.").to_string()
        } else {
            trf!("Key of type {type}.", type = item.detail)
        }),
        _ => find(redis::DOCS, &item.label)
            .or_else(|| find(redis::DOCS, &format!("{} {}", item.detail, item.label)))
            .or_else(|| (item.label == "HELP").then(|| pick(redis::HELP.0, redis::HELP.1)))
            .map(str::to_string),
    }
}

fn mongodb_doc(item: &Suggestion, catalog: Option<&Catalog>) -> Option<String> {
    let fields = || columns(any_relation(catalog, &item.label), tr("Fields"));
    match item.kind {
        Kind::Table => Some(format!("{}{}", tr("Collection of the database."), fields())),
        Kind::View => Some(format!("{}{}", tr("View of the database."), fields())),
        Kind::Column => Some(trf!(
            "Field of type {type}, seen in a sample of the documents.",
            type = item.detail
        )),
        _ => find_in_context(mongodb::DOCS, item).map(str::to_string),
    }
}

fn elasticsearch_doc(item: &Suggestion, catalog: Option<&Catalog>) -> Option<String> {
    match item.kind {
        Kind::Table => Some(format!(
            "{}{}",
            tr("Index of the cluster."),
            columns(any_relation(catalog, &item.label), tr("Fields"))
        )),
        Kind::View => Some(tr("Alias or data stream: a name that points to indices.").to_string()),
        Kind::Column => Some(trf!("Field of type {type}.", type = item.detail)),
        _ => find_in_context(elasticsearch::DOCS, item).map(str::to_string),
    }
}

fn sql_doc(item: &Suggestion, catalog: Option<&Catalog>) -> Option<String> {
    let name = item.label.to_lowercase();
    let relation = || {
        columns(
            catalog.and_then(|c| c.relation(Some(&item.detail), &item.label)),
            tr("Columns"),
        )
    };
    match item.kind {
        Kind::Column => Some(match item.detail.rsplit_once(" (") {
            Some((ty, from)) => trf!(
                "Column of {table}, of type {type}.",
                table = from.trim_end_matches(')'),
                type = ty
            ),
            None => tr("Column.").to_string(),
        }),
        Kind::Alias => Some(trf!(
            "Alias of {table} in this command.",
            table = item.detail
        )),
        Kind::Table => Some(format!(
            "{}{}",
            trf!("Table of the schema {schema}.", schema = item.detail),
            relation()
        )),
        Kind::View => Some(format!(
            "{}{}",
            trf!("View of the schema {schema}.", schema = item.detail),
            relation()
        )),
        Kind::Schema => Some(tr("Schema: groups tables, views and functions.").to_string()),
        Kind::Function => find(sql::FUNCTIONS, &name)
            .or_else(|| find(sql::KEYWORDS, &name))
            .map(str::to_string)
            .or_else(|| {
                Some(trf!(
                    "Function of the schema {schema}.",
                    schema = item.detail
                ))
            }),
        Kind::Keyword => find(sql::KEYWORDS, &name)
            .or_else(|| find(sql::FUNCTIONS, &name))
            .map(str::to_string),
    }
}

/// O que a sugestão faz, no idioma da interface. `None` se não há o que
/// dizer dela.
pub fn explain(dialect: Dialect, item: &Suggestion, catalog: Option<&Catalog>) -> Option<String> {
    match dialect {
        Dialect::Redis => redis_doc(item),
        Dialect::MongoDb => mongodb_doc(item, catalog),
        Dialect::Elasticsearch => elasticsearch_doc(item, catalog),
        Dialect::Postgres | Dialect::TSql => sql_doc(item, catalog),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{KEYWORDS, TSQL_KEYWORDS};
    use std::collections::HashSet;

    fn keyword(label: &str, detail: &str) -> Suggestion {
        Suggestion {
            label: label.to_string(),
            insert: label.to_string(),
            kind: Kind::Keyword,
            detail: detail.to_string(),
        }
    }

    fn has(dialect: Dialect, label: &str, detail: &str) -> bool {
        explain(dialect, &keyword(label, detail), None).is_some()
    }

    /// Cada tabela: sem nome repetido, com os dois idiomas preenchidos e
    /// terminados em ponto.
    #[test]
    fn tables_are_well_formed() {
        let tables: [(&str, &[Doc]); 5] = [
            ("redis", redis::DOCS),
            ("mongodb", mongodb::DOCS),
            ("elasticsearch", elasticsearch::DOCS),
            ("sql keywords", sql::KEYWORDS),
            ("sql functions", sql::FUNCTIONS),
        ];
        for (table, docs) in tables {
            let mut seen = HashSet::new();
            for (name, en, pt) in docs {
                assert!(seen.insert(*name), "{table}: {name} repetido");
                for text in [en, pt] {
                    assert!(
                        text.len() > 5 && text.ends_with(['.', ')']),
                        "{table}: {name}: {text:?}"
                    );
                }
                assert_ne!(en, pt, "{table}: {name} sem tradução");
            }
        }
    }

    #[test]
    fn every_redis_command_and_subcommand_is_explained() {
        for (name, _, _) in crate::redis::COMMANDS {
            assert!(has(Dialect::Redis, name, "key value"), "{name}");
        }
        for (command, subs) in crate::redis::SUBCOMMANDS {
            for sub in *subs {
                assert!(has(Dialect::Redis, sub, command), "{command} {sub}");
            }
        }
    }

    #[test]
    fn every_mongodb_suggestion_is_explained() {
        use crate::mongodb::*;
        let methods = COLLECTION_METHODS
            .iter()
            .chain(DATABASE_METHODS)
            .chain(CURSOR_METHODS)
            .chain(VALUES);
        for (name, detail) in methods {
            assert!(has(Dialect::MongoDb, name, detail), "{name}");
        }
        for (names, detail) in [
            (QUERY_OPERATORS, "filtro"),
            (UPDATE_OPERATORS, "atualização"),
            (STAGES, "estágio"),
            (EXPRESSIONS, "expressão"),
        ] {
            for name in names {
                assert!(has(Dialect::MongoDb, name, detail), "{name}");
            }
        }
        for name in ["db", "show collections", "show dbs", "use"] {
            assert!(has(Dialect::MongoDb, name, ""), "{name}");
        }
    }

    #[test]
    fn every_elasticsearch_suggestion_is_explained() {
        use crate::elasticsearch::*;
        // As palavras escritas direto no `complete` e no `top_level`.
        const OTHER: &[&str] = &[
            "query",
            "script",
            "doc",
            "doc_as_upsert",
            "upsert",
            "conflicts",
            "dest",
            "source",
            "actions",
            "create",
            "delete",
            "index",
            "update",
            "fetch_size",
            "filter",
            "params",
            "time_zone",
            "analyzer",
            "field",
            "text",
            "tokenizer",
            "dynamic",
            "properties",
            "runtime",
            "docs",
            "ids",
            "aliases",
            "mappings",
            "settings",
            "fields",
            "post_tags",
            "pre_tags",
            "analysis",
            "number_of_replicas",
            "number_of_shards",
            "refresh_interval",
            "lang",
            "add",
            "remove",
            "remove_index",
            "alias",
            "is_write_index",
            "_id",
            "_index",
            "path",
            "default_operator",
            "type",
            "asc",
            "desc",
            "day",
            "hour",
            "minute",
            "month",
            "quarter",
            "week",
            "year",
            "and",
            "or",
            "brazilian",
            "english",
            "keyword",
            "portuguese",
            "simple",
            "standard",
            "whitespace",
        ];
        let lists: [(&[&str], &str); 13] = [
            (crate::lexer::ELASTICSEARCH_METHODS, ""),
            (ROOT_APIS, "API"),
            (INDEX_APIS, "API"),
            (PARAMS, ""),
            (SEARCH_KEYS, ""),
            (QUERY_TYPES, "consulta"),
            (BOOL_KEYS, ""),
            (RANGE_KEYS, ""),
            (AGG_TYPES, "agregação"),
            (AGG_KEYS, ""),
            (MAPPING_KEYS, ""),
            (FIELD_TYPES, "tipo"),
            (OTHER, ""),
        ];
        for (names, detail) in lists {
            for name in names {
                assert!(has(Dialect::Elasticsearch, name, detail), "{detail}:{name}");
            }
        }
        for (api, subs) in SUB_APIS {
            for sub in *subs {
                assert!(has(Dialect::Elasticsearch, sub, api), "{api}/{sub}");
            }
        }
    }

    #[test]
    fn every_sql_keyword_is_explained() {
        for name in KEYWORDS.iter().chain(TSQL_KEYWORDS) {
            assert!(has(Dialect::Postgres, &name.to_uppercase(), ""), "{name}");
        }
    }

    #[test]
    fn the_same_name_is_explained_by_its_context() {
        let text = |detail| explain(Dialect::MongoDb, &keyword("$set", detail), None).unwrap();
        assert_ne!(text("estágio"), text("atualização"));
        let text =
            |detail| explain(Dialect::Elasticsearch, &keyword("terms", detail), None).unwrap();
        assert!(text("agregação").contains("GROUP BY"));
        assert!(!text("consulta").contains("GROUP BY"));
    }

    #[test]
    fn tables_and_columns_are_described_from_the_catalog() {
        use pgstudio_core::catalog::{Column, RelationKind, Schema};
        let catalog = Catalog {
            engine: Default::default(),
            database: "loja".into(),
            search_path: vec!["public".into()],
            schemas: vec![Schema {
                name: "public".into(),
                relations: vec![Relation {
                    name: "cliente".into(),
                    kind: RelationKind::Table,
                    columns: ["id", "nome"]
                        .iter()
                        .map(|c| Column {
                            name: (*c).into(),
                            ty: "integer".into(),
                        })
                        .collect(),
                }],
                functions: vec![],
            }],
        };
        let item = |kind, label: &str, detail: &str| Suggestion {
            label: label.into(),
            insert: label.into(),
            kind,
            detail: detail.into(),
        };
        let table = explain(
            Dialect::Postgres,
            &item(Kind::Table, "cliente", "public"),
            Some(&catalog),
        )
        .unwrap();
        assert!(
            table.contains("public") && table.contains("id, nome"),
            "{table}"
        );
        let column = explain(
            Dialect::Postgres,
            &item(Kind::Column, "nome", "integer (c)"),
            Some(&catalog),
        )
        .unwrap();
        assert!(
            column.contains("integer") && column.contains(" c,"),
            "{column}"
        );
        let function = explain(
            Dialect::Postgres,
            &item(Kind::Function, "calcula_total", "public"),
            Some(&catalog),
        )
        .unwrap();
        assert!(function.contains("public"), "{function}");
        // As conhecidas têm a explicação delas.
        let count = explain(
            Dialect::Postgres,
            &item(Kind::Function, "count", "pg_catalog"),
            Some(&catalog),
        )
        .unwrap();
        assert!(!count.contains("pg_catalog"), "{count}");
    }
}
