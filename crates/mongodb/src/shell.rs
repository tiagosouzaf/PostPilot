//! Lê consultas no estilo do mongosh (`db.pedidos.find({total: {$gt: 10}})
//! .sort({data: -1}).limit(20)`) e as transforma nos comandos que o
//! servidor entende (`{find: "pedidos", filter: ..., sort: ..., limit: 20}`).
//!
//! Não é JavaScript: só chamadas de método com valores literais. Os valores
//! aceitam o que o mongosh aceita no dia a dia: chaves sem aspas, aspas
//! simples, `ObjectId("...")`, `ISODate("...")`, `NumberLong(...)`,
//! `/regex/i`, vírgula sobrando no fim.

use mongodb::bson::{self, Bson, Document, doc, oid::ObjectId};
use pgstudio_i18n::{tr, trf};

/// Erro de leitura, com a posição (em bytes) no texto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub at: usize,
}

/// O que rodar no servidor para um comando do texto.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `use nome`: troca o banco da janela.
    Use(String),
    /// Comando que devolve um cursor (`find`, `aggregate`, `listIndexes`):
    /// os documentos viram as linhas da grade.
    Cursor { command: Document, admin: bool },
    /// Comando de resposta única, mostrado conforme [`Shape`].
    Single {
        command: Document,
        admin: bool,
        shape: Shape,
    },
}

/// Como mostrar a resposta de um comando de resposta única.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A resposta inteira, um campo por linha (`runCommand`, `stats`).
    Reply,
    /// Uma lista de documentos dentro da resposta, um por linha
    /// (`databases` do `listDatabases`).
    List(&'static str),
    /// Um valor só, numa coluna com este nome (`count`).
    Value {
        field: &'static str,
        column: &'static str,
    },
    /// Os valores de uma lista, um por linha (`distinct`).
    Values(&'static str),
    /// Escrita: a mensagem diz quantos documentos mudaram.
    Write(Write),
    /// Só "feito" em Messages (`createCollection`, `drop`).
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Insert,
    Update,
    Delete,
}

/// Um comando do texto: onde começa e o que roda.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// Byte onde o comando começa no texto.
    pub start: usize,
    /// O texto do comando, para as mensagens.
    pub text: String,
    pub action: Action,
}

impl Statement {
    /// Se o comando só lê.
    pub fn is_read_only(&self) -> bool {
        match &self.action {
            Action::Use(_) => true,
            Action::Cursor { command, .. } | Action::Single { command, .. } => {
                command_is_read_only(command)
            }
        }
    }

    /// Nome do comando do servidor (`find`, `insert`...), para as
    /// mensagens.
    pub fn command_name(&self) -> String {
        match &self.action {
            Action::Use(_) => "use".into(),
            Action::Cursor { command, .. } | Action::Single { command, .. } => {
                command.keys().next().cloned().unwrap_or_default()
            }
        }
    }
}

/// Comandos do servidor que só leem.
const READ_ONLY: &[&str] = &[
    "aggregate",
    "buildInfo",
    "buildinfo",
    "collStats",
    "connectionStatus",
    "count",
    "currentOp",
    "dbStats",
    "dbstats",
    "distinct",
    "explain",
    "find",
    "getParameter",
    "hello",
    "hostInfo",
    "isMaster",
    "ismaster",
    "listCollections",
    "listCommands",
    "listDatabases",
    "listIndexes",
    "ping",
    "replSetGetStatus",
    "serverStatus",
    "usersInfo",
    "rolesInfo",
    "whatsmyuri",
];

/// Se um comando do servidor só lê. Um `aggregate` com `$out` ou `$merge`
/// escreve.
pub fn command_is_read_only(command: &Document) -> bool {
    let Some(name) = command.keys().next() else {
        return true;
    };
    if !READ_ONLY.contains(&name.as_str()) {
        return false;
    }
    let pipeline = match name.as_str() {
        "aggregate" => command.get_array("pipeline").ok(),
        "explain" => {
            return command
                .get_document("explain")
                .is_ok_and(command_is_read_only);
        }
        _ => None,
    };
    !pipeline.into_iter().flatten().any(|stage| {
        stage
            .as_document()
            .is_some_and(|s| s.contains_key("$out") || s.contains_key("$merge"))
    })
}

/// O primeiro comando de `text` que escreve, pelo nome (`insert`,
/// `update`...), ou None se o texto só lê. Texto que não dá para ler conta
/// como escrita, por segurança.
pub fn first_write(text: &str) -> Option<String> {
    match parse(text) {
        Ok(statements) => statements
            .iter()
            .find(|s| !s.is_read_only())
            .map(Statement::command_name),
        Err(_) => Some("?".into()),
    }
}

/// Lê todos os comandos de `text`.
pub fn parse(text: &str) -> Result<Vec<Statement>, ParseError> {
    let mut p = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
    };
    let mut out = Vec::new();
    loop {
        p.skip_separators();
        if p.pos >= p.bytes.len() {
            return Ok(out);
        }
        let start = p.pos;
        let action = p.statement()?;
        let end = p.pos;
        p.end_of_statement()?;
        out.push(Statement {
            start,
            text: text[start..end].trim().to_string(),
            action,
        });
    }
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

/// Uma chamada `nome(argumentos)` e onde ela começa.
struct Call {
    name: String,
    args: Vec<Bson>,
    at: usize,
}

