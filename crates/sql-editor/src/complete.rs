//! IntelliSense: o que sugerir no cursor, a partir dos tokens do texto e do
//! catálogo em cache.
//!
//! Não há um parser de verdade: o contexto vem dos tokens em volta do
//! cursor, e as tabelas do comando vêm das cláusulas `FROM`, `JOIN`,
//! `UPDATE` e `INTO`. Isso cobre o que o SSMS sugere no dia a dia:
//!
//! - depois de `FROM`/`JOIN`: tabelas, views e schemas;
//! - depois de `alias.` ou `tabela.`: colunas;
//! - depois de `schema.`: tabelas, views e funções do schema;
//! - no resto: colunas das tabelas do comando, aliases, palavras-chave,
//!   tabelas e funções.

use std::ops::Range;

use pgstudio_core::catalog::{Catalog, Relation, RelationKind, normalize_ident, quote_ident};

use crate::lexer::{KEYWORDS, TSQL_KEYWORDS, Token, TokenKind};

/// Tipo de uma sugestão (define o ícone e a ordem).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Column,
    Alias,
    Table,
    View,
    Schema,
    Function,
    Keyword,
}

impl Kind {
    /// Letra do ícone na lista.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Column => "▪",
            Kind::Alias => "≡",
            Kind::Table => "▦",
            Kind::View => "◫",
            Kind::Schema => "▤",
            Kind::Function => "ƒ",
            Kind::Keyword => "◆",
        }
    }
}

/// Uma linha da lista de sugestões.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Como aparece na lista.
    pub label: String,
    /// O que entra no texto (com aspas se o nome precisar).
    pub insert: String,
    pub kind: Kind,
    /// Detalhe em cinza: tipo da coluna, tabela de origem.
    pub detail: String,
}

/// Resultado de [`complete`]: trocar `replace` (em bytes) por uma das
/// sugestões.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub replace: Range<usize>,
    /// O que já foi digitado da palavra.
    pub prefix: String,
    pub items: Vec<Suggestion>,
}

/// Uma tabela citada no comando.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

/// Palavras depois das quais vem o nome de uma tabela.
const RELATION_KEYWORDS: &[&str] = &["from", "join", "update", "into", "table"];

/// Palavras que encerram a lista de tabelas de um `FROM`.
const CLAUSE_KEYWORDS: &[&str] = &[
    "where",
    "group",
    "order",
    "having",
    "limit",
    "offset",
    "on",
    "using",
    "set",
    "values",
    "returning",
    "union",
    "except",
    "intersect",
    "window",
    "select",
    "for",
    "fetch",
];

fn significant(t: &Token) -> bool {
    !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment)
}

fn word<'a>(text: &'a str, t: &Token) -> &'a str {
    &text[t.range.clone()]
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier)
}

fn is_kw(text: &str, t: &Token, kws: &[&str]) -> bool {
    t.kind == TokenKind::Keyword && kws.iter().any(|k| word(text, t).eq_ignore_ascii_case(k))
}

fn is_punct(text: &str, t: &Token, p: &str) -> bool {
    t.kind == TokenKind::Punctuation && word(text, t) == p
}

/// Tokens do comando em volta de `at` (entre `;`).
fn statement(text: &str, tokens: &[Token], at: usize) -> Range<usize> {
    let i = tokens.partition_point(|t| t.range.end <= at);
    let start = tokens[..i.min(tokens.len())]
        .iter()
        .rposition(|t| is_punct(text, t, ";"))
        .map_or(0, |p| p + 1);
    let end = tokens[i.min(tokens.len())..]
        .iter()
        .position(|t| is_punct(text, t, ";"))
        .map_or(tokens.len(), |p| i + p);
    start..end
}

/// Tabelas citadas nos `FROM`, `JOIN`, `UPDATE` e `INTO` do comando.
pub fn table_refs(text: &str, tokens: &[Token]) -> Vec<TableRef> {
    let toks: Vec<&Token> = tokens.iter().filter(|t| significant(t)).collect();
    let mut refs = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if !is_kw(text, toks[i], RELATION_KEYWORDS) {
            i += 1;
            continue;
        }
        let in_from = is_kw(text, toks[i], &["from"]);
        i += 1;
        // [schema .] nome
        while let Some(first) = toks.get(i).filter(|t| is_name(t)) {
            let mut schema = None;
            let mut name = word(text, first).to_string();
            i += 1;
            if toks.get(i).is_some_and(|t| is_punct(text, t, "."))
                && let Some(second) = toks.get(i + 1).filter(|t| is_name(t))
            {
                schema = Some(name);
                name = word(text, second).to_string();
                i += 2;
            }
            // [AS] alias
            let mut alias = None;
            if toks.get(i).is_some_and(|t| is_kw(text, t, &["as"])) {
                i += 1;
            }
            if let Some(a) = toks.get(i).filter(|t| is_name(t)) {
                alias = Some(word(text, a).to_string());
                i += 1;
            }
            refs.push(TableRef {
                schema,
                name,
                alias,
            });
            // FROM a, b, c
            if in_from && toks.get(i).is_some_and(|t| is_punct(text, t, ",")) {
                i += 1;
                continue;
            }
            break;
        }
    }
    refs
}

