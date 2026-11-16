//! Sugestões da IA para o comando em edição: o que a IA conectada propõe
//! depois de ler o comando do cursor e o catálogo (um JOIN pela chave
//! estrangeira, um filtro, um índice, um conserto, o resto do comando).
//!
//! O editor só mostra e aplica: quem pergunta à IA, e quando, é a casca.
//! As sugestões valem para o comando em `range` enquanto ele for `base`;
//! aplicar uma troca o comando inteiro (ou acrescenta outro depois dele)
//! num passo só do desfazer.

use std::ops::Range;

use pgstudio_i18n::tr;

use crate::lexer::{Dialect, ELASTICSEARCH_METHODS, TokenKind, tokenize_in};

/// O assunto de uma sugestão (o texto cinza da lista).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HintKind {
    Join,
    Filter,
    Columns,
    Aggregate,
    Sort,
    Performance,
    Index,
    Fix,
    Safety,
    Complete,
    Idiom,
    #[default]
    Other,
}

impl HintKind {
    /// Todos, na ordem do pedido à IA.
    pub const ALL: [HintKind; 12] = [
        HintKind::Join,
        HintKind::Filter,
        HintKind::Columns,
        HintKind::Aggregate,
        HintKind::Sort,
        HintKind::Performance,
        HintKind::Index,
        HintKind::Fix,
        HintKind::Safety,
        HintKind::Complete,
        HintKind::Idiom,
        HintKind::Other,
    ];

    /// O nome que a IA usa na resposta.
    pub fn id(self) -> &'static str {
        match self {
            HintKind::Join => "join",
            HintKind::Filter => "filter",
            HintKind::Columns => "columns",
            HintKind::Aggregate => "aggregate",
            HintKind::Sort => "sort",
            HintKind::Performance => "performance",
            HintKind::Index => "index",
            HintKind::Fix => "fix",
            HintKind::Safety => "safety",
            HintKind::Complete => "complete",
            HintKind::Idiom => "idiom",
            HintKind::Other => "other",
        }
    }

    /// Lê o nome da resposta; o que não conhece vira [`HintKind::Other`].
    pub fn parse(id: &str) -> HintKind {
        let id = id.trim().to_ascii_lowercase();
        HintKind::ALL
            .into_iter()
            .find(|k| k.id() == id)
            .unwrap_or_default()
    }

    /// Nome na interface.
    pub fn label(self) -> &'static str {
        match self {
            HintKind::Join => tr("Join"),
            HintKind::Filter => tr("Filter"),
            HintKind::Columns => tr("Columns"),
            HintKind::Aggregate => tr("Summary"),
            HintKind::Sort => tr("Order and limit"),
            HintKind::Performance => tr("Performance"),
            HintKind::Index => tr("Index"),
            HintKind::Fix => tr("Fix"),
            HintKind::Safety => tr("Safety"),
            HintKind::Complete => tr("Complete"),
            HintKind::Idiom => tr("Better way"),
            HintKind::Other => tr("Idea"),
        }
    }
}

/// O que aplicar uma sugestão faz no texto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintAction {
    /// Troca o comando por este.
    Replace(String),
    /// Acrescenta este comando depois do atual.
    Insert(String),
    /// Só um conselho.
    None,
}

impl HintAction {
    /// O código que a sugestão traz, se traz.
    pub fn code(&self) -> Option<&str> {
        match self {
            HintAction::Replace(code) | HintAction::Insert(code) => Some(code),
            HintAction::None => None,
        }
    }
}

/// Uma sugestão da IA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub kind: HintKind,
    /// Uma linha: o que fazer.
    pub title: String,
    /// Uma ou duas frases: por quê.
    pub detail: String,
    pub action: HintAction,
}

/// As sugestões para um comando do texto.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    pub items: Vec<Hint>,
    /// Onde o comando está no texto, em bytes.
    pub range: Range<usize>,
    /// O comando que a IA leu: as sugestões só valem enquanto o texto em
    /// `range` for este.
    pub base: String,
    /// Quem sugeriu ("Claude Code", o nome do provedor).
    pub source: String,
}

