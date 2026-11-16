//! O protocolo do Redis (RESP2 e RESP3): escrever um comando e ler uma
//! resposta.

use std::future::Future;
use std::io;
use std::pin::Pin;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

/// Uma resposta do servidor.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `nil` (`$-1`, `*-1` ou `_`).
    Null,
    /// Resposta de status, como `OK`.
    Simple(String),
    /// Erro do servidor, como `ERR unknown command`.
    Error(String),
    Int(i64),
    /// Texto ou bytes.
    Bulk(Vec<u8>),
    Array(Vec<Value>),
    /// RESP3: pares de chave e valor (`HGETALL`, `CONFIG GET`...).
    Map(Vec<(Value, Value)>),
    /// RESP3: conjunto (`SMEMBERS`).
    Set(Vec<Value>),
    /// RESP3: número de ponto flutuante, como o servidor escreveu.
    Double(String),
    Bool(bool),
    /// RESP3: inteiro grande, como o servidor escreveu.
    BigNumber(String),
    /// RESP3: mensagem fora de pedido (pub/sub).
    Push(Vec<Value>),
}

impl Value {
    /// O valor como texto de uma célula: `None` é nil; listas e mapas
    /// dentro de uma célula saem como JSON.
    pub fn to_cell(&self) -> Option<String> {
        match self {
            Value::Null => None,
            Value::Simple(s) | Value::Error(s) | Value::Double(s) | Value::BigNumber(s) => {
                Some(s.clone())
            }
            Value::Int(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Bulk(b) => Some(bytes_to_text(b)),
            Value::Array(_) | Value::Map(_) | Value::Set(_) | Value::Push(_) => {
                let mut out = String::new();
                self.write_json(&mut out);
                Some(out)
            }
        }
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::Bool(b) => out.push_str(&b.to_string()),
            Value::Double(s) if s.parse::<f64>().is_ok_and(f64::is_finite) => out.push_str(s),
            Value::BigNumber(s) => out.push_str(s),
            Value::Simple(s) | Value::Error(s) | Value::Double(s) => json_string(s, out),
            Value::Bulk(b) => json_string(&bytes_to_text(b), out),
            Value::Array(items) | Value::Set(items) | Value::Push(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            Value::Map(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    json_string(&k.to_cell().unwrap_or_default(), out);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }

    /// O texto de uma resposta simples ou em bytes.
    pub fn as_text(&self) -> Option<String> {
        match self {
            Value::Simple(s) => Some(s.clone()),
            Value::Bulk(b) => Some(bytes_to_text(b)),
            Value::Int(n) => Some(n.to_string()),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Bulk(_) | Value::Simple(_) => self.as_text()?.parse().ok(),
            _ => None,
        }
    }

    /// Os itens de uma lista, conjunto ou push; um mapa vira chave, valor,
    /// chave, valor (como no RESP2).
    pub fn into_items(self) -> Vec<Value> {
        match self {
            Value::Array(items) | Value::Set(items) | Value::Push(items) => items,
            Value::Map(pairs) => pairs.into_iter().flat_map(|(k, v)| [k, v]).collect(),
            Value::Null => Vec::new(),
            other => vec![other],
        }
    }

    /// Pares de um mapa (RESP3) ou de uma lista chave, valor (RESP2).
    pub fn into_pairs(self) -> Vec<(Value, Value)> {
        match self {
            Value::Map(pairs) => pairs,
            other => {
                let mut items = other.into_items().into_iter();
                let mut pairs = Vec::new();
                while let (Some(k), Some(v)) = (items.next(), items.next()) {
                    pairs.push((k, v));
                }
                pairs
            }
        }
    }
}

fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Bytes como texto: UTF-8 quando é; senão, como o redis-cli mostra
/// (`\xNN` para o que não é ASCII imprimível).
pub fn bytes_to_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let mut out = String::with_capacity(bytes.len());
            for &b in bytes {
                match b {
                    b'\\' => out.push_str("\\\\"),
                    0x20..=0x7e => out.push(b as char),
                    b'\n' => out.push_str("\\n"),
                    b'\r' => out.push_str("\\r"),
                    b'\t' => out.push_str("\\t"),
                    _ => out.push_str(&format!("\\x{b:02x}")),
                }
            }
            out
        }
    }
}

/// Um comando no formato do protocolo: lista de textos em bytes.
pub fn encode(args: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + args.iter().map(|a| a.len() + 16).sum::<usize>());
    out.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
    for a in args {
        out.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
        out.extend_from_slice(a);
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// Maior resposta em bytes que o app aceita ler de uma vez (512 MB, o
/// limite de um valor no Redis).
const MAX_BULK: i64 = 512 * 1024 * 1024;

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

async fn line<R: AsyncBufRead + Unpin + Send>(reader: &mut R) -> io::Result<String> {
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf).await?;
    if !buf.ends_with(b"\r\n") {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the server closed the connection",
        ));
    }
    buf.truncate(buf.len() - 2);
    String::from_utf8(buf).map_err(|_| bad("invalid reply from the server"))
}

fn number(text: &str) -> io::Result<i64> {
    text.parse()
        .map_err(|_| bad(format!("invalid length in the reply: {text:?}")))
}

