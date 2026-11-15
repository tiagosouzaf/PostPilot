//! A conversa do painel com um provedor por chave de API ([`crate::llm`]).
//!
//! Faz o papel que o `claude` faz no [`crate::agent`]: recebe as mensagens
//! do usuário, chama o modelo, atende as ferramentas que ele pedir (as
//! mesmas do MCP, direto pelo canal da interface) e devolve tudo como
//! [`AgentEvent`], para o painel desenhar igual.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use serde_json::{Value, json};

use crate::agent::AgentEvent;
use crate::hub::{Reply, Waker};
use crate::llm::{self, Connection};
use crate::tools::ToolSpec;

/// Quantas vezes o modelo pode pedir ferramentas numa mesma pergunta.
const MAX_ROUNDS: usize = 40;

/// Instruções do painel para um modelo que não é o Claude Code.
pub const PANEL_PROMPT: &str = "You are the AI assistant panel inside PostPilot, the user's \
desktop database manager (PostgreSQL and SQL Server). The user is looking at PostPilot right now \
and sees every tool you call as a card in the panel. Use the tools to look at their servers, \
schema, query windows and results; you have no shell or file tools here. Reading is free; \
anything that changes data or schema shows them a confirmation dialog. When they ask for a \
query, write it into their editor with edit_query_window unless they ask you to run it. Each \
message starts with a short [PostPilot context] line saying which query window is active; read \
the window with get_query_window when you need its text. Answer in the user's language, \
briefly, with SQL in ```sql blocks.";

/// Quem atende uma ferramenta (no app, o canal até a interface).
pub type ToolCaller = Arc<dyn Fn(&str, Value) -> Reply + Send + Sync>;

/// A ferramenta no formato `tools` da OpenAI.
pub fn openai_tool(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.input_schema,
        },
    })
}

/// Uma conversa em andamento.
pub struct ChatAgent {
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<AgentEvent>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for ChatAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChatAgent")
    }
}

impl ChatAgent {
    /// Começa uma conversa com `conn`. Os eventos chegam por
    /// [`ChatAgent::try_recv`]; `waker` é chamado a cada um.
    pub fn start(
        conn: Connection,
        system: &str,
        tools: &[ToolSpec],
        call: ToolCaller,
        waker: Waker,
    ) -> ChatAgent {
        let (tx, user_rx) = mpsc::channel::<String>();
        let (event_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let tools: Vec<Value> = tools.iter().map(openai_tool).collect();
        let mut history = vec![json!({ "role": "system", "content": system })];
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let emit = |e: AgentEvent| {
                let sent = event_tx.send(e).is_ok();
                waker();
                sent
            };
            emit(AgentEvent::Started {
                session_id: String::new(),
                model: conn.model.clone(),
            });
            while let Ok(text) = user_rx.recv() {
                stop.store(false, Ordering::SeqCst);
                let before = history.len();
                history.push(json!({ "role": "user", "content": text }));
                match turn(&conn, &mut history, &tools, &call, &stop, &emit) {
                    Ok(()) => {
                        emit(AgentEvent::TurnDone {
                            is_error: false,
                            text: String::new(),
                            cost_usd: None,
                            session_id: String::new(),
                        });
                    }
                    // Parar: a pergunta interrompida sai da conversa, e o
                    // painel já avisou o usuário.
                    Err(e) if e == llm::CANCELLED => history.truncate(before),
                    Err(e) => {
                        history.truncate(before);
                        emit(AgentEvent::TurnDone {
                            is_error: true,
                            text: e,
                            cost_usd: None,
                            session_id: String::new(),
                        });
                    }
                }
            }
        });
        ChatAgent { tx, rx, cancel }
    }

    /// Manda uma mensagem do usuário.
    pub fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.tx
            .send(text.to_string())
            .map_err(|_| std::io::Error::other("the conversation has ended"))
    }

    /// O próximo evento, sem esperar.
    pub fn try_recv(&self) -> Option<AgentEvent> {
        self.rx.try_recv().ok()
    }

    /// Interrompe a resposta em andamento (o botão Parar). A conversa
    /// continua valendo para a próxima mensagem.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