/// Se `name` combina com o que foi digitado (começo, sem diferenciar
/// maiúsculas).
fn matches(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
}

/// Sugestões para o cursor em `at` (bytes). Dentro de comentários e strings
/// não há sugestões.
pub fn complete(text: &str, tokens: &[Token], at: usize, catalog: Option<&Catalog>) -> Completion {
    let none = Completion {
        replace: at..at,
        ..Completion::default()
    };
    // Token que termina no cursor (a palavra sendo digitada) ou que o
    // contém.
    let i = tokens.partition_point(|t| t.range.end < at);
    let current = tokens.get(i).filter(|t| t.range.start < at);
    let mut start = at;
    if let Some(t) = current {
        match t.kind {
            TokenKind::Comment | TokenKind::String | TokenKind::Number => return none,
            TokenKind::Identifier | TokenKind::Keyword | TokenKind::QuotedIdentifier => {
                start = t.range.start;
            }
            _ => {}
        }
    }
    let prefix_raw = &text[start..at];
    let prefix = prefix_raw.trim_start_matches(['"', '[']);
    let before: Vec<&Token> = tokens[..tokens.partition_point(|t| t.range.end <= start)]
        .iter()
        .filter(|t| significant(t))
        .collect();

    // Qualificador: `a.` ou `s.t.` antes da palavra.
    let mut qualifier: Vec<&str> = Vec::new();
    let mut k = before.len();
    while k >= 2 && is_punct(text, before[k - 1], ".") && is_name(before[k - 2]) {
        qualifier.insert(0, word(text, before[k - 2]));
        k -= 2;
    }
    let prev = k.checked_sub(1).map(|k| before[k]);

    let stmt = statement(text, tokens, start);
    let refs = table_refs(text, &tokens[stmt]);
    let mut items = Vec::new();
    let cat = catalog;
    // Aspas (ou colchetes) e comparação de nomes pelas regras do motor.
    let quote = |name: &str| cat.map_or_else(|| quote_ident(name), |c| c.quote(name));
    let normalize = |name: &str| cat.map_or_else(|| normalize_ident(name), |c| c.normalize(name));
    let tsql = cat.is_some_and(|c| c.engine == pgstudio_core::connection::Engine::SqlServer);

    let push_columns = |items: &mut Vec<Suggestion>, rel: &Relation, from: &str| {
        for c in &rel.columns {
            if matches(&c.name, prefix) {
                items.push(Suggestion {
                    label: c.name.clone(),
                    insert: quote(&c.name),
                    kind: Kind::Column,
                    detail: format!("{} ({from})", c.ty),
                });
            }
        }
    };
    let push_relations = |items: &mut Vec<Suggestion>, schema: &pgstudio_core::catalog::Schema| {
        for r in &schema.relations {
            if matches(&r.name, prefix) {
                items.push(Suggestion {
                    label: r.name.clone(),
                    insert: quote(&r.name),
                    kind: match r.kind {
                        RelationKind::Table => Kind::Table,
                        RelationKind::View => Kind::View,
                    },
                    detail: schema.name.clone(),
                });
            }
        }
    };
    let push_functions = |items: &mut Vec<Suggestion>, schema: &pgstudio_core::catalog::Schema| {
        for f in &schema.functions {
            if matches(f, prefix) {
                items.push(Suggestion {
                    label: f.clone(),
                    // As internas do T-SQL (LEFT, RIGHT...) entram como são.
                    insert: if tsql && schema.name == "sys" {
                        f.clone()
                    } else {
                        quote(f)
                    },
                    kind: Kind::Function,
                    detail: schema.name.clone(),
                });
            }
        }
    };
    let push_schemas = |items: &mut Vec<Suggestion>, cat: &Catalog| {
        for s in &cat.schemas {
            if s.name != "pg_catalog" && !(tsql && s.name == "sys") && matches(&s.name, prefix) {
                items.push(Suggestion {
                    label: s.name.clone(),
                    insert: quote(&s.name),
                    kind: Kind::Schema,
                    detail: String::new(),
                });
            }
        }
    };
    // Relação de uma referência do comando.
    let resolve = |r: &TableRef| cat.and_then(|c| c.relation(r.schema.as_deref(), &r.name));

    if !qualifier.is_empty() {
        let Some(cat) = cat else {
            return none;
        };
        let last = normalize(qualifier[qualifier.len() - 1]);
        // alias. ou tabela.
        let by_ref = (qualifier.len() == 1)
            .then(|| {
                refs.iter().find(|r| {
                    r.alias.as_deref().map(normalize) == Some(last.clone())
                        || (r.alias.is_none() && normalize(&r.name) == last)
                })
            })
            .flatten();
        if let Some((r, rel)) = by_ref.and_then(|r| Some((r, resolve(r)?))) {
            push_columns(&mut items, rel, &r.name);
        } else if qualifier.len() == 2 {
            if let Some(rel) = cat.relation(Some(qualifier[0]), qualifier[1]) {
                push_columns(&mut items, rel, qualifier[1]);
            }
        } else if let Some(schema) = cat.schema(qualifier[0]) {
            push_relations(&mut items, schema);
            push_functions(&mut items, schema);
        } else if let Some(rel) = cat.relation(None, qualifier[0]) {
            push_columns(&mut items, rel, qualifier[0]);
        }
    } else {
        let after_relation_keyword = prev.is_some_and(|t| is_kw(text, t, RELATION_KEYWORDS));
        // `FROM a, |`: vírgula na lista de tabelas.
        let in_from_list = prev.is_some_and(|t| is_punct(text, t, ","))
            && before[..k]
                .iter()
                .rev()
                .find(|t| is_kw(text, t, CLAUSE_KEYWORDS) || is_kw(text, t, &["from"]))
                .is_some_and(|t| is_kw(text, t, &["from"]));
        if after_relation_keyword || in_from_list {
            if let Some(cat) = cat {
                for s in cat.visible_schemas() {
                    push_relations(&mut items, s);
                }
                push_schemas(&mut items, cat);
            }
        } else {
            for r in &refs {
                let shown = r.alias.as_deref().unwrap_or(&r.name);
                if let Some(rel) = resolve(r) {
                    push_columns(&mut items, rel, shown);
                }
                if matches(shown, prefix) {
                    items.push(Suggestion {
                        label: shown.to_string(),
                        insert: shown.to_string(),
                        kind: Kind::Alias,
                        detail: r.name.clone(),
                    });
                }
            }
            if let Some(cat) = cat {
                for s in cat.visible_schemas() {
                    push_relations(&mut items, s);
                    push_functions(&mut items, s);
                }
                push_schemas(&mut items, cat);
            }
            let upper = !prefix.chars().any(|c| c.is_lowercase());
            let extra: &[&str] = if tsql { TSQL_KEYWORDS } else { &[] };
            for kw in KEYWORDS.iter().chain(extra) {
                if matches(kw, prefix) {
                    items.push(Suggestion {
                        label: kw.to_uppercase(),
                        insert: if upper || prefix.is_empty() {
                            kw.to_uppercase()
                        } else {
                            kw.to_string()
                        },
                        kind: Kind::Keyword,
                        detail: String::new(),
                    });
                }
            }
        }
    }
    // Ordem alfabética, sem repetir; num empate, a ordem dos tipos (colunas
    // antes de tabelas antes de palavras-chave).
    items.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
            .then(a.kind.cmp(&b.kind))
    });
    items.dedup_by(|a, b| a.label == b.label && a.kind == b.kind);
    Completion {
        replace: start..at,
        prefix: prefix_raw.to_string(),
        items,
    }
}

