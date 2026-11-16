//! IntelliSense do Elasticsearch: o método, o caminho (índices e APIs) e,
//! no corpo em JSON, as chaves que cabem em cada lugar (`query`, `bool`,
//! os tipos de consulta e de agregação...) e os campos do índice.
//!
//! Os índices vêm do catálogo em cache: cada índice é uma "tabela" do
//! schema sem nome e os campos do mapeamento são as colunas dela.

use pgstudio_core::catalog::{Catalog, Relation, RelationKind};

use crate::complete::{Completion, Kind, Suggestion};
use crate::lexer::{ELASTICSEARCH_METHODS, Token, TokenKind};

/// APIs na raiz do caminho.
pub const ROOT_APIS: &[&str] = &[
    "_aliases",
    "_all",
    "_analyze",
    "_bulk",
    "_cat",
    "_cluster",
    "_count",
    "_data_stream",
    "_index_template",
    "_ingest",
    "_mapping",
    "_mget",
    "_msearch",
    "_nodes",
    "_refresh",
    "_reindex",
    "_search",
    "_security",
    "_settings",
    "_snapshot",
    "_sql",
    "_tasks",
];

/// APIs de um índice.
pub const INDEX_APIS: &[&str] = &[
    "_alias",
    "_analyze",
    "_bulk",
    "_close",
    "_count",
    "_create",
    "_delete_by_query",
    "_doc",
    "_explain",
    "_field_caps",
    "_forcemerge",
    "_mapping",
    "_mget",
    "_msearch",
    "_open",
    "_refresh",
    "_rollover",
    "_search",
    "_settings",
    "_source",
    "_stats",
    "_termvectors",
    "_update",
    "_update_by_query",
    "_validate",
];

/// O que vem depois de uma API que tem partes.
const SUB_APIS: &[(&str, &[&str])] = &[
    (
        "_cat",
        &[
            "aliases",
            "allocation",
            "count",
            "health",
            "indices",
            "master",
            "nodes",
            "pending_tasks",
            "plugins",
            "recovery",
            "repositories",
            "segments",
            "shards",
            "snapshots",
            "tasks",
            "templates",
            "thread_pool",
        ],
    ),
    (
        "_cluster",
        &[
            "allocation",
            "health",
            "pending_tasks",
            "reroute",
            "settings",
            "state",
            "stats",
        ],
    ),
    ("_ingest", &["pipeline"]),
    ("_nodes", &["hot_threads", "stats", "usage"]),
    ("_security", &["_authenticate", "api_key", "role", "user"]),
    ("_validate", &["query"]),
];

/// Parâmetros comuns da query string.
const PARAMS: &[&str] = &[
    "expand_wildcards=",
    "format=",
    "from=",
    "h=",
    "ignore_unavailable=true",
    "pretty",
    "q=",
    "refresh=true",
    "routing=",
    "s=",
    "size=",
    "timeout=",
    "v",
];

const SEARCH_KEYS: &[&str] = &[
    "_source",
    "aggs",
    "collapse",
    "explain",
    "fields",
    "from",
    "highlight",
    "knn",
    "min_score",
    "post_filter",
    "profile",
    "query",
    "runtime_mappings",
    "script_fields",
    "search_after",
    "size",
    "sort",
    "suggest",
    "timeout",
    "track_total_hits",
];

pub const QUERY_TYPES: &[&str] = &[
    "bool",
    "constant_score",
    "dis_max",
    "exists",
    "function_score",
    "fuzzy",
    "geo_distance",
    "ids",
    "match",
    "match_all",
    "match_phrase",
    "match_phrase_prefix",
    "multi_match",
    "nested",
    "prefix",
    "query_string",
    "range",
    "regexp",
    "simple_query_string",
    "term",
    "terms",
    "wildcard",
];

const BOOL_KEYS: &[&str] = &[
    "filter",
    "minimum_should_match",
    "must",
    "must_not",
    "should",
];
const RANGE_KEYS: &[&str] = &["format", "gt", "gte", "lt", "lte", "time_zone"];

pub const AGG_TYPES: &[&str] = &[
    "aggs",
    "avg",
    "cardinality",
    "composite",
    "date_histogram",
    "date_range",
    "extended_stats",
    "filter",
    "filters",
    "histogram",
    "max",
    "min",
    "missing",
    "nested",
    "percentiles",
    "range",
    "significant_terms",
    "stats",
    "sum",
    "terms",
    "top_hits",
    "value_count",
];

