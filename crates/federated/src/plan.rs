//! Lê o texto de uma consulta entre servidores: acha as fontes (o que vem
//! de cada servidor) e troca cada uma pelo nome da tabela local onde as
//! linhas dela vão ficar.
//!
//! Uma fonte aparece onde o SQL aceita uma tabela (depois de `FROM`,
//! `JOIN` ou de uma vírgula na lista do `FROM`), de dois jeitos:
//!
//! - o endereço de uma tabela, `servidor.banco.schema.tabela` (ou mais
//!   curto, veja [`direct`]): o PostPilot lê a tabela inteira;
//! - uma consulta na língua do servidor, `servidor.banco($$ ... $$)` ou
//!   `servidor($$ ... $$)`: o servidor roda o texto e devolve só o que ele
//!   pedir. É assim que o filtro roda no servidor, e é assim que entram
//!   Redis, MongoDB e Elasticsearch com os comandos deles.
//!
//! O nome do servidor é o da conexão guardada. Vale sem aspas quando é um
//! nome simples (`pg_vendas` também acha "PG Vendas"), ou entre `[ ]`,
//! `" "` ou crases.

use pgstudio_core::connection::Engine;
use pgstudio_i18n::{tr, trf};

/// Um servidor que pode ser fonte: uma conexão guardada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// Nome da conexão, como o usuário escreve na consulta.
    pub name: String,
    pub engine: Engine,
    /// Banco do registro: vale quando a consulta não diz o banco.
    pub database: String,
}

/// O que ler de um servidor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Índice em `servers`.
    pub server: usize,
    pub database: String,
    /// O texto que o servidor roda, na língua dele.
    pub query: String,
    /// Como a fonte aparece nas mensagens (o que o usuário escreveu).
    pub label: String,
    /// Tabela local com as linhas da fonte.
    pub table: String,
    /// A fonte é o endereço de uma tabela (lida inteira).
    pub whole: bool,
    /// Numa tabela do PostgreSQL ou do SQL Server, o nome dela como vai no
    /// SQL do servidor: dá para montar um SELECT com só as colunas, os
    /// filtros e as chaves que a consulta usa (veja [`crate::pushdown`]).
    pub relation: Option<String>,
    /// Quantas vezes a consulta cita a fonte.
    pub uses: usize,
}

/// O servidor da janela: as tabelas escritas sem servidor na frente são
/// dele.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Home {
    /// Índice em `servers`.
    pub server: usize,
    /// O banco da janela.
    pub database: String,
}

/// Um comando para o SQLite local, já com as fontes trocadas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub sql: String,
    /// Onde o comando começa no texto original, em caracteres.
    pub offset: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub sources: Vec<Source>,
    pub statements: Vec<Statement>,
}

/// Erro no texto, com a posição em caracteres a partir de 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanError {
    pub message: String,
    pub position: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// Palavra sem aspas.
    Word,
    /// Nome entre `"`, `[ ]` ou crases, já sem elas.
    Quoted(String),
    /// Texto entre `'` ou `$$`, já sem os delimitadores.
    Text(String),
    Number,
    Punct(char),
}

#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    /// Em bytes no texto.
    start: usize,
    end: usize,
}

