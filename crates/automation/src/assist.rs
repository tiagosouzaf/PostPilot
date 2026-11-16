//! As sugestões da IA para o comando que a pessoa está escrevendo: a
//! pergunta que vai para o modelo (o comando, onde o cursor está e o que
//! o catálogo sabe das tabelas citadas) e a leitura da resposta.
//!
//! Nada aqui roda no banco: a IA lê o texto do comando e os nomes do
//! catálogo que o IntelliSense já tem; valores das tabelas não vão.

use std::collections::BTreeSet;

use pgstudio_core::catalog::{Catalog, Relation, RelationKind};
use pgstudio_core::connection::Engine;
use pgstudio_core::schema::{Model, Table};
use serde_json::Value;

/// Quanto do comando vai para o modelo.
pub const MAX_STATEMENT: usize = 6_000;
/// Quanto do catálogo vai para o modelo.
pub const MAX_CONTEXT: usize = 14_000;
/// Quantas sugestões a lista mostra.
pub const MAX_SUGGESTIONS: usize = 8;
/// Onde o cursor está, no comando mandado.
pub const CURSOR: &str = "‸";

const MAX_COLUMNS: usize = 80;
const MAX_NEIGHBOUR_COLUMNS: usize = 30;
const MAX_NAMES: usize = 200;
/// Quantas das tabelas não citadas vão com as colunas.
const MAX_DETAILED: usize = 12;
const MAX_TITLE: usize = 90;
const MAX_DETAIL: usize = 400;

/// Os assuntos que a resposta pode usar em `kind`.
pub const KINDS: &[&str] = &[
    "join",
    "filter",
    "columns",
    "aggregate",
    "sort",
    "performance",
    "index",
    "fix",
    "safety",
    "complete",
    "idiom",
    "other",
];

/// O que aplicar a sugestão faz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `code` é o comando inteiro reescrito.
    Replace,
    /// `code` é outro comando, para acrescentar depois.
    Insert,
    /// Só um conselho.
    None,
}

/// Uma sugestão lida da resposta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Um dos [`KINDS`].
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub action: Action,
    pub code: String,
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::PostgreSql => "PostgreSQL",
        Engine::SqlServer => "Microsoft SQL Server",
        Engine::Redis => "Redis",
        Engine::MongoDb => "MongoDB",
        Engine::Elasticsearch => "Elasticsearch",
    }
}

