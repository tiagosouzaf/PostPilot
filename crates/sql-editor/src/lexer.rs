//! Tokenizador de SQL no dialeto do PostgreSQL.
//!
//! Não valida nada: só separa o texto em pedaços para o realce (e, mais
//! tarde, para o IntelliSense). Nunca falha: texto inesperado vira
//! [`TokenKind::Other`], e um comentário ou string sem fim vai até o fim do
//! texto, como o servidor leria.

use std::ops::Range;

/// Tipo de um pedaço do texto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Whitespace,
    /// `-- até o fim da linha` ou `/* bloco, que pode ter /* outro */ dentro */`.
    Comment,
    /// Palavra reservada ou comum do SQL (`SELECT`, `FROM`, `JOIN`…).
    Keyword,
    /// Nome sem aspas.
    Identifier,
    /// Nome entre aspas duplas: `"Minha Tabela"`.
    QuotedIdentifier,
    /// `'texto'`, `E'com \' escape'`, `$$corpo$$`, `$tag$corpo$tag$`.
    String,
    /// `42`, `3.14`, `1e10`, `0x1F`.
    Number,
    /// Parâmetro posicional: `$1`.
    Parameter,
    /// `+ - * / < > = ! ~ @ # % ^ & | ` e combinações (`<=`, `::`, `->>`).
    Operator,
    /// `( ) , ; [ ] .`
    Punctuation,
    Other,
}

/// Um pedaço do texto, em bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: Range<usize>,
}

/// Palavras-chave realçadas, em minúsculas e em ordem alfabética (busca
/// binária).
pub const KEYWORDS: &[&str] = &[
    "abort",
    "absolute",
    "access",
    "action",
    "add",
    "admin",
    "after",
    "aggregate",
    "all",
    "also",
    "alter",
    "always",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "at",
    "authorization",
    "before",
    "begin",
    "between",
    "bigint",
    "bigserial",
    "binary",
    "bit",
    "boolean",
    "both",
    "by",
    "cache",
    "call",
    "called",
    "cascade",
    "case",
    "cast",
    "char",
    "character",
    "check",
    "checkpoint",
    "close",
    "cluster",
    "coalesce",
    "collate",
    "column",
    "comment",
    "commit",
    "committed",
    "concurrently",
    "conflict",
    "constraint",
    "constraints",
    "copy",
    "create",
    "cross",
    "cube",
    "current",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "cycle",
    "data",
    "database",
    "date",
    "day",
    "deallocate",
    "dec",
    "decimal",
    "declare",
    "default",
    "deferrable",
    "deferred",
    "delete",
    "desc",
    "distinct",
    "do",
    "domain",
    "double",
    "drop",
    "each",
    "else",
    "enable",
    "end",
    "enum",
    "escape",
    "except",
    "exclude",
    "execute",
    "exists",
    "explain",
    "extension",
    "external",
    "extract",
    "false",
    "fetch",
    "filter",
    "first",
    "float",
    "following",
    "for",
    "force",
    "foreign",
    "from",
    "full",
    "function",
    "functions",
    "generated",
    "grant",
    "granted",
    "group",
    "grouping",
    "having",
    "hour",
    "identity",
    "if",
    "ilike",
    "immutable",
    "in",
    "include",
    "index",
    "inherit",
    "initially",
    "inner",
    "inout",
    "insert",
    "instead",
    "int",
    "integer",
    "intersect",
    "interval",
    "into",
    "is",
    "isnull",
    "isolation",
    "join",
    "json",
    "jsonb",
    "key",
    "language",
    "last",
    "lateral",
    "leading",
    "left",
    "level",
    "like",
    "limit",
    "listen",
    "local",
    "localtime",
    "localtimestamp",
    "lock",
    "logged",
    "matched",
    "materialized",
    "maxvalue",
    "merge",
    "minute",
    "minvalue",
    "mode",
    "month",
    "move",
    "natural",
    "next",
    "no",
    "none",
    "not",
    "nothing",
    "notify",
    "notnull",
    "nowait",
    "null",
    "nullif",
    "nulls",
    "numeric",
    "of",
    "off",
    "offset",
    "oids",
    "on",
    "only",
    "or",
    "order",
    "out",
    "outer",
    "over",
    "overlaps",
    "overriding",
    "owned",
    "owner",
    "parallel",
    "partition",
    "placing",
    "policy",
    "position",
    "preceding",
    "precision",
    "prepare",
    "primary",
    "prior",
    "privileges",
    "procedure",
    "procedures",
    "publication",
    "range",
    "read",
    "real",
    "recursive",
    "references",
    "referencing",
    "refresh",
    "reindex",
    "release",
    "rename",
    "repeatable",
    "replace",
    "replica",
    "reset",
    "restart",
    "restrict",
    "return",
    "returning",
    "returns",
    "revoke",
    "right",
    "role",
    "rollback",
    "rollup",
    "row",
    "rows",
    "rule",
    "savepoint",
    "schema",
    "schemas",
    "second",
    "security",
    "select",
    "sequence",
    "sequences",
    "serial",
    "serializable",
    "session",
    "session_user",
    "set",
    "setof",
    "sets",
    "share",
    "show",
    "similar",
    "smallint",
    "some",
    "stable",
    "start",
    "statement",
    "statistics",
    "stored",
    "strict",
    "subscription",
    "symmetric",
    "system",
    "table",
    "tables",
    "tablesample",
    "tablespace",
    "temp",
    "temporary",
    "text",
    "then",
    "ties",
    "time",
    "timestamp",
    "timestamptz",
    "to",
    "trailing",
    "transaction",
    "trigger",
    "trim",
    "true",
    "truncate",
    "type",
    "unbounded",
    "uncommitted",
    "union",
    "unique",
    "unknown",
    "unlisten",
    "unlogged",
    "until",
    "update",
    "user",
    "using",
    "uuid",
    "vacuum",
    "valid",
    "values",
    "varchar",
    "variadic",
    "varying",
    "verbose",
    "view",
    "views",
    "volatile",
    "when",
    "where",
    "window",
    "with",
    "within",
    "without",
    "work",
    "write",
    "year",
    "zone",
];

