//! O que o `*` de um `SELECT` representa: as colunas das tabelas do
//! `FROM`/`JOIN` daquele `SELECT`, na ordem em que o banco as devolve.
//! Parar o mouse em cima do asterisco mostra a lista.
//!
//! Como no IntelliSense, não há um parser: as tabelas vêm dos tokens do
//! mesmo nível de parênteses do asterisco (as de uma subconsulta não
//! contam) e do mesmo lado de um `UNION`.

use std::ops::Range;

use pgstudio_core::catalog::{Catalog, Column, normalize_ident};

use crate::complete::{TableRef, refs_after, statement};
use crate::lexer::{Token, TokenKind};

/// Uma tabela que o asterisco expande.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StarTable {
    /// Como o comando chama a tabela: `schema.nome`, como foi escrito.
    pub name: String,
    pub alias: Option<String>,
    /// As colunas, na ordem da tabela; `None` se o catálogo não conhece a
    /// tabela (uma CTE, uma função, uma tabela temporária) ou ainda não
    /// foi lido.
    pub columns: Option<Vec<Column>>,
}

/// O asterisco sob o ponteiro e o que ele representa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Star {
    /// Onde está o `*` no texto, em bytes.
    pub range: Range<usize>,
    /// O que trocar pelos nomes das colunas: o `*` ou, em `l.*`, o
    /// `l.*` inteiro.
    pub replace: Range<usize>,
    /// Se é `alias.*`.
    pub qualified: bool,
    pub tables: Vec<StarTable>,
}

impl Star {
    /// Quantas colunas o asterisco traz (das tabelas conhecidas).
    pub fn column_count(&self) -> usize {
        self.tables
            .iter()
            .filter_map(|t| t.columns.as_ref())
            .map(Vec::len)
            .sum()
    }

    /// Os nomes das colunas `chosen` (tabela, coluna), na ordem do banco,
    /// como entram no lugar do asterisco: com o alias na frente se o
    /// asterisco tinha, ou se há mais de uma tabela. `quote` põe as aspas
    /// que o nome precisar.
    pub fn names(&self, chosen: &[(usize, usize)], quote: impl Fn(&str) -> String) -> Vec<String> {
        let qualify = self.qualified || self.tables.len() > 1;
        let mut chosen = chosen.to_vec();
        chosen.sort_unstable();
        chosen.dedup();
        chosen
            .iter()
            .filter_map(|&(t, c)| {
                let table = self.tables.get(t)?;
                let column = table.columns.as_ref()?.get(c)?;
                let name = quote(&column.name);
                Some(if qualify {
                    format!("{}.{name}", table.alias.as_deref().unwrap_or(&table.name))
                } else {
                    name
                })
            })
            .collect()
    }

    /// Todas as colunas, como [`Star::names`] as espera.
    pub fn all(&self) -> Vec<(usize, usize)> {
        self.tables
            .iter()
            .enumerate()
            .flat_map(|(t, table)| {
                (0..table.columns.as_ref().map_or(0, Vec::len)).map(move |c| (t, c))
            })
            .collect()
    }
}

const SET_OPERATORS: &[&str] = &["union", "except", "intersect"];

fn significant(t: &Token) -> bool {
    !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment)
}

fn word<'a>(text: &'a str, t: &Token) -> &'a str {
    &text[t.range.clone()]
}

fn is_word(text: &str, t: &Token, words: &[&str]) -> bool {
    matches!(t.kind, TokenKind::Keyword | TokenKind::Identifier)
        && words.iter().any(|w| word(text, t).eq_ignore_ascii_case(w))
}

fn is_punct(text: &str, t: &Token, p: &str) -> bool {
    t.kind == TokenKind::Punctuation && word(text, t) == p
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, TokenKind::Identifier | TokenKind::QuotedIdentifier)
}

