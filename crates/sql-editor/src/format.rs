//! Formatação de SQL (Format Document, Shift+Alt+F).
//!
//! O formatador trabalha sobre os tokens do [`lexer`](crate::lexer), não
//! sobre uma árvore de sintaxe: assim os comentários ficam onde estavam e
//! SQL que nenhum parser entende (dialetos, trechos incompletos, `$$`
//! corpos de função) continua formatável. Ele só troca espaços em branco
//! entre tokens e a caixa das palavras-chave; strings, nomes entre aspas,
//! comentários e corpos `$$` saem byte a byte como entraram.
//!
//! Para garantir que o significado não muda, o resultado é tokenizado de
//! novo e comparado com o original (mesmos tokens, na mesma ordem, só a
//! caixa das palavras-chave pode mudar). Se algo não bater, ou se o texto
//! tiver string, comentário ou parêntese sem fecho, nada é formatado e o
//! erro diz por quê.

use crate::diagnostics;
use crate::edit::{self, Edited};
use crate::lexer::{TokenKind, tokenize};

/// Caixa das palavras-chave no resultado.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeywordCase {
    /// `SELECT`, `FROM` (padrão, como no SSMS).
    #[default]
    Upper,
    /// `select`, `from`.
    Lower,
    /// Como foram digitadas.
    Preserve,
}

/// Opções do formatador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatOptions {
    /// Espaços por nível de indentação.
    pub indent: usize,
    pub keyword_case: KeywordCase,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            indent: 4,
            keyword_case: KeywordCase::Upper,
        }
    }
}

/// Por que o texto não foi formatado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError {
    pub message: String,
    /// Onde está o problema, em bytes do texto formatado (se houver um
    /// lugar).
    pub at: Option<usize>,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FormatError {}

/// Formata `text` inteiro.
pub fn format(text: &str, opts: &FormatOptions) -> Result<String, FormatError> {
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    let tokens = tokenize(text);
    if let Some(d) = diagnostics::check(text, &tokens).into_iter().next() {
        return Err(FormatError {
            message: d.message,
            at: Some(d.range.start),
        });
    }
    let items = items(text);
    check_supported(&items)?;
    let mut p = Printer::new(&items, opts);
    p.run();
    let mut out = p.out;
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    if text.trim_end() != text && text[text.trim_end().len()..].contains('\n') {
        out.push('\n');
    }
    verify(text, &out)?;
    Ok(out)
}

/// Formata a seleção `[a, b]` (caracteres, em qualquer ordem), estendida a
/// linhas inteiras, ou o documento inteiro se a seleção estiver vazia. A
/// seleção nova cobre as linhas formatadas; sem seleção, o cursor fica no
/// mesmo ponto do código.
pub fn format_selection(
    text: &str,
    a: usize,
    b: usize,
    opts: &FormatOptions,
) -> Result<Edited, FormatError> {
    if a == b {
        let new = format(text, opts)?;
        let c = map_position(text, &new, a);
        return Ok(Edited {
            text: new,
            selection: (c, c),
        });
    }
    let lines = edit::selected_lines(text, a, b);
    let start = edit::byte_of(text, edit::line_start(text, lines.start));
    let end = if lines.end >= edit::line_count(text) {
        text.len()
    } else {
        edit::byte_of(text, edit::line_start(text, lines.end)) - 1
    };
    let chunk = &text[start..end];
    let prefix: String = chunk
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l[..l.len() - l.trim_start().len()].to_string())
        .unwrap_or_default();
    let formatted = format(chunk, opts).map_err(|e| FormatError {
        message: e.message,
        at: e.at.map(|at| start + at),
    })?;
    let indented: Vec<String> = formatted
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect();
    let replacement = indented.join("\n");
    let mut new = String::with_capacity(text.len() + replacement.len());
    new.push_str(&text[..start]);
    new.push_str(&replacement);
    new.push_str(&text[end..]);
    let sel_start = edit::char_of(&new, start);
    let sel_end = sel_start + replacement.trim_end().chars().count();
    Ok(Edited {
        text: new,
        selection: (sel_start, sel_end),
    })
}