/// O que o aviso no canto do editor mostra quando não há sugestões.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HintStatus {
    /// Sem IA conectada, ou sugestões desligadas: nada.
    #[default]
    Off,
    /// Esperando a pessoa parar de digitar: nada.
    Idle,
    /// Só quando a pessoa pede: o aviso mostra o atalho.
    Ask,
    /// A IA está lendo o comando.
    Thinking,
    /// A IA não respondeu; a mensagem aparece ao parar o mouse.
    Failed(String),
}

/// As sugestões dentro do editor.
#[derive(Clone, Debug, Default)]
pub(crate) struct HintState {
    pub hints: Option<Hints>,
    pub status: HintStatus,
    /// Lista aberta: a linha escolhida.
    pub open: Option<usize>,
    /// Abrir a lista no próximo frame (atalho, menu).
    pub want_open: bool,
    /// A pessoa pediu sugestões e não há nenhuma: a casca pergunta à IA.
    pub requested: bool,
    /// Sugestão aplicada neste frame.
    pub applied: Option<Hint>,
    /// Atalho do comando, para o aviso.
    pub shortcut: String,
}

impl HintState {
    /// Quantas sugestões há para mostrar.
    pub fn count(&self) -> usize {
        self.hints.as_ref().map_or(0, |h| h.items.len())
    }
}

/// O comando em que o cursor (byte `at`) está: o que contém o cursor ou,
/// entre dois comandos, o de antes. Em bytes, sem o `;` nem os espaços em
/// volta. `None` num texto sem comando nenhum.
pub fn statement_at(text: &str, dialect: Dialect, at: usize) -> Option<Range<usize>> {
    let all = match dialect {
        Dialect::Postgres | Dialect::TSql => sql_statements(text, dialect),
        Dialect::Redis => line_statements(text),
        Dialect::MongoDb => mongo_statements(text),
        Dialect::Elasticsearch => elasticsearch_statements(text),
    };
    all.iter()
        .rev()
        .find(|r| r.start <= at)
        .or(all.first())
        .cloned()
}

/// Comandos SQL: separados por `;` e, no T-SQL, pelas linhas `GO`.
fn sql_statements(text: &str, dialect: Dialect) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for t in tokenize_in(text, dialect) {
        let word = &text[t.range.clone()];
        match t.kind {
            TokenKind::Whitespace | TokenKind::Comment => {}
            TokenKind::Punctuation if word == ";" => out.extend(current.take()),
            _ if dialect == Dialect::TSql
                && word.eq_ignore_ascii_case("go")
                && alone_on_line(text, &t.range) =>
            {
                out.extend(current.take())
            }
            _ => match &mut current {
                Some(r) => r.end = t.range.end,
                None => current = Some(t.range.clone()),
            },
        }
    }
    out.extend(current);
    out
}

fn alone_on_line(text: &str, range: &Range<usize>) -> bool {
    let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |i| range.end + i);
    text[start..range.start].trim().is_empty() && text[range.end..end].trim().is_empty()
}

/// Um comando por linha (Redis); linhas de comentário ficam de fora.
fn line_statements(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !["#", "--", "//"].iter().any(|c| trimmed.starts_with(c)) {
            let lead = line.len() - line.trim_start().len();
            out.push(start + lead..start + lead + trimmed.len());
        }
        start += line.len();
    }
    out
}

/// Comandos do mongosh: terminam no `;` ou no fim da linha, fora de
/// parênteses, chaves, colchetes e textos; uma linha que começa com `.`
/// continua a chamada de cima.
fn mongo_statements(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut end = 0;
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' | b'\'' | b'`' => {
                start.get_or_insert(i);
                i += 1;
                while i < bytes.len() && bytes[i] != b {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                end = (i + 1).min(bytes.len());
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |j| i + 2 + j + 2);
                continue;
            }
            b'(' | b'[' | b'{' => {
                start.get_or_insert(i);
                depth += 1;
                end = i + 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                end = i + 1;
            }
            b';' if depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(s..end);
                }
            }
            b'\n' if depth == 0 => {
                let chained = text[i + 1..].trim_start().starts_with('.');
                if !chained && let Some(s) = start.take() {
                    out.push(s..end);
                }
            }
            _ if b.is_ascii_whitespace() => {}
            _ => {
                start.get_or_insert(i);
                end = i + 1;
            }
        }
        i += 1;
    }
    if let Some(s) = start {
        out.push(s..end.max(s).min(bytes.len()));
    }
    // Um corte no meio de um caractere não pode acontecer: os limites são
    // sempre bytes ASCII ou o fim do texto.
    out.retain(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    out
}