const AGG_KEYS: &[&str] = &[
    "calendar_interval",
    "field",
    "fixed_interval",
    "format",
    "interval",
    "min_doc_count",
    "missing",
    "order",
    "script",
    "size",
];

const MAPPING_KEYS: &[&str] = &[
    "analyzer",
    "copy_to",
    "fields",
    "format",
    "ignore_above",
    "index",
    "null_value",
    "properties",
    "type",
];

pub const FIELD_TYPES: &[&str] = &[
    "binary",
    "boolean",
    "byte",
    "date",
    "dense_vector",
    "double",
    "flattened",
    "float",
    "geo_point",
    "geo_shape",
    "integer",
    "ip",
    "keyword",
    "long",
    "nested",
    "object",
    "scaled_float",
    "short",
    "text",
];

/// Consultas cuja chave de dentro é um campo: `{ "match": { "campo": ...`.
const FIELD_QUERIES: &[&str] = &[
    "fuzzy",
    "match",
    "match_phrase",
    "match_phrase_prefix",
    "prefix",
    "range",
    "regexp",
    "term",
    "terms",
    "wildcard",
];

/// Chaves cujo valor é um campo (ou uma lista de campos).
const FIELD_VALUES: &[&str] = &[
    "_source",
    "default_field",
    "docvalue_fields",
    "excludes",
    "field",
    "fields",
    "includes",
    "path",
    "sort",
    "stored_fields",
];

/// Onde o cursor está no JSON do corpo.
#[derive(Debug, Default)]
struct Body<'a> {
    /// As chaves em que cada objeto ou lista aberta está, de fora para
    /// dentro (os itens de uma lista herdam a chave dela).
    path: Vec<&'a str>,
    /// Se o que abre por último é uma lista.
    in_array: bool,
    /// A última chave escrita no objeto de dentro.
    key: Option<&'a str>,
    /// O token com sentido antes do cursor.
    previous: Option<&'a str>,
}

fn unquote(text: &str) -> &str {
    text.trim_matches('"')
}

/// Lê os tokens do corpo até o cursor.
fn body_context<'a>(text: &'a str, tokens: &[&'a Token]) -> Body<'a> {
    // (chave em que abriu, é lista, última chave de dentro)
    let mut stack: Vec<(Option<&str>, bool, Option<&str>)> = Vec::new();
    let mut previous = None;
    for t in tokens {
        let word = &text[t.range.clone()];
        match (t.kind, word) {
            (TokenKind::Punctuation, "{" | "[") => {
                let key = match stack.last() {
                    Some((key, true, _)) => *key,
                    Some((_, false, current)) => *current,
                    None => None,
                };
                stack.push((key, word == "[", None));
            }
            (TokenKind::Punctuation, "}" | "]") => {
                stack.pop();
            }
            (TokenKind::QuotedIdentifier, _) => {
                if let Some(top) = stack.last_mut() {
                    top.2 = Some(unquote(word));
                }
            }
            _ => {}
        }
        previous = Some(word);
    }
    Body {
        path: stack.iter().skip(1).filter_map(|(key, ..)| *key).collect(),
        in_array: stack.last().is_some_and(|(_, array, _)| *array),
        key: stack
            .last()
            .and_then(|(key, array, current)| if *array { *key } else { *current }),
        previous,
    }
}

/// O primeiro pedaço do caminho quando é um índice (ou vários).
fn target(path: &str) -> Option<&str> {
    let first = path.trim_start_matches('/').split(['/', '?']).next()?;
    (!first.is_empty() && !first.starts_with('_')).then_some(first)
}

/// Os índices do catálogo que `target` nomeia: nomes separados por
/// vírgula, com `*` no fim.
fn relations<'a>(catalog: Option<&'a Catalog>, target: Option<&str>) -> Vec<&'a Relation> {
    let all = || {
        catalog
            .into_iter()
            .flat_map(|c| &c.schemas)
            .flat_map(|s| &s.relations)
    };
    let named: Vec<&Relation> = match target {
        Some(target) => all()
            .filter(|r| {
                target.split(',').any(|part| match part.split_once('*') {
                    Some((prefix, _)) => r.name.starts_with(prefix),
                    None => r.name == part,
                })
            })
            .filter(|r| !r.columns.is_empty())
            .collect(),
        None => Vec::new(),
    };
    // Um alias (ou nenhum índice no caminho): os campos de todos.
    if named.is_empty() {
        all().collect()
    } else {
        named
    }
}