impl Parser<'_> {
    fn error<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        self.error_at(message, self.pos)
    }

    fn error_at<T>(&self, message: impl Into<String>, at: usize) -> Result<T, ParseError> {
        Err(ParseError {
            message: message.into(),
            at,
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    /// Pula espaços e comentários; com `newlines`, quebras de linha também.
    fn skip_space(&mut self, newlines: bool) {
        loop {
            match self.peek() {
                Some(b'\n') if !newlines => return,
                Some(c) if c.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.rest().starts_with("//") => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.rest().starts_with("/*") => {
                    self.pos = match self.rest().find("*/") {
                        Some(i) => self.pos + i + 2,
                        None => self.bytes.len(),
                    };
                }
                _ => return,
            }
        }
    }

    fn skip_separators(&mut self) {
        loop {
            self.skip_space(true);
            if self.peek() == Some(b';') {
                self.pos += 1;
            } else {
                return;
            }
        }
    }

    /// Depois de um comando: fim do texto, `;` ou quebra de linha.
    fn end_of_statement(&mut self) -> Result<(), ParseError> {
        self.skip_space(false);
        match self.peek() {
            None | Some(b';' | b'\n') => Ok(()),
            Some(_) => self.error(tr("Unexpected text after the command.")),
        }
    }

    fn ident(&mut self) -> Option<&str> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80)
        {
            self.pos += 1;
        }
        (self.pos > start).then(|| &self.text[start..self.pos])
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), ParseError> {
        if self.eat(c) {
            Ok(())
        } else {
            self.error(trf!("Expected '{c}'.", c = c as char))
        }
    }

    /// O resto da linha, sem o `;` do fim (para `use` e `show`).
    fn word_to_end(&mut self) -> String {
        let start = self.pos;
        while self.peek().is_some_and(|c| c != b'\n' && c != b';') {
            self.pos += 1;
        }
        self.text[start..self.pos].trim().to_string()
    }

    fn statement(&mut self) -> Result<Action, ParseError> {
        let start = self.pos;
        let Some(first) = self.ident().map(str::to_string) else {
            return self.error(tr(
                "Expected a command: db.collection.find(...), show dbs, use database.",
            ));
        };
        match first.as_str() {
            "use" => {
                let name = self.word_to_end();
                let name = name.trim_matches(['"', '\'']).to_string();
                if name.is_empty() {
                    return self.error(tr("Expected the database name after use."));
                }
                Ok(Action::Use(name))
            }
            "show" => {
                let what = self.word_to_end();
                match what.as_str() {
                    "dbs" | "databases" => Ok(list_databases()),
                    "collections" | "tables" => Ok(list_collections()),
                    _ => self.error_at(
                        tr("Only show dbs and show collections are available."),
                        start,
                    ),
                }
            }
            "db" => self.db_expression(),
            _ => self.error_at(
                tr("Expected a command: db.collection.find(...), show dbs, use database."),
                start,
            ),
        }
    }

    /// Depois de `db`: `.metodo(...)`, `.colecao.metodo(...)...`,
    /// `.getCollection("nome").metodo(...)` ou `["nome"].metodo(...)`.
    fn db_expression(&mut self) -> Result<Action, ParseError> {
        let mut collection: Option<String> = None;
        // Nomes separados por ponto até a primeira chamada: o último é o
        // método, os de antes formam o nome da coleção (`db.a.b.find()`).
        let mut names: Vec<String> = Vec::new();
        let mut at = self.pos;
        loop {
            self.skip_space(true);
            if self.eat(b'[') {
                self.skip_space(true);
                let name = self.string()?;
                self.skip_space(true);
                self.expect(b']')?;
                names.push(name);
                continue;
            }
            if !self.eat(b'.') {
                break;
            }
            self.skip_space(true);
            at = self.pos;
            let Some(name) = self.ident().map(str::to_string) else {
                return self.error(tr("Expected a name after the dot."));
            };
            self.skip_space(false);
            if self.peek() == Some(b'(') {
                let args = self.arguments()?;
                if names.is_empty() && name == "getCollection" {
                    collection = Some(self.one_string(&args, at)?);
                    continue;
                }
                if collection.is_none() && !names.is_empty() {
                    collection = Some(names.join("."));
                }
                let first = Call { name, args, at };
                let mut calls = vec![first];
                // O resto da cadeia: `.sort(...)`, `.limit(...)`.
                loop {
                    let save = self.pos;
                    self.skip_space(true);
                    if !self.eat(b'.') {
                        self.pos = save;
                        break;
                    }
                    self.skip_space(true);
                    let at = self.pos;
                    let Some(name) = self.ident().map(str::to_string) else {
                        return self.error(tr("Expected a name after the dot."));
                    };
                    self.skip_space(false);
                    if self.peek() != Some(b'(') {
                        return self.error(tr("Expected '(' after the method name."));
                    }
                    let args = self.arguments()?;
                    calls.push(Call { name, args, at });
                }
                return match collection {
                    Some(c) => self.collection_action(&c, calls),
                    None => self.database_action(calls),
                };
            }
            names.push(name);
        }
        self.error_at(
            tr("Expected a method call, as in db.collection.find()."),
            at,
        )
    }

    fn arguments(&mut self) -> Result<Vec<Bson>, ParseError> {
        self.expect(b'(')?;
        let mut args = Vec::new();
        loop {
            self.skip_space(true);
            if self.eat(b')') {
                return Ok(args);
            }
            args.push(self.value()?);
            self.skip_space(true);
            if !self.eat(b',') && self.peek() != Some(b')') {
                return self.error(tr("Expected ',' or ')'."));
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        let Some(quote) = self.peek().filter(|c| matches!(c, b'"' | b'\'' | b'`')) else {
            return self.error(tr("Expected a text between quotes."));
        };
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.rest().chars().next() else {
                return self.error_at(tr("Unclosed quote."), start);
            };
            self.pos += c.len_utf8();
            if c as u32 == quote as u32 {
                return Ok(out);
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let Some(e) = self.rest().chars().next() else {
                return self.error_at(tr("Unclosed quote."), start);
            };
            self.pos += e.len_utf8();
            match e {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                '0' => out.push('\0'),
                'u' => {
                    let hex = self.rest().get(..4).unwrap_or_default();
                    match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                        Some(c) if hex.len() == 4 => {
                            out.push(c);
                            self.pos += 4;
                        }
                        _ => return self.error(tr("Invalid \\u escape.")),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn value(&mut self) -> Result<Bson, ParseError> {
        self.skip_space(true);
        let at = self.pos;
        match self.peek() {
            None => self.error(tr("Expected a value.")),
            Some(b'{') => Ok(Bson::Document(self.document()?)),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space(true);
                    if self.eat(b']') {
                        return Ok(Bson::Array(items));
                    }
                    items.push(self.value()?);
                    self.skip_space(true);
                    if !self.eat(b',') && self.peek() != Some(b']') {
                        return self.error(tr("Expected ',' or ']'."));
                    }
                }
            }
            Some(b'"' | b'\'' | b'`') => Ok(Bson::String(self.string()?)),
            Some(b'/') => self.regex(),
            Some(c) if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.') => self.number(),
            Some(_) => {
                let Some(name) = self.ident().map(str::to_string) else {
                    return self.error(tr("Expected a value."));
                };
                match name.as_str() {
                    "true" => Ok(Bson::Boolean(true)),
                    "false" => Ok(Bson::Boolean(false)),
                    "null" => Ok(Bson::Null),
                    "undefined" => Ok(Bson::Null),
                    "Infinity" => Ok(Bson::Double(f64::INFINITY)),
                    "NaN" => Ok(Bson::Double(f64::NAN)),
                    "MinKey" | "MaxKey" => {
                        self.skip_space(false);
                        if self.peek() == Some(b'(') {
                            self.arguments()?;
                        }
                        Ok(if name == "MinKey" {
                            Bson::MinKey
                        } else {
                            Bson::MaxKey
                        })
                    }
                    "new" => {
                        self.skip_space(false);
                        let at = self.pos;
                        let Some(name) = self.ident().map(str::to_string) else {
                            return self.error(tr("Expected a type name after new."));
                        };
                        self.constructor(&name, at)
                    }
                    _ => self.constructor(&name, at),
                }
            }
        }
    }

    /// `Nome(argumentos)`: os tipos do BSON que o mongosh escreve assim.
    fn constructor(&mut self, name: &str, at: usize) -> Result<Bson, ParseError> {
        self.skip_space(false);
        if self.peek() != Some(b'(') {
            return self.error_at(
                trf!(
                    "Unknown value '{name}'. Texts go between quotes.",
                    name = name
                ),
                at,
            );
        }
        let args = self.arguments()?;
        let text = |i: usize| -> Option<String> {
            match args.get(i)? {
                Bson::String(s) => Some(s.clone()),
                Bson::Int32(n) => Some(n.to_string()),
                Bson::Int64(n) => Some(n.to_string()),
                Bson::Double(n) => Some(n.to_string()),
                _ => None,
            }
        };
        let bad = |what: &str| {
            self.error_at(trf!("Invalid value for {type_name}.", type_name = what), at)
        };
        match name {
            "ObjectId" => match args.first() {
                None => Ok(Bson::ObjectId(ObjectId::new())),
                Some(Bson::String(s)) => match ObjectId::parse_str(s) {
                    Ok(id) => Ok(Bson::ObjectId(id)),
                    Err(_) => bad("ObjectId"),
                },
                Some(_) => bad("ObjectId"),
            },
            "ISODate" | "Date" => match args.first() {
                None => Ok(Bson::DateTime(bson::DateTime::now())),
                Some(Bson::String(s)) => match parse_date(s) {
                    Some(d) => Ok(Bson::DateTime(d)),
                    None => bad("ISODate"),
                },
                Some(Bson::Int32(n)) => Ok(Bson::DateTime(bson::DateTime::from_millis(*n as i64))),
                Some(Bson::Int64(n)) => Ok(Bson::DateTime(bson::DateTime::from_millis(*n))),
                Some(Bson::Double(n)) => Ok(Bson::DateTime(bson::DateTime::from_millis(*n as i64))),
                Some(_) => bad("ISODate"),
            },
            "NumberLong" | "Long" => match text(0).and_then(|t| t.parse::<i64>().ok()) {
                Some(n) => Ok(Bson::Int64(n)),
                None => bad("NumberLong"),
            },
            "NumberInt" | "Int32" => match text(0).and_then(|t| t.parse::<i32>().ok()) {
                Some(n) => Ok(Bson::Int32(n)),
                None => bad("NumberInt"),
            },
            "Double" => match text(0).and_then(|t| t.parse::<f64>().ok()) {
                Some(n) => Ok(Bson::Double(n)),
                None => bad("Double"),
            },
            "NumberDecimal" | "Decimal128" => {
                match text(0).and_then(|t| t.parse::<bson::Decimal128>().ok()) {
                    Some(n) => Ok(Bson::Decimal128(n)),
                    None => bad("NumberDecimal"),
                }
            }
            "UUID" => match text(0).and_then(|t| bson::Uuid::parse_str(t).ok()) {
                Some(u) => Ok(Bson::Binary(bson::Binary::from_uuid(u))),
                None => bad("UUID"),
            },
            "Timestamp" => {
                let part = |i: usize| text(i).and_then(|t| t.parse::<u32>().ok());
                match (part(0), part(1)) {
                    (Some(time), Some(increment)) => {
                        Ok(Bson::Timestamp(bson::Timestamp { time, increment }))
                    }
                    _ => bad("Timestamp"),
                }
            }
            "RegExp" => match (text(0), args.get(1)) {
                (Some(pattern), None) => Ok(regex(pattern, String::new())),
                (Some(pattern), Some(Bson::String(options))) => Ok(regex(pattern, options.clone())),
                _ => bad("RegExp"),
            },
            _ => self.error_at(
                trf!(
                    "Unknown value '{name}'. Texts go between quotes.",
                    name = name
                ),
                at,
            ),
        }
    }

    fn regex(&mut self) -> Result<Bson, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut pattern = String::new();
        loop {
            let Some(c) = self.rest().chars().next() else {
                return self.error_at(tr("Unclosed regular expression."), start);
            };
            self.pos += c.len_utf8();
            match c {
                '/' => break,
                '\n' => return self.error_at(tr("Unclosed regular expression."), start),
                '\\' => {
                    let Some(e) = self.rest().chars().next() else {
                        return self.error_at(tr("Unclosed regular expression."), start);
                    };
                    self.pos += e.len_utf8();
                    if e != '/' {
                        pattern.push('\\');
                    }
                    pattern.push(e);
                }
                c => pattern.push(c),
            }
        }
        let flags = self.ident().unwrap_or_default().to_string();
        Ok(regex(pattern, flags))
    }

    fn number(&mut self) -> Result<Bson, ParseError> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'-' | b'+')) {
            self.pos += 1;
        }
        if self.rest().starts_with("Infinity") {
            self.pos += "Infinity".len();
            let negative = self.bytes[start] == b'-';
            return Ok(Bson::Double(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }));
        }
        while self.peek().is_some_and(|c| {
            c.is_ascii_digit()
                || c == b'.'
                || c == b'_'
                || matches!(c, b'e' | b'E' | b'x' | b'X')
                || c.is_ascii_hexdigit()
                || (matches!(c, b'-' | b'+')
                    && matches!(self.bytes[self.pos - 1], b'e' | b'E')
                    && !self.text[start..self.pos].contains(['x', 'X']))
        }) {
            self.pos += 1;
        }
        let raw = self.text[start..self.pos].replace('_', "");
        let digits = raw.trim_start_matches(['-', '+']);
        let negative = raw.starts_with('-');
        if let Some(hex) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            return match i64::from_str_radix(hex, 16) {
                Ok(n) => Ok(integer(if negative { -n } else { n })),
                Err(_) => self.error_at(tr("Invalid number."), start),
            };
        }
        if !digits.contains(['.', 'e', 'E'])
            && let Ok(n) = raw.parse::<i64>()
        {
            return Ok(integer(n));
        }
        match raw.parse::<f64>() {
            Ok(n) if !digits.is_empty() => Ok(Bson::Double(n)),
            _ => self.error_at(tr("Invalid number."), start),
        }
    }

    fn document(&mut self) -> Result<Document, ParseError> {
        self.expect(b'{')?;
        let mut out = Document::new();
        loop {
            self.skip_space(true);
            if self.eat(b'}') {
                return Ok(out);
            }
            let key = match self.peek() {
                Some(b'"' | b'\'' | b'`') => self.string()?,
                _ => {
                    let start = self.pos;
                    // Chave sem aspas: nome, `$operador`, número ou
                    // caminho com ponto.
                    while self.peek().is_some_and(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$' | b'.') || c >= 0x80
                    }) {
                        self.pos += 1;
                    }
                    if self.pos == start {
                        return self.error(tr("Expected a field name."));
                    }
                    self.text[start..self.pos].to_string()
                }
            };
            self.skip_space(true);
            self.expect(b':')?;
            let value = self.value()?;
            out.insert(key, value);
            self.skip_space(true);
            if !self.eat(b',') && self.peek() != Some(b'}') {
                return self.error(tr("Expected ',' or '}'."));
            }
        }
    }

    fn one_string(&self, args: &[Bson], at: usize) -> Result<String, ParseError> {
        match args {
            [Bson::String(s)] if !s.is_empty() => Ok(s.clone()),
            _ => self.error_at(tr("Expected one name between quotes."), at),
        }
    }

    /// `db.metodo(...)`.
    fn database_action(&self, calls: Vec<Call>) -> Result<Action, ParseError> {
        let call = &calls[0];
        if let Some(extra) = calls.get(1) {
            return self.error_at(
                trf!("Unknown method '{name}'.", name = extra.name.as_str()),
                extra.at,
            );
        }
        let single = |command: Document, shape: Shape| Action::Single {
            command,
            admin: false,
            shape,
        };
        Ok(match call.name.as_str() {
            "runCommand" | "adminCommand" => {
                let command = match call.args.first() {
                    Some(Bson::Document(d)) if !d.is_empty() => d.clone(),
                    Some(Bson::String(name)) => doc! { name: 1 },
                    _ => {
                        return self.error_at(
                            tr("runCommand expects a document, as in db.runCommand({ping: 1})."),
                            call.at,
                        );
                    }
                };
                Action::Single {
                    command,
                    admin: call.name == "adminCommand",
                    shape: Shape::Reply,
                }
            }
            "getCollectionNames" | "getCollectionInfos" => list_collections(),
            "stats" => single(doc! { "dbStats": 1 }, Shape::Reply),
            "serverStatus" => single(doc! { "serverStatus": 1 }, Shape::Reply),
            "version" => single(
                doc! { "buildInfo": 1 },
                Shape::Value {
                    field: "version",
                    column: "version",
                },
            ),
            "createCollection" => {
                let name = match call.args.first() {
                    Some(Bson::String(s)) if !s.is_empty() => s.clone(),
                    _ => {
                        return self.error_at(tr("Expected one name between quotes."), call.at);
                    }
                };
                let mut command = doc! { "create": name };
                if let Some(Bson::Document(options)) = call.args.get(1) {
                    command.extend(options.clone());
                }
                single(command, Shape::Done)
            }
            "dropDatabase" => single(doc! { "dropDatabase": 1 }, Shape::Done),
            "aggregate" => {
                let pipeline = self.pipeline(call)?;
                Action::Cursor {
                    command: doc! { "aggregate": 1, "pipeline": pipeline, "cursor": {} },
                    admin: false,
                }
            }
            other => {
                return self.error_at(trf!("Unknown method '{name}'.", name = other), call.at);
            }
        })
    }

    fn pipeline(&self, call: &Call) -> Result<Vec<Bson>, ParseError> {
        match call.args.as_slice() {
            [Bson::Array(stages), ..] => Ok(stages.clone()),
            stages if stages.iter().all(|s| matches!(s, Bson::Document(_))) => Ok(stages.to_vec()),
            _ => self.error_at(
                tr("aggregate expects a list of stages, as in aggregate([{$match: {}}])."),
                call.at,
            ),
        }
    }

    /// O argumento `i` como documento (vazio quando falta).
    fn doc_arg(&self, call: &Call, i: usize) -> Result<Document, ParseError> {
        match call.args.get(i) {
            None | Some(Bson::Null) => Ok(Document::new()),
            Some(Bson::Document(d)) => Ok(d.clone()),
            Some(_) => self.error_at(
                trf!(
                    "Argument {n} of {name} must be a document.",
                    n = i + 1,
                    name = call.name.as_str()
                ),
                call.at,
            ),
        }
    }

    fn required_doc(&self, call: &Call, i: usize) -> Result<Document, ParseError> {
        match call.args.get(i) {
            Some(Bson::Document(d)) => Ok(d.clone()),
            _ => self.error_at(
                trf!(
                    "Argument {n} of {name} must be a document.",
                    n = i + 1,
                    name = call.name.as_str()
                ),
                call.at,
            ),
        }
    }

    /// `db.colecao.metodo(...)` e o resto da cadeia.
    fn collection_action(&self, coll: &str, calls: Vec<Call>) -> Result<Action, ParseError> {
        let call = &calls[0];
        let rest = &calls[1..];
        let no_chain = |p: &Self| -> Result<(), ParseError> {
            match rest.first() {
                Some(extra) => p.error_at(
                    trf!("Unknown method '{name}'.", name = extra.name.as_str()),
                    extra.at,
                ),
                None => Ok(()),
            }
        };
        let single = |command: Document, shape: Shape| Action::Single {
            command,
            admin: false,
            shape,
        };
        let cursor = |command: Document| Action::Cursor {
            command,
            admin: false,
        };
        let upsert = |options: &Document| options.get_bool("upsert").unwrap_or(false);
        Ok(match call.name.as_str() {
            "find" | "findOne" => {
                let mut command = doc! { "find": coll, "filter": self.doc_arg(call, 0)? };
                let projection = self.doc_arg(call, 1)?;
                if !projection.is_empty() {
                    command.insert("projection", projection);
                }
                if call.name == "findOne" {
                    command.insert("limit", 1);
                    no_chain(self)?;
                    return Ok(cursor(command));
                }
                let mut count = false;
                let mut explain = false;
                for m in rest {
                    let number = |p: &Self| match m.args.first() {
                        Some(Bson::Int32(n)) if *n >= 0 => Ok(*n as i64),
                        Some(Bson::Int64(n)) if *n >= 0 => Ok(*n),
                        _ => p.error_at(
                            trf!("{name} expects a number.", name = m.name.as_str()),
                            m.at,
                        ),
                    };
                    match m.name.as_str() {
                        "sort" => {
                            command.insert("sort", self.required_doc(m, 0)?);
                        }
                        "limit" => {
                            command.insert("limit", number(self)?);
                        }
                        "skip" => {
                            command.insert("skip", number(self)?);
                        }
                        "projection" | "project" => {
                            command.insert("projection", self.required_doc(m, 0)?);
                        }
                        "hint" => match m.args.first() {
                            Some(h @ (Bson::Document(_) | Bson::String(_))) => {
                                command.insert("hint", h.clone());
                            }
                            _ => {
                                return self
                                    .error_at(tr("hint expects an index name or its keys."), m.at);
                            }
                        },
                        "collation" => {
                            command.insert("collation", self.required_doc(m, 0)?);
                        }
                        "maxTimeMS" => {
                            command.insert("maxTimeMS", number(self)?);
                        }
                        "count" | "itcount" | "size" => count = true,
                        "explain" => explain = true,
                        "toArray" | "pretty" => {}
                        other => {
                            return self
                                .error_at(trf!("Unknown method '{name}'.", name = other), m.at);
                        }
                    }
                }
                if count {
                    let mut count = doc! { "count": coll };
                    for (from, to) in [("filter", "query"), ("limit", "limit"), ("skip", "skip")] {
                        if let Some(v) = command.get(from) {
                            count.insert(to, v.clone());
                        }
                    }
                    single(
                        count,
                        Shape::Value {
                            field: "n",
                            column: "count",
                        },
                    )
                } else if explain {
                    single(doc! { "explain": command }, Shape::Reply)
                } else {
                    cursor(command)
                }
            }
            "countDocuments" | "count" => {
                no_chain(self)?;
                single(
                    doc! { "count": coll, "query": self.doc_arg(call, 0)? },
                    Shape::Value {
                        field: "n",
                        column: "count",
                    },
                )
            }
            "estimatedDocumentCount" => {
                no_chain(self)?;
                single(
                    doc! { "count": coll },
                    Shape::Value {
                        field: "n",
                        column: "count",
                    },
                )
            }
            "distinct" => {
                no_chain(self)?;
                let Some(Bson::String(key)) = call.args.first() else {
                    return self.error_at(
                        tr("distinct expects the field name between quotes."),
                        call.at,
                    );
                };
                single(
                    doc! { "distinct": coll, "key": key, "query": self.doc_arg(call, 1)? },
                    Shape::Values("values"),
                )
            }
            "aggregate" => {
                let pipeline = self.pipeline(call)?;
                let command = doc! { "aggregate": coll, "pipeline": pipeline, "cursor": {} };
                match rest {
                    [] => cursor(command),
                    [m] if matches!(m.name.as_str(), "toArray" | "pretty") => cursor(command),
                    [m] if m.name == "explain" => single(doc! { "explain": command }, Shape::Reply),
                    [m, ..] => {
                        return self.error_at(
                            trf!("Unknown method '{name}'.", name = m.name.as_str()),
                            m.at,
                        );
                    }
                }
            }
            "insertOne" | "insertMany" | "insert" => {
                no_chain(self)?;
                let documents: Vec<Bson> = match call.args.first() {
                    Some(Bson::Document(d)) if call.name != "insertMany" => {
                        vec![Bson::Document(d.clone())]
                    }
                    Some(Bson::Array(items))
                        if call.name != "insertOne"
                            && !items.is_empty()
                            && items.iter().all(|i| matches!(i, Bson::Document(_))) =>
                    {
                        items.clone()
                    }
                    _ => {
                        return self.error_at(
                            tr("insertOne expects a document; insertMany, a list of documents."),
                            call.at,
                        );
                    }
                };
                // O `_id` nasce aqui, como nos drivers, para a mensagem
                // poder dizer qual foi.
                let documents: Vec<Bson> = documents
                    .into_iter()
                    .map(|d| match d {
                        Bson::Document(d) if !d.contains_key("_id") => {
                            let mut with_id = doc! { "_id": ObjectId::new() };
                            with_id.extend(d);
                            Bson::Document(with_id)
                        }
                        other => other,
                    })
                    .collect();
                single(
                    doc! { "insert": coll, "documents": documents },
                    Shape::Write(Write::Insert),
                )
            }
            "updateOne" | "updateMany" | "replaceOne" => {
                no_chain(self)?;
                let filter = self.required_doc(call, 0)?;
                let update = match call.args.get(1) {
                    Some(u @ Bson::Document(d)) => {
                        let operators = d.keys().all(|k| k.starts_with('$')) && !d.is_empty();
                        if call.name == "replaceOne" && operators {
                            return self.error_at(
                                tr("replaceOne expects the new document, without $ operators."),
                                call.at,
                            );
                        }
                        if call.name != "replaceOne" && !operators {
                            return self.error_at(
                                tr("The update must use operators, as in {$set: {field: value}}."),
                                call.at,
                            );
                        }
                        u.clone()
                    }
                    Some(u @ Bson::Array(_)) if call.name != "replaceOne" => u.clone(),
                    _ => {
                        return self.error_at(
                            trf!(
                                "Argument {n} of {name} must be a document.",
                                n = 2,
                                name = call.name.as_str()
                            ),
                            call.at,
                        );
                    }
                };
                let options = self.doc_arg(call, 2)?;
                let mut entry = doc! {
                    "q": filter,
                    "u": update,
                    "multi": call.name == "updateMany",
                    "upsert": upsert(&options),
                };
                if let Ok(filters) = options.get_array("arrayFilters") {
                    entry.insert("arrayFilters", filters.clone());
                }
                single(
                    doc! { "update": coll, "updates": [entry] },
                    Shape::Write(Write::Update),
                )
            }
            "deleteOne" | "deleteMany" => {
                no_chain(self)?;
                // Sem filtro seria apagar tudo por engano: o `{}` tem de
                // ser escrito.
                let filter = self.required_doc(call, 0)?;
                let limit = if call.name == "deleteOne" { 1 } else { 0 };
                single(
                    doc! { "delete": coll, "deletes": [{ "q": filter, "limit": limit }] },
                    Shape::Write(Write::Delete),
                )
            }
            "createIndex" => {
                no_chain(self)?;
                let keys = self.required_doc(call, 0)?;
                let options = self.doc_arg(call, 1)?;
                let name = match options.get_str("name") {
                    Ok(name) => name.to_string(),
                    Err(_) => keys
                        .iter()
                        .map(|(k, v)| format!("{k}_{}", index_direction(v)))
                        .collect::<Vec<_>>()
                        .join("_"),
                };
                let mut index = doc! { "key": keys, "name": name };
                for (k, v) in options {
                    if k != "name" {
                        index.insert(k, v);
                    }
                }
                single(
                    doc! { "createIndexes": coll, "indexes": [index] },
                    Shape::Done,
                )
            }
            "getIndexes" => {
                no_chain(self)?;
                cursor(doc! { "listIndexes": coll })
            }
            "dropIndex" => {
                no_chain(self)?;
                match call.args.first() {
                    Some(i @ (Bson::String(_) | Bson::Document(_))) => single(
                        doc! { "dropIndexes": coll, "index": i.clone() },
                        Shape::Done,
                    ),
                    _ => {
                        return self.error_at(
                            tr("dropIndex expects the index name or its keys."),
                            call.at,
                        );
                    }
                }
            }
            "dropIndexes" => {
                no_chain(self)?;
                single(doc! { "dropIndexes": coll, "index": "*" }, Shape::Done)
            }
            "drop" => {
                no_chain(self)?;
                single(doc! { "drop": coll }, Shape::Done)
            }
            "stats" => {
                no_chain(self)?;
                single(doc! { "collStats": coll }, Shape::Reply)
            }
            other => {
                return self.error_at(trf!("Unknown method '{name}'.", name = other), call.at);
            }
        })
    }
}

