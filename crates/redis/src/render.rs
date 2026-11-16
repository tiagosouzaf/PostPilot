//! Uma resposta do Redis como grade: colunas e linhas de texto.

use crate::command::Command;
use crate::resp::{Value, bytes_to_text};

/// O que a janela de consulta mostra para uma resposta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rendered {
    /// Resposta de status (`OK`, `PONG`): só uma mensagem.
    Status(String),
    /// Erro do servidor.
    Error(String),
    /// Uma grade, com uma mensagem opcional (o cursor do `SCAN`).
    Grid {
        columns: Vec<String>,
        rows: Vec<Vec<Option<String>>>,
        note: Option<String>,
    },
}

fn grid(columns: &[&str], rows: Vec<Vec<Option<String>>>) -> Rendered {
    Rendered::Grid {
        columns: columns.iter().map(|c| (*c).to_string()).collect(),
        rows,
        note: None,
    }
}

fn has_option(command: &Command, option: &str) -> bool {
    command
        .args
        .iter()
        .skip(1)
        .any(|a| a.eq_ignore_ascii_case(option.as_bytes()))
}

/// Nomes das duas colunas quando a resposta do comando é de pares.
fn pair_columns(command: &Command) -> Option<[&'static str; 2]> {
    let name = command.name();
    match name.as_str() {
        "HGETALL" | "HSCAN" => Some(["field", "value"]),
        "HRANDFIELD" if has_option(command, "WITHVALUES") => Some(["field", "value"]),
        "CONFIG" if command.sub() == "GET" => Some(["parameter", "value"]),
        "ZSCAN" => Some(["member", "score"]),
        "ZPOPMIN" | "ZPOPMAX" => Some(["member", "score"]),
        n if n.starts_with('Z')
            && (has_option(command, "WITHSCORES") || has_option(command, "WITHSCORE")) =>
        {
            Some(["member", "score"])
        }
        "XINFO" | "MEMORY" | "HELLO" => Some(["property", "value"]),
        "PUBSUB" if command.sub() == "NUMSUB" => Some(["channel", "subscribers"]),
        _ => None,
    }
}

/// A resposta de `command` como o que mostrar.
pub fn render(command: &Command, value: Value) -> Rendered {
    let name = command.name();
    match value {
        Value::Simple(s) => Rendered::Status(s),
        Value::Error(e) => Rendered::Error(e),
        Value::Bulk(text) if name == "INFO" => info(&bytes_to_text(&text)),
        Value::Bulk(text) if name == "CLIENT" && command.sub() == "LIST" => {
            key_value_lines(&bytes_to_text(&text))
        }
        Value::Bulk(text) if name == "CLUSTER" && command.sub() == "INFO" => {
            let rows = bytes_to_text(&text)
                .lines()
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| vec![Some(k.to_string()), Some(v.trim().to_string())])
                .collect();
            grid(&["property", "value"], rows)
        }
        Value::Map(pairs) => {
            let columns = pair_columns(command).unwrap_or(["field", "value"]);
            grid(
                &columns,
                pairs
                    .into_iter()
                    .map(|(k, v)| vec![k.to_cell(), v.to_cell()])
                    .collect(),
            )
        }
        // Cursor e itens.
        Value::Array(mut items)
            if matches!(name.as_str(), "SCAN" | "HSCAN" | "SSCAN" | "ZSCAN")
                && items.len() == 2 =>
        {
            let found = items.pop().unwrap_or(Value::Null);
            let cursor = items.pop().and_then(|c| c.to_cell()).unwrap_or_default();
            let rendered = match pair_columns(command) {
                Some(columns) if !has_option(command, "NOVALUES") => pairs(&columns, found),
                _ => list(
                    if name == "SCAN" { "key" } else { "member" },
                    found.into_items(),
                ),
            };
            match rendered {
                Rendered::Grid { columns, rows, .. } => Rendered::Grid {
                    columns,
                    rows,
                    note: Some(format!("cursor: {cursor}")),
                },
                other => other,
            }
        }
        Value::Array(items) if matches!(name.as_str(), "XRANGE" | "XREVRANGE") => {
            let rows = items
                .into_iter()
                .map(|entry| {
                    let mut parts = entry.into_items().into_iter();
                    let id = parts.next().and_then(|v| v.to_cell());
                    let fields = parts
                        .next()
                        .map(|f| Value::Map(f.into_pairs()))
                        .and_then(|f| f.to_cell());
                    vec![id, fields]
                })
                .collect();
            grid(&["id", "fields"], rows)
        }
        value @ (Value::Array(_) | Value::Set(_) | Value::Push(_)) => {
            let items = match value {
                Value::Array(i) | Value::Set(i) | Value::Push(i) => i,
                _ => Vec::new(),
            };
            let nested = |v: &Value| matches!(v, Value::Array(_) | Value::Set(_));
            if let Some(columns) = pair_columns(command) {
                // RESP3 manda os pares já em listas de dois; RESP2, tudo
                // numa lista só.
                if !items.is_empty() && items.iter().all(nested) {
                    return table(&columns, items);
                }
                let composite = |v: &Value| nested(v) || matches!(v, Value::Map(_));
                if items.len() % 2 == 0 && !items.iter().any(composite) {
                    return pairs(&columns, Value::Array(items));
                }
            }
            if !items.is_empty() && items.iter().all(|v| matches!(v, Value::Map(_))) {
                return maps(items);
            }
            let width = match items.first() {
                Some(Value::Array(first)) => first.len(),
                _ => 0,
            };
            if width > 1
                && items
                    .iter()
                    .all(|v| matches!(v, Value::Array(a) if a.len() == width))
            {
                let names: Vec<String> = (1..=width).map(|i| i.to_string()).collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                return table(&names, items);
            }
            let column = match name.as_str() {
                "KEYS" => "key",
                "SMEMBERS" | "SINTER" | "SUNION" | "SDIFF" | "SRANDMEMBER" | "SPOP" => "member",
                "HKEYS" => "field",
                n if n.starts_with('Z') => "member",
                _ => "value",
            };
            list(column, items)
        }
        scalar => grid(&["value"], vec![vec![scalar.to_cell()]]),
    }
}

