//! O texto de uma janela de consulta do Elasticsearch, na sintaxe do
//! console do Kibana:
//!
//! ```text
//! GET /_cat/indices
//!
//! POST /pedidos/_search
//! {
//!   "query": { "match": { "cliente": "ana" } }
//! }
//! ```
//!
//! Cada requisição começa numa linha `MÉTODO caminho`; as linhas seguintes,
//! até a próxima requisição, são o corpo. Linhas que começam com `#` ou
//! `//` são comentários. Um corpo com vários JSON (o `_bulk` e o
//! `_msearch`) vai um por linha, como a API pede, mesmo escrito em várias.

use pgstudio_i18n::{tr, trf};

use crate::json::Json;

pub const METHODS: &[&str] = &["DELETE", "GET", "HEAD", "PATCH", "POST", "PUT"];

/// Uma requisição do texto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Onde a linha do método começa (bytes).
    pub start: usize,
    /// Em maiúsculas.
    pub method: String,
    /// Com a `/` do começo e a query string, como foi escrito.
    pub path: String,
    /// O corpo como foi escrito, sem os comentários.
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub at: usize,
    pub message: String,
}

/// O método e o caminho de uma linha `GET /_search`.
fn request_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let (method, rest) = line.split_once(char::is_whitespace)?;
    let method = method.to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return None;
    }
    let path = rest.trim();
    // Um caminho não tem espaço: `get the docs` num corpo não é método.
    if path.is_empty() || path.contains(char::is_whitespace) {
        return None;
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Some((method, path))
}

/// Só o método, sem caminho.
fn bare_method(line: &str) -> bool {
    METHODS.contains(&line.trim().to_ascii_uppercase().as_str())
}

/// As requisições de `text`, em ordem.
pub fn parse(text: &str) -> Result<Vec<Request>, ParseError> {
    let mut requests: Vec<Request> = Vec::new();
    let mut at = 0;
    // Dentro de um texto `"""..."""`, que pode ter linhas com `#`.
    let mut triple = false;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed = line.trim();
        if !triple {
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                continue;
            }
            if let Some((method, path)) = request_line(line) {
                requests.push(Request {
                    start: start + (line.len() - line.trim_start().len()),
                    method,
                    path,
                    body: String::new(),
                });
                continue;
            }
            if bare_method(line) {
                return Err(ParseError {
                    at: start,
                    message: trf!(
                        "Missing the path after {method}, as in GET /_search.",
                        method = trimmed.to_ascii_uppercase()
                    ),
                });
            }
        }
        let Some(current) = requests.last_mut() else {
            return Err(ParseError {
                at: start,
                message: tr(
                    "Start each request with the method and the path, as in GET /_cat/indices.",
                )
                .to_string(),
            });
        };
        if line.matches("\"\"\"").count() % 2 == 1 {
            triple = !triple;
        }
        current.body.push_str(line);
    }
    Ok(requests)
}

