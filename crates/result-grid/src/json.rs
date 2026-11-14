//! JSON das células: reconhecer, ler numa árvore que guarda a ordem das
//! chaves e o texto exato dos números, e escrever formatado.
//!
//! O PostgreSQL manda `json` e `jsonb` como texto, sem dizer o tipo da
//! coluna na consulta simples. Por isso a grade reconhece o JSON pelo
//! conteúdo: um objeto ou um array que o parser aceita. Arrays do
//! PostgreSQL (`{1,2}`) não são JSON válido, então não confundem.
//!
//! O parser é próprio (e pequeno) para não perder nada do que o banco
//! mandou: a ordem das chaves de um `json` e números maiores que um `f64`
//! (um `numeric` dentro do `jsonb`) aparecem como vieram.

use std::fmt::Write as _;

/// Um valor JSON.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    /// O número como estava no texto.
    Number(String),
    String(String),
    Array(Vec<Node>),
    /// Pares na ordem do texto.
    Object(Vec<(String, Node)>),
}

/// Um passo do caminho até um nó: chave de objeto ou posição de array.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Key(String),
    Index(usize),
}

/// Erro de leitura, com linha e coluna (a partir de 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (line {}, column {})",
            self.message, self.line, self.column
        )
    }
}

/// Nós mais fundos que isto são recusados, para uma célula maliciosa não
/// estourar a pilha.
const MAX_DEPTH: usize = 512;

/// Lê um texto JSON inteiro (espaços em volta são aceitos).
pub fn parse(text: &str) -> Result<Node, ParseError> {
    let mut p = Parser {
        bytes: text.as_bytes(),
        text,
        at: 0,
    };
    p.skip_ws();
    let node = p.value(0)?;
    p.skip_ws();
    if p.at < p.bytes.len() {
        return Err(p.error("Unexpected text after the JSON value"));
    }
    Ok(node)
}

/// Se o texto parece um objeto ou array JSON (sem ler tudo): começa com
/// `{` ou `[` e termina com `}` ou `]`. Serve para decidir rápido se vale
/// chamar [`parse`].
pub fn looks_like_container(text: &str) -> bool {
    let t = text.trim();
    matches!(
        (t.as_bytes().first(), t.as_bytes().last()),
        (Some(b'{'), Some(b'}')) | (Some(b'['), Some(b']'))
    )
}

/// O valor da célula, se for um objeto ou array JSON.
pub fn container(text: &str) -> Option<Node> {
    if !looks_like_container(text) {
        return None;
    }
    parse(text)
        .ok()
        .filter(|n| matches!(n, Node::Array(_) | Node::Object(_)))
}

/// Se uma coluna é JSON, pela amostra dos valores não nulos: todos são
/// JSON válido e pelo menos um é objeto ou array (escalares sozinhos, como
/// `5` ou `true`, são ambíguos demais).
pub fn is_json_column<'a>(values: impl IntoIterator<Item = &'a str>) -> bool {
    let mut container_seen = false;
    for v in values {
        if looks_like_container(v) {
            if container(v).is_none() {
                return false;
            }
            container_seen = true;
        } else if parse(v).is_err() {
            return false;
        }
    }
    container_seen
}

impl Node {
    /// O nó no fim do caminho.
    pub fn get(&self, path: &[Step]) -> Option<&Node> {
        let mut node = self;
        for step in path {
            node = match (node, step) {
                (Node::Object(pairs), Step::Key(k)) => {
                    // Em chaves repetidas vale a última, como no jsonb.
                    &pairs.iter().rev().find(|(key, _)| key == k)?.1
                }
                (Node::Array(items), Step::Index(i)) => items.get(*i)?,
                _ => return None,
            };
        }
        Some(node)
    }

    /// Nome do tipo, como o `jsonb_typeof` diz.
    pub fn type_name(&self) -> &'static str {
        match self {
            Node::Null => "null",
            Node::Bool(_) => "boolean",
            Node::Number(_) => "number",
            Node::String(_) => "string",
            Node::Array(_) => "array",
            Node::Object(_) => "object",
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(self, Node::Array(_) | Node::Object(_))
    }