/// O que o modelo precisa saber de cada motor: a sintaxe que a janela de
/// consulta do PostPilot aceita e o que vale a pena propor.
fn engine_rules(engine: Engine) -> &'static str {
    match engine {
        Engine::PostgreSql => {
            "Write PostgreSQL. Ideas worth proposing: joins that follow the foreign keys listed \
             in the schema (say which key), filters on indexed columns, date ranges written so \
             an index can be used, EXISTS instead of IN or DISTINCT, window functions, CTEs, \
             FILTER in aggregates, LIMIT with ORDER BY, naming columns instead of SELECT *, \
             a missing index for the WHERE/JOIN/ORDER BY (as a separate CREATE INDEX statement), \
             RETURNING, ON CONFLICT, and EXPLAIN (as a separate statement) for a heavy query."
        }
        Engine::SqlServer => {
            "Write T-SQL for SQL Server: TOP or OFFSET/FETCH instead of LIMIT, [brackets] only \
             when a name needs them, GETDATE()/SYSDATETIME(), ISNULL/COALESCE, STRING_AGG, \
             CROSS APPLY, no trailing semicolon required. Ideas worth proposing: joins that \
             follow the foreign keys listed in the schema (say which key), sargable filters \
             (no function around an indexed column), EXISTS instead of IN or DISTINCT, window \
             functions, CTEs, naming columns instead of SELECT *, a missing index (as a \
             separate CREATE INDEX statement, with INCLUDE when it helps), OUTPUT, \
             TRY_CONVERT, and SET STATISTICS IO ON for a heavy query."
        }
        Engine::Redis => {
            "The window runs Redis commands, one per line (not Lua, not redis-cli flags). The \
             schema lists existing keys with their type. Ideas worth proposing: the command \
             that fits the type of the key (HGETALL for a hash, LRANGE for a list, SMEMBERS, \
             ZRANGE ... WITHSCORES, XRANGE), SCAN ... MATCH ... COUNT instead of KEYS, TTL / \
             EXPIRE / SET ... EX for keys that should expire, MGET / HMGET instead of many \
             GETs, TYPE / OBJECT ENCODING / MEMORY USAGE to inspect a key, UNLINK instead of \
             DEL for big keys, and a warning before FLUSHDB, FLUSHALL or KEYS * on a big \
             database. A replacement must be a single command line."
        }
        Engine::MongoDb => {
            "The window runs mongosh-style calls with literal values only (no variables, loops \
             or functions): db.collection.find({...}).sort({...}).limit(n).skip(n)\
             .projection({...}).hint(...), findOne, countDocuments, distinct, aggregate([...]), \
             getIndexes, insertOne/insertMany, updateOne/updateMany, deleteOne/deleteMany, \
             createIndex, plus `use db` and `show collections`. Values accept ObjectId(\"...\"), \
             ISODate(\"...\"), NumberLong(...), /regex/i. The schema lists collections with \
             fields sampled from documents. Ideas worth proposing: query operators that fit \
             the field type ($gte/$lte ranges on numbers and dates, $in, $ne, $exists, $regex, \
             $elemMatch on arrays, $or), a projection instead of whole documents, sort with \
             limit, an aggregate pipeline ($match first, $group, $lookup to join another \
             collection through a field that looks like its key, $unwind, $project, $sort, \
             $limit), countDocuments instead of find().count(), an index for the filter and \
             sort (as a separate db.collection.createIndex call), .explain(), and a warning \
             for updateMany or deleteMany with an empty filter."
        }
        Engine::Elasticsearch => {
            "The window runs requests in Kibana console syntax: a line `METHOD /path` followed \
             by an optional JSON body. The schema lists indices with their mapped fields and \
             types. Ideas worth proposing: term/terms on keyword fields and match on text \
             fields (say which one fits the field type), bool with filter for exact \
             conditions (cached, no scoring) and must for full text, range on dates and \
             numbers (with date math such as now-7d/d), aggregations (terms, date_histogram, \
             avg/sum, cardinality) with \"size\": 0, _source filtering, sort with size and \
             from or search_after, highlight, _count instead of a search to count, \
             track_total_hits, GET /index/_mapping to check a field, _explain / profile for a \
             slow search, and a warning for _delete_by_query or DELETE on an index. Keep the \
             JSON body valid and pretty printed."
        }
    }
}