/// O `*` de um `SELECT` (ou `alias.*`) no byte `at`, com as colunas que
/// ele representa. `None` se ali não há um asterisco de lista de colunas
/// (uma multiplicação, `count(*)`, um comentário).
pub fn star_at(text: &str, tokens: &[Token], at: usize, catalog: Option<&Catalog>) -> Option<Star> {
    let stmt = statement(text, tokens, at);
    let toks: Vec<&Token> = tokens[stmt].iter().filter(|t| significant(t)).collect();
    let s = toks
        .iter()
        .position(|t| t.range.start <= at && at < t.range.end)?;
    let star = toks[s];
    if star.kind != TokenKind::Operator || word(text, star) != "*" {
        return None;
    }
    let prev = s.checked_sub(1).map(|i| toks[i]);
    let next = toks.get(s + 1).copied();
    let qualified = prev.is_some_and(|t| is_punct(text, t, "."));

    // Depois do asterisco de uma lista de colunas vem uma vírgula, o
    // `FROM` (ou outra palavra-chave) ou o fim; depois de uma
    // multiplicação vem o outro operando.
    let next_ok = next.is_none_or(|t| {
        t.kind == TokenKind::Keyword || is_punct(text, t, ",") || is_punct(text, t, ")")
    });
    if !next_ok {
        return None;
    }
    if !qualified {
        // `SELECT TOP 10 *` do SQL Server: o número não é um operando.
        let after_top = s >= 2 && is_word(text, toks[s - 2], &["top"]);
        let operand = prev.is_some_and(|t| {
            matches!(
                t.kind,
                TokenKind::Identifier
                    | TokenKind::QuotedIdentifier
                    | TokenKind::String
                    | TokenKind::Parameter
            ) || (t.kind == TokenKind::Number && !after_top)
        });
        // `count(*)` conta linhas, não traz colunas.
        if operand || prev.is_none() || prev.is_some_and(|t| is_punct(text, t, "(")) {
            return None;
        }
    }

    // O `SELECT` do asterisco: os tokens do mesmo nível de parênteses,
    // até o `UNION` de cada lado. Os parênteses de uma subconsulta ficam
    // (assim `FROM (...) x` não vira uma tabela `x`); o que há dentro
    // deles, não.
    let mut depth = 0i32;
    let depths: Vec<i32> = toks
        .iter()
        .map(|t| {
            if is_punct(text, t, ")") {
                depth -= 1;
            }
            let d = depth;
            if is_punct(text, t, "(") {
                depth += 1;
            }
            d
        })
        .collect();
    let level = depths[s];
    let boundary = |i: usize| {
        depths[i] < level || (depths[i] == level && is_word(text, toks[i], SET_OPERATORS))
    };
    let start = (0..s).rev().find(|&i| boundary(i)).map_or(0, |i| i + 1);
    let end = (s + 1..toks.len())
        .find(|&i| boundary(i))
        .unwrap_or(toks.len());
    let scope: Vec<Token> = (start..end)
        .filter(|&i| depths[i] == level)
        .map(|i| toks[i].clone())
        .collect();
    let mut refs = refs_after(text, &scope, &["from", "join"]);
    if refs.is_empty() {
        // `INSERT INTO t ... RETURNING *`, `UPDATE t ... RETURNING *`.
        refs = refs_after(text, &scope, &["update", "into"]);
    }

    let normalize =
        |name: &str| catalog.map_or_else(|| normalize_ident(name), |c| c.normalize(name));
    let table = |r: &TableRef| StarTable {
        name: match &r.schema {
            Some(s) => format!("{s}.{}", r.name),
            None => r.name.clone(),
        },
        alias: r.alias.clone(),
        columns: catalog
            .and_then(|c| c.relation(r.schema.as_deref(), &r.name))
            .map(|rel| rel.columns.clone()),
    };

    let mut replace = star.range.clone();
    let tables = if qualified {
        // `l.*`, `lotes.*` ou `public.lotes.*`.
        let name = s.checked_sub(2).map(|i| toks[i]).filter(|t| is_name(t))?;
        let schema = (s >= 4 && is_punct(text, toks[s - 3], ".") && is_name(toks[s - 4]))
            .then(|| word(text, toks[s - 4]));
        replace.start = match schema {
            Some(_) => toks[s - 4].range.start,
            None => name.range.start,
        };
        let written = word(text, name);
        let wanted = normalize(written);
        let by_ref = refs.iter().find(|r| match &r.alias {
            Some(a) => schema.is_none() && normalize(a) == wanted,
            None => {
                normalize(&r.name) == wanted
                    && schema.is_none_or(|s| {
                        r.schema
                            .as_deref()
                            .is_none_or(|rs| normalize(rs) == normalize(s))
                    })
            }
        });
        match by_ref {
            Some(r) => vec![table(r)],
            None => vec![table(&TableRef {
                schema: schema.map(str::to_string),
                name: written.to_string(),
                alias: None,
            })],
        }
    } else {
        refs.iter().map(table).collect()
    };
    if tables.is_empty() {
        return None;
    }
    Some(Star {
        range: star.range.clone(),
        replace,
        qualified,
        tables,
    })
}

#[cfg(test)]
mod tests {
    use pgstudio_core::catalog::{Relation, RelationKind, Schema};
    use pgstudio_core::connection::Engine;

    use super::*;
    use crate::lexer::{Dialect, tokenize_in};