fn lex(sql: &str) -> Result<Vec<Token>, PlanError> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let unterminated = |what: &str, at: usize| PlanError {
        message: trf!("Unterminated {what}.", what = what),
        position: sql[..at].chars().count() + 1,
    };
    while i < bytes.len() {
        let c = sql[i..].chars().next().unwrap_or(' ');
        let start = i;
        if c.is_whitespace() {
            i += c.len_utf8();
        } else if sql[i..].starts_with("--") {
            i = sql[i..].find('\n').map_or(bytes.len(), |n| i + n);
        } else if sql[i..].starts_with("/*") {
            i = match sql[i + 2..].find("*/") {
                Some(n) => i + 2 + n + 2,
                None => return Err(unterminated(tr("comment"), start)),
            };
        } else if c == '\'' || c == '"' || c == '`' {
            // A aspa dobrada é a aspa dentro do texto.
            let mut text = String::new();
            let mut j = i + 1;
            loop {
                let Some(n) = sql[j..].find(c) else {
                    return Err(unterminated(tr("quoted text"), start));
                };
                text.push_str(&sql[j..j + n]);
                j += n + 1;
                if sql[j..].starts_with(c) {
                    text.push(c);
                    j += 1;
                } else {
                    break;
                }
            }
            i = j;
            let kind = if c == '\'' {
                Kind::Text(text)
            } else {
                Kind::Quoted(text)
            };
            out.push(Token {
                kind,
                start,
                end: i,
            });
        } else if c == '[' {
            let Some(n) = sql[i + 1..].find(']') else {
                return Err(unterminated(tr("name in brackets"), start));
            };
            let name = sql[i + 1..i + 1 + n].to_string();
            i += n + 2;
            out.push(Token {
                kind: Kind::Quoted(name),
                start,
                end: i,
            });
        } else if c == '$' {
            // `$$ ... $$` ou `$nome$ ... $nome$`, como no PostgreSQL.
            let tag_len = sql[i + 1..]
                .char_indices()
                .find(|(_, ch)| !(ch.is_alphanumeric() || *ch == '_'))
                .map(|(n, _)| n);
            match tag_len {
                Some(n) if sql[i + 1 + n..].starts_with('$') => {
                    let tag = &sql[i..i + n + 2];
                    let body = i + n + 2;
                    let Some(close) = sql[body..].find(tag) else {
                        return Err(unterminated(tr("$$ text"), start));
                    };
                    i = body + close + tag.len();
                    out.push(Token {
                        kind: Kind::Text(sql[body..body + close].to_string()),
                        start,
                        end: i,
                    });
                }
                _ => {
                    i += 1;
                    out.push(Token {
                        kind: Kind::Punct('$'),
                        start,
                        end: i,
                    });
                }
            }
        } else if c.is_alphabetic() || c == '_' {
            while let Some(ch) = sql[i..].chars().next() {
                if ch.is_alphanumeric() || ch == '_' {
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
            out.push(Token {
                kind: Kind::Word,
                start,
                end: i,
            });
        } else if c.is_ascii_digit() {
            while let Some(ch) = sql[i..].chars().next() {
                if ch.is_alphanumeric() || ch == '_' {
                    i += ch.len_utf8();
                } else if ch == '.'
                    && sql[i + 1..]
                        .chars()
                        .next()
                        .is_some_and(|d| d.is_ascii_digit())
                {
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Token {
                kind: Kind::Number,
                start,
                end: i,
            });
        } else {
            i += c.len_utf8();
            out.push(Token {
                kind: Kind::Punct(c),
                start,
                end: i,
            });
        }
    }
    Ok(out)
}

/// Uma parte de um endereço: o nome e se veio entre aspas ou colchetes.
#[derive(Clone, Debug)]
struct Part {
    name: String,
    quoted: bool,
}

/// Nome simples para comparar: minúsculas, e o que não é letra nem número
/// vira `_`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// O nome do servidor como entra numa consulta: sem nada quando é simples,
/// entre aspas quando tem espaço ou outro sinal.
pub fn written_name(name: &str) -> String {
    let simple = name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !is_keyword(name);
    if simple {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

fn find_server(servers: &[Server], part: &Part) -> Option<usize> {
    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    server_named(&names, &part.name)
}

/// Qual dos servidores `names` é o `written` de uma consulta (sem as
/// aspas): o de mesmo nome, sem diferenciar maiúsculas, ou o de mesmo nome
/// simples (`pg_vendas` é "PG Vendas").
pub fn server_named(names: &[&str], written: &str) -> Option<usize> {
    let wanted = written.to_lowercase();
    names
        .iter()
        .position(|n| n.to_lowercase() == wanted)
        .or_else(|| {
            let wanted = slug(written);
            (!wanted.is_empty())
                .then(|| names.iter().position(|n| slug(n) == wanted))
                .flatten()
        })
}

/// Palavras que, depois de uma tabela, não são o apelido dela.
const AFTER_TABLE: &[&str] = &[
    "join",
    "inner",
    "left",
    "right",
    "full",
    "outer",
    "cross",
    "natural",
    "on",
    "using",
    "where",
    "group",
    "having",
    "order",
    "limit",
    "offset",
    "union",
    "intersect",
    "except",
    "window",
    "indexed",
    "not",
];

/// Palavras que fecham a lista de tabelas do `FROM`.
const END_OF_FROM: &[&str] = &[
    "select",
    "on",
    "using",
    "where",
    "group",
    "having",
    "order",
    "limit",
    "offset",
    "union",
    "intersect",
    "except",
    "window",
    "set",
    "values",
    "returning",
];

fn is_keyword(word: &str) -> bool {
    let w = word.to_lowercase();
    AFTER_TABLE.contains(&w.as_str())
        || END_OF_FROM.contains(&w.as_str())
        || matches!(w.as_str(), "from" | "as" | "with" | "and" | "or" | "by")
}

fn quote_sql(engine: Engine, part: &Part) -> String {
    if !part.quoted {
        return part.name.clone();
    }
    match engine {
        Engine::SqlServer => format!("[{}]", part.name.replace(']', "]]")),
        _ => format!("\"{}\"", part.name.replace('"', "\"\"")),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Quantos documentos uma busca do Elasticsearch devolve de uma vez (o
/// teto padrão do servidor, `index.max_result_window`).
pub const ELASTICSEARCH_WINDOW: usize = 10_000;

/// A consulta que lê uma tabela inteira, pelo endereço dela (as partes
/// depois do servidor). Devolve o banco e o texto, ou o motivo de o
/// endereço não servir.
///
/// - PostgreSQL e SQL Server: `tabela`, `schema.tabela` ou
///   `banco.schema.tabela`;
/// - MongoDB: `coleção` ou `banco.coleção`;
/// - Elasticsearch: `índice`;
/// - Redis: `padrão` ou `banco.padrão`, as chaves que casam com o padrão
///   (uma linha por chave, com `key`, `type` e `value`).
fn direct(server: &Server, parts: &[Part]) -> Result<(String, String), String> {
    let engine = server.engine;
    match engine {
        Engine::PostgreSql | Engine::SqlServer => {
            let (database, rest) = match parts.len() {
                1 | 2 => (server.database.clone(), parts),
                3 => (parts[0].name.clone(), &parts[1..]),
                _ => {
                    return Err(tr(
                        "Write the table as server.database.schema.table, server.schema.table or server.table.",
                    )
                    .to_string());
                }
            };
            let name: Vec<String> = rest.iter().map(|p| quote_sql(engine, p)).collect();
            Ok((database, format!("SELECT * FROM {}", name.join("."))))
        }
        Engine::MongoDb => {
            let (database, collection) = match parts {
                [c] => (server.database.clone(), c),
                [d, c] => (d.name.clone(), c),
                _ => {
                    return Err(tr(
                        "Write the collection as server.database.collection or server.collection.",
                    )
                    .to_string());
                }
            };
            Ok((
                database,
                format!(
                    "db.getCollection({}).find({{}})",
                    json_string(&collection.name)
                ),
            ))
        }
        Engine::Elasticsearch => match parts {
            [index] => Ok((
                server.database.clone(),
                format!(
                    "GET /{}/_search\n{{\"size\": {ELASTICSEARCH_WINDOW}}}",
                    index.name
                ),
            )),
            _ => Err(tr("Write the index as server.index.").to_string()),
        },
        Engine::Redis => {
            let (database, pattern) = match parts {
                [p] => (server.database.clone(), p),
                [d, p] => (d.name.clone(), p),
                _ => {
                    return Err(tr(
                        "Write the keys as server.\"pattern\" or server.database.\"pattern\", like cache.\"user:*\".",
                    )
                    .to_string());
                }
            };
            Ok((
                database,
                format!("POSTPILOT KEYS {}", Engine::Redis.ident(&pattern.name)),
            ))
        }
    }
}

/// Os nomes definidos num `WITH nome AS (...)`: são tabelas da própria
/// consulta, não de um servidor.
fn cte_names(sql: &str, tokens: &[Token]) -> Vec<String> {
    let name = |t: &Token| match &t.kind {
        Kind::Word => Some(sql[t.start..t.end].to_lowercase()),
        Kind::Quoted(n) => Some(n.to_lowercase()),
        _ => None,
    };
    let is_word = |t: Option<&Token>, w: &str| {
        t.is_some_and(|t| t.kind == Kind::Word && sql[t.start..t.end].eq_ignore_ascii_case(w))
    };
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if !is_word(Some(t), "as") || i == 0 {
            continue;
        }
        // `AS (`, `AS MATERIALIZED (` ou `AS NOT MATERIALIZED (`.
        let mut j = i + 1;
        if is_word(tokens.get(j), "not") {
            j += 1;
        }
        if is_word(tokens.get(j), "materialized") {
            j += 1;
        }
        if tokens.get(j).is_none_or(|t| t.kind != Kind::Punct('(')) {
            continue;
        }
        // Antes do `AS`: o nome, ou o nome com a lista de colunas.
        let mut k = i - 1;
        if tokens[k].kind == Kind::Punct(')') {
            let mut depth = 0usize;
            loop {
                match tokens[k].kind {
                    Kind::Punct(')') => depth += 1,
                    Kind::Punct('(') => depth -= 1,
                    _ => {}
                }
                if depth == 0 || k == 0 {
                    break;
                }
                k -= 1;
            }
            if k == 0 {
                continue;
            }
            k -= 1;
        }
        if let Some(n) = name(&tokens[k]) {
            out.push(n);
        }
    }
    out
}

/// Monta o plano de `sql` com as conexões guardadas `servers`. Sem servidor
/// da janela: toda tabela precisa dizer o servidor.
pub fn plan(sql: &str, servers: &[Server]) -> Result<Plan, PlanError> {
    plan_with(sql, servers, None)
}

/// Monta o plano de `sql` com as conexões guardadas `servers`; as tabelas
/// sem servidor na frente são do servidor da janela, `home`.
pub fn plan_with(sql: &str, servers: &[Server], home: Option<&Home>) -> Result<Plan, PlanError> {
    let tokens = lex(sql)?;
    let ctes = cte_names(sql, &tokens);
    let word = |t: &Token| match t.kind {
        Kind::Word => Some(sql[t.start..t.end].to_lowercase()),
        _ => None,
    };
    let part = |t: &Token| match &t.kind {
        // O número é o banco do Redis (`cache.3`).
        Kind::Word | Kind::Number => Some(Part {
            name: sql[t.start..t.end].to_string(),
            quoted: false,
        }),
        Kind::Quoted(name) => Some(Part {
            name: name.clone(),
            quoted: true,
        }),
        _ => None,
    };
    let fail = |message: String, t: &Token| PlanError {
        message,
        position: sql[..t.start].chars().count() + 1,
    };

    let mut plan = Plan::default();
    // Trocas no texto: (início, fim, o que entra).
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    // Para cada nível de parênteses, se está na lista de tabelas do FROM.
    let mut in_from = vec![false];
    // O sinal ou palavra antes do token atual.
    let mut before: Option<String> = None;
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        let here = word(t);
        let table_position = matches!(before.as_deref(), Some("from" | "join"))
            || (before.as_deref() == Some(",") && *in_from.last().unwrap_or(&false));
        if table_position
            && t.kind != Kind::Number
            && let Some(first) = part(t)
        {
            // O endereço: nomes separados por ponto.
            let mut parts = vec![first];
            let mut j = i + 1;
            while tokens.get(j).is_some_and(|t| t.kind == Kind::Punct('.'))
                && let Some(next) = tokens.get(j + 1).and_then(part)
            {
                parts.push(next);
                j += 2;
            }
            // `( $$ texto $$ )` logo depois: consulta na língua do servidor.
            let native = match (tokens.get(j), tokens.get(j + 1), tokens.get(j + 2)) {
                (Some(open), Some(text), Some(close))
                    if open.kind == Kind::Punct('(') && close.kind == Kind::Punct(')') =>
                {
                    match &text.kind {
                        Kind::Text(body) => Some(body.clone()),
                        _ => None,
                    }
                }
                _ => None,
            };
            let local = matches!(parts[0].name.to_lowercase().as_str(), "main" | "temp")
                && !parts[0].quoted;
            // O servidor que o endereço diz; sem ele, a tabela é do
            // servidor da janela.
            let named =
                find_server(servers, &parts[0]).filter(|_| native.is_some() || parts.len() >= 2);
            let call = tokens.get(j).is_some_and(|t| t.kind == Kind::Punct('('));
            let own = parts.len() == 1
                && (call
                    || ctes.contains(&parts[0].name.to_lowercase())
                    || parts[0].name.to_lowercase().starts_with("sqlite_"));
            // (servidor, banco do servidor, partes depois do servidor)
            let target: Option<(usize, String, &[Part])> = match (named, home) {
                (Some(i), _) => Some((i, servers[i].database.clone(), &parts[1..])),
                // `json_each(...)`, um nome do WITH, `main.x`: do SQLite.
                (None, _) if local || own => None,
                (None, Some(home)) if servers.get(home.server).is_some() => {
                    Some((home.server, home.database.clone(), &parts[..]))
                }
                (None, _) => {
                    let known: Vec<String> =
                        servers.iter().map(|s| written_name(&s.name)).collect();
                    let message = if parts.len() == 1 && !known.is_empty() {
                        trf!(
                            "The table \"{name}\" does not say its server. Write server.{name}; the servers are: {servers}.",
                            name = parts[0].name,
                            servers = known.join(", ")
                        )
                    } else if known.is_empty() {
                        trf!(
                            "\"{name}\" is not a saved connection. Register the servers in the workspace or in Registered Servers.",
                            name = parts[0].name
                        )
                    } else {
                        trf!(
                            "\"{name}\" is not a saved connection. The servers are: {servers}.",
                            name = parts[0].name,
                            servers = known.join(", ")
                        )
                    };
                    return Err(fail(message, t));
                }
            };
            if let Some((server_index, default_database, rest)) = target {
                let server = &Server {
                    database: default_database,
                    ..servers[server_index].clone()
                };
                let end_token = if native.is_some() { j + 2 } else { j - 1 };
                let (database, query, whole) = match native {
                    Some(body) => {
                        let database = match rest.len() {
                            0 => server.database.clone(),
                            1 => rest[0].name.clone(),
                            _ => {
                                return Err(fail(
                                    tr("Write a server query as server($$ ... $$) or server.database($$ ... $$).")
                                        .to_string(),
                                    t,
                                ));
                            }
                        };
                        let body = body.trim().to_string();
                        if body.is_empty() {
                            return Err(fail(tr("The server query is empty.").to_string(), t));
                        }
                        (database, body, false)
                    }
                    None => {
                        let (database, query) = direct(server, rest).map_err(|m| fail(m, t))?;
                        (database, query, true)
                    }
                };
                let label = sql[t.start..tokens[end_token].end]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let existing = plan
                    .sources
                    .iter()
                    .find(|s| {
                        s.server == server_index && s.database == database && s.query == query
                    })
                    .map(|s| s.table.clone());
                let relation = (whole
                    && matches!(server.engine, Engine::PostgreSql | Engine::SqlServer))
                .then(|| query.strip_prefix("SELECT * FROM ").map(str::to_string))
                .flatten();
                if let Some(s) = plan.sources.iter_mut().find(|s| {
                    s.server == server_index && s.database == database && s.query == query
                }) {
                    s.uses += 1;
                }
                let table = match existing {
                    Some(table) => table,
                    None => {
                        let table = format!("pp_{}", plan.sources.len() + 1);
                        plan.sources.push(Source {
                            server: server_index,
                            database,
                            query,
                            label,
                            table: table.clone(),
                            whole,
                            relation,
                            uses: 1,
                        });
                        table
                    }
                };
                // Sem apelido, a tabela fica com o último nome do endereço,
                // para `clientes.nome` continuar valendo.
                let has_alias = match tokens.get(end_token + 1) {
                    Some(next) => match &next.kind {
                        Kind::Quoted(_) => true,
                        Kind::Word => !AFTER_TABLE
                            .contains(&sql[next.start..next.end].to_lowercase().as_str()),
                        _ => false,
                    },
                    None => false,
                };
                let mut replacement = format!("main.\"{table}\"");
                if !has_alias && whole {
                    let last = &parts[parts.len() - 1].name;
                    replacement.push_str(&format!(" AS \"{}\"", last.replace('"', "\"\"")));
                }
                edits.push((t.start, tokens[end_token].end, replacement));
                before = Some(String::new());
                i = end_token + 1;
                continue;
            }
        }
        match (&t.kind, here.as_deref()) {
            (Kind::Punct('('), _) => in_from.push(false),
            (Kind::Punct(')'), _) => {
                if in_from.len() > 1 {
                    in_from.pop();
                }
            }
            (_, Some("from")) => {
                if let Some(top) = in_from.last_mut() {
                    *top = true;
                }
            }
            (_, Some(w)) if END_OF_FROM.contains(&w) => {
                if let Some(top) = in_from.last_mut() {
                    *top = false;
                }
            }
            _ => {}
        }
        before = Some(match &t.kind {
            Kind::Punct(c) => c.to_string(),
            _ => here.unwrap_or_default(),
        });
        i += 1;
    }

    // Os comandos, separados por `;` fora de parênteses, com as trocas.
    let mut depth = 0usize;
    let mut starts = vec![0usize];
    for t in &tokens {
        match t.kind {
            Kind::Punct('(') => depth += 1,
            Kind::Punct(')') => depth = depth.saturating_sub(1),
            Kind::Punct(';') if depth == 0 => starts.push(t.end),
            _ => {}
        }
    }
    starts.push(sql.len() + 1);
    for pair in starts.windows(2) {
        let (from, to) = (pair[0], (pair[1] - 1).min(sql.len()));
        let has_code = tokens
            .iter()
            .any(|t| t.start >= from && t.end <= to && t.kind != Kind::Punct(';'));
        if !has_code {
            continue;
        }
        let mut text = String::new();
        let mut at = from;
        for (start, end, replacement) in edits.iter().filter(|e| e.0 >= from && e.1 <= to) {
            text.push_str(&sql[at..*start]);
            text.push_str(replacement);
            at = *end;
        }
        text.push_str(&sql[at..to]);
        plan.statements.push(Statement {
            sql: text,
            offset: sql[..from].chars().count(),
        });
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servers() -> Vec<Server> {
        let s = |name: &str, engine, database: &str| Server {
            name: name.into(),
            engine,
            database: database.into(),
        };
        vec![
            s("PG Vendas", Engine::PostgreSql, "loja"),
            s("financeiro", Engine::SqlServer, "erp"),
            s("mongo", Engine::MongoDb, "app"),
            s("busca", Engine::Elasticsearch, "cluster"),
            s("cache", Engine::Redis, "0"),
        ]
    }

    #[test]
    fn a_table_address_becomes_a_source_read_whole() {
        let p = plan(
            "select c.nome, p.total from pg_vendas.public.clientes c\n join financeiro.erp2.dbo.pedidos p on p.cliente = c.id",
            &servers(),
        )
        .unwrap();
        assert_eq!(p.sources.len(), 2);
        assert_eq!(p.sources[0].server, 0);
        assert_eq!(p.sources[0].database, "loja");
        assert_eq!(p.sources[0].query, "SELECT * FROM public.clientes");
        assert_eq!(p.sources[1].database, "erp2");
        assert_eq!(p.sources[1].query, "SELECT * FROM dbo.pedidos");
        assert_eq!(
            p.statements[0].sql,
            "select c.nome, p.total from main.\"pp_1\" c\n join main.\"pp_2\" p on p.cliente = c.id"
        );
    }

    #[test]
    fn names_with_spaces_go_in_brackets_and_keep_their_quotes() {
        let p = plan(
            "select * from [PG Vendas].\"Loja Nova\".public.\"Itens\"",
            &servers(),
        )
        .unwrap();
        assert_eq!(p.sources[0].database, "Loja Nova");
        assert_eq!(p.sources[0].query, "SELECT * FROM public.\"Itens\"");
        // Sem apelido, vale o nome da tabela.
        assert_eq!(
            p.statements[0].sql,
            "select * from main.\"pp_1\" AS \"Itens\""
        );
    }

    #[test]
    fn a_server_query_runs_as_written() {
        let p = plan(
            "select * from pg_vendas($$ select id from clientes where nome = 'a;b' $$) c \
             join mongo.app($$ db.sessoes.find({ativo: true}) $$) s on s.usuario = c.id \
             join cache.3('HGETALL usuario:1') h on 1 = 1",
            &servers(),
        )
        .unwrap();
        assert_eq!(
            p.sources[0].query,
            "select id from clientes where nome = 'a;b'"
        );
        assert!(!p.sources[0].whole);
        assert_eq!(p.sources[1].query, "db.sessoes.find({ativo: true})");
        assert_eq!(p.sources[1].database, "app");
        assert_eq!(p.sources[2].database, "3");
        assert_eq!(p.sources[2].query, "HGETALL usuario:1");
        assert_eq!(p.statements.len(), 1);
        assert!(p.statements[0].sql.contains("main.\"pp_3\" h on 1 = 1"));
    }

    #[test]
    fn each_engine_has_its_way_to_read_everything() {
        let p = plan(
            "select * from mongo.pedidos m, busca.\"logs-*\" l, cache.\"user:*\" r, mongo.outro.itens",
            &servers(),
        )
        .unwrap();
        assert_eq!(p.sources[0].query, "db.getCollection(\"pedidos\").find({})");
        assert_eq!(p.sources[1].query, "GET /logs-*/_search\n{\"size\": 10000}");
        assert_eq!(p.sources[2].query, "POSTPILOT KEYS user:*");
        assert_eq!(p.sources[3].database, "outro");
    }

    #[test]
    fn columns_and_local_tables_are_left_alone() {
        let sql = "with t as (select 1 a) select mongo.x, t.a from t, json_each('[1]') j where mongo.y in (select cache.z from t)";
        let p = plan(sql, &servers()).unwrap();
        assert!(p.sources.is_empty());
        assert_eq!(p.statements[0].sql, sql);
    }

    #[test]
    fn the_same_source_is_read_once() {
        let p = plan(
            "select * from mongo.pedidos a join mongo.pedidos b on a.x = b.y",
            &servers(),
        )
        .unwrap();
        assert_eq!(p.sources.len(), 1);
    }

    #[test]
    fn an_unknown_server_is_an_error_with_the_position() {
        let e = plan("select * from nada.public.t", &servers()).unwrap_err();
        assert_eq!(e.position, 15);
        assert!(e.message.contains("nada"), "{}", e.message);
        assert!(e.message.contains("\"PG Vendas\""), "{}", e.message);
    }

    #[test]
    fn statements_split_on_semicolons() {
        let p = plan("select 1; -- nada\n select ';' from mongo.a;\n", &servers()).unwrap();
        assert_eq!(p.statements.len(), 2);
        assert_eq!(p.statements[1].offset, 9);
        assert!(p.statements[1].sql.contains("main.\"pp_1\" AS \"a\""));
    }

    #[test]
    fn written_names() {
        assert_eq!(written_name("pg_vendas"), "pg_vendas");
        assert_eq!(written_name("PG Vendas"), "\"PG Vendas\"");
        assert_eq!(written_name("order"), "\"order\"");
    }

    #[test]
    fn a_table_without_a_server_is_of_the_window_server() {
        let servers = servers();
        let home = Home {
            server: 0,
            database: "tiibcontrol".into(),
        };
        let p = plan_with(
            "with recentes as (select * from documento where id > 1) \
             select * from recentes r join public.caixa c on c.id = r.caixa_id \
             join json_each('[1]') j on 1 = 1 join main.x on 1 = 1 join outro.dbo.t t on 1 = 1",
            &servers,
            Some(&home),
        )
        .expect("plan");
        let got: Vec<(usize, &str, &str)> = p
            .sources
            .iter()
            .map(|s| (s.server, s.database.as_str(), s.query.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (0, "tiibcontrol", "SELECT * FROM documento"),
                (0, "tiibcontrol", "SELECT * FROM public.caixa"),
                (0, "outro", "SELECT * FROM dbo.t"),
            ]
        );
        assert_eq!(p.sources[0].relation.as_deref(), Some("documento"));
        assert_eq!(p.sources[0].uses, 1);
        let sql = &p.statements[0].sql;
        assert!(
            sql.contains("from main.\"pp_1\" AS \"documento\" where"),
            "{sql}"
        );
        assert!(sql.contains("from recentes r"), "{sql}");
        assert!(sql.contains("json_each('[1]') j"), "{sql}");
        assert!(sql.contains("join main.x on"), "{sql}");
    }

    #[test]
    fn without_a_window_server_a_bare_table_says_how_to_write_it() {
        let e = plan("select * from documento d", &servers()).unwrap_err();
        assert!(
            e.message
                .starts_with("The table \"documento\" does not say its server."),
            "{}",
            e.message
        );
        assert_eq!(e.position, 15);
    }
}