/// Instruções para o modelo. `language` é o idioma da interface, em
/// inglês ("Brazilian Portuguese").
pub fn system_prompt(engine: Engine, language: &str) -> String {
    format!(
        "You are the query assistant built into PostPilot, a database client. The user is \
         writing a {engine} query in an editor and you look over their shoulder: read the \
         statement, the cursor position ({CURSOR}) and the schema, and propose what they could \
         do next or do better. The statement may be unfinished because they are still typing.\n\
         \n\
         {rules}\n\
         \n\
         Answer with ONE JSON object and nothing else (no code fence, no text around it):\n\
         {{\"suggestions\":[{{\"kind\":\"...\",\"title\":\"...\",\"detail\":\"...\",\
         \"action\":\"replace\",\"code\":\"...\"}}]}}\n\
         \n\
         - kind: one of {kinds}. join = relate another table/collection; filter = a condition \
         or operator; columns = which fields to return; aggregate = group, count, sum; sort = \
         order, limit, paging; performance = make it faster; index = create or use an index; \
         fix = a syntax error, a wrong or misspelled name, a type mismatch, a logic slip; \
         safety = a risk (UPDATE or DELETE without WHERE, dropping, locking); complete = \
         finish the unfinished statement the most likely way; idiom = a cleaner or more \
         idiomatic way to write the same thing.\n\
         - title: what to do, in the imperative, at most 50 characters, naming the table, \
         column or operator involved.\n\
         - detail: one or two short sentences saying why it helps, with the concrete names.\n\
         - action \"replace\": code is the WHOLE statement rewritten with this one suggestion \
         applied, ready to run, keeping what the user already wrote and their style. action \
         \"insert\": code is a separate statement to add after the user's (an index, an \
         EXPLAIN, a follow-up query). action \"none\": advice only, code is \"\".\n\
         - Never put the {CURSOR} mark in code.\n\
         - From 0 to 6 suggestions, the most useful first, each one a different idea. If the \
         statement has an error or is unfinished, put the fix or the completion first. An \
         empty list is fine when there is nothing worth saying; do not pad.\n\
         - Use only tables, columns, fields, keys and indexes that are in the schema or in \
         the statement. Never invent names. If the schema is missing what you would need, \
         leave that idea out.\n\
         - Do not propose anything that changes or deletes data or structure unless the user \
         is already writing such a statement; creating an index may be proposed as an \
         \"insert\".\n\
         - Write title and detail in {language}. Keep identifiers and code as they are.",
        engine = engine_name(engine),
        rules = engine_rules(engine),
        kinds = KINDS.join(", "),
    )
}

/// As palavras do comando, em minúsculas, para achar as tabelas citadas.
fn words(statement: &str, engine: Engine) -> BTreeSet<String> {
    let separator = |c: char| match engine {
        // As chaves do Redis têm `:`, `-`, `.` e `/` no nome.
        Engine::Redis => c.is_whitespace() || c == '"' || c == '\'',
        // Um índice do Elasticsearch pode ter `-` e `.`.
        Engine::Elasticsearch => !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')),
        _ => !(c.is_alphanumeric() || c == '_' || c == '$' || c == '#'),
    };
    statement
        .split(separator)
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn kind_name(engine: Engine, relation: &Relation) -> &'static str {
    match (engine, relation.kind) {
        (Engine::MongoDb, RelationKind::View) => "view",
        (Engine::MongoDb, _) => "collection",
        (Engine::Elasticsearch, RelationKind::View) => "alias or data stream",
        (Engine::Elasticsearch, _) => "index",
        (Engine::Redis, _) => "key",
        (_, RelationKind::View) => "view",
        (_, RelationKind::Table) => "table",
    }
}

fn qualified(schema: &str, name: &str) -> String {
    if schema.is_empty() {
        name.to_string()
    } else {
        format!("{schema}.{name}")
    }
}

fn describe(out: &mut String, engine: Engine, schema: &str, relation: &Relation, max: usize) {
    out.push_str(&format!(
        "{} {}",
        kind_name(engine, relation),
        qualified(schema, &relation.name)
    ));
    if engine == Engine::Redis {
        let ty = relation.columns.first().map_or("", |c| c.ty.as_str());
        out.push_str(&format!(" ({ty})\n"));
        return;
    }
    out.push_str(": ");
    let columns: Vec<String> = relation
        .columns
        .iter()
        .take(max)
        .map(|c| {
            if c.ty.is_empty() {
                c.name.clone()
            } else {
                format!("{} {}", c.name, c.ty)
            }
        })
        .collect();
    out.push_str(&columns.join(", "));
    if relation.columns.len() > max {
        out.push_str(&format!(", ... ({} more)", relation.columns.len() - max));
    }
    out.push('\n');
}