/// Campos sugeridos de uma vez.
const MAX_FIELDS: usize = 300;

fn starts_with(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

struct Items<'a> {
    prefix: &'a str,
    /// O que entra depois do nome (a aspa que fecha, quando falta).
    suffix: &'a str,
    items: Vec<Suggestion>,
}

impl Items<'_> {
    fn words(&mut self, names: &[&str], kind: Kind, detail: &str) {
        for name in names {
            if starts_with(name, self.prefix) && !self.items.iter().any(|i| i.label == *name) {
                self.items.push(Suggestion {
                    label: name.to_string(),
                    insert: format!("{name}{}", self.suffix),
                    kind,
                    detail: detail.to_string(),
                });
            }
        }
    }

    fn fields(&mut self, relations: &[&Relation]) {
        for column in relations.iter().flat_map(|r| &r.columns) {
            if self.items.len() >= MAX_FIELDS {
                return;
            }
            if starts_with(&column.name, self.prefix)
                && !self.items.iter().any(|i| i.label == column.name)
            {
                self.items.push(Suggestion {
                    label: column.name.clone(),
                    insert: format!("{}{}", column.name, self.suffix),
                    kind: Kind::Column,
                    detail: column.ty.clone(),
                });
            }
        }
    }

    fn indices(&mut self, catalog: Option<&Catalog>) {
        for r in catalog
            .into_iter()
            .flat_map(|c| &c.schemas)
            .flat_map(|s| &s.relations)
        {
            if starts_with(&r.name, self.prefix) {
                self.items.push(Suggestion {
                    label: r.name.clone(),
                    insert: format!("{}{}", r.name, self.suffix),
                    kind: match r.kind {
                        RelationKind::Table => Kind::Table,
                        RelationKind::View => Kind::View,
                    },
                    detail: String::new(),
                });
            }
        }
    }
}

/// As sugestões do caminho escrito até o cursor (`written`, sem o
/// método).
fn path_items(written: &str, catalog: Option<&Catalog>) -> (usize, Vec<Suggestion>) {
    let start = written.rfind(['/', ',', '?', '&']).map_or(0, |i| i + 1);
    let mut out = Items {
        prefix: &written[start..],
        suffix: "",
        items: Vec::new(),
    };
    if written.contains('?') {
        if !out.prefix.contains('=') {
            out.words(PARAMS, Kind::Keyword, "");
        }
        return (start, out.items);
    }
    let segments: Vec<&str> = written[..start]
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let list = written[..start].ends_with(',');
    match segments.last() {
        // Mais um índice de uma lista (`a,b`).
        _ if list => out.indices(catalog),
        None => {
            out.indices(catalog);
            out.words(ROOT_APIS, Kind::Function, "API");
        }
        Some(last) => {
            if let Some((_, subs)) = SUB_APIS.iter().find(|(api, _)| api == last) {
                out.words(subs, Kind::Function, last);
            } else if !last.starts_with('_') && segments.len() == 1 {
                out.words(INDEX_APIS, Kind::Function, "API");
            } else if matches!(*last, "indices" | "shards" | "count" | "segments")
                || matches!(*last, "_mapping" | "_settings" | "_alias" | "_data_stream")
            {
                // `_cat/indices/<índice>`, `_mapping/...`: um índice.
                out.indices(catalog);
            }
        }
    }
    (start, out.items)
}

