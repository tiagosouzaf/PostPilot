//! O canal entre quem chama as ferramentas (o servidor MCP, o painel do
//! Claude) e a interface, que atende os pedidos na thread dela, a cada
//! frame.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

/// De onde veio a chamada, para a janela Output e o diálogo de
/// confirmação.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Claude Code pelo MCP (`pgstudio mcp`).
    Mcp,
    /// O painel do Claude dentro do app.
    Panel,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Mcp => "Claude Code",
            Origin::Panel => "Claude",
        }
    }
}

/// Resposta de uma ferramenta: texto (quase sempre JSON) e se é erro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    pub is_error: bool,
}

impl Reply {
    pub fn ok(value: &Value) -> Self {
        Reply {
            text: serde_json::to_string_pretty(value).unwrap_or_default(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Reply {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Quem responde uma chamada. Pode ir para outra thread (o fim de uma
/// consulta chega na thread do banco).
#[derive(Clone, Debug)]
pub struct Responder(mpsc::Sender<Reply>);

impl Responder {
    pub fn send(&self, reply: Reply) {
        let _ = self.0.send(reply);
    }
}

/// Uma chamada esperando a interface.
#[derive(Debug)]
pub struct Pending {
    pub tool: String,
    pub args: Value,
    pub origin: Origin,
    pub responder: Responder,
}

/// Acorda a interface quando chega um pedido (no app, um
/// `request_repaint`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Lado de quem chama. Pode ser clonado e mandado para outras threads.
#[derive(Clone)]
pub struct Client {
    tx: mpsc::Sender<Pending>,
    waker: Waker,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client")
    }
}

/// Lado da interface.
#[derive(Debug)]
pub struct Inbox {
    rx: mpsc::Receiver<Pending>,
}

/// Cria o canal. `waker` é chamado a cada pedido.
pub fn channel(waker: Waker) -> (Client, Inbox) {
    let (tx, rx) = mpsc::channel();
    (Client { tx, waker }, Inbox { rx })
}

/// Quanto uma chamada pode esperar: a confirmação depende do usuário.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

impl Client {
    /// Manda a chamada e devolve onde a resposta vai chegar.
    pub fn send(&self, tool: &str, args: Value, origin: Origin) -> mpsc::Receiver<Reply> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let pending = Pending {
            tool: tool.to_string(),
            args,
            origin,
            responder: Responder(reply_tx),
        };
        if let Err(mpsc::SendError(p)) = self.tx.send(pending) {
            p.responder
                .send(Reply::error("PostPilot is closing; the call was not made."));
        }
        (self.waker)();
        reply_rx
    }

    /// Manda a chamada e espera a resposta (fora da thread da interface).
    pub fn call(&self, tool: &str, args: Value, origin: Origin) -> Reply {
        match self.send(tool, args, origin).recv_timeout(CALL_TIMEOUT) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => Reply::error(
                "PostPilot did not answer in time (is a dialog waiting for the user?).",
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Reply::error("PostPilot dropped the call without answering.")
            }
        }
    }
}

impl Inbox {
    /// O próximo pedido, sem esperar.
    pub fn try_next(&self) -> Option<Pending> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn round_trip() {
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let (client, inbox) = channel(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let t = std::thread::spawn(move || client.call("list_servers", Value::Null, Origin::Mcp));
        let pending = loop {
            if let Some(p) = inbox.try_next() {
                break p;
            }
            std::thread::yield_now();
        };
        assert_eq!(pending.tool, "list_servers");
        pending.responder.send(Reply::error("x"));
        assert_eq!(t.join().unwrap(), Reply::error("x"));
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn closed_inbox_answers_with_error() {
        let (client, inbox) = channel(Arc::new(|| {}));
        drop(inbox);
        assert!(
            client
                .call("list_servers", Value::Null, Origin::Mcp)
                .is_error
        );
    }
}