fn describe_keys(out: &mut String, table: &Table) {
    if let Some(pk) = &table.primary_key {
        out.push_str(&format!("  primary key ({})\n", pk.columns.join(", ")));
    }
    for fk in &table.foreign_keys {
        out.push_str(&format!(
            "  foreign key ({}) references {} ({})\n",
            fk.columns.join(", "),
            qualified(&fk.ref_schema, &fk.ref_table),
            fk.ref_columns.join(", ")
        ));
    }
    for unique in &table.uniques {
        out.push_str(&format!("  unique ({})\n", unique.columns.join(", ")));
    }
    for index in &table.indexes {
        out.push_str(&format!(
            "  {}index {} ({}){}\n",
            if index.unique { "unique " } else { "" },
            index.name,
            index.columns.join(", "),
            index
                .predicate
                .as_ref()
                .map_or(String::new(), |p| format!(" where {p}"))
        ));
    }
}

/// O que o catálogo sabe e importa para este comando: as tabelas (ou
/// coleções, índices, chaves) citadas, com colunas, chaves e índices; as
/// vizinhas por chave estrangeira; e os nomes das outras. `model` traz as
/// chaves e os índices (só nos bancos relacionais).
pub fn context(catalog: &Catalog, model: Option<&Model>, statement: &str) -> String {
    let engine = catalog.engine;
    let words = words(statement, engine);
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let table_of = |schema: &str, name: &str| {
        model?
            .tables
            .iter()
            .find(|t| same(&t.schema, schema) && same(&t.name, name))
    };
    // Schemas do usuário primeiro; o `pg_catalog` não interessa.
    let schemas: Vec<_> = catalog
        .schemas
        .iter()
        .filter(|s| s.name != "pg_catalog" && !s.relations.is_empty())
        .collect();
    let mut cited: Vec<(&str, &Relation)> = Vec::new();
    for schema in &schemas {
        for relation in &schema.relations {
            if words.contains(&relation.name.to_lowercase()) {
                cited.push((&schema.name, relation));
            }
        }
    }
    // As vizinhas: para onde as chaves estrangeiras das citadas apontam e
    // quem aponta para elas.
    let mut neighbours: Vec<(&str, &Relation)> = Vec::new();
    if let Some(model) = model {
        let is_cited = |schema: &str, name: &str| {
            cited
                .iter()
                .any(|(s, r)| same(s, schema) && same(&r.name, name))
        };
        let mut wanted: Vec<(String, String)> = Vec::new();
        for table in &model.tables {
            let table_cited = is_cited(&table.schema, &table.name);
            for fk in &table.foreign_keys {
                let target_cited = is_cited(&fk.ref_schema, &fk.ref_table);
                if table_cited && !target_cited {
                    wanted.push((fk.ref_schema.clone(), fk.ref_table.clone()));
                } else if target_cited && !table_cited {
                    wanted.push((table.schema.clone(), table.name.clone()));
                }
            }
        }
        for schema in &schemas {
            for relation in &schema.relations {
                if wanted
                    .iter()
                    .any(|(s, n)| same(s, &schema.name) && same(n, &relation.name))
                    && !neighbours
                        .iter()
                        .any(|(s, r)| *s == schema.name && r.name == relation.name)
                {
                    neighbours.push((&schema.name, relation));
                }
            }
        }
    }

    let mut out = String::new();
    let section = |out: &mut String, title: &str, list: &[(&str, &Relation)], max: usize| {
        if list.is_empty() {
            return;
        }
        out.push_str(title);
        out.push('\n');
        for (schema, relation) in list {
            if out.len() > MAX_CONTEXT {
                out.push_str("[schema truncated]\n");
                return;
            }
            describe(out, engine, schema, relation, max);
            if let Some(table) = table_of(schema, &relation.name) {
                describe_keys(out, table);
            }
        }
        out.push('\n');
    };
    section(&mut out, "Used in the statement:", &cited, MAX_COLUMNS);
    section(
        &mut out,
        "Related by foreign key:",
        &neighbours,
        MAX_NEIGHBOUR_COLUMNS,
    );
    let listed = |schema: &str, relation: &Relation| {
        cited
            .iter()
            .chain(&neighbours)
            .any(|(s, r)| *s == schema && r.name == relation.name)
    };
    let others: Vec<(&str, &Relation)> = schemas
        .iter()
        .flat_map(|s| s.relations.iter().map(|r| (s.name.as_str(), r)))
        .filter(|(s, r)| !listed(s, r))
        .collect();
    // As primeiras com as colunas (num banco pequeno, todas): a IA pode
    // propor por onde começar ou com quem relacionar. O resto, só o nome.
    let detailed = if engine == Engine::Redis {
        0
    } else {
        others.len().min(MAX_DETAILED)
    };
    let title = match engine {
        Engine::Redis => "Other keys:",
        Engine::MongoDb => "Other collections:",
        Engine::Elasticsearch => "Other indices:",
        _ => "Other tables and views:",
    };
    section(&mut out, title, &others[..detailed], MAX_NEIGHBOUR_COLUMNS);
    let names: Vec<String> = others[detailed..]
        .iter()
        .take(MAX_NAMES)
        .map(|(schema, r)| match engine {
            Engine::Redis => format!(
                "{} ({})",
                r.name,
                r.columns.first().map_or("", |c| c.ty.as_str())
            ),
            _ => qualified(schema, &r.name),
        })
        .collect();
    if !names.is_empty() {
        out.push_str(if detailed == 0 { title } else { "More names:" });
        out.push(' ');
        out.push_str(&names.join(", "));
        let rest = others.len() - detailed;
        if rest > MAX_NAMES {
            out.push_str(&format!(", ... ({} more)", rest - MAX_NAMES));
        }
        out.push('\n');
    }
    if out.trim().is_empty() {
        out.push_str("(the schema has not been read yet)\n");
    }
    out
}