fn list(column: &str, items: Vec<Value>) -> Rendered {
    grid(
        &[column],
        items.into_iter().map(|v| vec![v.to_cell()]).collect(),
    )
}

fn pairs(columns: &[&str], value: Value) -> Rendered {
    grid(
        columns,
        value
            .into_pairs()
            .into_iter()
            .map(|(k, v)| vec![k.to_cell(), v.to_cell()])
            .collect(),
    )
}

/// Linhas que são listas do mesmo tamanho.
fn table(columns: &[&str], items: Vec<Value>) -> Rendered {
    let width = columns.len();
    grid(
        columns,
        items
            .into_iter()
            .map(|row| {
                let mut cells: Vec<Option<String>> =
                    row.into_items().iter().map(Value::to_cell).collect();
                cells.resize(width, None);
                cells
            })
            .collect(),
    )
}

/// Lista de mapas: uma coluna por chave do primeiro.
fn maps(items: Vec<Value>) -> Rendered {
    let mut columns: Vec<String> = Vec::new();
    let rows: Vec<Vec<(String, Option<String>)>> = items
        .into_iter()
        .map(|m| {
            m.into_pairs()
                .into_iter()
                .map(|(k, v)| (k.to_cell().unwrap_or_default(), v.to_cell()))
                .collect()
        })
        .collect();
    for row in &rows {
        for (k, _) in row {
            if !columns.contains(k) {
                columns.push(k.clone());
            }
        }
    }
    let rows = rows
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| {
                    row.iter()
                        .find(|(k, _)| k == c)
                        .and_then(|(_, v)| v.clone())
                })
                .collect()
        })
        .collect();
    Rendered::Grid {
        columns,
        rows,
        note: None,
    }
}

/// O texto do `INFO`: seção, propriedade e valor.
fn info(text: &str) -> Rendered {
    let mut section = String::new();
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('#') {
            section = name.trim().to_string();
        } else if let Some((k, v)) = line.split_once(':') {
            rows.push(vec![
                Some(section.clone()),
                Some(k.to_string()),
                Some(v.to_string()),
            ]);
        }
    }
    grid(&["section", "property", "value"], rows)
}