/// A posição em `new` que corresponde ao caractere `pos` de `old`, contando
/// só os caracteres que não são espaço (a formatação só mexe nos espaços).
pub fn map_position(old: &str, new: &str, pos: usize) -> usize {
    let solid = old
        .chars()
        .take(pos)
        .filter(|c| !c.is_whitespace())
        .count();
    // Cursor colado no token seguinte fica antes dele; senão, logo depois
    // do anterior.
    let before_next = old.chars().nth(pos).is_some_and(|c| !c.is_whitespace());
    let mut seen = 0;
    for (i, c) in new.chars().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        if before_next && seen == solid {
            return i;
        }
        seen += 1;
        if !before_next && seen == solid {
            return i + 1;
        }
    }
    if solid == 0 {
        return 0;
    }
    new.chars().count()
}

/// Um token que não é espaço, com o que havia antes dele.
#[derive(Clone, Debug)]
struct Item<'a> {
    kind: TokenKind,
    text: &'a str,
    start: usize,
    /// Quebras de linha no espaço antes dele.
    newlines: usize,
    /// Colado no token anterior (sem espaço entre eles).
    glued: bool,
}

fn items(text: &str) -> Vec<Item<'_>> {
    let mut out = Vec::new();
    let mut newlines = 0;
    let mut glued = false;
    for t in tokenize(text) {
        let piece = &text[t.range.clone()];
        if t.kind == TokenKind::Whitespace {
            newlines += piece.matches('\n').count();
            glued = false;
            continue;
        }
        out.push(Item {
            kind: t.kind,
            text: piece,
            start: t.range.start,
            newlines,
            glued,
        });
        newlines = 0;
        glued = true;
    }
    out
}

/// Trechos em que espaço e quebra de linha fazem parte do comando e que o
/// formatador não sabe tratar.
fn check_supported(items: &[Item<'_>]) -> Result<(), FormatError> {
    for (i, it) in items.iter().enumerate() {
        if it.kind == TokenKind::Other && it.text.starts_with('\\') {
            return Err(FormatError {
                message: "psql meta-commands (\\...) can't be formatted.".to_string(),
                at: Some(it.start),
            });
        }
        if it.text.eq_ignore_ascii_case("stdin")
            && i > 0
            && items[i - 1].text.eq_ignore_ascii_case("from")
        {
            return Err(FormatError {
                message: "COPY ... FROM STDIN with inline data can't be formatted.".to_string(),
                at: Some(it.start),
            });
        }
    }
    Ok(())
}

/// Confere que `new` tem os mesmos tokens de `old`.
fn verify(old: &str, new: &str) -> Result<(), FormatError> {
    let a = items(old);
    let b = items(new);
    let fail = |at: Option<usize>| {
        Err(FormatError {
            message: "The SQL couldn't be formatted without changing it.".to_string(),
            at,
        })
    };
    if a.len() != b.len() {
        return fail(None);
    }
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        let same_text = if x.kind == TokenKind::Keyword {
            x.text.eq_ignore_ascii_case(y.text)
        } else {
            x.text == y.text
        };
        if x.kind != y.kind || !same_text {
            return fail(Some(x.start));
        }
        // 'a'
        // 'b' é uma string só ('ab'); na mesma linha, é erro.
        if i > 0
            && x.kind == TokenKind::String
            && a[i - 1].kind == TokenKind::String
            && (x.newlines > 0) != (y.newlines > 0)
        {
            return fail(Some(x.start));
        }
    }
    Ok(())
}

