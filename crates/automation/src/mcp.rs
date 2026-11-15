//! O servidor MCP do PostPilot: `postpilot mcp` (marco 7.2).
//!
//! O Claude Code abre `postpilot mcp` e conversa com ele por stdio, uma
//! mensagem JSON-RPC 2.0 por linha. A lista de ferramentas sai de
//! [`crate::tools`] e funciona mesmo com o app fechado; cada `tools/call`
//! vai para o app aberto pelo [`crate::endpoint`]. As chamadas rodam em
//! paralelo: uma esperando a confirmação do usuário não segura as outras.
//!
//! Só o necessário do protocolo: `initialize`, `ping`, `tools/list`,
//! `tools/call` e as notificações (ignoradas).

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::hub::Reply;

/// Versões do protocolo MCP que este servidor entende, a mais nova
/// primeiro.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Nome do servidor no `initialize` (o Claude Code mostra as ferramentas
/// como `mcp__postpilot__...` quando o servidor é adicionado com esse
/// nome).
pub const SERVER_NAME: &str = "postpilot";

/// Instruções que o Claude recebe ao conectar.
pub const INSTRUCTIONS: &str = "PostPilot is the user's desktop database manager for PostgreSQL \
and SQL Server (like SQL Server Management Studio). These tools act on the PostPilot window that is open on the user's \
machine: its connected servers, its query windows (editor tabs) and their results. Start with \
list_servers, which also says the engine of each server: write PostgreSQL SQL or T-SQL to match. Explore with list_databases, list_objects and describe_table before writing SQL. \
Reading is free: run_query runs read-only SQL at once (inside a READ ONLY transaction on \
PostgreSQL, inside a transaction that is always rolled back on SQL Server). Anything \
that can change data or schema opens a confirmation dialog in PostPilot showing the SQL; the \
user decides. When the user asks you to write or fix a query, prefer edit_query_window (they see \
it in their editor and can undo with Ctrl+Z) and let them run it, unless they ask you to run it. \
Results sent to you are capped; use LIMIT (TOP on SQL Server) and aggregates instead of reading whole tables.";

/// Quem atende `tools/call` (o app pelo canal local, ou um falso nos
/// testes).
pub type Caller = Arc<dyn Fn(&str, Value) -> Reply + Send + Sync>;

/// Lê mensagens de `input` até o fim e escreve as respostas em `output`.
/// Volta quando a entrada fecha e as chamadas em andamento terminam.
pub fn serve<R: BufRead, W: Write + Send + 'static>(input: R, output: W, call: Caller) {
    let output = Arc::new(Mutex::new(output));
    let mut workers = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                send(
                    &output,
                    &error(Value::Null, -32700, &format!("Parse error: {e}")),
                );
                continue;
            }
        };
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        // Notificação (sem id) ou resposta a algo nosso: nada a responder.
        let Some(id) = id else { continue };
        if method.is_empty() {
            continue;
        }
        match method.as_str() {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .copied()
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                send(
                    &output,
                    &result(
                        id,
                        json!({
                            "protocolVersion": version,
                            "capabilities": { "tools": { "listChanged": false } },
                            "serverInfo": {
                                "name": SERVER_NAME,
                                "title": "PostPilot",
                                "version": env!("CARGO_PKG_VERSION"),
                            },
                            "instructions": INSTRUCTIONS,
                        }),
                    ),
                );
            }
            "ping" => send(&output, &result(id, json!({}))),
            "tools/list" => {
                let tools: Vec<Value> = crate::tools::all().iter().map(|t| t.to_mcp()).collect();
                send(&output, &result(id, json!({ "tools": tools })));
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if crate::tools::find(&name).is_none() {
                    send(
                        &output,
                        &error(id, -32602, &format!("Unknown tool: {name}")),
                    );
                    continue;
                }
                let (output, call) = (output.clone(), call.clone());
                workers.push(std::thread::spawn(move || {
                    let reply = call(&name, args);
                    send(
                        &output,
                        &result(
                            id,
                            json!({
                                "content": [{ "type": "text", "text": reply.text }],
                                "isError": reply.is_error,
                            }),
                        ),
                    );
                }));
            }
            other => send(
                &output,
                &error(id, -32601, &format!("Method not found: {other}")),
            ),
        }
        workers.retain(|w| !w.is_finished());
    }
    for w in workers {
        let _ = w.join();
    }
}