/// A pergunta: o motor, o banco, o catálogo e o comando com a marca do
/// cursor. `cursor` é o byte do cursor dentro de `statement`, se ele
/// estiver lá.
pub fn user_prompt(
    engine: Engine,
    database: &str,
    context: &str,
    statement: &str,
    cursor: Option<usize>,
) -> String {
    let mut text = String::with_capacity(statement.len() + 4);
    match cursor.filter(|c| *c <= statement.len() && statement.is_char_boundary(*c)) {
        Some(c) => {
            text.push_str(&statement[..c]);
            text.push_str(CURSOR);
            text.push_str(&statement[c..]);
        }
        None => text.push_str(statement),
    }
    let cut: String = text.chars().take(MAX_STATEMENT).collect();
    format!(
        "Engine: {}\nDatabase: {database}\n\nSchema:\n{context}\nStatement ({CURSOR} marks the \
         cursor):\n{cut}\n",
        engine_name(engine)
    )
}

/// O objeto JSON dentro da resposta: alguns modelos põem cerca de código,
/// raciocínio em `<think>` ou uma frase antes.
fn json_of(answer: &str) -> Option<Value> {
    let mut text = answer.trim();
    if let Some(end) = text.rfind("</think>") {
        text = text[end + "</think>".len()..].trim();
    }
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Some(value);
    }
    // Do primeiro `{` ao último `}`; se não fechar, tenta os começos
    // seguintes (uma frase com chaves antes do JSON).
    let end = text.rfind('}')?;
    let mut from = 0;
    while let Some(start) = text[from..=end].find('{').map(|i| from + i) {
        if let Ok(value) = serde_json::from_str::<Value>(&text[start..=end]) {
            return Some(value);
        }
        from = start + 1;
    }
    None
}