    /// O nó em JSON compacto, numa linha (como o `jsonb` mostra, mas
    /// sem espaços).
    pub fn compact(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None, 0);
        out
    }

    /// O nó em JSON indentado com dois espaços.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(2), 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        let newline = |out: &mut String, depth: usize| {
            if let Some(n) = indent {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', n * depth));
            }
        };
        match self {
            Node::Null => out.push_str("null"),
            Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Node::Number(n) => out.push_str(n),
            Node::String(s) => write_string(out, s),
            Node::Array(items) if items.is_empty() => out.push_str("[]"),
            Node::Object(pairs) if pairs.is_empty() => out.push_str("{}"),
            Node::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    item.write(out, indent, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Node::Object(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    write_string(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }

    /// Resumo de uma linha, para a árvore: o escalar como JSON, ou
    /// `{3 keys}` e `[5 items]`.
    pub fn preview(&self) -> String {
        match self {
            Node::Array(items) => match items.len() {
                1 => "[1 item]".to_string(),
                n => format!("[{n} items]"),
            },
            Node::Object(pairs) => match pairs.len() {
                1 => "{1 key}".to_string(),
                n => format!("{{{n} keys}}"),
            },
            other => other.compact(),
        }
    }
}

/// Uma string em JSON, com as aspas e os escapes.
pub fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// O caminho no formato do SQL/JSON path (`$.items[0]."first name"`).
pub fn json_path(path: &[Step]) -> String {
    let mut out = String::from("$");
    for step in path {
        match step {
            Step::Key(k) if is_path_identifier(k) => {
                out.push('.');
                out.push_str(k);
            }
            Step::Key(k) => {
                out.push('.');
                write_string(&mut out, k);
            }
            Step::Index(i) => {
                let _ = write!(out, "[{i}]");
            }
        }
    }
    out
}

/// Chave que o jsonpath aceita sem aspas.
fn is_path_identifier(k: &str) -> bool {
    let mut chars = k.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$');
    // Palavras do jsonpath não podem ser chave sem aspas.
    const RESERVED: &[&str] = &[
        "true",
        "false",
        "null",
        "is",
        "like_regex",
        "flag",
        "starts",
        "with",
        "exists",
        "strict",
        "lax",
        "last",
        "to",
        "unknown",
    ];
    first_ok
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !RESERVED.contains(&k.to_ascii_lowercase().as_str())
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        let before = &self.text[..self.at.min(self.text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        ParseError {
            message: message.to_string(),
            line,
            column,
        }
    }

    fn skip_ws(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("JSON nested too deeply"));
        }
        match self.bytes.get(self.at) {
            None => Err(self.error("Unexpected end of JSON")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Node::String),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ if self.eat("true") => Ok(Node::Bool(true)),
            _ if self.eat("false") => Ok(Node::Bool(false)),
            _ if self.eat("null") => Ok(Node::Null),
            _ => Err(self.error("Expected a JSON value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Node, ParseError> {
        self.at += 1;
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.eat("}") {
            return Ok(Node::Object(pairs));
        }
        loop {
            self.skip_ws();
            if self.bytes.get(self.at) != Some(&b'"') {
                return Err(self.error("Expected a key in double quotes"));
            }
            let key = self.string()?;
            self.skip_ws();
            if !self.eat(":") {
                return Err(self.error("Expected ':' after the key"));
            }
            self.skip_ws();
            let value = self.value(depth + 1)?;
            pairs.push((key, value));
            self.skip_ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("}") {
                return Ok(Node::Object(pairs));
            }
            return Err(self.error("Expected ',' or '}'"));
        }
    }

    fn array(&mut self, depth: usize) -> Result<Node, ParseError> {
        self.at += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.eat("]") {
            return Ok(Node::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("]") {
                return Ok(Node::Array(items));
            }
            return Err(self.error("Expected ',' or ']'"));
        }
    }

    fn number(&mut self) -> Result<Node, ParseError> {
        let start = self.at;
        let digits = |p: &mut Self| {
            let s = p.at;
            while p.bytes.get(p.at).is_some_and(u8::is_ascii_digit) {
                p.at += 1;
            }
            p.at > s
        };
        self.eat("-");
        if self.eat("0") {
            // Zero à esquerda não é JSON.
        } else if !digits(self) {
            return Err(self.error("Invalid number"));
        }
        if self.eat(".") && !digits(self) {
            return Err(self.error("Invalid number"));
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !digits(self) {
                return Err(self.error("Invalid number"));
            }
        }
        Ok(Node::Number(self.text[start..self.at].to_string()))
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.at..];
            // Copia de uma vez até a próxima aspa, barra ou controle.
            let plain = rest
                .find(|c: char| c == '"' || c == '\\' || (c as u32) < 0x20)
                .unwrap_or(rest.len());
            out.push_str(&rest[..plain]);
            self.at += plain;
            match self.bytes.get(self.at) {
                None => return Err(self.error("Unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let c = match self.bytes.get(self.at) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{08}',
                        Some(b'f') => '\u{0C}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.at += 1;
                            let c = self.unicode_escape()?;
                            out.push(c);
                            continue;
                        }
                        _ => return Err(self.error("Invalid escape in string")),
                    };
                    self.at += 1;
                    out.push(c);
                }
                Some(_) => return Err(self.error("Control character in string")),
            }
        }
    }

    /// Lê o `XXXX` de `\uXXXX` (já depois do `u`), juntando pares
    /// substitutos.
    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let hex = |p: &mut Self| -> Result<u32, ParseError> {
            let s = p
                .text
                .get(p.at..p.at + 4)
                .ok_or_else(|| p.error("Invalid \\u escape"))?;
            let v = u32::from_str_radix(s, 16).map_err(|_| p.error("Invalid \\u escape"))?;
            p.at += 4;
            Ok(v)
        };
        let high = hex(self)?;
        if (0xD800..0xDC00).contains(&high) {
            if self.eat("\\u") {
                let low = hex(self)?;
                if (0xDC00..0xE000).contains(&low) {
                    let c = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                    return char::from_u32(c).ok_or_else(|| self.error("Invalid \\u escape"));
                }
            }
            return Err(self.error("Unpaired surrogate in \\u escape"));
        }
        char::from_u32(high).ok_or_else(|| self.error("Invalid \\u escape"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_key_order_and_number_text() {
        let n = parse(r#"{"b": 1, "a": 12345678901234567890.50, "c": [true, null]}"#).unwrap();
        let Node::Object(pairs) = &n else {
            panic!("{n:?}")
        };
        let keys: Vec<_> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["b", "a", "c"]);
        assert_eq!(
            n.get(&[Step::Key("a".into())]),
            Some(&Node::Number("12345678901234567890.50".into()))
        );
        assert_eq!(
            n.compact(),
            r#"{"b":1,"a":12345678901234567890.50,"c":[true,null]}"#
        );
    }

    #[test]
    fn pretty_prints_with_two_spaces() {
        let n = parse(r#"{"a":[1,{"b":"x"}],"e":{},"f":[]}"#).unwrap();
        assert_eq!(
            n.pretty(),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": \"x\"\n    }\n  ],\n  \"e\": {},\n  \"f\": []\n}"
        );
    }

    #[test]
    fn strings_round_trip_escapes() {
        let n = parse(r#"["a\"b\\c\né😀\/", "\u0001"]"#).unwrap();
        assert_eq!(
            n.get(&[Step::Index(0)]),
            Some(&Node::String("a\"b\\c\né😀/".into()))
        );
        assert_eq!(n.compact(), r#"["a\"b\\c\né😀/","\u0001"]"#);
    }

    #[test]
    fn rejects_invalid_json_with_position() {
        let e = parse("{\n  \"a\": 1,\n  b: 2\n}").unwrap_err();
        assert_eq!((e.line, e.column), (3, 3));
        assert!(parse("[1,]").is_err());
        assert!(parse("01").is_err());
        assert!(parse("[1] x").is_err());
        assert!(parse("\"abc").is_err());
        assert!(parse(&"[".repeat(1000)).is_err());
    }

    #[test]
    fn recognizes_json_cells_but_not_postgres_arrays() {
        assert!(container(r#"{"a": 1}"#).is_some());
        assert!(container(" [1, 2] ").is_some());
        assert!(container("{1,2,3}").is_none());
        assert!(container(r#"{"a","b"}"#).is_none());
        assert!(container("[1,5)").is_none());
        assert!(container("42").is_none());
        assert!(is_json_column([r#"{"a":1}"#, "42", r#""x""#]));
        assert!(!is_json_column(["42", "true"]));
        assert!(!is_json_column([r#"{"a":1}"#, "hello"]));
        assert!(!is_json_column(["{1,2}"]));
    }

    #[test]
    fn json_path_quotes_unusual_keys() {
        let path = [
            Step::Key("items".into()),
            Step::Index(0),
            Step::Key("first name".into()),
            Step::Key("last".into()),
        ];
        assert_eq!(json_path(&path), r#"$.items[0]."first name"."last""#);
        assert_eq!(json_path(&[]), "$");
    }

    #[test]
    fn duplicate_keys_use_the_last_one() {
        let n = parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(
            n.get(&[Step::Key("a".into())]),
            Some(&Node::Number("2".into()))
        );
    }
}
