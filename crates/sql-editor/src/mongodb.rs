//! IntelliSense do MongoDB: coleções depois de `db.`, métodos depois da
//! coleção, modificadores do cursor depois de `find(...)`, operadores
//! (`$gte`, `$set`, `$group`...) e os campos da coleção onde cabe um.
//!
//! As coleções e os campos vêm do catálogo em cache: cada coleção é uma
//! "tabela" do schema sem nome e os campos (lidos de uma amostra de
//! documentos) são as colunas dela.

use pgstudio_core::catalog::{Catalog, Relation, RelationKind};

use crate::complete::{Completion, Kind, Suggestion};
use crate::lexer::{Token, TokenKind};

/// Métodos de uma coleção, com os argumentos como na documentação.
pub const COLLECTION_METHODS: &[(&str, &str)] = &[
    ("aggregate", "([ { $match: {...} }, ... ])"),
    ("countDocuments", "({ filtro })"),
    ("createIndex", "({ campo: 1 }, { opções })"),
    ("deleteMany", "({ filtro })"),
    ("deleteOne", "({ filtro })"),
    ("distinct", "(\"campo\", { filtro })"),
    ("drop", "()"),
    ("dropIndex", "(\"nome\")"),
    ("dropIndexes", "()"),
    ("estimatedDocumentCount", "()"),
    ("find", "({ filtro }, { projeção })"),
    ("findOne", "({ filtro }, { projeção })"),
    ("getIndexes", "()"),
    ("insertMany", "([ { documento }, ... ])"),
    ("insertOne", "({ documento })"),
    ("replaceOne", "({ filtro }, { documento }, { upsert })"),
    ("stats", "()"),
    ("updateMany", "({ filtro }, { $set: {...} }, { upsert })"),
    ("updateOne", "({ filtro }, { $set: {...} }, { upsert })"),
];

/// Métodos do `db`.
pub const DATABASE_METHODS: &[(&str, &str)] = &[
    ("adminCommand", "({ comando: 1 })"),
    ("createCollection", "(\"nome\", { opções })"),
    ("dropDatabase", "()"),
    ("getCollection", "(\"nome\")"),
    ("getCollectionNames", "()"),
    ("runCommand", "({ comando: 1 })"),
    ("serverStatus", "()"),
    ("stats", "()"),
    ("version", "()"),
];

/// O que vem depois de um `find(...)`.
pub const CURSOR_METHODS: &[(&str, &str)] = &[
    ("collation", "({ locale: \"pt\" })"),
    ("count", "()"),
    ("explain", "()"),
    ("hint", "({ campo: 1 })"),
    ("limit", "(n)"),
    ("maxTimeMS", "(ms)"),
    ("projection", "({ campo: 1 })"),
    ("skip", "(n)"),
    ("sort", "({ campo: -1 })"),
];

/// Operadores de filtro.
pub const QUERY_OPERATORS: &[&str] = &[
    "$all",
    "$and",
    "$elemMatch",
    "$eq",
    "$exists",
    "$expr",
    "$gt",
    "$gte",
    "$in",
    "$lt",
    "$lte",
    "$mod",
    "$ne",
    "$nin",
    "$nor",
    "$not",
    "$options",
    "$or",
    "$regex",
    "$size",
    "$text",
    "$type",
];

/// Operadores de um `update`.
pub const UPDATE_OPERATORS: &[&str] = &[
    "$addToSet",
    "$currentDate",
    "$each",
    "$inc",
    "$max",
    "$min",
    "$mul",
    "$pop",
    "$pull",
    "$push",
    "$rename",
    "$set",
    "$setOnInsert",
    "$unset",
];

/// Estágios de um `aggregate`.
pub const STAGES: &[&str] = &[
    "$addFields",
    "$bucket",
    "$count",
    "$facet",
    "$group",
    "$limit",
    "$lookup",
    "$match",
    "$merge",
    "$out",
    "$project",
    "$replaceRoot",
    "$sample",
    "$set",
    "$skip",
    "$sort",
    "$sortByCount",
    "$unionWith",
    "$unset",
    "$unwind",
];

/// Operadores de expressão de um `aggregate`.
pub const EXPRESSIONS: &[&str] = &[
    "$add",
    "$arrayElemAt",
    "$avg",
    "$concat",
    "$cond",
    "$dateToString",
    "$dayOfMonth",
    "$divide",
    "$filter",
    "$first",
    "$ifNull",
    "$last",
    "$map",
    "$max",
    "$min",
    "$month",
    "$multiply",
    "$push",
    "$round",
    "$subtract",
    "$sum",
    "$toDate",
    "$toDecimal",
    "$toInt",
    "$toLower",
    "$toString",
    "$toUpper",
    "$year",
];