impl Drop for ChatAgent {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Uma pergunta do usuário até a resposta final, atendendo as ferramentas
/// no caminho.
fn turn(
    conn: &Connection,
    history: &mut Vec<Value>,
    tools: &[Value],
    call: &ToolCaller,
    cancel: &AtomicBool,
    emit: &dyn Fn(AgentEvent) -> bool,
) -> Result<(), String> {
    for _ in 0..MAX_ROUNDS {
        let answer = llm::chat(conn, history, tools, cancel, &mut |piece| {
            emit(AgentEvent::TextDelta(piece.to_string()));
        })?;
        if !answer.content.is_empty() {
            emit(AgentEvent::Text(answer.content.clone()));
        }
        if answer.tool_calls.is_empty() {
            history.push(json!({ "role": "assistant", "content": answer.content }));
            return Ok(());
        }
        let calls: Vec<Value> = answer
            .tool_calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments },
                })
            })
            .collect();
        history.push(json!({
            "role": "assistant",
            "content": answer.content,
            "tool_calls": calls,
        }));
        for request in answer.tool_calls {
            let parsed = serde_json::from_str::<Value>(&request.arguments);
            emit(AgentEvent::ToolUse {
                id: request.id.clone(),
                name: request.name.clone(),
                input: parsed.as_ref().ok().cloned().unwrap_or(Value::Null),
            });
            let reply = match parsed {
                // Depois de Parar, nada novo roda.
                _ if cancel.load(Ordering::SeqCst) => return Err(llm::CANCELLED.into()),
                Ok(args) => call(&request.name, args),
                Err(e) => Reply::error(format!("The arguments are not valid JSON: {e}")),
            };
            emit(AgentEvent::ToolResult {
                id: request.id.clone(),
                is_error: reply.is_error,
                text: reply.text.clone(),
            });
            history.push(json!({
                "role": "tool",
                "tool_call_id": request.id,
                "content": reply.text,
            }));
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(llm::CANCELLED.into());
        }
    }
    Err("the model kept calling tools without answering".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::tests::{fake_server, sse};
    use std::time::{Duration, Instant};

    #[test]
    fn calls_the_tool_and_answers() {
        let first = sse(&[json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1",
            "function":{"name":"list_servers","arguments":"{}"}}]}}]})]);
        let second = sse(&[
            json!({"choices":[{"delta":{"content":"Um "}}]}),
            json!({"choices":[{"delta":{"content":"servidor."}}]}),
        ]);
        let (url, server) = fake_server(vec![
            (200, "text/event-stream", first),
            (200, "text/event-stream", second),
        ]);
        let conn = Connection {
            base_url: url,
            api_key: "k".into(),
            model: "m".into(),
        };
        let call: ToolCaller = Arc::new(|tool, _| Reply::ok(&json!({ "tool": tool })));
        let mut agent = ChatAgent::start(
            conn,
            "sys",
            &crate::tools::all(),
            call,
            Arc::new(|| {}),
        );
        agent.send("quais servidores?").unwrap();
        let mut events = Vec::new();
        let start = Instant::now();
        while !matches!(events.last(), Some(AgentEvent::TurnDone { .. })) {
            assert!(start.elapsed() < Duration::from_secs(10), "{events:?}");
            match agent.try_recv() {
                Some(e) => events.push(e),
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        assert!(matches!(&events[0], AgentEvent::Started { model, .. } if model == "m"));
        assert!(matches!(&events[1], AgentEvent::ToolUse { name, .. } if name == "list_servers"));
        assert!(
            matches!(&events[2], AgentEvent::ToolResult { is_error: false, text, .. } if text.contains("list_servers"))
        );
        assert_eq!(events[3], AgentEvent::TextDelta("Um ".into()));
        assert_eq!(events[5], AgentEvent::Text("Um servidor.".into()));
        assert!(matches!(&events[6], AgentEvent::TurnDone { is_error: false, .. }));

        // A segunda chamada leva a conversa inteira, com o resultado.
        let seen = server.join().unwrap();
        let sent: Value = serde_json::from_str(seen[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let roles: Vec<&str> = sent["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "user", "assistant", "tool"]);
        assert_eq!(sent["messages"][3]["tool_call_id"], "c1");
        assert_eq!(sent["tools"].as_array().unwrap().len(), crate::tools::all().len());
    }

    #[test]
    fn an_error_ends_the_turn() {
        let (url, _server) = fake_server(vec![(
            429,
            "application/json",
            json!({"error":{"message":"sem crédito"}}).to_string(),
        )]);
        let conn = Connection {
            base_url: url,
            api_key: String::new(),
            model: "m".into(),
        };
        let call: ToolCaller = Arc::new(|_, _| Reply::error("x"));
        let mut agent = ChatAgent::start(conn, "sys", &[], call, Arc::new(|| {}));
        agent.send("oi").unwrap();
        let start = Instant::now();
        loop {
            assert!(start.elapsed() < Duration::from_secs(10));
            if let Some(AgentEvent::TurnDone { is_error, text, .. }) = agent.try_recv() {
                assert!(is_error);
                assert!(text.contains("sem crédito"), "{text}");
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