    fn catalog(engine: Engine) -> Catalog {
        let rel = |name: &str, cols: &[(&str, &str)]| Relation {
            name: name.into(),
            kind: RelationKind::Table,
            columns: cols
                .iter()
                .map(|(n, t)| Column {
                    name: (*n).into(),
                    ty: (*t).into(),
                })
                .collect(),
        };
        let public = if engine == Engine::SqlServer {
            "dbo"
        } else {
            "public"
        };
        Catalog {
            engine,
            database: "loja".into(),
            search_path: vec![public.into()],
            schemas: vec![
                Schema {
                    name: public.into(),
                    relations: vec![
                        rel(
                            "lotes",
                            &[
                                ("id", "integer"),
                                ("numero", "text"),
                                ("fazenda_id", "integer"),
                            ],
                        ),
                        rel("fazendas", &[("id", "integer"), ("nome", "text")]),
                    ],
                    functions: Vec::new(),
                },
                Schema {
                    name: "arquivo".into(),
                    relations: vec![rel("lotes", &[("id", "integer"), ("fechado_em", "date")])],
                    functions: Vec::new(),
                },
            ],
        }
    }

    /// O asterisco número `nth` (a partir de 0) do texto.
    fn star_in(dialect: Dialect, engine: Engine, sql: &str, nth: usize) -> Option<Star> {
        let tokens = tokenize_in(sql, dialect);
        let at = sql.match_indices('*').nth(nth).expect("asterisco").0;
        star_at(sql, &tokens, at, Some(&catalog(engine)))
    }

    fn star(sql: &str, nth: usize) -> Option<Star> {
        star_in(Dialect::Postgres, Engine::PostgreSql, sql, nth)
    }

    /// `tabela: coluna, coluna` de cada tabela do asterisco.
    fn shape(s: &Star) -> Vec<String> {
        s.tables
            .iter()
            .map(|t| {
                let cols = match &t.columns {
                    Some(c) => c
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    None => "?".into(),
                };
                format!("{}: {cols}", t.name)
            })
            .collect()
    }

    #[test]
    fn the_star_lists_the_columns_of_the_table() {
        let s = star("select * from lotes l where l.", 0).expect("asterisco");
        assert_eq!(shape(&s), ["lotes: id, numero, fazenda_id"]);
        assert_eq!(s.tables[0].alias.as_deref(), Some("l"));
        assert_eq!(s.column_count(), 3);
        assert_eq!(s.range, 7..8);
    }

    #[test]
    fn joins_bring_the_columns_of_every_table_in_order() {
        let s = star(
            "select * from lotes l join fazendas f on f.id = l.fazenda_id",
            0,
        )
        .expect("asterisco");
        assert_eq!(
            shape(&s),
            ["lotes: id, numero, fazenda_id", "fazendas: id, nome"]
        );
        assert_eq!(s.column_count(), 5);
    }

    #[test]
    fn a_qualified_star_brings_only_that_table() {
        let sql = "select f.*, l.numero from lotes l join fazendas f on f.id = l.fazenda_id";
        assert_eq!(shape(&star(sql, 0).unwrap()), ["fazendas: id, nome"]);
        // Pelo nome da tabela, sem alias, e com o schema.
        let sql = "select lotes.* from lotes";
        assert_eq!(
            shape(&star(sql, 0).unwrap()),
            ["lotes: id, numero, fazenda_id"]
        );
        let sql = "select arquivo.lotes.* from arquivo.lotes";
        assert_eq!(
            shape(&star(sql, 0).unwrap()),
            ["arquivo.lotes: id, fechado_em"]
        );
    }

    #[test]
    fn the_schema_written_in_the_from_is_respected() {
        let s = star("select * from arquivo.lotes", 0).unwrap();
        assert_eq!(shape(&s), ["arquivo.lotes: id, fechado_em"]);
    }

    #[test]
    fn a_multiplication_or_count_is_not_a_column_list() {
        assert_eq!(star("select id * 2 from lotes", 0), None);
        assert_eq!(star("select 2 * id from lotes", 0), None);
        assert_eq!(star("select (id + 1) * id from lotes", 0), None);
        assert_eq!(star("select count(*) from lotes", 0), None);
        assert_eq!(star("select id /* tudo: * */ from lotes", 0), None);
        assert_eq!(star("select '*' from lotes", 0), None);
    }