/// Espaço antes de um token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sep {
    None,
    Space,
    Line(usize),
    Blank(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameKind {
    /// O comando, no nível de fora.
    Statement,
    /// `( SELECT ... )`: quebra linhas como um comando.
    Subquery,
    /// Colunas de `CREATE TABLE t ( ... )`: uma por linha.
    Columns,
    /// Outros parênteses: tudo numa linha.
    Inline,
    /// `CASE ... END`.
    Case,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clause {
    None,
    Select,
    From,
    Where,
    Having,
    Set,
    Values,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    kind: FrameKind,
    /// Indentação do conteúdo.
    base: usize,
    /// Indentação da linha que abriu o bloco (onde fica o fecho).
    open: usize,
    clause: Clause,
    /// Depois de `BETWEEN`: o próximo `AND` é dele.
    between: bool,
    /// Tokens já escritos neste bloco.
    count: usize,
    /// `CASE` dentro de parênteses de uma linha não quebra.
    broken: bool,
}

impl Frame {
    fn new(kind: FrameKind, base: usize, open: usize, broken: bool) -> Frame {
        Frame {
            kind,
            base,
            open,
            clause: Clause::None,
            between: false,
            count: 0,
            broken,
        }
    }
}

/// O que fazer com um token: o espaço antes e a cláusula que ele abre.
struct Decision {
    sep: Sep,
    clause: Option<Clause>,
}

/// Operadores binários que ganham um espaço de cada lado.
const SPACED_OPERATORS: &[&str] = &[
    "=", "<>", "!=", "<", ">", "<=", ">=", "||", "+", "-", "*", "/", "%", "^", "->", "->>", "#>",
    "#>>", "@>", "<@", "&&", "~", "~*", "!~", "!~*", "~~", "!~~", "~~*", "!~~*", "?", "?|", "?&",
    "@@",
];

/// Palavras-chave que terminam um valor (o operador depois delas é
/// binário).
const VALUE_KEYWORDS: &[&str] = &[
    "null",
    "true",
    "false",
    "end",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "localtime",
    "localtimestamp",
    "session_user",
];

struct Printer<'a, 'o> {
    items: &'a [Item<'a>],
    opts: &'o FormatOptions,
    out: String,
    /// Indentação da linha atual.
    line_indent: usize,
    frames: Vec<Frame>,
    /// Último token que não é comentário, no comando atual.
    prev: Option<usize>,
    /// O último token escrito foi um operador binário espaçado.
    prev_binary: bool,
    /// O próximo token tem de ir para outra linha, com esta indentação
    /// (depois de um comentário `--`).
    force_line: Option<usize>,
    /// Acabou de passar um `;`.
    after_semicolon: bool,
    /// Primeira palavra-chave do comando.
    first_keyword: Option<String>,
    seen_table: bool,
    seen_as: bool,
    columns_done: bool,
}

impl<'a, 'o> Printer<'a, 'o> {
    fn new(items: &'a [Item<'a>], opts: &'o FormatOptions) -> Self {
        Printer {
            items,
            opts,
            out: String::new(),
            line_indent: 0,
            frames: vec![Frame::new(FrameKind::Statement, 0, 0, true)],
            prev: None,
            prev_binary: false,
            force_line: None,
            after_semicolon: false,
            first_keyword: None,
            seen_table: false,
            seen_as: false,
            columns_done: false,
        }
    }

    fn top(&self) -> &Frame {
        self.frames.last().expect("there is always a statement frame")
    }

    fn top_mut(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("there is always a statement frame")
    }

    /// Se o bloco atual quebra linhas.
    fn structural(&self) -> bool {
        let top = self.top();
        top.kind != FrameKind::Inline && top.broken
    }

    /// Indentação de uma linha que continua a anterior.
    fn cont_indent(&self) -> usize {
        let top = self.top();
        match top.kind {
            FrameKind::Inline => top.base,
            _ => top.base + self.opts.indent,
        }
    }

    fn run(&mut self) {
        for i in 0..self.items.len() {
            if self.items[i].kind == TokenKind::Comment {
                self.comment(i);
            } else {
                self.token(i);
            }
        }
    }

    fn comment(&mut self, i: usize) {
        let it = &self.items[i];
        let own_line = self.out.is_empty() || it.newlines > 0;
        let sep = if self.out.is_empty() {
            Sep::None
        } else if it.newlines == 0 {
            Sep::Space
        } else if self.after_semicolon {
            self.statement_sep(it)
        } else {
            let indent = match self.next_token(i).map(|j| self.decide(j).sep) {
                Some(Sep::Line(n) | Sep::Blank(n)) => n,
                _ => self.force_line.unwrap_or_else(|| self.cont_indent()),
            };
            Sep::Line(indent)
        };
        let sep = self.forced(sep);
        self.emit(sep, it.text);
        let line_comment = it.text.starts_with("--");
        let next_on_new_line = self.items.get(i + 1).is_some_and(|n| n.newlines > 0);
        if line_comment || (own_line && next_on_new_line) {
            self.force_line = Some(if own_line {
                self.line_indent
            } else {
                self.cont_indent()
            });
        }
    }

    fn token(&mut self, i: usize) {
        let it = &self.items[i];
        let decision = self.decide(i);
        let sep = if self.out.is_empty() {
            Sep::None
        } else if self.after_semicolon {
            self.statement_sep(it)
        } else {
            decision.sep
        };
        let sep = self.forced(sep);
        let text = self.cased(i);
        self.emit(sep, &text);
        self.after_semicolon = false;
        self.advance(i, decision.clause);
    }

    /// Espaço antes do primeiro item de um comando.
    fn statement_sep(&self, it: &Item<'_>) -> Sep {
        if it.newlines >= 2 {
            Sep::Blank(0)
        } else {
            Sep::Line(0)
        }
    }

    fn forced(&mut self, sep: Sep) -> Sep {
        match (self.force_line.take(), sep) {
            (Some(n), Sep::None | Sep::Space) => Sep::Line(n),
            (_, sep) => sep,
        }
    }

    fn next_token(&self, i: usize) -> Option<usize> {
        (i + 1..self.items.len()).find(|&j| self.items[j].kind != TokenKind::Comment)
    }

    fn keyword(&self, i: Option<usize>) -> Option<String> {
        let it = &self.items[i?];
        (it.kind == TokenKind::Keyword).then(|| it.text.to_ascii_lowercase())
    }

    fn is_punct(&self, i: Option<usize>, p: &str) -> bool {
        i.is_some_and(|i| self.items[i].kind == TokenKind::Punctuation && self.items[i].text == p)
    }

    /// Texto do token, com a caixa das palavras-chave.
    fn cased(&self, i: usize) -> String {
        let it = &self.items[i];
        if it.kind != TokenKind::Keyword {
            return it.text.to_string();
        }
        // `t.data`, `public.user`: é nome, não palavra-chave.
        let next = i + 1;
        let qualified = self.is_punct(self.prev, ".")
            || (self.items.get(next).is_some_and(|n| n.glued)
                && self.is_punct(Some(next).filter(|&n| n < self.items.len()), "."));
        match self.opts.keyword_case {
            _ if qualified => it.text.to_string(),
            KeywordCase::Upper => it.text.to_ascii_uppercase(),
            KeywordCase::Lower => it.text.to_ascii_lowercase(),
            KeywordCase::Preserve => it.text.to_string(),
        }
    }

    /// Se o token `p` termina um valor (o operador depois dele é binário).
    fn ends_value(&self, p: Option<usize>) -> bool {
        let Some(p) = p else {
            return false;
        };
        let it = &self.items[p];
        match it.kind {
            TokenKind::Identifier
            | TokenKind::QuotedIdentifier
            | TokenKind::Number
            | TokenKind::String
            | TokenKind::Parameter => true,
            TokenKind::Punctuation => it.text == ")" || it.text == "]",
            TokenKind::Keyword => VALUE_KEYWORDS.contains(&it.text.to_ascii_lowercase().as_str()),
            _ => false,
        }
    }

    fn decide(&self, i: usize) -> Decision {
        let it = &self.items[i];
        let p = self.prev;
        let top = *self.top();
        let line = |n| Decision {
            sep: Sep::Line(n),
            clause: None,
        };
        // Primeiro token dentro de `( SELECT` ou das colunas.
        if self.is_punct(p, "(") && matches!(top.kind, FrameKind::Subquery | FrameKind::Columns) {
            return Decision {
                sep: Sep::Line(top.base),
                clause: self.keyword_decision(i, &top).and_then(|d| d.clause),
            };
        }
        // 'a'
        // 'b': a quebra de linha junta as duas strings.
        if it.kind == TokenKind::String
            && it.newlines > 0
            && p.is_some_and(|p| p + 1 == i && self.items[p].kind == TokenKind::String)
        {
            return line(self.cont_indent());
        }
        if it.kind == TokenKind::Punctuation && it.text == ")" {
            let frame = self.frames.iter().rev().find(|f| f.kind != FrameKind::Case);
            if let Some(f) = frame
                && matches!(f.kind, FrameKind::Subquery | FrameKind::Columns)
            {
                return line(f.open);
            }
        }
        if let Some(d) = self.keyword_decision(i, &top) {
            return d;
        }
        if self.is_punct(p, ",") && self.structural() {
            if top.kind == FrameKind::Columns {
                return line(top.base);
            }
            if matches!(top.kind, FrameKind::Statement | FrameKind::Subquery)
                && matches!(top.clause, Clause::Select | Clause::Set | Clause::Values)
            {
                return line(top.base + self.opts.indent);
            }
        }
        Decision {
            sep: self.spacing(i),
            clause: None,
        }
    }

    /// Quebras de linha das palavras-chave: cláusulas, `JOIN`, `AND`/`OR`
    /// do `WHERE` e o `CASE`.
    fn keyword_decision(&self, i: usize, top: &Frame) -> Option<Decision> {
        let w = self.keyword(Some(i))?;
        if !self.structural() {
            return None;
        }
        let prev = self.keyword(self.prev);
        let next = self.keyword(self.next_token(i));
        let prev = prev.as_deref();
        let next = next.as_deref();
        let base = top.base;
        let at = |n: usize, clause: Option<Clause>| {
            Some(Decision {
                sep: Sep::Line(n),
                clause,
            })
        };
        if top.kind == FrameKind::Case {
            return match w.as_str() {
                "when" | "else" => at(base, None),
                "end" => at(top.open, None),
                _ => None,
            };
        }
        if top.kind == FrameKind::Columns {
            return None;
        }
        let first = top.count == 0;
        let clause = match w.as_str() {
            "select" => Some(Clause::Select),
            "from" if !matches!(prev, Some("delete" | "distinct")) => Some(Clause::From),
            "where" => Some(Clause::Where),
            "having" => Some(Clause::Having),
            "values" if prev != Some("default") => Some(Clause::Values),
            "set" if prev == Some("update")
                || (self.first_keyword.as_deref() == Some("update")
                    && self.frames.len() == 1
                    && top.clause == Clause::None) =>
            {
                Some(Clause::Set)
            }
            "group" | "order" if next == Some("by") && prev != Some("within") => {
                Some(Clause::Other)
            }
            "limit" | "offset" | "returning" | "window" | "union" | "intersect" | "except" => {
                Some(Clause::Other)
            }
            "on" if next == Some("conflict") => Some(Clause::Other),
            _ => None,
        };
        if let Some(c) = clause {
            return if first {
                Some(Decision {
                    sep: self.spacing(i),
                    clause: Some(c),
                })
            } else {
                at(base, Some(c))
            };
        }
        let join = match w.as_str() {
            "join" => !matches!(
                prev,
                Some("left" | "right" | "full" | "inner" | "cross" | "natural" | "outer")
            ),
            "left" | "right" | "full" => matches!(next, Some("join" | "outer")),
            "inner" | "cross" => next == Some("join"),
            "natural" => true,
            _ => false,
        };
        if join && !first {
            return at(base, Some(Clause::From));
        }
        let logical = match w.as_str() {
            "and" => !top.between,
            "or" => true,
            _ => false,
        };
        if logical && matches!(top.clause, Clause::Where | Clause::Having) {
            return at(base + self.opts.indent, None);
        }
        None
    }

    /// Espaço entre tokens numa mesma linha.
    fn spacing(&self, i: usize) -> Sep {
        let it = &self.items[i];
        let p = self.prev.map(|p| &self.items[p]);
        let punct = |t: &Item<'_>, s: &[&str]| t.kind == TokenKind::Punctuation && s.contains(&t.text);
        let op = |t: &Item<'_>, s: &str| t.kind == TokenKind::Operator && t.text == s;
        let keep = if it.glued { Sep::None } else { Sep::Space };
        let Some(p) = p else {
            return keep;
        };
        // Um comentário entre os dois: o espaço original é em relação a ele.
        let prev_is_last = self.prev == Some(i - 1);
        if punct(it, &[",", ";", ")", "]", "."]) && prev_is_last {
            return Sep::None;
        }
        if punct(p, &["(", "[", "."]) && prev_is_last {
            return Sep::None;
        }
        if (op(it, "::") || op(p, "::")) && prev_is_last {
            return Sep::None;
        }
        if punct(p, &[","]) {
            return Sep::Space;
        }
        if it.kind == TokenKind::Operator
            && SPACED_OPERATORS.contains(&it.text)
            && self.ends_value(self.prev)
        {
            return Sep::Space;
        }
        if self.prev_binary {
            return Sep::Space;
        }
        keep
    }

    fn emit(&mut self, sep: Sep, text: &str) {
        match sep {
            Sep::None => {}
            Sep::Space => self.out.push(' '),
            Sep::Line(n) | Sep::Blank(n) => {
                let end = self.out.trim_end_matches([' ', '\t']).len();
                self.out.truncate(end);
                self.out.push('\n');
                if sep == Sep::Blank(n) {
                    self.out.push('\n');
                }
                self.out.extend(std::iter::repeat_n(' ', n));
                self.line_indent = n;
            }
        }
        self.out.push_str(text);
    }

    /// Atualiza blocos, cláusulas e o estado do comando depois de escrever
    /// o token `i`.
    fn advance(&mut self, i: usize, clause: Option<Clause>) {
        let it = &self.items[i];
        let w = self.keyword(Some(i));
        let prev = self.prev;
        // Se o token abriu a linha (para alinhar o fecho de `CASE`).
        let written = self.out.len() - it.text.len();
        let starts_line = self.out[..written].trim_end_matches(' ').ends_with('\n')
            || self.out[..written].trim().is_empty();
        self.prev_binary = it.kind == TokenKind::Operator
            && SPACED_OPERATORS.contains(&it.text)
            && self.ends_value(prev);
        self.top_mut().count += 1;
        if let Some(c) = clause {
            let top = self.top_mut();
            top.clause = c;
            top.between = false;
        }
        if let Some(w) = w.as_deref() {
            if self.first_keyword.is_none() && prev.is_none() {
                self.first_keyword = Some(w.to_string());
            }
            match w {
                "table" => self.seen_table = true,
                "as" => self.seen_as = true,
                "between" => self.top_mut().between = true,
                "and" => self.top_mut().between = false,
                "case" => {
                    let broken = self.structural();
                    let open = if starts_line {
                        self.line_indent
                    } else {
                        self.cont_indent()
                    };
                    let frame =
                        Frame::new(FrameKind::Case, open + self.opts.indent, open, broken);
                    self.frames.push(frame);
                }
                "end" if self.top().kind == FrameKind::Case => {
                    self.frames.pop();
                }
                _ => {}
            }
        }
        if it.kind == TokenKind::Punctuation {
            match it.text {
                "(" => {
                    let next = self.keyword(self.next_token(i));
                    let subquery = matches!(next.as_deref(), Some("select" | "with" | "values"));
                    let columns = !subquery
                        && self.frames.len() == 1
                        && self.first_keyword.as_deref() == Some("create")
                        && self.seen_table
                        && !self.seen_as
                        && !self.columns_done
                        && prev.is_some_and(|p| {
                            matches!(
                                self.items[p].kind,
                                TokenKind::Identifier | TokenKind::QuotedIdentifier
                            )
                        });
                    let kind = if subquery {
                        FrameKind::Subquery
                    } else if columns {
                        self.columns_done = true;
                        FrameKind::Columns
                    } else {
                        FrameKind::Inline
                    };
                    let open = self.line_indent;
                    let frame = Frame::new(kind, open + self.opts.indent, open, true);
                    self.frames.push(frame);
                }
                ")" => {
                    while self.frames.len() > 1 && self.top().kind == FrameKind::Case {
                        self.frames.pop();
                    }
                    if self.frames.len() > 1 {
                        self.frames.pop();
                    }
                }
                ";" => {
                    self.frames.truncate(1);
                    self.frames[0] = Frame::new(FrameKind::Statement, 0, 0, true);
                    self.after_semicolon = true;
                    self.first_keyword = None;
                    self.seen_table = false;
                    self.seen_as = false;
                    self.columns_done = false;
                    self.prev = None;
                    self.prev_binary = false;
                    return;
                }
                _ => {}
            }
        }
        self.prev = Some(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(sql: &str) -> String {
        format(sql, &FormatOptions::default()).unwrap()
    }

    #[test]
    fn select_with_joins_and_where() {
        let sql = "select c.id, c.name, count(*) as total from customers c left join orders o on o.customer_id=c.id where c.active and o.created_at>now()-interval '30 days' group by c.id, c.name order by total desc limit 10;";
        assert_eq!(
            fmt(sql),
            "SELECT c.id,\n    c.name,\n    count(*) AS total\nFROM customers c\nLEFT JOIN orders o ON o.customer_id = c.id\nWHERE c.active\n    AND o.created_at > now() - INTERVAL '30 days'\nGROUP BY c.id, c.name\nORDER BY total DESC\nLIMIT 10;"
        );
    }

    #[test]
    fn subqueries_and_ctes_are_indented() {
        let sql = "with a as (select id from t where x between 1 and 2 and y = 3) select * from a where id in (select id from u)";
        assert_eq!(
            fmt(sql),
            "WITH a AS (\n    SELECT id\n    FROM t\n    WHERE x BETWEEN 1 AND 2\n        AND y = 3\n)\nSELECT *\nFROM a\nWHERE id IN (\n    SELECT id\n    FROM u\n)"
        );
    }

    #[test]
    fn comments_are_kept() {
        let sql = "-- cabeçalho\n/* bloco */\nselect a, -- primeira\n  b /* inline */ from t -- fim\nwhere x = 1;\n";
        assert_eq!(
            fmt(sql),
            "-- cabeçalho\n/* bloco */\nSELECT a, -- primeira\n    b /* inline */\nFROM t -- fim\nWHERE x = 1;\n"
        );
    }

    #[test]
    fn statements_keep_blank_lines_between_them() {
        assert_eq!(
            fmt("select 1; select 2;\n\n\nselect 3;"),
            "SELECT 1;\nSELECT 2;\n\nSELECT 3;"
        );
    }

    #[test]
    fn create_table_lists_one_column_per_line() {
        let sql = "create table t (id int primary key, name text not null, check (id > 0));";
        assert_eq!(
            fmt(sql),
            "CREATE TABLE t (\n    id INT PRIMARY KEY,\n    name TEXT NOT NULL,\n    CHECK (id > 0)\n);"
        );
    }

    #[test]
    fn insert_values_and_update_set() {
        assert_eq!(
            fmt("insert into t (a, b) values (1, 'x'), (2, 'y') returning a"),
            "INSERT INTO t (a, b)\nVALUES (1, 'x'),\n    (2, 'y')\nRETURNING a"
        );
        assert_eq!(
            fmt("update t set a = 1, b = -2 where id = $1"),
            "UPDATE t\nSET a = 1,\n    b = -2\nWHERE id = $1"
        );
        assert_eq!(fmt("delete from t where a=1"), "DELETE FROM t\nWHERE a = 1");
    }

    #[test]
    fn case_expressions_break_inside_statements_only() {
        assert_eq!(
            fmt("select case when a then 1 else 2 end as x from t"),
            "SELECT CASE\n        WHEN a THEN 1\n        ELSE 2\n    END AS x\nFROM t"
        );
        assert_eq!(
            fmt("select coalesce(case when a then 1 end, 0)"),
            "SELECT COALESCE(CASE WHEN a THEN 1 END, 0)"
        );
    }

    #[test]
    fn strings_bodies_and_quoted_names_are_untouched() {
        let sql = "create function f() returns int as $$\n  select   1;\n$$ language sql;\nselect 'a  b', \"Minha  Coluna\", E'x\\'y' from \"T\";";
        let out = fmt(sql);
        assert!(out.contains("$$\n  select   1;\n$$"), "{out}");
        assert!(
            out.contains("'a  b',\n    \"Minha  Coluna\",\n    E'x\\'y'\nFROM \"T\""),
            "{out}"
        );
    }

    #[test]
    fn glued_tokens_stay_glued() {
        // U&'...' e B'...' mudam de sentido com um espaço no meio.
        assert_eq!(
            fmt("select U&'d\\0061t', B'0101', x::int, t.*, arr[1]"),
            "SELECT U&'d\\0061t',\n    B'0101',\n    x::INT,\n    t.*,\n    arr[1]"
        );
    }

    #[test]
    fn string_continuation_keeps_its_line_break() {
        let sql = "select 'a'\n  'b' as s";
        assert_eq!(fmt(sql), "SELECT 'a'\n    'b' AS s");
    }

    #[test]
    fn qualified_keywords_keep_their_case() {
        assert_eq!(fmt("select t.date from t"), "SELECT t.date\nFROM t");
    }

    #[test]
    fn keyword_case_options() {
        let lower = FormatOptions {
            keyword_case: KeywordCase::Lower,
            ..Default::default()
        };
        assert_eq!(format("SELECT 1 FROM t", &lower).unwrap(), "select 1\nfrom t");
        let keep = FormatOptions {
            keyword_case: KeywordCase::Preserve,
            ..Default::default()
        };
        assert_eq!(format("Select 1 From t", &keep).unwrap(), "Select 1\nFrom t");
    }

    #[test]
    fn broken_sql_is_refused() {
        let e = format("select 'abc", &FormatOptions::default()).unwrap_err();
        assert_eq!(e.at, Some(7));
        assert!(format("select (1", &FormatOptions::default()).is_err());
        assert!(format("\\d tabela", &FormatOptions::default()).is_err());
        assert!(format("copy t from stdin;\n1\t2\n\\.\n", &FormatOptions::default()).is_err());
    }

    #[test]
    fn formatting_is_idempotent() {
        let samples = [
            "select c.id, c.name, count(*) as total from customers c left join orders o on o.customer_id=c.id where c.active and o.x>1 group by 1, 2",
            "with a as (select 1) select * from a union all select 2",
            "-- x\nselect a, -- y\n b from t /* z */ where a in (select b from c where d or e);",
            "create table t (id int, n text); insert into t values (1, 'a');",
        ];
        for s in samples {
            let once = fmt(s);
            assert_eq!(fmt(&once), once, "{s}");
        }
    }

    #[test]
    fn whitespace_only_is_left_alone() {
        assert_eq!(fmt("  \n"), "  \n");
    }

    #[test]
    fn selection_formats_whole_lines_with_their_indentation() {
        let text = "-- antes\n  select a,b from t where x=1\n-- depois";
        let (a, b) = (12, 20); // dentro da linha 2
        let e = format_selection(text, a, b, &FormatOptions::default()).unwrap();
        assert_eq!(
            e.text,
            "-- antes\n  SELECT a,\n      b\n  FROM t\n  WHERE x = 1\n-- depois"
        );
        let sel: String = e
            .text
            .chars()
            .skip(e.selection.0)
            .take(e.selection.1 - e.selection.0)
            .collect();
        assert_eq!(sel, "  SELECT a,\n      b\n  FROM t\n  WHERE x = 1");
    }

    #[test]
    fn empty_selection_formats_document_and_keeps_cursor() {
        let text = "select a from t where x=1";
        let cursor = text.find("x=1").unwrap();
        let e = format_selection(text, cursor, cursor, &FormatOptions::default()).unwrap();
        assert_eq!(e.text, "SELECT a\nFROM t\nWHERE x = 1");
        let at: String = e.text.chars().skip(e.selection.1).take(3).collect();
        assert_eq!(at, "x =");
    }

    #[test]
    fn verify_catches_changes() {
        assert!(verify("select a", "select b").is_err());
        assert!(verify("select 'a'\n'b'", "select 'a' 'b'").is_err());
        assert!(verify("SELECT a", "select   a").is_ok());
    }
}