/// Requisições do console: cada uma começa numa linha `MÉTODO /caminho` e
/// vai até a próxima.
fn elasticsearch_statements(text: &str) -> Vec<Range<usize>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let word = trimmed.split_whitespace().next().unwrap_or("");
        if ELASTICSEARCH_METHODS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(word))
        {
            starts.push(offset + line.len() - trimmed.len());
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            s..s + text[s..end].trim_end().len()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, dialect: Dialect, cursor: usize) -> &str {
        statement_at(text, dialect, cursor).map_or("", |r| &text[r])
    }

    #[test]
    fn sql_statement_under_the_cursor() {
        let text = "select 1;\n\nselect * from pedidos p\nwhere p.total > 10 ;  \nselect 3";
        assert_eq!(at(text, Dialect::Postgres, 3), "select 1");
        let second = "select * from pedidos p\nwhere p.total > 10";
        assert_eq!(at(text, Dialect::Postgres, 20), second);
        // Entre dois comandos, o de antes.
        assert_eq!(
            at(text, Dialect::Postgres, text.find("  \n").unwrap()),
            second
        );
        assert_eq!(at(text, Dialect::Postgres, text.len()), "select 3");
        assert_eq!(at("  \n", Dialect::Postgres, 1), "");
        // Um `;` dentro de um texto não separa.
        assert_eq!(
            at("select ';' as x", Dialect::Postgres, 4),
            "select ';' as x"
        );
    }

    #[test]
    fn tsql_batches_end_at_go() {
        let text = "SELECT TOP 5 * FROM Vendas\nGO\nSELECT 2\ngo\n";
        assert_eq!(at(text, Dialect::TSql, 5), "SELECT TOP 5 * FROM Vendas");
        assert_eq!(at(text, Dialect::TSql, text.len()), "SELECT 2");
    }

    #[test]
    fn redis_commands_are_lines() {
        let text = "GET user:1\n# nota\n  HGETALL pedido:7  \n";
        assert_eq!(at(text, Dialect::Redis, 2), "GET user:1");
        assert_eq!(at(text, Dialect::Redis, text.len()), "HGETALL pedido:7");
    }

    #[test]
    fn mongo_calls_span_lines_and_chains() {
        let text = "use loja\ndb.pedidos.find({ total: { $gte: 100 },\n  nome: \"a;b\" })\n  .sort({ data: -1 })\n\ndb.clientes.countDocuments({}); db.x.drop()";
        assert_eq!(at(text, Dialect::MongoDb, 2), "use loja");
        assert_eq!(
            at(text, Dialect::MongoDb, 30),
            "db.pedidos.find({ total: { $gte: 100 },\n  nome: \"a;b\" })\n  .sort({ data: -1 })"
        );
        assert_eq!(at(text, Dialect::MongoDb, text.len()), "db.x.drop()");
        assert_eq!(
            at(
                "db.vendas.find({ cidade: \"São Paulo\" ",
                Dialect::MongoDb,
                5
            ),
            "db.vendas.find({ cidade: \"São Paulo\""
        );
    }

    #[test]
    fn elasticsearch_requests_start_at_the_method() {
        let text =
            "GET /pedidos/_search\n{\n  \"query\": { \"match_all\": {} }\n}\n\nPOST /_bulk\n{}\n";
        assert_eq!(
            at(text, Dialect::Elasticsearch, 30),
            "GET /pedidos/_search\n{\n  \"query\": { \"match_all\": {} }\n}"
        );
        assert_eq!(
            at(text, Dialect::Elasticsearch, text.len()),
            "POST /_bulk\n{}"
        );
    }

    #[test]
    fn kinds_come_from_the_answer() {
        assert_eq!(HintKind::parse(" Join "), HintKind::Join);
        assert_eq!(HintKind::parse("whatever"), HintKind::Other);
        for kind in HintKind::ALL {
            assert_eq!(HintKind::parse(kind.id()), kind);
        }
    }
}