/// Linhas de `chave=valor chave=valor` (o `CLIENT LIST`): uma coluna por
/// chave.
fn key_value_lines(text: &str) -> Rendered {
    let mut columns: Vec<String> = Vec::new();
    let parsed: Vec<Vec<(&str, &str)>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(' ').filter_map(|p| p.split_once('=')).collect())
        .collect();
    for row in &parsed {
        for (k, _) in row {
            if !columns.iter().any(|c| c == k) {
                columns.push((*k).to_string());
            }
        }
    }
    let rows = parsed
        .into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| {
                    row.iter()
                        .find(|(k, _)| k == c)
                        .map(|(_, v)| (*v).to_string())
                })
                .collect()
        })
        .collect();
    Rendered::Grid {
        columns,
        rows,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::split;

    fn cmd(text: &str) -> Command {
        split(text).unwrap().remove(0)
    }

    fn b(s: &str) -> Value {
        Value::Bulk(s.as_bytes().to_vec())
    }

    fn cells(r: &Rendered) -> (Vec<&str>, Vec<Vec<&str>>) {
        match r {
            Rendered::Grid { columns, rows, .. } => (
                columns.iter().map(String::as_str).collect(),
                rows.iter()
                    .map(|r| r.iter().map(|c| c.as_deref().unwrap_or("NULL")).collect())
                    .collect(),
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scalars_statuses_and_errors() {
        assert_eq!(
            render(&cmd("SET a 1"), Value::Simple("OK".into())),
            Rendered::Status("OK".into())
        );
        assert_eq!(
            cells(&render(&cmd("GET a"), b("1"))),
            (vec!["value"], vec![vec!["1"]])
        );
        assert_eq!(
            cells(&render(&cmd("GET nada"), Value::Null)),
            (vec!["value"], vec![vec!["NULL"]])
        );
        assert_eq!(
            render(&cmd("GET h"), Value::Error("WRONGTYPE x".into())),
            Rendered::Error("WRONGTYPE x".into())
        );
    }

    #[test]
    fn pairs_in_both_protocols() {
        let expected = (vec!["field", "value"], vec![vec!["a", "1"], vec!["b", "2"]]);
        let flat = Value::Array(vec![b("a"), b("1"), b("b"), b("2")]);
        assert_eq!(cells(&render(&cmd("hgetall h"), flat)), expected);
        let map = Value::Map(vec![(b("a"), b("1")), (b("b"), b("2"))]);
        assert_eq!(cells(&render(&cmd("HGETALL h"), map)), expected);
        let z3 = Value::Array(vec![Value::Array(vec![
            b("m"),
            Value::Double("1.5".into()),
        ])]);
        assert_eq!(
            cells(&render(&cmd("ZRANGE z 0 -1 WITHSCORES"), z3)),
            (vec!["member", "score"], vec![vec!["m", "1.5"]])
        );
        let z2 = Value::Array(vec![b("m"), b("1.5")]);
        assert_eq!(
            cells(&render(&cmd("ZRANGE z 0 -1 withscores"), z2)),
            (vec!["member", "score"], vec![vec!["m", "1.5"]])
        );
        assert_eq!(
            cells(&render(&cmd("ZRANGE z 0 -1"), Value::Array(vec![b("m")]))),
            (vec!["member"], vec![vec!["m"]])
        );
    }

    #[test]
    fn scan_shows_the_cursor() {
        let v = Value::Array(vec![b("17"), Value::Array(vec![b("k1"), b("k2")])]);
        match render(&cmd("SCAN 0"), v) {
            Rendered::Grid {
                columns,
                rows,
                note,
            } => {
                assert_eq!(columns, ["key"]);
                assert_eq!(rows.len(), 2);
                assert_eq!(note.as_deref(), Some("cursor: 17"));
            }
            other => panic!("{other:?}"),
        }
        let v = Value::Array(vec![b("0"), Value::Array(vec![b("f"), b("v")])]);
        assert_eq!(
            cells(&render(&cmd("HSCAN h 0"), v)),
            (vec!["field", "value"], vec![vec!["f", "v"]])
        );
    }

    #[test]
    fn info_client_list_and_streams() {
        let r = render(
            &cmd("INFO"),
            b("# Server\r\nredis_version:7.2.4\r\n\r\n# Keyspace\r\ndb0:keys=2,expires=0\r\n"),
        );
        assert_eq!(
            cells(&r),
            (
                vec!["section", "property", "value"],
                vec![
                    vec!["Server", "redis_version", "7.2.4"],
                    vec!["Keyspace", "db0", "keys=2,expires=0"]
                ]
            )
        );
        let r = render(
            &cmd("CLIENT LIST"),
            b("id=3 addr=1.2.3.4:5 name=\nid=4 addr=x db=1\n"),
        );
        assert_eq!(
            cells(&r),
            (
                vec!["id", "addr", "name", "db"],
                vec![
                    vec!["3", "1.2.3.4:5", "", "NULL"],
                    vec!["4", "x", "NULL", "1"]
                ]
            )
        );
        let entry = Value::Array(vec![b("1-0"), Value::Array(vec![b("a"), b("1")])]);
        assert_eq!(
            cells(&render(&cmd("XRANGE s - +"), Value::Array(vec![entry]))),
            (vec!["id", "fields"], vec![vec!["1-0", r#"{"a":"1"}"#]])
        );
    }

    #[test]
    fn nested_lists_become_columns_or_json() {
        let v = Value::Array(vec![
            Value::Array(vec![Value::Int(1), b("a"), Value::Array(vec![b("x")])]),
            Value::Array(vec![Value::Int(2), b("b"), Value::Null]),
        ]);
        assert_eq!(
            cells(&render(&cmd("SLOWLOG GET"), v)),
            (
                vec!["1", "2", "3"],
                vec![vec!["1", "a", r#"["x"]"#], vec!["2", "b", "NULL"]]
            )
        );
        let v = Value::Array(vec![
            Value::Map(vec![(b("name"), b("g1")), (b("pending"), Value::Int(0))]),
            Value::Map(vec![(b("name"), b("g2")), (b("lag"), Value::Int(3))]),
        ]);
        assert_eq!(
            cells(&render(&cmd("COMMAND DOCS x"), v)),
            (
                vec!["name", "pending", "lag"],
                vec![vec!["g1", "0", "NULL"], vec!["g2", "NULL", "3"]]
            )
        );
    }
}