/// Melhor sugestão para o que foi digitado: a primeira que começa igual,
/// preferindo os tipos mais específicos (colunas, tabelas).
pub fn best(c: &Completion) -> usize {
    let prefix = c.prefix.trim_start_matches(['"', '[']);
    if prefix.is_empty() {
        return 0;
    }
    let exact = |keyword: bool| {
        c.items.iter().position(|s| {
            (s.kind == Kind::Keyword) == keyword && s.label.eq_ignore_ascii_case(prefix)
        })
    };
    exact(false)
        .or_else(|| exact(true))
        .or_else(|| c.items.iter().position(|s| s.kind != Kind::Keyword))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use pgstudio_core::catalog::{Column, Schema};

    fn rel(name: &str, kind: RelationKind, cols: &[&str]) -> Relation {
        Relation {
            name: name.into(),
            kind,
            columns: cols
                .iter()
                .map(|c| Column {
                    name: (*c).into(),
                    ty: "integer".into(),
                })
                .collect(),
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            engine: Default::default(),
            database: "loja".into(),
            search_path: vec!["public".into(), "pg_catalog".into()],
            schemas: vec![
                Schema {
                    name: "public".into(),
                    relations: vec![
                        rel("cliente", RelationKind::Table, &["id", "nome", "email"]),
                        rel(
                            "pedido",
                            RelationKind::Table,
                            &["id", "cliente_id", "total"],
                        ),
                        rel("pedidos_grandes", RelationKind::View, &["id"]),
                        rel("Mixed", RelationKind::Table, &["Col"]),
                    ],
                    functions: vec!["calcula_total".into()],
                },
                Schema {
                    name: "vendas".into(),
                    relations: vec![rel("nota", RelationKind::Table, &["numero", "valor"])],
                    functions: vec!["emite".into()],
                },
                Schema {
                    name: "pg_catalog".into(),
                    relations: vec![],
                    functions: vec!["count".into(), "now".into()],
                },
            ],
        }
    }

    /// Sugestões com o cursor no `|`.
    fn at(sql: &str) -> Completion {
        let pos = sql.find('|').unwrap();
        let text = sql.replace('|', "");
        complete(&text, &tokenize(&text), pos, Some(&catalog()))
    }

    fn labels(c: &Completion) -> Vec<String> {
        c.items.iter().map(|s| s.label.clone()).collect()
    }

    #[test]
    fn after_from_lists_tables_views_and_schemas() {
        let c = at("select * from |");
        assert_eq!(
            labels(&c),
            [
                "cliente",
                "Mixed",
                "pedido",
                "pedidos_grandes",
                "public",
                "vendas"
            ]
        );
        let c = at("select * from ped|");
        assert_eq!(labels(&c), ["pedido", "pedidos_grandes"]);
        assert_eq!(c.replace, 14..17);
        let c = at("SELECT * FROM cliente c JOIN p|");
        assert_eq!(labels(&c), ["pedido", "pedidos_grandes", "public"]);
        let c = at("select * from cliente, p|");
        assert_eq!(labels(&c), ["pedido", "pedidos_grandes", "public"]);
    }

    #[test]
    fn names_that_need_quotes_are_inserted_with_them() {
        let c = at("select * from Mi|");
        assert_eq!(c.items[0].insert, "\"Mixed\"");
    }

    #[test]
    fn alias_dot_lists_the_columns_of_that_table() {
        let c = at("select p.| from cliente c join pedido p on p.cliente_id = c.id");
        assert_eq!(labels(&c), ["cliente_id", "id", "total"]);
        let c = at("select c.n| from cliente c");
        assert_eq!(labels(&c), ["nome"]);
        assert_eq!(c.items[0].detail, "integer (cliente)");
        let c = at("select cliente.| from cliente");
        assert_eq!(labels(&c), ["email", "id", "nome"]);
    }

    #[test]
    fn schema_dot_lists_its_tables_and_functions() {
        let c = at("select * from vendas.|");
        assert_eq!(labels(&c), ["emite", "nota"]);
        let c = at("select vendas.nota.| from vendas.nota");
        assert_eq!(labels(&c), ["numero", "valor"]);
        let c = at("select n.v| from vendas.nota as n");
        assert_eq!(labels(&c), ["valor"]);
    }

    #[test]
    fn elsewhere_columns_of_the_statement_come_with_keywords_and_functions() {
        let c = at("select n| from cliente");
        let l = labels(&c);
        assert!(l.contains(&"nome".to_string()) && l.contains(&"now".to_string()));
        assert!(l.contains(&"NOT".to_string()));
        assert_eq!(c.items[best(&c)].label, "nome");
        let c = at("select * from pedido p where p.total > 0 and cli|");
        assert_eq!(labels(&c), ["cliente", "cliente_id"]);
        // Só as tabelas do comando atual.
        let c = at("select * from cliente; select em| from pedido");
        assert!(labels(&c).is_empty(), "{:?}", labels(&c));
    }

    fn tsql_catalog() -> Catalog {
        Catalog {
            engine: pgstudio_core::connection::Engine::SqlServer,
            database: "vendas".into(),
            search_path: vec!["dbo".into(), "sys".into()],
            schemas: vec![
                Schema {
                    name: "dbo".into(),
                    relations: vec![
                        rel("Clientes", RelationKind::Table, &["Id", "Nome"]),
                        rel("Itens do Pedido", RelationKind::Table, &["Id", "Order"]),
                    ],
                    functions: vec!["Listar".into()],
                },
                Schema {
                    name: "sys".into(),
                    relations: vec![],
                    functions: vec!["GETDATE".into(), "LEFT".into()],
                },
            ],
        }
    }

    fn tsql(sql: &str) -> Completion {
        let pos = sql.find('|').unwrap();
        let text = sql.replace('|', "");
        let tokens = crate::lexer::tokenize_in(&text, crate::lexer::Dialect::TSql);
        complete(&text, &tokens, pos, Some(&tsql_catalog()))
    }

    #[test]
    fn sql_server_names_ignore_case_and_use_brackets_only_when_needed() {
        let c = tsql("SELECT * FROM cli|");
        assert_eq!(labels(&c), ["Clientes"]);
        assert_eq!(c.items[0].insert, "Clientes");
        let c = tsql("SELECT * FROM [it|");
        assert_eq!(c.items[0].insert, "[Itens do Pedido]");
        assert_eq!(c.replace.start, 14);
        // `sys` guarda só as funções internas: não aparece como schema.
        assert_eq!(
            labels(&tsql("SELECT * FROM |")),
            ["Clientes", "dbo", "Itens do Pedido"]
        );
    }

    #[test]
    fn sql_server_columns_come_through_brackets_and_aliases() {
        let c = tsql("SELECT c.| FROM [DBO].[clientes] AS c");
        assert_eq!(labels(&c), ["Id", "Nome"]);
        let c = tsql("SELECT i.o| FROM [Itens do Pedido] i");
        assert_eq!(c.items[0].insert, "[Order]");
        let c = tsql("SELECT * FROM dbo.|");
        assert_eq!(labels(&c), ["Clientes", "Itens do Pedido", "Listar"]);
    }

    #[test]
    fn sql_server_builtin_functions_and_keywords() {
        let c = tsql("SELECT GET| FROM Clientes");
        assert_eq!(c.items[best(&c)].insert, "GETDATE");
        let c = tsql("SELECT LEF| FROM Clientes");
        assert!(
            c.items
                .iter()
                .any(|s| s.kind == Kind::Function && s.insert == "LEFT")
        );
        assert!(labels(&tsql("NVARC|")).contains(&"NVARCHAR".to_string()));
        assert!(!labels(&at("NVARC|")).contains(&"NVARCHAR".to_string()));
    }

    #[test]
    fn keywords_follow_the_case_typed() {
        assert_eq!(at("sel|").items[0].insert, "select");
        assert_eq!(at("SEL|").items[0].insert, "SELECT");
    }

    #[test]
    fn nothing_inside_strings_and_comments() {
        assert!(at("select 'cli|'").items.is_empty());
        assert!(at("select 1 -- from cli|").items.is_empty());
    }

    #[test]
    fn best_prefers_objects_over_keywords() {
        let c = at("select * from pedido where tot|");
        assert_eq!(c.items[best(&c)].label, "total");
        let c = at("select * from cliente where id = 1 or|");
        assert_eq!(c.items[best(&c)].label, "OR");
    }

    #[test]
    fn five_thousand_tables_suggest_quickly() {
        let mut cat = catalog();
        cat.schemas[0].relations = (0..5000)
            .map(|i| {
                rel(
                    &format!("tabela_{i:04}"),
                    RelationKind::Table,
                    &["a", "b", "c"],
                )
            })
            .collect();
        cat.schemas[2].functions = (0..3000).map(|i| format!("fn_{i}")).collect();
        let text = "select * from tabela_0001 t join tabela_0002 u on t.a = u.a where ";
        let tokens = tokenize(text);
        let start = std::time::Instant::now();
        let c = complete(text, &tokens, text.len(), Some(&cat));
        let elapsed = start.elapsed();
        eprintln!("sugestões com 5 mil tabelas: {elapsed:?}");
        assert!(c.items.len() > 8000);
        let c2 = complete(
            "select * from ",
            &tokenize("select * from "),
            14,
            Some(&cat),
        );
        assert_eq!(c2.items.len(), 5002);
        // O critério do plano é 50 ms; em build de teste fica bem abaixo.
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "{elapsed:?}"
        );
    }
}