/// As chaves do primeiro nível do corpo, pela API do caminho.
fn top_level(method: &str, path: &str) -> &'static [&'static str] {
    let api = path
        .split('?')
        .next()
        .unwrap_or_default()
        .split('/')
        .find(|p| p.starts_with('_'))
        .unwrap_or("");
    match api {
        "_search" | "_msearch" | "_async_search" => SEARCH_KEYS,
        "_count" | "_delete_by_query" | "_explain" | "_validate" => &["query"],
        "_update_by_query" => &["query", "script"],
        "_update" => &["doc", "doc_as_upsert", "script", "upsert"],
        "_reindex" => &["conflicts", "dest", "script", "source"],
        "_aliases" => &["actions"],
        "_bulk" => &["create", "delete", "index", "update"],
        "_sql" => &["fetch_size", "filter", "params", "query", "time_zone"],
        "_analyze" => &["analyzer", "field", "filter", "text", "tokenizer"],
        "_mapping" => &["dynamic", "properties", "runtime"],
        "_settings" => &["index"],
        "_mget" => &["docs", "ids"],
        "" if method == "PUT" && target(path).is_some() => &["aliases", "mappings", "settings"],
        _ => &[],
    }
}

/// Sugestões para o cursor em `at` (bytes) num texto de requisições do
/// Elasticsearch.
pub fn complete(text: &str, tokens: &[Token], at: usize, catalog: Option<&Catalog>) -> Completion {
    let none = Completion {
        replace: at..at,
        ..Completion::default()
    };
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..at];
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return none;
    }
    // A linha do método: o método ou o caminho.
    if let Some((method, rest)) = trimmed.split_once([' ', '\t'])
        && ELASTICSEARCH_METHODS.contains(&method.to_ascii_uppercase().as_str())
    {
        let written = rest.trim_start();
        if written.contains(char::is_whitespace) {
            return none;
        }
        let (start, items) = path_items(written, catalog);
        let start = at - (written.len() - start);
        return Completion {
            replace: start..at,
            prefix: text[start..at].to_string(),
            items,
        };
    }
    // A requisição em que o cursor está: a última linha de método antes.
    let mut request: Option<(String, &str, usize)> = None;
    let mut offset = 0;
    for l in text[..line_start].split_inclusive('\n') {
        if let Some((method, path)) = l.trim().split_once([' ', '\t']) {
            let method = method.to_ascii_uppercase();
            if ELASTICSEARCH_METHODS.contains(&method.as_str()) && !path.trim().contains(' ') {
                request = Some((method, path.trim(), offset + l.len()));
            }
        }
        offset += l.len();
    }
    let Some((method, path, body_start)) = request else {
        // Antes de qualquer requisição: o método.
        if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
            let start = at - trimmed.len();
            let mut out = Items {
                prefix: trimmed,
                suffix: "",
                items: Vec::new(),
            };
            out.words(ELASTICSEARCH_METHODS, Kind::Keyword, "");
            return Completion {
                replace: start..at,
                prefix: trimmed.to_string(),
                items: out.items,
            };
        }
        return none;
    };

    // O token no cursor: só dentro de um texto (chave ou valor) há o que
    // sugerir no corpo, fora o método de uma requisição nova.
    let i = tokens.partition_point(|t| t.range.end < at);
    let token = tokens.get(i).filter(|t| t.range.start < at);
    let Some(token) = token.filter(|t| {
        matches!(t.kind, TokenKind::String | TokenKind::QuotedIdentifier)
            && text[t.range.start..at].matches('"').count() == 1
    }) else {
        if !trimmed.is_empty()
            && trimmed.chars().all(|c| c.is_ascii_alphabetic())
            && token.is_some_and(|t| t.kind == TokenKind::Identifier)
        {
            let start = at - trimmed.len();
            let mut out = Items {
                prefix: trimmed,
                suffix: "",
                items: Vec::new(),
            };
            out.words(ELASTICSEARCH_METHODS, Kind::Keyword, "");
            return Completion {
                replace: start..at,
                prefix: trimmed.to_string(),
                items: out.items,
            };
        }
        return none;
    };
    if text[token.range.clone()].starts_with("\"\"\"") {
        return none;
    }
    let start = token.range.start + 1;
    let prefix = &text[start..at];
    if prefix.contains(char::is_whitespace) {
        return none;
    }
    let before: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.range.start >= body_start && t.range.end <= token.range.start)
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    let body = body_context(text, &before);
    let mut out = Items {
        prefix,
        // Fecha a aspa se ainda não há uma logo depois.
        suffix: if text[at..].starts_with('"') {
            ""
        } else {
            "\""
        },
        items: Vec::new(),
    };
    let fields = relations(catalog, target(path));
    let parent = body.path.last().copied();
    let grandparent = body.path.iter().rev().nth(1).copied();
    let in_aggs = |key: Option<&str>| matches!(key, Some("aggs" | "aggregations"));
    let key_position = !body.in_array && matches!(body.previous, Some("{" | ","));
    if key_position {
        let great = body.path.iter().rev().nth(2).copied();
        match parent {
            None => out.words(top_level(&method, path), Kind::Keyword, ""),
            // O nome de uma agregação é de quem escreve.
            _ if in_aggs(parent) => {}
            _ if in_aggs(grandparent) => out.words(AGG_TYPES, Kind::Function, "agregação"),
            Some(p) if in_aggs(great) && AGG_TYPES.contains(&p) && p != "filter" => {
                out.words(AGG_KEYS, Kind::Keyword, "")
            }
            Some("query" | "post_filter" | "must" | "filter" | "should" | "must_not") => {
                out.words(QUERY_TYPES, Kind::Function, "consulta")
            }
            Some("bool") => out.words(BOOL_KEYS, Kind::Keyword, ""),
            Some(p) if FIELD_QUERIES.contains(&p) => out.fields(&fields),
            Some("sort" | "doc" | "upsert") => out.fields(&fields),
            Some("fields") if grandparent == Some("highlight") => out.fields(&fields),
            _ if grandparent == Some("range") => out.words(RANGE_KEYS, Kind::Keyword, ""),
            Some("highlight") => out.words(&["fields", "post_tags", "pre_tags"], Kind::Keyword, ""),
            Some("mappings") => out.words(&["dynamic", "properties"], Kind::Keyword, ""),
            Some("settings") => out.words(
                &[
                    "analysis",
                    "number_of_replicas",
                    "number_of_shards",
                    "refresh_interval",
                ],
                Kind::Keyword,
                "",
            ),
            _ if grandparent == Some("properties") || grandparent == Some("fields") => {
                out.words(MAPPING_KEYS, Kind::Keyword, "")
            }
            Some("script") => out.words(&["lang", "params", "source"], Kind::Keyword, ""),
            Some("source" | "dest") => out.words(&["index", "query"], Kind::Keyword, ""),
            Some("actions") => out.words(&["add", "remove", "remove_index"], Kind::Keyword, ""),
            Some("add" | "remove") => out.words(
                &["alias", "filter", "index", "is_write_index"],
                Kind::Keyword,
                "",
            ),
            Some("index" | "create" | "update" | "delete") => {
                out.words(&["_id", "_index"], Kind::Keyword, "")
            }
            Some("nested") => out.words(&["path", "query"], Kind::Keyword, ""),
            Some("multi_match" | "query_string" | "simple_query_string") => out.words(
                &["default_operator", "fields", "query", "type"],
                Kind::Keyword,
                "",
            ),
            _ => {}
        }
    } else if body.in_array || body.previous == Some(":") {
        match body.key {
            Some(key) if FIELD_VALUES.contains(&key) => out.fields(&fields),
            Some("type") if body.path.contains(&"properties") => {
                out.words(FIELD_TYPES, Kind::Keyword, "tipo")
            }
            Some("index" | "_index") => out.indices(catalog),
            Some("order") => out.words(&["asc", "desc"], Kind::Keyword, ""),
            Some("calendar_interval") => out.words(
                &["day", "hour", "minute", "month", "quarter", "week", "year"],
                Kind::Keyword,
                "",
            ),
            Some("operator" | "default_operator") => out.words(&["and", "or"], Kind::Keyword, ""),
            Some("analyzer") => out.words(
                &[
                    "brazilian",
                    "english",
                    "keyword",
                    "portuguese",
                    "simple",
                    "standard",
                    "whitespace",
                ],
                Kind::Keyword,
                "",
            ),
            // `"sort": [{ "data": "desc" }]` e `"sort": { "data": "desc" }`.
            Some(_) if parent == Some("sort") && !body.in_array => {
                out.words(&["asc", "desc"], Kind::Keyword, "")
            }
            _ => {}
        }
    }
    Completion {
        replace: start..at,
        prefix: prefix.to_string(),
        items: out.items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{Dialect, tokenize_in};
    use pgstudio_core::catalog::{Column, Schema};
    use pgstudio_core::connection::Engine;

    fn catalog() -> Catalog {
        let relation = |name: &str, kind, fields: &[(&str, &str)]| Relation {
            name: name.to_string(),
            kind,
            columns: fields
                .iter()
                .map(|(name, ty)| Column {
                    name: name.to_string(),
                    ty: ty.to_string(),
                })
                .collect(),
        };
        Catalog {
            engine: Engine::Elasticsearch,
            database: "cluster".into(),
            search_path: vec![String::new()],
            schemas: vec![Schema {
                name: String::new(),
                relations: vec![
                    relation("clientes", RelationKind::Table, &[("nome", "text")]),
                    relation(
                        "pedidos",
                        RelationKind::Table,
                        &[
                            ("cliente.cidade", "keyword"),
                            ("data", "date"),
                            ("pago", "boolean"),
                            ("total", "double"),
                        ],
                    ),
                    relation("pedidos-atual", RelationKind::View, &[]),
                ],
                functions: Vec::new(),
            }],
        }
    }

    /// As sugestões com o cursor no `|` de `text`.
    fn at(text: &str) -> (Vec<String>, Completion) {
        let pos = text.find('|').unwrap();
        let text = text.replace('|', "");
        let tokens = tokenize_in(&text, Dialect::Elasticsearch);
        let catalog = catalog();
        let c = complete(&text, &tokens, pos, Some(&catalog));
        (c.items.iter().map(|s| s.label.clone()).collect(), c)
    }

    fn labels(text: &str) -> Vec<String> {
        at(text).0
    }

    #[test]
    fn lists_are_sorted_and_unique() {
        for list in [
            ROOT_APIS,
            INDEX_APIS,
            PARAMS,
            SEARCH_KEYS,
            QUERY_TYPES,
            BOOL_KEYS,
            RANGE_KEYS,
            AGG_TYPES,
            AGG_KEYS,
            MAPPING_KEYS,
            FIELD_TYPES,
            FIELD_QUERIES,
            FIELD_VALUES,
        ] {
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
        }
        assert!(SUB_APIS.windows(2).all(|w| w[0].0 < w[1].0));
        for (_, subs) in SUB_APIS {
            assert!(subs.windows(2).all(|w| w[0] < w[1]), "{subs:?}");
        }
    }

    #[test]
    fn methods_start_a_request() {
        assert_eq!(labels("ge|"), ["GET"]);
        assert_eq!(labels("P|"), ["PATCH", "POST", "PUT"]);
        // Uma requisição nova depois de outra.
        assert_eq!(labels("GET /a/_search\n{ \"size\": 1 }\nde|"), ["DELETE"]);
        assert!(labels("# ge|").is_empty());
    }

    #[test]
    fn paths_offer_indices_and_apis() {
        let (got, c) = at("GET /|");
        assert_eq!(&got[..3], ["clientes", "pedidos", "pedidos-atual"]);
        assert!(got.contains(&"_search".to_string()));
        assert_eq!(c.items[0].kind, Kind::Table);
        assert_eq!(c.items[2].kind, Kind::View);
        // Sem a barra também.
        assert_eq!(labels("GET ped|"), ["pedidos", "pedidos-atual"]);
        let (got, c) = at("GET /pedidos/_se|");
        assert_eq!(got, ["_search", "_settings"]);
        assert_eq!(c.prefix, "_se");
        assert_eq!(c.replace.len(), 3);
        assert_eq!(labels("GET /_cat/in|"), ["indices"]);
        assert_eq!(labels("GET /_cat/indices/cl|"), ["clientes"]);
        assert_eq!(labels("GET /_cluster/he|"), ["health"]);
        assert_eq!(labels("GET /pedidos,cl|"), ["clientes"]);
        assert_eq!(labels("GET /pedidos/_search?s|"), ["s=", "size="]);
        assert!(labels("GET /pedidos/_search?size=1|").is_empty());
        assert!(labels("GET /pedidos/_doc/1|").is_empty());
    }

    #[test]
    fn body_keys_follow_the_place() {
        let (got, c) = at("GET /pedidos/_search\n{ \"qu|");
        assert_eq!(got, ["query"]);
        // A aspa que falta entra junto.
        assert_eq!(c.items[0].insert, "query\"");
        let (_, c) = at("GET /pedidos/_search\n{ \"qu|\" }");
        assert_eq!(c.items[0].insert, "query");
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"|").len(),
            SEARCH_KEYS.len()
        );
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"query\": { \"ma|"),
            ["match", "match_all", "match_phrase", "match_phrase_prefix"]
        );
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"query\": { \"bool\": { \"m|"),
            ["minimum_should_match", "must", "must_not"]
        );
        // Dentro de uma lista do bool, outra consulta.
        assert_eq!(
            labels(
                "GET /pedidos/_search\n{ \"query\": { \"bool\": { \"filter\": [\n{ \"term\": {} },\n{ \"ra|"
            ),
            ["range"]
        );
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"query\": { \"range\": { \"data\": { \"g|"),
            ["gt", "gte"]
        );
        assert_eq!(labels("POST /pedidos/_count\n{ \"|"), ["query"]);
        assert_eq!(labels("PUT /novo\n{ \"m|"), ["mappings"]);
        assert_eq!(
            labels("PUT /novo\n{ \"mappings\": { \"properties\": { \"nome\": { \"t|"),
            ["type"]
        );
        assert_eq!(
            labels("PUT /novo\n{ \"mappings\": { \"properties\": { \"nome\": { \"type\": \"ke|"),
            ["keyword"]
        );
        assert_eq!(labels("POST /_bulk\n{ \"in|"), ["index"]);
    }

    #[test]
    fn fields_come_from_the_index_of_the_path() {
        let (got, c) = at("GET /pedidos/_search\n{ \"query\": { \"term\": { \"|");
        assert_eq!(got, ["cliente.cidade", "data", "pago", "total"]);
        assert_eq!(c.items[0].detail, "keyword");
        assert_eq!(c.items[0].kind, Kind::Column);
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"sort\": [{ \"d|"),
            ["data"]
        );
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"sort\": [{ \"data\": \"d|"),
            ["desc"]
        );
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"_source\": [\"total\", \"p|"),
            ["pago"]
        );
        assert_eq!(
            labels("GET /clientes/_search\n{ \"query\": { \"match\": { \"|"),
            ["nome"]
        );
        // Um alias (sem campos no catálogo): os campos de todos os índices.
        assert_eq!(
            labels("GET /pedidos-atual/_search\n{ \"query\": { \"match\": { \"n|"),
            ["nome"]
        );
        assert_eq!(
            labels("GET /ped*/_search\n{ \"query\": { \"exists\": { \"field\": \"to|"),
            ["total"]
        );
        // O valor de um campo não tem sugestão.
        assert!(
            labels("GET /pedidos/_search\n{ \"query\": { \"term\": { \"pago\": \"t|").is_empty()
        );
    }

    #[test]
    fn aggregations() {
        let text = "GET /pedidos/_search\n{ \"aggs\": { \"por_cidade\": { \"te|";
        assert_eq!(labels(text), ["terms"]);
        // O nome da agregação é livre.
        assert!(labels("GET /pedidos/_search\n{ \"aggs\": { \"po|").is_empty());
        assert_eq!(
            labels("GET /pedidos/_search\n{ \"aggs\": { \"por_cidade\": { \"terms\": { \"fi|"),
            ["field", "fixed_interval"]
        );
        assert_eq!(
            labels(
                "GET /pedidos/_search\n{ \"aggs\": { \"por_cidade\": { \"terms\": { \"field\": \"cl|"
            ),
            ["cliente.cidade"]
        );
        assert_eq!(
            labels(
                "GET /pedidos/_search\n{ \"aggs\": { \"d\": { \"date_histogram\": { \"field\": \"data\", \"calendar_interval\": \"m|"
            ),
            ["minute", "month"]
        );
        // Uma agregação dentro da outra.
        assert_eq!(
            labels(
                "GET /pedidos/_search\n{ \"aggs\": { \"a\": { \"terms\": {}, \"aggs\": { \"b\": { \"su|"
            ),
            ["sum"]
        );
    }

    #[test]
    fn other_requests_do_not_leak_into_the_body() {
        // O corpo é o da requisição do cursor, não o da anterior.
        let text =
            "GET /clientes/_search\n{ \"query\": { \"match\": {\n\nPOST /pedidos/_count\n{ \"|";
        assert_eq!(labels(text), ["query"]);
        assert!(labels("{ \"|").is_empty());
    }
}