fn list_databases() -> Action {
    Action::Single {
        command: doc! { "listDatabases": 1 },
        admin: true,
        shape: Shape::List("databases"),
    }
}

fn list_collections() -> Action {
    Action::Cursor {
        command: doc! { "listCollections": 1, "nameOnly": true },
        admin: false,
    }
}

fn integer(n: i64) -> Bson {
    match i32::try_from(n) {
        Ok(n) => Bson::Int32(n),
        Err(_) => Bson::Int64(n),
    }
}

fn regex(pattern: String, options: String) -> Bson {
    // O servidor quer as opções em ordem alfabética; `g` não existe lá.
    let mut options: Vec<char> = options.chars().filter(|c| *c != 'g').collect();
    options.sort_unstable();
    options.dedup();
    Bson::RegularExpression(bson::Regex {
        pattern,
        options: options.into_iter().collect(),
    })
}

fn index_direction(v: &Bson) -> String {
    match v {
        Bson::Int32(n) => n.to_string(),
        Bson::Int64(n) => n.to_string(),
        Bson::Double(n) => (*n as i64).to_string(),
        Bson::String(s) => s.clone(),
        _ => "1".into(),
    }
}

/// Uma data como o `ISODate` aceita: `2024-05-01`, `2024-05-01T10:00:00Z`,
/// com ou sem fuso (sem, vale UTC).
fn parse_date(text: &str) -> Option<bson::DateTime> {
    let text = text.trim();
    let full = match text.len() {
        10 => format!("{text}T00:00:00Z"),
        _ => {
            let text = text.replacen(' ', "T", 1);
            let time = text.split_once('T').map_or("", |(_, t)| t);
            if time.ends_with(['Z', 'z']) || time.contains(['+', '-']) {
                text
            } else {
                format!("{text}Z")
            }
        }
    };
    bson::DateTime::parse_rfc3339_str(full).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Action {
        let mut all = parse(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert_eq!(all.len(), 1, "{text}");
        all.remove(0).action
    }

    fn command(text: &str) -> Document {
        match one(text) {
            Action::Cursor { command, .. } | Action::Single { command, .. } => command,
            other => panic!("{other:?}"),
        }
    }

    fn error(text: &str) -> ParseError {
        parse(text).expect_err(text)
    }

    #[test]
    fn find_with_a_chain_becomes_a_find_command() {
        let c = command(
            "db.pedidos.find({ total: { $gt: 10 }, 'cliente.nome': \"Ana\" }, { total: 1 })\n  .sort({ data: -1 })\n  .skip(5).limit(20);",
        );
        assert_eq!(
            c,
            doc! {
                "find": "pedidos",
                "filter": { "total": { "$gt": 10 }, "cliente.nome": "Ana" },
                "projection": { "total": 1 },
                "sort": { "data": -1 },
                "skip": 5_i64,
                "limit": 20_i64,
            }
        );
        assert_eq!(
            command("db.pedidos.find()"),
            doc! { "find": "pedidos", "filter": {} }
        );
        assert_eq!(
            command("db.getCollection('meus pedidos').findOne({})"),
            doc! { "find": "meus pedidos", "filter": {}, "limit": 1 }
        );
        assert_eq!(
            command("db['a-b'].find()"),
            doc! { "find": "a-b", "filter": {} }
        );
        assert_eq!(
            command("db.sistema.logs.find()"),
            doc! { "find": "sistema.logs", "filter": {} }
        );
    }

    #[test]
    fn values_follow_the_shell_syntax() {
        let c = command(
            r#"db.c.insertOne({
                _id: ObjectId("507f1f77bcf86cd799439011"),
                quando: ISODate("2024-05-01T10:30:00Z"),
                dia: new Date("2024-05-01"),
                longo: NumberLong("9007199254740993"),
                grande: 3000000000,
                real: 1.5, negativo: -2, hex: 0x10, cientifico: 1e3,
                nome: /^an/i,
                lista: [1, 'dois', null, true,],
                "com espaço": `crase`,
                decimal: NumberDecimal("1.10"),
                escapes: "a\"b\né", // comentário
                /* outro */ vazio: {},
            })"#,
        );
        let d = c.get_array("documents").unwrap()[0].as_document().unwrap();
        assert_eq!(
            d.get_object_id("_id").unwrap().to_hex(),
            "507f1f77bcf86cd799439011"
        );
        assert_eq!(
            d.get_datetime("quando").unwrap().timestamp_millis(),
            1_714_559_400_000
        );
        assert_eq!(
            d.get_datetime("dia").unwrap().timestamp_millis(),
            1_714_521_600_000
        );
        assert_eq!(d.get_i64("longo").unwrap(), 9_007_199_254_740_993);
        assert_eq!(d.get_i64("grande").unwrap(), 3_000_000_000);
        assert_eq!(d.get_f64("real").unwrap(), 1.5);
        assert_eq!(d.get_i32("negativo").unwrap(), -2);
        assert_eq!(d.get_i32("hex").unwrap(), 16);
        assert_eq!(d.get_f64("cientifico").unwrap(), 1000.0);
        assert_eq!(
            d.get("nome"),
            Some(&Bson::RegularExpression(bson::Regex {
                pattern: "^an".into(),
                options: "i".into()
            }))
        );
        assert_eq!(
            d.get_array("lista").unwrap(),
            &vec![
                Bson::Int32(1),
                Bson::String("dois".into()),
                Bson::Null,
                Bson::Boolean(true)
            ]
        );
        assert_eq!(d.get_str("com espaço").unwrap(), "crase");
        assert_eq!(d.get("decimal").unwrap().to_string(), "1.10");
        assert_eq!(d.get_str("escapes").unwrap(), "a\"b\né");
        assert_eq!(d.get_document("vazio").unwrap(), &doc! {});
    }

    #[test]
    fn writes_become_write_commands() {
        // Sem `_id`, ele nasce aqui e vem primeiro.
        let c = command("db.c.insertMany([{a: 1}, {_id: 7, a: 2}])");
        let docs = c.get_array("documents").unwrap();
        let first = docs[0].as_document().unwrap();
        assert_eq!(first.keys().collect::<Vec<_>>(), ["_id", "a"]);
        assert!(first.get_object_id("_id").is_ok());
        assert_eq!(docs[1].as_document().unwrap(), &doc! { "_id": 7, "a": 2 });

        assert_eq!(
            command("db.c.updateMany({a: 1}, {$set: {b: 2}}, {upsert: true})"),
            doc! { "update": "c", "updates": [
                { "q": { "a": 1 }, "u": { "$set": { "b": 2 } }, "multi": true, "upsert": true }
            ] }
        );
        assert_eq!(
            command("db.c.replaceOne({a: 1}, {a: 2})"),
            doc! { "update": "c", "updates": [
                { "q": { "a": 1 }, "u": { "a": 2 }, "multi": false, "upsert": false }
            ] }
        );
        assert_eq!(
            command("db.c.deleteOne({a: 1})"),
            doc! { "delete": "c", "deletes": [{ "q": { "a": 1 }, "limit": 1 }] }
        );
        assert_eq!(
            command("db.c.deleteMany({})"),
            doc! { "delete": "c", "deletes": [{ "q": {}, "limit": 0 }] }
        );
        assert_eq!(
            command("db.c.createIndex({a: 1, b: -1}, {unique: true})"),
            doc! { "createIndexes": "c", "indexes": [
                { "key": { "a": 1, "b": -1 }, "name": "a_1_b_-1", "unique": true }
            ] }
        );
        // Esquecer o operador ou o filtro é erro, não uma troca do
        // documento inteiro nem um apagar tudo.
        assert!(
            error("db.c.updateOne({a: 1}, {b: 2})")
                .message
                .contains("operators")
        );
        assert!(error("db.c.deleteMany()").message.contains("document"));
    }

    #[test]
    fn counts_aggregates_and_database_commands() {
        assert_eq!(
            one("db.c.countDocuments({a: 1})"),
            Action::Single {
                command: doc! { "count": "c", "query": { "a": 1 } },
                admin: false,
                shape: Shape::Value {
                    field: "n",
                    column: "count"
                },
            }
        );
        assert_eq!(
            command("db.c.find({a: 1}).count()"),
            doc! { "count": "c", "query": { "a": 1 } }
        );
        assert_eq!(
            command("db.c.aggregate([{$match: {a: 1}}, {$group: {_id: '$b', n: {$sum: 1}}}])"),
            doc! { "aggregate": "c", "pipeline": [
                { "$match": { "a": 1 } },
                { "$group": { "_id": "$b", "n": { "$sum": 1 } } },
            ], "cursor": {} }
        );
        assert_eq!(
            command("db.c.find({a: 1}).explain()"),
            doc! { "explain": { "find": "c", "filter": { "a": 1 } } }
        );
        assert_eq!(
            one("db.c.distinct('cidade', {uf: 'PE'})"),
            Action::Single {
                command: doc! { "distinct": "c", "key": "cidade", "query": { "uf": "PE" } },
                admin: false,
                shape: Shape::Values("values"),
            }
        );
        assert_eq!(command("db.runCommand({ping: 1})"), doc! { "ping": 1 });
        assert_eq!(
            one("db.adminCommand('listDatabases')"),
            Action::Single {
                command: doc! { "listDatabases": 1 },
                admin: true,
                shape: Shape::Reply,
            }
        );
        assert_eq!(
            command("db.createCollection('nova', {capped: true, size: 100})"),
            doc! { "create": "nova", "capped": true, "size": 100 }
        );
        assert_eq!(command("db.c.getIndexes()"), doc! { "listIndexes": "c" });
    }

    #[test]
    fn several_statements_and_shell_helpers() {
        let all = parse(
            "use loja\nshow dbs;\nshow collections\n\n// comentário\ndb.c.find({a: ';'}); db.d.drop()\n",
        )
        .unwrap();
        let names: Vec<String> = all.iter().map(Statement::command_name).collect();
        assert_eq!(
            names,
            ["use", "listDatabases", "listCollections", "find", "drop"]
        );
        assert_eq!(all[0].action, Action::Use("loja".into()));
        assert_eq!(all[3].text, "db.c.find({a: ';'})");
        assert_eq!(&"use loja\nshow dbs"[all[1].start..], "show dbs");
        assert!(parse("  \n// nada\n").unwrap().is_empty());
    }

    #[test]
    fn read_only_is_told_from_the_command() {
        assert_eq!(
            first_write("show dbs\ndb.c.find()\ndb.c.find().count()"),
            None
        );
        assert_eq!(first_write("db.c.aggregate([{$match: {}}])"), None);
        assert_eq!(
            first_write("db.c.aggregate([{$match: {}}, {$out: 'x'}])"),
            Some("aggregate".into())
        );
        assert_eq!(
            first_write("db.c.find()\ndb.c.deleteMany({})"),
            Some("delete".into())
        );
        assert_eq!(first_write("db.c.drop()"), Some("drop".into()));
        assert_eq!(
            first_write("db.runCommand({dropDatabase: 1})"),
            Some("dropDatabase".into())
        );
        assert_eq!(first_write("db.runCommand({ping: 1})"), None);
        assert_eq!(first_write("db.c.find({a: 1}).explain()"), None);
        // O que não dá para ler conta como escrita.
        assert_eq!(first_write("db.c.find({"), Some("?".into()));
    }

    #[test]
    fn errors_say_where() {
        let e = error("db.c.find({a: 1)");
        assert_eq!(e.at, 15, "{e:?}");
        let e = error("db.c.find({a: abc})");
        assert!(e.message.contains("abc"), "{e:?}");
        assert_eq!(e.at, 14);
        let e = error("db.c.find()\ndb.c.voar()");
        assert!(e.message.contains("voar"), "{e:?}");
        assert_eq!(e.at, 17);
        assert!(error("select 1").message.contains("db.collection.find"));
        assert!(
            error("db.c.find() abc")
                .message
                .contains("after the command")
        );
        assert!(error("db.c.find({a: 'x})").message.contains("quote"));
        assert!(error("db.c").message.contains("method"));
        assert!(error("db.c.find().limit('x')").message.contains("number"));
        assert!(
            error("db.c.find({_id: ObjectId('xyz')})")
                .message
                .contains("ObjectId")
        );
    }
}