/// Valores que não são JSON puro.
const VALUES: &[(&str, &str)] = &[
    ("ISODate", "(\"2026-01-31T00:00:00Z\")"),
    ("NumberDecimal", "(\"0.00\")"),
    ("NumberLong", "(0)"),
    ("ObjectId", "(\"24 dígitos hexadecimais\")"),
    ("UUID", "(\"...\")"),
    ("false", ""),
    ("null", ""),
    ("true", ""),
];

/// Onde o cursor está dentro de uma chamada `db.colecao.metodo(...)`.
struct Call<'a> {
    collection: Option<&'a str>,
    method: &'a str,
    /// Qual argumento (do zero).
    argument: usize,
    /// O parêntese, chave ou colchete aberto mais de dentro.
    innermost: &'a str,
    /// Quantas `{` abertas dentro da chamada.
    braces: usize,
}

/// O nome de uma coleção escrito depois de `db.` ou em
/// `getCollection("...")`.
fn unquote(text: &str) -> &str {
    text.trim_matches(|c| c == '"' || c == '\'' || c == '`')
}

/// A coleção de uma cadeia que termina em `tokens[..end]`, logo antes do
/// `.metodo`: `db.nome` ou `db.getCollection("nome")`.
fn collection_before<'a>(text: &'a str, tokens: &[&'a Token], end: usize) -> Option<&'a str> {
    let word = |i: usize| tokens.get(i).map(|t| &text[t.range.clone()]);
    let last = end.checked_sub(1)?;
    if word(last) == Some(")") {
        // db . getCollection ( "nome" )
        let name = tokens.get(last.checked_sub(1)?)?;
        if name.kind == TokenKind::String
            && word(last.checked_sub(2)?) == Some("(")
            && word(last.checked_sub(3)?) == Some("getCollection")
        {
            return Some(unquote(&text[name.range.clone()]));
        }
        return None;
    }
    if word(last.checked_sub(1)?) == Some(".") && word(last.checked_sub(2)?) == Some("db") {
        return word(last);
    }
    None
}

/// A chamada aberta em volta do fim de `tokens`.
fn enclosing<'a>(text: &'a str, tokens: &[&'a Token]) -> Option<Call<'a>> {
    let word = |i: usize| &text[tokens[i].range.clone()];
    let mut depth = 0usize;
    let mut argument = 0;
    let mut braces = 0;
    let mut innermost = None;
    for i in (0..tokens.len()).rev() {
        if tokens[i].kind != TokenKind::Punctuation {
            continue;
        }
        match word(i) {
            ")" | "}" | "]" => depth += 1,
            open @ ("(" | "{" | "[") => {
                if depth > 0 {
                    depth -= 1;
                    continue;
                }
                innermost.get_or_insert(open);
                if open == "{" {
                    braces += 1;
                }
                if open != "(" {
                    argument = 0;
                    continue;
                }
                let method = i.checked_sub(1)?;
                if !matches!(
                    tokens[method].kind,
                    TokenKind::Identifier | TokenKind::Keyword
                ) {
                    return None;
                }
                // Um valor como `ObjectId(` não é a chamada: segue para
                // fora.
                if method == 0 || word(method - 1) != "." {
                    argument = 0;
                    continue;
                }
                // Um modificador (`.sort(`) pertence à coleção do começo
                // da cadeia.
                let mut start = method - 1;
                let mut collection = collection_before(text, tokens, start);
                while collection.is_none() && start > 0 && word(start - 1) == ")" {
                    let mut d = 0usize;
                    let mut j = start - 1;
                    loop {
                        match word(j) {
                            ")" => d += 1,
                            "(" => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j = j.checked_sub(1)?;
                    }
                    // `j` é o `(`; antes dele o método e o ponto.
                    start = j.checked_sub(2)?;
                    if word(start) != "." {
                        break;
                    }
                    collection = collection_before(text, tokens, start);
                }
                return Some(Call {
                    collection,
                    method: word(method),
                    argument,
                    innermost: innermost.unwrap_or("("),
                    braces,
                });
            }
            // As vírgulas desde o último que abriu: no fim, as da chamada.
            "," if depth == 0 => argument += 1,
            _ => {}
        }
    }
    None
}

fn relation<'a>(catalog: Option<&'a Catalog>, name: &str) -> Option<&'a Relation> {
    catalog?
        .schemas
        .iter()
        .flat_map(|s| &s.relations)
        .find(|r| r.name == name)
}

/// Um nome que pode ser escrito sem aspas depois de `db.` ou como chave.
fn plain(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c == '$' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}

fn push_all(items: &mut Vec<Suggestion>, prefix: &str, names: &[(&str, &str)], kind: Kind) {
    for (name, detail) in names {
        if name.starts_with(prefix) {
            items.push(Suggestion {
                label: name.to_string(),
                insert: name.to_string(),
                kind,
                detail: detail.to_string(),
            });
        }
    }
}

fn push_operators(items: &mut Vec<Suggestion>, prefix: &str, names: &[&str], detail: &str) {
    for name in names {
        if name.starts_with(prefix) && !items.iter().any(|i| i.label == *name) {
            items.push(Suggestion {
                label: name.to_string(),
                insert: name.to_string(),
                kind: Kind::Keyword,
                detail: detail.to_string(),
            });
        }
    }
}

/// Os campos de `relation` que começam com `prefix`. `quote` põe aspas
/// nos que têm ponto (`"cliente.cidade"`), como uma chave pede.
fn push_fields(items: &mut Vec<Suggestion>, prefix: &str, relation: &Relation, quote: bool) {
    for column in &relation.columns {
        if column.name.starts_with(prefix) {
            items.push(Suggestion {
                label: column.name.clone(),
                insert: if quote && !plain(&column.name) {
                    format!("\"{}\"", column.name)
                } else {
                    column.name.clone()
                },
                kind: Kind::Column,
                detail: column.ty.clone(),
            });
        }
    }
}

/// Sugestões para o cursor em `at` (bytes) num texto de consultas do
/// MongoDB.
pub fn complete(text: &str, tokens: &[Token], at: usize, catalog: Option<&Catalog>) -> Completion {
    let none = Completion {
        replace: at..at,
        ..Completion::default()
    };
    let i = tokens.partition_point(|t| t.range.end < at);
    let mut start = at;
    let mut in_string = false;
    if let Some(t) = tokens.get(i).filter(|t| t.range.start < at) {
        match t.kind {
            TokenKind::Comment | TokenKind::Number => return none,
            // Dentro de `"$campo"`: uma referência a um campo.
            TokenKind::String => {
                let written = &text[t.range.start..at];
                let inside = written.get(1..).unwrap_or_default();
                if !written.starts_with(['"', '\'']) || !inside.starts_with('$') {
                    return none;
                }
                // O texto fechado antes do cursor não é mais o texto.
                if t.range.end == at && written.len() > 1 && written.ends_with(&written[..1]) {
                    return none;
                }
                start = t.range.start + 2;
                in_string = true;
            }
            TokenKind::Identifier | TokenKind::Keyword | TokenKind::Parameter => {
                start = t.range.start
            }
            _ => {}
        }
    }
    let prefix = &text[start..at];
    let before: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.range.end <= start)
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();
    let word = |i: usize| before.get(i).map(|t| &text[t.range.clone()]);
    let last = before.len().checked_sub(1);
    let previous = last.and_then(word);
    let call = enclosing(text, &before);
    let mut items = Vec::new();

    if in_string {
        let fields = call
            .as_ref()
            .and_then(|c| c.collection)
            .and_then(|c| relation(catalog, c));
        if let Some(fields) = fields {
            push_fields(&mut items, prefix, fields, false);
        }
    } else if prefix.starts_with('$') {
        match call.as_ref().map(|c| (c.method, c.argument)) {
            Some(("aggregate", _)) => {
                push_operators(&mut items, prefix, STAGES, "estágio");
                push_operators(&mut items, prefix, EXPRESSIONS, "expressão");
                push_operators(&mut items, prefix, QUERY_OPERATORS, "filtro");
            }
            Some(("updateOne" | "updateMany", 1)) => {
                push_operators(&mut items, prefix, UPDATE_OPERATORS, "atualização");
            }
            _ => push_operators(&mut items, prefix, QUERY_OPERATORS, "filtro"),
        }
    } else if previous == Some(".") {
        let last = last.unwrap_or_default();
        let owner = last.checked_sub(1).and_then(word);
        let chained = last >= 2 && word(last - 2) == Some(".");
        if owner == Some("db") && !chained {
            for r in catalog
                .into_iter()
                .flat_map(|c| &c.schemas)
                .flat_map(|s| &s.relations)
                .filter(|r| r.name.starts_with(prefix))
            {
                items.push(Suggestion {
                    label: r.name.clone(),
                    insert: if plain(&r.name) {
                        r.name.clone()
                    } else {
                        format!("getCollection(\"{}\")", r.name)
                    },
                    kind: match r.kind {
                        RelationKind::Table => Kind::Table,
                        RelationKind::View => Kind::View,
                    },
                    detail: String::new(),
                });
            }
            push_all(&mut items, prefix, DATABASE_METHODS, Kind::Function);
        } else if collection_before(text, &before, last).is_some() {
            push_all(&mut items, prefix, COLLECTION_METHODS, Kind::Function);
        } else if owner == Some(")") {
            push_all(&mut items, prefix, CURSOR_METHODS, Kind::Function);
        }
    } else if previous == Some("show") {
        push_all(
            &mut items,
            prefix,
            &[("collections", ""), ("dbs", "")],
            Kind::Keyword,
        );
    } else if previous == Some("use") {
        return none;
    } else if let Some(call) = &call {
        let key = call.innermost == "{" && matches!(previous, Some("{" | ","));
        let fields = call.collection.and_then(|c| relation(catalog, c));
        // As chaves de um documento novo ou de um estágio não são campos
        // para filtrar.
        let filters = match call.method {
            "insertOne" | "insertMany" | "createIndex" | "runCommand" | "adminCommand" => {
                call.method == "createIndex" && call.argument == 0
            }
            "aggregate" => call.braces >= 2,
            _ => true,
        };
        if key {
            if let (Some(fields), true) = (fields, filters) {
                push_fields(&mut items, prefix, fields, true);
            }
        } else if matches!(previous, Some(":" | "[" | "(" | ",")) && !prefix.is_empty() {
            push_all(&mut items, prefix, VALUES, Kind::Keyword);
        }
    } else {
        // Começo de um comando: nada antes na linha.
        let line = text[..start].rfind('\n').map_or(0, |i| i + 1);
        if before.last().is_none_or(|t| t.range.end <= line) || previous == Some(";") {
            push_all(
                &mut items,
                prefix,
                &[
                    ("db", "db.colecao.find({ ... })"),
                    ("show collections", ""),
                    ("show dbs", ""),
                    ("use", "use banco"),
                ],
                Kind::Keyword,
            );
        }
    }
    Completion {
        replace: start..at,
        prefix: prefix.to_string(),
        items,
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
            engine: Engine::MongoDb,
            database: "loja".into(),
            search_path: vec![String::new()],
            schemas: vec![Schema {
                name: String::new(),
                relations: vec![
                    relation(
                        "pedidos",
                        RelationKind::Table,
                        &[
                            ("_id", "objectId"),
                            ("cliente", "object"),
                            ("cliente.cidade", "string"),
                            ("pago", "bool"),
                            ("total", "decimal"),
                        ],
                    ),
                    relation("pedidos_pagos", RelationKind::View, &[]),
                    relation("log-2026", RelationKind::Table, &[("nivel", "string")]),
                ],
                functions: Vec::new(),
            }],
        }
    }

    /// As sugestões com o cursor no `|` de `text`.
    fn at(text: &str) -> (Vec<String>, Completion) {
        let pos = text.find('|').unwrap();
        let text = text.replace('|', "");
        let tokens = tokenize_in(&text, Dialect::MongoDb);
        let catalog = catalog();
        let c = complete(&text, &tokens, pos, Some(&catalog));
        (c.items.iter().map(|s| s.label.clone()).collect(), c)
    }

    fn labels(text: &str) -> Vec<String> {
        at(text).0
    }

    #[test]
    fn lists_are_sorted_and_unique() {
        for list in [COLLECTION_METHODS, DATABASE_METHODS, CURSOR_METHODS, VALUES] {
            assert!(list.windows(2).all(|w| w[0].0 < w[1].0), "{list:?}");
        }
        for list in [QUERY_OPERATORS, UPDATE_OPERATORS, STAGES, EXPRESSIONS] {
            assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
        }
    }

    #[test]
    fn a_command_starts_with_db_use_or_show() {
        assert_eq!(labels("|"), ["db", "show collections", "show dbs", "use"]);
        assert_eq!(
            labels("db.pedidos.find({})\nsh|"),
            ["show collections", "show dbs"]
        );
        assert_eq!(labels("show |"), ["collections", "dbs"]);
        assert_eq!(labels("show d|"), ["dbs"]);
        assert!(labels("use lo|").is_empty());
        assert!(labels("// db|").is_empty());
    }

    #[test]
    fn collections_and_database_methods_follow_db() {
        let (got, c) = at("db.|");
        assert_eq!(&got[..3], ["pedidos", "pedidos_pagos", "log-2026"]);
        assert!(got.contains(&"runCommand".to_string()));
        assert_eq!(c.items[0].kind, Kind::Table);
        assert_eq!(c.items[1].kind, Kind::View);
        // Um nome que não cabe depois do ponto entra pelo getCollection.
        assert_eq!(c.items[2].insert, "getCollection(\"log-2026\")");
        let (got, c) = at("db.ped|");
        assert_eq!(got, ["pedidos", "pedidos_pagos"]);
        assert_eq!(c.replace, 3..6);
        assert_eq!(labels("db.getC|"), ["getCollection", "getCollectionNames"]);
    }

    #[test]
    fn methods_follow_the_collection() {
        assert_eq!(labels("db.pedidos.fi|"), ["find", "findOne"]);
        assert_eq!(
            labels("db.getCollection('log-2026').de|"),
            ["deleteMany", "deleteOne"]
        );
        assert_eq!(labels("db.pedidos.|").len(), COLLECTION_METHODS.len());
        // Depois do find, os modificadores do cursor.
        assert_eq!(
            labels("db.pedidos.find({ pago: true }).s|"),
            ["skip", "sort"]
        );
        assert_eq!(
            labels("db.pedidos.find({})\n  .sort({ total: -1 })\n  .l|"),
            ["limit"]
        );
    }

    #[test]
    fn operators_depend_on_the_method() {
        assert_eq!(
            labels("db.pedidos.find({ total: { $gt| } })"),
            ["$gt", "$gte"]
        );
        assert_eq!(
            labels("db.pedidos.updateMany({ pago: false }, { $s| })"),
            ["$set", "$setOnInsert"]
        );
        // No filtro do update, os operadores de filtro.
        assert_eq!(
            labels("db.pedidos.updateMany({ total: { $l| } }, {})"),
            ["$lt", "$lte"]
        );
        let got = labels("db.pedidos.aggregate([{ $| }])");
        assert_eq!(got[0], "$addFields");
        assert!(got.contains(&"$sum".to_string()) && got.contains(&"$gte".to_string()));
        // `$set` é estágio e não repete.
        assert_eq!(got.iter().filter(|g| *g == "$set").count(), 1);
    }

    #[test]
    fn fields_come_from_the_collection() {
        let (got, c) = at("db.pedidos.find({ |");
        assert_eq!(got, ["_id", "cliente", "cliente.cidade", "pago", "total"]);
        assert_eq!(c.items[2].insert, "\"cliente.cidade\"");
        assert_eq!(c.items[4].detail, "decimal");
        assert_eq!(labels("db.pedidos.find({ pago: true, t|"), ["total"]);
        // Dentro de um valor de documento também (um `$or`, por exemplo).
        assert_eq!(labels("db.pedidos.find({ $or: [{ p| }] })"), ["pago"]);
        // No sort de um find, os campos da coleção do começo.
        assert_eq!(labels("db.pedidos.find({}).sort({ to|"), ["total"]);
        assert_eq!(
            labels("db.getCollection(\"log-2026\").find({ n|"),
            ["nivel"]
        );
        // Um documento novo não tem campos para sugerir.
        assert!(labels("db.pedidos.insertOne({ t|").is_empty());
        // Num aggregate, só dentro do estágio.
        assert!(labels("db.pedidos.aggregate([{ t|").is_empty());
        assert_eq!(
            labels("db.pedidos.aggregate([{ $match: { t| } }])"),
            ["total"]
        );
    }

    #[test]
    fn field_references_inside_a_string() {
        let (got, c) = at("db.pedidos.aggregate([{ $group: { _id: \"$cl|\" } }])");
        assert_eq!(got, ["cliente", "cliente.cidade"]);
        assert_eq!(c.items[1].insert, "cliente.cidade");
        assert_eq!(c.prefix, "cl");
        assert!(labels("db.pedidos.find({ nome: \"an|\" })").is_empty());
    }

    #[test]
    fn values_that_are_not_json() {
        assert_eq!(labels("db.pedidos.find({ _id: Ob| })"), ["ObjectId"]);
        assert_eq!(labels("db.pedidos.find({ pago: t| })"), ["true"]);
        assert!(labels("db.pedidos.find({ pago: | })").is_empty());
        // Dentro do valor, a chamada continua sendo o find.
        assert!(labels("db.pedidos.find({ _id: ObjectId(|").is_empty());
    }
}