/// Se `word` é palavra-chave (sem diferenciar maiúsculas).
pub fn is_keyword(word: &str) -> bool {
    if word.len() > 24 || !word.is_ascii() {
        return false;
    }
    // Sem alocar: a palavra cabe em 24 bytes.
    let mut buf = [0u8; 24];
    let lower = &mut buf[..word.len()];
    lower.copy_from_slice(word.as_bytes());
    lower.make_ascii_lowercase();
    let lower = std::str::from_utf8(lower).expect("ascii");
    KEYWORDS.binary_search(&lower).is_ok()
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

const OPERATOR_CHARS: &str = "+-*/<>=~!@#%^&|`?:";

/// Separa `text` em tokens. Os tokens cobrem o texto inteiro, em ordem.
pub fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut lexer = Lexer { text, pos: 0 };
    while lexer.pos < text.len() {
        let start = lexer.pos;
        let kind = lexer.next_kind();
        debug_assert!(lexer.pos > start, "lexer must advance");
        tokens.push(Token {
            kind,
            range: start..lexer.pos,
        });
    }
    tokens
}

/// Os comandos de `text`, separados por `;` fora de strings, comentários e
/// corpos `$$`: o intervalo (em bytes) de cada um, do primeiro ao último
/// token que não é espaço nem comentário, sem o `;`. Trechos só com espaço e
/// comentários ficam de fora.
pub fn statements(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for t in tokenize(text) {
        match t.kind {
            TokenKind::Whitespace | TokenKind::Comment => {}
            TokenKind::Punctuation if &text[t.range.clone()] == ";" => {
                out.extend(current.take());
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

/// Refaz `tokens` depois de trocar `removed` bytes em `at` por `inserted`
/// bytes; `text` já é o texto novo. Só relê do começo da linha da edição
/// até um ponto depois dela em que os tokens voltam a coincidir.
///
/// Funciona porque o lexer não guarda estado entre tokens: os tokens a partir
/// de uma fronteira dependem só do texto dali em diante, que não mudou depois
/// da edição. Antes da edição, um token só olha até dois caracteres depois do
/// fim (ou até o fim da linha, numa tag `$tag$` sem fechamento), então reler
/// a partir da linha anterior basta.
pub fn retokenize(text: &str, tokens: &mut Vec<Token>, at: usize, removed: usize, inserted: usize) {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let back = line_start.saturating_sub(8);
    let first = tokens.partition_point(|t| t.range.end <= back);
    let restart = match tokens.get(first) {
        Some(t) => t.range.start,
        None => tokens.last().map_or(0, |t| t.range.end),
    };
    let edit_end = at + inserted;
    let old_edit_end = at + removed;
    let shift = |p: usize| p + removed - inserted; // posição nova -> antiga
    let mut fresh = Vec::new();
    let mut lexer = Lexer { text, pos: restart };
    let mut k = first;
    let mut synced = false;
    while lexer.pos < text.len() {
        if lexer.pos >= edit_end {
            let old = shift(lexer.pos);
            while k < tokens.len() && tokens[k].range.start < old {
                k += 1;
            }
            if k < tokens.len() && tokens[k].range.start == old && old >= old_edit_end {
                synced = true;
                break;
            }
        }
        let start = lexer.pos;
        let kind = lexer.next_kind();
        fresh.push(Token {
            kind,
            range: start..lexer.pos,
        });
    }
    if !synced {
        k = tokens.len();
    }
    let n = fresh.len();
    tokens.splice(first..k, fresh);
    if inserted != removed {
        for t in &mut tokens[first + n..] {
            t.range = t.range.start + inserted - removed..t.range.end + inserted - removed;
        }
    }
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek2(&self) -> Option<char> {
        self.rest().chars().nth(1)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat_while(&mut self, f: impl Fn(char) -> bool) {
        while let Some(c) = self.peek() {
            if !f(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn next_kind(&mut self) -> TokenKind {
        let c = self.peek().expect("not at end");
        match c {
            c if c.is_whitespace() => {
                self.eat_while(char::is_whitespace);
                TokenKind::Whitespace
            }
            '-' if self.peek2() == Some('-') => {
                self.eat_while(|c| c != '\n');
                TokenKind::Comment
            }
            '/' if self.peek2() == Some('*') => {
                self.block_comment();
                TokenKind::Comment
            }
            '\'' => {
                self.bump();
                self.quoted('\'', false);
                TokenKind::String
            }
            'e' | 'E' if self.peek2() == Some('\'') => {
                self.bump();
                self.bump();
                self.quoted('\'', true);
                TokenKind::String
            }
            '"' => {
                self.bump();
                self.quoted('"', false);
                TokenKind::QuotedIdentifier
            }
            '$' => self.dollar(),
            c if c.is_ascii_digit() => {
                self.number();
                TokenKind::Number
            }
            '.' if self.peek2().is_some_and(|c| c.is_ascii_digit()) => {
                self.number();
                TokenKind::Number
            }
            c if is_ident_start(c) => {
                let start = self.pos;
                self.eat_while(is_ident_char);
                if is_keyword(&self.text[start..self.pos]) {
                    TokenKind::Keyword
                } else {
                    TokenKind::Identifier
                }
            }
            '(' | ')' | ',' | ';' | '[' | ']' | '.' => {
                self.bump();
                TokenKind::Punctuation
            }
            c if OPERATOR_CHARS.contains(c) => {
                // Para antes de um comentário colado: `a+--x`. O primeiro
                // caractere nunca abre comentário (os ramos acima pegam).
                while let Some(c) = self.peek() {
                    let starts_comment = (c == '-' && self.peek2() == Some('-'))
                        || (c == '/' && self.peek2() == Some('*'));
                    if !OPERATOR_CHARS.contains(c) || starts_comment {
                        break;
                    }
                    self.bump();
                }
                TokenKind::Operator
            }
            _ => {
                self.bump();
                TokenKind::Other
            }
        }
    }

    fn block_comment(&mut self) {
        self.pos += 2;
        let mut depth = 1;
        while depth > 0 {
            let rest = self.rest();
            if rest.is_empty() {
                return;
            }
            if rest.starts_with("/*") {
                depth += 1;
                self.pos += 2;
            } else if rest.starts_with("*/") {
                depth -= 1;
                self.pos += 2;
            } else {
                self.bump();
            }
        }
    }

    /// Lê até a aspa de fechamento; aspa dobrada é escape. Com `backslash`,
    /// `\` escapa o próximo caractere (strings `E'...'`).
    fn quoted(&mut self, quote: char, backslash: bool) {
        while let Some(c) = self.bump() {
            if backslash && c == '\\' {
                self.bump();
            } else if c == quote {
                if self.peek() == Some(quote) {
                    self.bump();
                } else {
                    return;
                }
            }
        }
    }

    fn dollar(&mut self) -> TokenKind {
        let rest = self.rest();
        // $1, $2: parâmetro.
        if rest[1..].starts_with(|c: char| c.is_ascii_digit()) {
            self.bump();
            self.eat_while(|c| c.is_ascii_digit());
            return TokenKind::Parameter;
        }
        // $tag$ ... $tag$: a tag é vazia ou um identificador sem `$`.
        let tag_len = rest[1..]
            .char_indices()
            .find(|&(i, c)| !(c.is_alphanumeric() || c == '_') || (i == 0 && c.is_ascii_digit()))
            .map(|(i, _)| i);
        if let Some(n) = tag_len
            && rest[1 + n..].starts_with('$')
        {
            let tag = &rest[..n + 2];
            let body_start = self.pos + tag.len();
            match self.text[body_start..].find(tag) {
                Some(end) => self.pos = body_start + end + tag.len(),
                None => self.pos = self.text.len(),
            }
            return TokenKind::String;
        }
        self.bump();
        TokenKind::Other
    }

    fn number(&mut self) {
        let rest = self.rest();
        if rest.starts_with("0x") || rest.starts_with("0X") {
            self.pos += 2;
            self.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
            return;
        }
        self.eat_while(|c| c.is_ascii_digit() || c == '_');
        if self.peek() == Some('.') && self.peek2() != Some('.') {
            self.bump();
            self.eat_while(|c| c.is_ascii_digit() || c == '_');
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = self.pos;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.eat_while(|c| c.is_ascii_digit());
            } else {
                self.pos = save;
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn statements_split_on_semicolons_outside_strings() {
        let text = "SELECT ';' AS a; -- só comentário ;\n  ;\nCREATE FUNCTION f() AS $$ SELECT 1; $$ LANGUAGE sql  \n/* fim */";
        let parts: Vec<&str> = statements(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(
            parts,
            [
                "SELECT ';' AS a",
                "CREATE FUNCTION f() AS $$ SELECT 1; $$ LANGUAGE sql"
            ]
        );
        assert!(statements("  -- nada\n").is_empty());
        assert_eq!(statements("SELECT 1"), vec![0..8_usize]);
    }

    use super::*;
    use TokenKind::*;

    fn kinds(text: &str) -> Vec<(TokenKind, &str)> {
        tokenize(text)
            .into_iter()
            .filter(|t| t.kind != Whitespace)
            .map(|t| (t.kind, &text[t.range]))
            .collect()
    }

    #[test]
    fn keywords_are_sorted_lowercase_and_unique() {
        for w in KEYWORDS.windows(2) {
            assert!(w[0] < w[1], "{} >= {}", w[0], w[1]);
        }
        assert!(
            KEYWORDS
                .iter()
                .all(|k| k.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        );
    }

    #[test]
    fn basic_select() {
        assert_eq!(
            kinds("SELECT id, \"Nome\" FROM public.t WHERE x >= 10.5;"),
            [
                (Keyword, "SELECT"),
                (Identifier, "id"),
                (Punctuation, ","),
                (QuotedIdentifier, "\"Nome\""),
                (Keyword, "FROM"),
                (Identifier, "public"),
                (Punctuation, "."),
                (Identifier, "t"),
                (Keyword, "WHERE"),
                (Identifier, "x"),
                (Operator, ">="),
                (Number, "10.5"),
                (Punctuation, ";"),
            ]
        );
    }

    #[test]
    fn strings_and_comments() {
        assert_eq!(
            kinds("'it''s' E'a\\'b' -- fim\n/* a /* b */ c */ x"),
            [
                (String, "'it''s'"),
                (String, "E'a\\'b'"),
                (Comment, "-- fim"),
                (Comment, "/* a /* b */ c */"),
                (Identifier, "x"),
            ]
        );
    }

    #[test]
    fn dollar_quotes_and_parameters() {
        assert_eq!(
            kinds("$$ select 'x' $$ $fn$ a $$ b $fn$ $1 a$b"),
            [
                (String, "$$ select 'x' $$"),
                (String, "$fn$ a $$ b $fn$"),
                (Parameter, "$1"),
                (Identifier, "a$b"),
            ]
        );
    }

    #[test]
    fn operators_casts_and_numbers() {
        assert_eq!(
            kinds("a::int + 1e3 - .5 ->> 'k' 0xFF"),
            [
                (Identifier, "a"),
                (Operator, "::"),
                (Keyword, "int"),
                (Operator, "+"),
                (Number, "1e3"),
                (Operator, "-"),
                (Number, ".5"),
                (Operator, "->>"),
                (String, "'k'"),
                (Number, "0xFF"),
            ]
        );
        assert_eq!(
            kinds("a+--c"),
            [(Identifier, "a"), (Operator, "+"), (Comment, "--c")]
        );
    }

    #[test]
    fn unterminated_pieces_run_to_the_end() {
        for text in ["'abc", "/* abc", "\"abc", "$x$ abc", "E'\\"] {
            let t = tokenize(text);
            assert_eq!(t.len(), 1, "{text:?}: {t:?}");
            assert_eq!(t[0].range, 0..text.len());
        }
    }

    #[test]
    fn tokens_cover_any_text() {
        for text in [
            "",
            "olá, mundo ção",
            "$",
            "$$",
            "1.",
            "1..2",
            "1e",
            "a.b.c",
            "\u{1F600} select",
            "--",
            "/*/",
        ] {
            let tokens = tokenize(text);
            let mut pos = 0;
            for t in &tokens {
                assert_eq!(t.range.start, pos, "{text:?}");
                pos = t.range.end;
            }
            assert_eq!(pos, text.len(), "{text:?}");
        }
    }

    #[test]
    fn retokenize_matches_a_full_tokenize_after_random_edits() {
        let pieces = [
            "select", " ", "\n", "'", "\"", "$$", "$a$", "$1", "/*", "*/", "--", "-", "+", "1",
            ".", "e", "5", "x", "ção", "(", ")", ";", "E'", "\\", "0x", "_", "$", "a",
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let mut text =
            std::string::String::from("select 1 from t;\n/* x */ select 'a' -- c\n$$ b $$");
        let mut tokens = tokenize(&text);
        for _ in 0..5000 {
            let bounds: Vec<usize> = (0..=text.len())
                .filter(|&i| text.is_char_boundary(i))
                .collect();
            let a = bounds[next(bounds.len())];
            let b = bounds
                [(bounds.iter().position(|&x| x == a).unwrap() + next(4)).min(bounds.len() - 1)];
            let insert: std::string::String =
                (0..next(3)).map(|_| pieces[next(pieces.len())]).collect();
            text.replace_range(a..b, &insert);
            retokenize(&text, &mut tokens, a, b - a, insert.len());
            assert_eq!(tokens, tokenize(&text), "{text:?}");
            if text.len() > 400 {
                text.truncate(text.char_indices().nth(100).map_or(text.len(), |(i, _)| i));
                tokens = tokenize(&text);
            }
        }
    }
}