    #[test]
    fn each_select_has_its_own_tables() {
        // A subconsulta não empresta as tabelas para o de fora, nem o
        // contrário.
        let sql = "select * from lotes where fazenda_id in (select * from fazendas)";
        assert_eq!(
            shape(&star(sql, 0).unwrap()),
            ["lotes: id, numero, fazenda_id"]
        );
        assert_eq!(shape(&star(sql, 1).unwrap()), ["fazendas: id, nome"]);
        // Cada lado de um UNION.
        let sql = "select * from lotes union all select * from fazendas";
        assert_eq!(
            shape(&star(sql, 0).unwrap()),
            ["lotes: id, numero, fazenda_id"]
        );
        assert_eq!(shape(&star(sql, 1).unwrap()), ["fazendas: id, nome"]);
        // Cada comando do arquivo.
        let sql = "select * from lotes;\nselect * from fazendas;";
        assert_eq!(shape(&star(sql, 1).unwrap()), ["fazendas: id, nome"]);
    }

    #[test]
    fn unknown_tables_are_listed_without_columns() {
        let sql = "with c as (select 1) select * from c join lotes on true";
        let s = star(sql, 0).unwrap();
        assert_eq!(shape(&s), ["c: ?", "lotes: id, numero, fazenda_id"]);
        // Sem tabela nenhuma não há o que mostrar.
        assert_eq!(star("select *", 0), None);
        assert_eq!(star("select * from (select 1) x", 0), None);
    }

    #[test]
    fn returning_star_uses_the_table_of_the_command() {
        let sql = "update lotes set numero = 'a' returning *";
        assert_eq!(
            shape(&star(sql, 0).unwrap()),
            ["lotes: id, numero, fazenda_id"]
        );
        let sql = "insert into fazendas (nome) values ('x') returning *";
        assert_eq!(shape(&star(sql, 0).unwrap()), ["fazendas: id, nome"]);
        let sql = "delete from fazendas where id = 1 returning *";
        assert_eq!(shape(&star(sql, 0).unwrap()), ["fazendas: id, nome"]);
    }

    #[test]
    fn distinct_and_other_columns_around_the_star() {
        assert!(star("select distinct * from lotes", 0).is_some());
        assert!(star("select distinct on (numero) * from lotes", 0).is_some());
        assert!(star("select numero, * from lotes", 0).is_some());
        assert!(star("select *, numero from lotes", 0).is_some());
    }

    #[test]
    fn sql_server_names_and_top() {
        let t = |sql: &str| star_in(Dialect::TSql, Engine::SqlServer, sql, 0);
        let s = t("SELECT TOP 10 * FROM [dbo].[Lotes] AS L").expect("asterisco");
        assert_eq!(shape(&s), ["[dbo].[Lotes]: id, numero, fazenda_id"]);
        assert_eq!(
            shape(&t("SELECT TOP (5) L.* FROM Lotes L").unwrap()),
            ["Lotes: id, numero, fazenda_id"]
        );
        assert_eq!(t("SELECT 10 * id FROM Lotes"), None);
    }

    #[test]
    fn the_chosen_columns_replace_the_star() {
        let q = |n: &str| n.to_string();
        // Uma tabela: só os nomes, na ordem do banco.
        let s = star("select * from lotes l", 0).unwrap();
        assert_eq!(s.replace, 7..8);
        assert_eq!(s.names(&[(0, 2), (0, 0)], q), ["id", "fazenda_id"]);
        assert_eq!(s.names(&s.all(), q), ["id", "numero", "fazenda_id"]);
        // `l.*`: o alias vai junto, e sai o `l.*` inteiro.
        let sql = "select l.* from lotes l";
        let s = star(sql, 0).unwrap();
        assert_eq!(&sql[s.replace.clone()], "l.*");
        assert_eq!(s.names(&[(0, 1)], q), ["l.numero"]);
        let sql = "select arquivo.lotes.* from arquivo.lotes";
        let s = star(sql, 0).unwrap();
        assert_eq!(&sql[s.replace.clone()], "arquivo.lotes.*");
        assert_eq!(s.names(&[(0, 0)], q), ["arquivo.lotes.id"]);
        // Mais de uma tabela: cada coluna com a tabela dela.
        let s = star("select * from lotes l join fazendas on true", 0).unwrap();
        assert_eq!(
            s.names(&[(1, 1), (0, 0)], |n| format!("\"{n}\"")),
            ["l.\"id\"", "fazendas.\"nome\""]
        );
    }

    #[test]
    fn without_a_catalog_the_tables_have_no_columns() {
        let sql = "select * from lotes";
        let tokens = tokenize_in(sql, Dialect::Postgres);
        let s = star_at(sql, &tokens, 7, None).unwrap();
        assert_eq!(shape(&s), ["lotes: ?"]);
        assert_eq!(s.column_count(), 0);
    }
}