/// O último pedaço do caminho, sem a query string: `_search`.
pub fn endpoint(path: &str) -> &str {
    let path = path.split('?').next().unwrap_or_default();
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// O primeiro pedaço do caminho que começa com `_`: a API (`_search`,
/// `_cat`, `_doc`), venha ou não depois de um índice.
pub fn api(path: &str) -> &str {
    path.split('?')
        .next()
        .unwrap_or_default()
        .split('/')
        .find(|part| part.starts_with('_'))
        .unwrap_or("")
}

/// APIs que só leem mesmo com POST.
const READ_APIS: &[&str] = &[
    "_analyze",
    "_async_search",
    "_cat",
    "_count",
    "_eql",
    "_explain",
    "_field_caps",
    "_knn_search",
    "_mget",
    "_msearch",
    "_mtermvectors",
    "_rank_eval",
    "_render",
    "_search",
    "_search_shards",
    "_sql",
    "_terms_enum",
    "_termvectors",
    "_validate",
];

/// Se a requisição só lê: `GET` e `HEAD`, e os `POST` das APIs de busca.
pub fn is_read_only(method: &str, path: &str) -> bool {
    match method {
        "GET" | "HEAD" => true,
        "POST" => READ_APIS.contains(&api(path)),
        _ => false,
    }
}

/// A primeira requisição de `text` que muda alguma coisa, como
/// `DELETE /pedidos`. `None` se todas só leem (ou o texto não é válido:
/// o erro aparece ao rodar).
pub fn first_write(text: &str) -> Option<String> {
    parse(text)
        .ok()?
        .into_iter()
        .find(|r| !is_read_only(&r.method, &r.path))
        .map(|r| format!("{} {}", r.method, r.path))
}

/// Troca os textos `"""..."""` do Kibana por textos JSON e separa os
/// valores do primeiro nível.
fn values(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("\"\"\"") {
            let (inner, tail) = match after.split_once("\"\"\"") {
                Some((inner, tail)) => (inner, tail),
                None => (after, ""),
            };
            current.push_str(&serde_json::to_string(inner).unwrap_or_default());
            rest = tail;
        } else if c == '"' {
            // Um texto JSON vai inteiro, com os escapes.
            let mut end = rest.len();
            let mut escaped = false;
            for (i, d) in rest.char_indices().skip(1) {
                match d {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    _ => {}
                }
            }
            current.push_str(&rest[..end]);
            rest = &rest[end..];
        } else {
            match c {
                '{' | '[' => depth += 1,
                '}' | ']' => depth = depth.saturating_sub(1),
                _ => {}
            }
            current.push(c);
            rest = &rest[c.len_utf8()..];
            if depth == 0 && matches!(c, '}' | ']') {
                out.push(std::mem::take(&mut current));
            }
        }
        if depth == 0 && current.trim().is_empty() {
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// O corpo como vai ao servidor e o `Content-Type`. `None` sem corpo.
pub fn body(request: &Request) -> Option<(String, &'static str)> {
    let mut values = values(&request.body);
    if values.is_empty() {
        return None;
    }
    let lines = matches!(endpoint(&request.path), "_bulk" | "_msearch") || values.len() > 1;
    if !lines {
        return Some((values.remove(0).trim().to_string(), "application/json"));
    }
    let mut out = String::new();
    for value in values {
        // Um JSON por linha; o que não for JSON vai como está para o
        // servidor dizer o que há de errado.
        match Json::parse(&value) {
            Ok(json) => out.push_str(&json.compact()),
            Err(_) => out.push_str(&value.replace(['\n', '\r'], " ")),
        }
        out.push('\n');
    }
    Some((out, "application/x-ndjson"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<(String, String, String)> {
        parse(text)
            .unwrap()
            .into_iter()
            .map(|r| (r.method, r.path, r.body.trim().to_string()))
            .collect()
    }

    #[test]
    fn requests_are_split_by_the_method_line() {
        let text = "# índices\nGET _cat/indices?v\n\npost /pedidos/_search\n{\n  \"size\": 1\n}\n// fim\nDELETE /pedidos";
        assert_eq!(
            lines(text),
            [
                ("GET".into(), "/_cat/indices?v".into(), String::new()),
                (
                    "POST".into(),
                    "/pedidos/_search".into(),
                    "{\n  \"size\": 1\n}".into()
                ),
                ("DELETE".into(), "/pedidos".into(), String::new()),
            ]
        );
        let requests = parse(text).unwrap();
        assert_eq!(&text[requests[1].start..requests[1].start + 4], "post");
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn errors_say_what_is_missing() {
        let e = parse("{ \"size\": 1 }").unwrap_err();
        assert_eq!(e.at, 0);
        assert!(e.message.contains("GET /_cat/indices"), "{}", e.message);
        let e = parse("GET /a\nPOST\n").unwrap_err();
        assert_eq!(e.at, 7);
        assert!(e.message.contains("POST"), "{}", e.message);
    }

    #[test]
    fn a_body_line_that_looks_like_text_is_not_a_request() {
        let text = "POST /a/_doc\n{ \"t\":\n\"get the docs\" }";
        assert_eq!(lines(text).len(), 1);
        // Dentro de `\"\"\"` nada é comentário nem requisição.
        let text = "POST /_scripts/x\n{ \"script\": { \"source\": \"\"\"\n# não é comentário\nGET /nada\n\"\"\" } }\nGET /b";
        let got = lines(text);
        assert_eq!(got.len(), 2);
        assert!(got[0].2.contains("# não é comentário"));
    }

    #[test]
    fn bodies_go_as_json_or_one_json_per_line() {
        let one = &parse("POST /a/_search\n{\n  \"size\": 1\n}\n").unwrap()[0];
        assert_eq!(
            body(one),
            Some(("{\n  \"size\": 1\n}".into(), "application/json"))
        );
        assert_eq!(body(&parse("GET /a").unwrap()[0]), None);
        let bulk = &parse(
            "POST /_bulk\n{ \"index\": { \"_index\": \"a\" } }\n{\n  \"nome\": \"x } y\"\n}\n",
        )
        .unwrap()[0];
        assert_eq!(
            body(bulk),
            Some((
                "{\"index\":{\"_index\":\"a\"}}\n{\"nome\":\"x } y\"}\n".into(),
                "application/x-ndjson"
            ))
        );
        // Um `_bulk` de uma linha só também termina com a quebra.
        let bulk = &parse("POST /a/_bulk\n{ \"delete\": { \"_id\": \"1\" } }").unwrap()[0];
        assert_eq!(body(bulk).unwrap().0, "{\"delete\":{\"_id\":\"1\"}}\n");
        // Texto de três aspas do Kibana.
        let script = &parse("POST /a/_search\n{ \"q\": \"\"\"a \"b\"\nc\"\"\" }").unwrap()[0];
        assert_eq!(body(script).unwrap().0, r#"{ "q": "a \"b\"\nc" }"#);
    }

    #[test]
    fn reads_and_writes_are_told_apart() {
        assert!(is_read_only("GET", "/a/_doc/1"));
        assert!(is_read_only("POST", "/a/_search?size=1"));
        assert!(is_read_only("POST", "/_sql?format=json"));
        assert!(is_read_only("POST", "/a/_search/scroll"));
        assert!(!is_read_only("POST", "/a/_doc"));
        assert!(!is_read_only("POST", "/a/_delete_by_query"));
        assert!(!is_read_only("PUT", "/a"));
        assert_eq!(first_write("GET /a\nPOST /a/_count"), None);
        assert_eq!(
            first_write("GET /a\ndelete /a\nPUT /b"),
            Some("DELETE /a".into())
        );
        assert_eq!(endpoint("/a/_bulk?refresh=true"), "_bulk");
        assert_eq!(api("/a/_doc/1"), "_doc");
        assert_eq!(api("/a"), "");
        assert!(METHODS.windows(2).all(|w| w[0] < w[1]));
        assert!(READ_APIS.windows(2).all(|w| w[0] < w[1]));
    }
}