async fn bulk<R: AsyncBufRead + Unpin + Send>(reader: &mut R, len: i64) -> io::Result<Vec<u8>> {
    if !(0..=MAX_BULK).contains(&len) {
        return Err(bad(format!("invalid length in the reply: {len}")));
    }
    let mut data = vec![0; len as usize + 2];
    reader.read_exact(&mut data).await?;
    data.truncate(len as usize);
    Ok(data)
}

/// Lê uma resposta inteira.
pub fn read_value<'a, R: AsyncBufRead + Unpin + Send>(
    reader: &'a mut R,
) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + 'a>> {
    Box::pin(async move {
        loop {
            let head = line(reader).await?;
            let Some(kind) = head.chars().next() else {
                return Err(bad("empty reply from the server"));
            };
            let rest = &head[kind.len_utf8()..];
            return Ok(match kind {
                '+' => Value::Simple(rest.to_string()),
                '-' => Value::Error(rest.to_string()),
                ':' => Value::Int(number(rest)?),
                '_' => Value::Null,
                ',' => Value::Double(rest.to_string()),
                '#' => Value::Bool(rest == "t"),
                '(' => Value::BigNumber(rest.to_string()),
                '$' | '=' | '!' => {
                    let len = number(rest)?;
                    if len < 0 {
                        Value::Null
                    } else {
                        let data = bulk(reader, len).await?;
                        match kind {
                            '!' => Value::Error(bytes_to_text(&data)),
                            // `txt:` ou `mkd:` antes do texto.
                            '=' if data.len() >= 4 && data[3] == b':' => {
                                Value::Bulk(data[4..].to_vec())
                            }
                            _ => Value::Bulk(data),
                        }
                    }
                }
                '*' | '~' | '>' => {
                    let len = number(rest)?;
                    if len < 0 {
                        Value::Null
                    } else {
                        let mut items = Vec::with_capacity(len.min(1024) as usize);
                        for _ in 0..len {
                            items.push(read_value(reader).await?);
                        }
                        match kind {
                            '~' => Value::Set(items),
                            '>' => Value::Push(items),
                            _ => Value::Array(items),
                        }
                    }
                }
                '%' | '|' => {
                    let len = number(rest)?;
                    let mut pairs = Vec::with_capacity(len.clamp(0, 1024) as usize);
                    for _ in 0..len.max(0) {
                        let k = read_value(reader).await?;
                        let v = read_value(reader).await?;
                        pairs.push((k, v));
                    }
                    if kind == '|' {
                        // Atributos: vêm antes da resposta de verdade.
                        continue;
                    }
                    Value::Map(pairs)
                }
                other => return Err(bad(format!("unknown reply type {other:?}"))),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Value {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut reader = tokio::io::BufReader::new(bytes);
        rt.block_on(read_value(&mut reader)).unwrap()
    }

    #[test]
    fn reads_resp2() {
        assert_eq!(parse(b"+OK\r\n"), Value::Simple("OK".into()));
        assert_eq!(parse(b"-ERR x\r\n"), Value::Error("ERR x".into()));
        assert_eq!(parse(b":42\r\n"), Value::Int(42));
        assert_eq!(parse(b"$-1\r\n"), Value::Null);
        assert_eq!(
            parse(b"$5\r\na\r\nbc\r\n"),
            Value::Bulk(b"a\r\nbc".to_vec())
        );
        assert_eq!(
            parse(b"*2\r\n$1\r\na\r\n*1\r\n:1\r\n"),
            Value::Array(vec![
                Value::Bulk(b"a".to_vec()),
                Value::Array(vec![Value::Int(1)])
            ])
        );
    }

    #[test]
    fn reads_resp3() {
        assert_eq!(
            parse(b"%1\r\n+a\r\n,1.5\r\n"),
            Value::Map(vec![(
                Value::Simple("a".into()),
                Value::Double("1.5".into())
            )])
        );
        assert_eq!(parse(b"#t\r\n"), Value::Bool(true));
        assert_eq!(parse(b"_\r\n"), Value::Null);
        assert_eq!(parse(b"=7\r\ntxt:abc\r\n"), Value::Bulk(b"abc".to_vec()));
        assert_eq!(parse(b"~1\r\n:1\r\n"), Value::Set(vec![Value::Int(1)]));
        // Atributo antes do valor.
        assert_eq!(parse(b"|1\r\n+ttl\r\n:10\r\n:7\r\n"), Value::Int(7));
    }

    #[test]
    fn cells_and_json() {
        let v = Value::Map(vec![(
            Value::Bulk(b"a\"b".to_vec()),
            Value::Array(vec![Value::Int(1), Value::Null, Value::Bulk(b"x".to_vec())]),
        )]);
        assert_eq!(v.to_cell().unwrap(), r#"{"a\"b":[1,null,"x"]}"#);
        assert_eq!(Value::Null.to_cell(), None);
        assert_eq!(bytes_to_text(&[0xff, b'a', b'\n']), "\\xffa\\n");
        assert_eq!(bytes_to_text("maçã".as_bytes()), "maçã");
    }

    #[test]
    fn encodes_commands() {
        assert_eq!(
            encode(&[b"SET".to_vec(), b"k".to_vec(), b"a b".to_vec()]),
            b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$3\r\na b\r\n"
        );
    }
}