/// `postpilot mcp`: stdio do processo, chamadas para o app aberto pelo
/// arquivo do canal.
pub fn serve_stdio() -> std::process::ExitCode {
    let Some(path) = crate::endpoint::default_path() else {
        eprintln!("postpilot mcp: could not find the configuration folder (HOME/APPDATA)");
        return std::process::ExitCode::FAILURE;
    };
    let call: Caller = Arc::new(move |tool, args| crate::endpoint::call(&path, tool, args));
    serve(std::io::stdin().lock(), std::io::stdout(), call);
    std::process::ExitCode::SUCCESS
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn send<W: Write>(output: &Mutex<W>, message: &Value) {
    let mut text = message.to_string();
    text.push('\n');
    if let Ok(mut out) = output.lock() {
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saída em memória que dá para ler depois.
    #[derive(Clone, Default)]
    struct Out(Arc<Mutex<Vec<u8>>>);

    impl Write for Out {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn run(lines: &[Value], call: Caller) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let out = Out::default();
        serve(std::io::Cursor::new(input), out.clone(), call);
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn by_id(replies: &[Value], id: i64) -> &Value {
        replies.iter().find(|r| r["id"] == id).expect("reply")
    }

    #[test]
    fn handshake_list_and_call() {
        let call: Caller =
            Arc::new(|tool, args| Reply::ok(&json!({ "tool": tool, "sql": args["sql"] })));
        let replies = run(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"run_query","arguments":{"sql":"select 1"}}}),
                json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":5,"method":"resources/list"}),
                json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope"}}),
            ],
            call,
        );
        assert_eq!(replies.len(), 6, "{replies:?}");
        let init = by_id(&replies, 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "postpilot");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let tools = by_id(&replies, 2)["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), crate::tools::all().len());
        assert!(tools.iter().any(|t| t["name"] == "describe_table"));
        let called = by_id(&replies, 3);
        assert_eq!(called["result"]["isError"], false);
        let text = called["result"]["content"][0]["text"].as_str().unwrap();
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v, json!({ "tool": "run_query", "sql": "select 1" }));
        assert_eq!(by_id(&replies, 4)["result"], json!({}));
        assert_eq!(by_id(&replies, 5)["error"]["code"], -32601);
        assert_eq!(by_id(&replies, 6)["error"]["code"], -32602);
    }

    #[test]
    fn unknown_version_gets_the_newest() {
        let replies = run(
            &[
                json!({"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
            ],
            Arc::new(|_, _| Reply::error("x")),
        );
        assert_eq!(replies[0]["id"], "a");
        assert_eq!(
            replies[0]["result"]["protocolVersion"],
            PROTOCOL_VERSIONS[0]
        );
    }

    #[test]
    fn slow_calls_do_not_block_fast_ones() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let rx = Arc::new(Mutex::new(rx));
        let call: Caller = Arc::new(move |tool, _| {
            if tool == "run_query" {
                // Espera a chamada rápida terminar, como uma confirmação.
                let _ = rx.lock().unwrap().recv();
                // A rápida já devolveu; dá tempo de ela escrever a resposta.
                std::thread::sleep(std::time::Duration::from_millis(300));
            } else {
                let _ = tx.send(());
            }
            Reply::ok(&json!(tool))
        });
        let replies = run(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run_query","arguments":{"sql":"delete from t"}}}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_servers"}}),
            ],
            call,
        );
        assert_eq!(replies[0]["id"], 2);
        assert_eq!(replies[1]["id"], 1);
    }

    #[test]
    fn errors_and_garbage() {
        let replies = run(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_servers"}}),
            ],
            Arc::new(|_, _| Reply::error(crate::endpoint::NOT_RUNNING)),
        );
        assert_eq!(replies[0]["result"]["isError"], true);
        let out = Out::default();
        serve(
            std::io::Cursor::new("not json\n"),
            out.clone(),
            Arc::new(|_, _| Reply::error("x")),
        );
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("-32700"));
    }
}