/// Setas e sinais que a fonte da interface não tem, como texto simples.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '→' | '⟶' | '➜' | '➔' => out.push_str("->"),
            '←' => out.push_str("<-"),
            '⇒' => out.push_str("=>"),
            '↔' => out.push_str("<->"),
            '≥' => out.push_str(">="),
            '≤' => out.push_str("<="),
            '≠' => out.push_str("<>"),
            c => out.push(c),
        }
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// Tira a cerca de código que alguns modelos põem em volta do `code`.
fn strip_fence(code: &str) -> &str {
    let code = code.trim();
    let Some(rest) = code.strip_prefix("```") else {
        return code;
    };
    let rest = rest.split_once('\n').map_or("", |(_, body)| body);
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Lê as sugestões da resposta. `statement` é o comando perguntado: uma
/// troca que não muda nada fica de fora. Erro quando a resposta não é o
/// JSON pedido.
pub fn parse(answer: &str, statement: &str) -> Result<Vec<Suggestion>, String> {
    let value = json_of(answer).ok_or_else(|| {
        format!(
            "the answer is not the JSON that was asked for: {}",
            clip(answer, 160)
        )
    })?;
    let list = match &value {
        Value::Array(list) => list,
        Value::Object(map) => map
            .get("suggestions")
            .and_then(Value::as_array)
            .ok_or("the answer has no \"suggestions\" list")?,
        _ => return Err("the answer has no \"suggestions\" list".into()),
    };
    let squeeze = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let asked = squeeze(statement.trim().trim_end_matches(';'));
    let mut out: Vec<Suggestion> = Vec::new();
    for item in list {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").trim();
        let title = clip(&plain(text("title")), MAX_TITLE);
        if title.is_empty() {
            continue;
        }
        let code = strip_fence(text("code")).replace(CURSOR, "");
        let mut action = match text("action").to_ascii_lowercase().as_str() {
            "replace" => Action::Replace,
            "insert" | "add" | "append" => Action::Insert,
            _ => Action::None,
        };
        if code.trim().is_empty() {
            action = Action::None;
        }
        if action == Action::Replace && squeeze(code.trim().trim_end_matches(';')) == asked {
            continue;
        }
        if out.iter().any(|s| s.title == title) {
            continue;
        }
        let kind = text("kind").to_ascii_lowercase();
        out.push(Suggestion {
            kind: if KINDS.contains(&kind.as_str()) {
                kind
            } else {
                "other".into()
            },
            title,
            detail: clip(&plain(text("detail")), MAX_DETAIL),
            action,
            code: if action == Action::None {
                String::new()
            } else {
                code
            },
        });
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_core::catalog::{Column, Schema};
    use pgstudio_core::schema::{ForeignKey, Index, Key};
    use std::sync::Arc;

    fn relation(name: &str, columns: &[(&str, &str)]) -> Relation {
        Relation {
            name: name.into(),
            kind: RelationKind::Table,
            columns: columns
                .iter()
                .map(|(name, ty)| Column {
                    name: (*name).into(),
                    ty: (*ty).into(),
                })
                .collect(),
        }
    }

    fn shop() -> (Catalog, Model) {
        let catalog = Catalog {
            engine: Engine::PostgreSql,
            database: "loja".into(),
            search_path: vec!["public".into()],
            schemas: vec![Schema {
                name: "public".into(),
                relations: vec![
                    relation("clientes", &[("id", "integer"), ("nome", "text")]),
                    relation(
                        "pedidos",
                        &[
                            ("id", "integer"),
                            ("cliente_id", "integer"),
                            ("total", "numeric(10,2)"),
                        ],
                    ),
                    relation("itens", &[("pedido_id", "integer"), ("produto", "text")]),
                    relation("auditoria", &[("id", "bigint")]),
                ],
                functions: Vec::new(),
            }],
        };
        let fk = |column: &str, table: &str| ForeignKey {
            name: format!("fk_{column}"),
            columns: vec![column.into()],
            ref_schema: "public".into(),
            ref_table: table.into(),
            ref_columns: vec!["id".into()],
            ..Default::default()
        };
        let table = |name: &str, fks: Vec<ForeignKey>, indexes: Vec<Index>| {
            Arc::new(Table {
                schema: "public".into(),
                name: name.into(),
                primary_key: Some(Key {
                    name: format!("{name}_pkey"),
                    columns: vec!["id".into()],
                }),
                foreign_keys: fks,
                indexes,
                ..Default::default()
            })
        };
        let model = Model {
            tables: vec![
                table("clientes", vec![], vec![]),
                table(
                    "pedidos",
                    vec![fk("cliente_id", "clientes")],
                    vec![Index {
                        name: "pedidos_cliente_idx".into(),
                        columns: vec!["cliente_id".into()],
                        ..Default::default()
                    }],
                ),
                table("itens", vec![fk("pedido_id", "pedidos")], vec![]),
            ],
            ..Default::default()
        };
        (catalog, model)
    }

    #[test]
    fn context_has_the_cited_tables_their_keys_and_the_neighbours() {
        let (catalog, model) = shop();
        let text = context(
            &catalog,
            Some(&model),
            "select * from Pedidos p where p.total > 10",
        );
        let used = text.split("Related by foreign key:").next().unwrap();
        assert!(
            used.contains(
                "table public.pedidos: id integer, cliente_id integer, total numeric(10,2)"
            ),
            "{text}"
        );
        assert!(
            used.contains("foreign key (cliente_id) references public.clientes (id)"),
            "{text}"
        );
        assert!(
            used.contains("index pedidos_cliente_idx (cliente_id)"),
            "{text}"
        );
        assert!(!used.contains("clientes: id"), "{text}");
        let related = text.split("Related by foreign key:").nth(1).unwrap();
        // Para onde aponta e quem aponta para ela.
        assert!(
            related.contains("table public.clientes: id integer, nome text"),
            "{text}"
        );
        assert!(related.contains("table public.itens:"), "{text}");
        assert!(
            related.contains("foreign key (pedido_id) references public.pedidos (id)"),
            "{text}"
        );
        assert!(
            text.contains("Other tables and views:\ntable public.auditoria: id bigint"),
            "{text}"
        );
    }

    #[test]
    fn context_without_a_cited_table_lists_the_columns_of_all() {
        let (catalog, _) = shop();
        let text = context(&catalog, None, "select ");
        assert!(text.contains("Other tables and views:"), "{text}");
        assert!(
            text.contains("table public.itens: pedido_id integer, produto text"),
            "{text}"
        );
        let empty = context(&Catalog::default(), None, "select 1");
        assert!(empty.contains("has not been read"), "{empty}");
    }

    #[test]
    fn a_big_database_sends_only_the_names_of_the_rest() {
        let (mut catalog, _) = shop();
        for i in 0..300 {
            catalog.schemas[0]
                .relations
                .push(relation(&format!("t{i:03}"), &[("id", "integer")]));
        }
        let text = context(&catalog, None, "select * from pedidos");
        assert!(text.contains("table public.t008: id integer"), "{text}");
        assert!(!text.contains("table public.t009:"), "{text}");
        assert!(
            text.contains("More names: public.t009, public.t010"),
            "{text}"
        );
        assert!(text.contains("public.t208, ... (91 more)"), "{text}");
        assert!(text.len() < MAX_CONTEXT, "{}", text.len());
    }

    #[test]
    fn context_of_the_other_engines_uses_their_names() {
        let (mut catalog, _) = shop();
        catalog.schemas[0].name = String::new();
        catalog.engine = Engine::MongoDb;
        let text = context(&catalog, None, "db.pedidos.find({})");
        assert!(text.contains("collection pedidos: id integer"), "{text}");
        assert!(
            text.contains("Other collections:\ncollection clientes: id integer, nome text"),
            "{text}"
        );

        catalog.engine = Engine::Elasticsearch;
        let text = context(&catalog, None, "GET /pedidos/_search");
        assert!(text.contains("index pedidos: id integer"), "{text}");

        catalog.engine = Engine::Redis;
        catalog.schemas[0].relations = vec![
            relation("user:1", &[("value", "hash")]),
            relation("fila", &[("value", "list")]),
        ];
        let text = context(&catalog, None, "GET user:1");
        assert!(text.contains("key user:1 (hash)"), "{text}");
        assert!(text.contains("Other keys: fila (list)"), "{text}");
    }

    #[test]
    fn prompt_marks_the_cursor_and_names_the_engine() {
        let prompt = user_prompt(
            Engine::SqlServer,
            "Vendas",
            "ctx\n",
            "SELECT * FROM T",
            Some(9),
        );
        assert!(prompt.contains("Engine: Microsoft SQL Server"), "{prompt}");
        assert!(prompt.contains("Database: Vendas"), "{prompt}");
        assert!(prompt.contains("SELECT * ‸FROM T"), "{prompt}");
        let system = system_prompt(Engine::MongoDb, "Brazilian Portuguese");
        assert!(system.contains("$gte"), "{system}");
        assert!(system.contains("Brazilian Portuguese"), "{system}");
        for engine in [
            Engine::PostgreSql,
            Engine::SqlServer,
            Engine::Redis,
            Engine::MongoDb,
            Engine::Elasticsearch,
        ] {
            assert!(system_prompt(engine, "English").contains("\"suggestions\""));
        }
    }

    #[test]
    fn reads_the_answer_even_with_noise_around_it() {
        let answer = "<think>hmm {x}</think>\nAqui está:\n```json\n{\"suggestions\":[\
            {\"kind\":\"join\",\"title\":\"Juntar com clientes\",\"detail\":\"Pela chave cliente_id → clientes.id.\",\
             \"action\":\"replace\",\"code\":\"select * from pedidos p join clientes c on c.id = p.cliente_id‸\"},\
            {\"kind\":\"weird\",\"title\":\"Criar índice\",\"detail\":\"\",\"action\":\"insert\",\
             \"code\":\"```sql\\ncreate index on pedidos (total)\\n```\"},\
            {\"kind\":\"safety\",\"title\":\"Cuidado\",\"detail\":\"d\",\"action\":\"replace\",\"code\":\"\"},\
            {\"kind\":\"idiom\",\"title\":\"Igual\",\"action\":\"replace\",\"code\":\"select *  from pedidos;\"},\
            {\"kind\":\"fix\",\"title\":\"\",\"code\":\"x\"}\
            ]}\n```";
        let got = parse(answer, "select * from pedidos").unwrap();
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].kind, "join");
        assert_eq!(got[0].action, Action::Replace);
        assert_eq!(got[0].detail, "Pela chave cliente_id -> clientes.id.");
        assert_eq!(
            got[0].code,
            "select * from pedidos p join clientes c on c.id = p.cliente_id"
        );
        assert_eq!(got[1].kind, "other");
        assert_eq!(got[1].action, Action::Insert);
        assert_eq!(got[1].code, "create index on pedidos (total)");
        // Sem código, é só um conselho.
        assert_eq!(got[2].action, Action::None);
        assert_eq!(got[2].code, "");
    }

    #[test]
    fn empty_and_broken_answers() {
        assert_eq!(parse("{\"suggestions\":[]}", "x").unwrap(), Vec::new());
        assert_eq!(parse("[]", "x").unwrap(), Vec::new());
        assert!(parse("Sorry, I cannot help.", "x").is_err());
        assert!(parse("{\"other\":1}", "x").is_err());
        let many: Vec<String> = (0..20)
            .map(|i| format!("{{\"title\":\"t{i}\",\"kind\":\"filter\"}}"))
            .collect();
        let got = parse(&format!("{{\"suggestions\":[{}]}}", many.join(",")), "x").unwrap();
        assert_eq!(got.len(), MAX_SUGGESTIONS);
    }
}
