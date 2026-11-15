//! O Claude Code instalado rodando por baixo do painel do Claude (marco
//! 7.4).
//!
//! O painel não fala com a API da Anthropic: ele abre o `claude` da
//! máquina em modo não interativo (`claude -p`), com entrada e saída em
//! `stream-json`, e conversa com ele por stdin e stdout. Assim o login, a
//! assinatura e o modelo são os do Claude Code do usuário, sem chave de
//! API. As ferramentas chegam pelo MCP do PostPilot (`postpilot mcp`), num
//! canal local só do painel; as ferramentas embutidas do Claude Code
//! (Bash, Edit...) ficam desligadas.
//!
//! Um processo vive enquanto a conversa dura: cada mensagem do usuário é
//! uma linha em stdin e a resposta chega aos poucos. "Parar" mata o
//! processo; a próxima mensagem abre outro com `--resume`, e a conversa
//! continua.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;

use serde_json::{Value, json};

use crate::hub::Waker;

/// Variável que aponta outro programa no lugar do `claude` (testes, ou uma
/// instalação fora do PATH).
pub const PROGRAM_ENV: &str = "PGSTUDIO_CLAUDE";

/// O que acontece na conversa, já lido do `stream-json`.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// O Claude Code começou (ou retomou) a sessão.
    Started { session_id: String, model: String },
    /// Pedaço de texto da resposta, chegando.
    TextDelta(String),
    /// Bloco de texto completo da resposta (fecha os pedaços).
    Text(String),
    /// O Claude chamou uma ferramenta.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Resposta da ferramenta.
    ToolResult {
        id: String,
        is_error: bool,
        text: String,
    },
    /// A vez do Claude acabou.
    TurnDone {
        is_error: bool,
        text: String,
        cost_usd: Option<f64>,
        session_id: String,
    },
    /// Linha do stderr (erros de login, de instalação...).
    Stderr(String),
    /// O processo terminou.
    Exited(Option<i32>),
}

/// Lê uma linha do `stream-json`. Linhas que o painel não usa voltam
/// vazias.
pub fn parse_line(line: &str) -> Vec<AgentEvent> {
    let Ok(m) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    match m["type"].as_str().unwrap_or_default() {
        "system" if m["subtype"] == "init" => vec![AgentEvent::Started {
            session_id: s(&m["session_id"]),
            model: s(&m["model"]),
        }],
        "stream_event" => {
            let e = &m["event"];
            if e["type"] == "content_block_delta" && e["delta"]["type"] == "text_delta" {
                vec![AgentEvent::TextDelta(s(&e["delta"]["text"]))]
            } else {
                Vec::new()
            }
        }
        "assistant" => blocks(&m["message"]["content"])
            .filter_map(|b| match b["type"].as_str()? {
                "text" => Some(AgentEvent::Text(s(&b["text"]))),
                "tool_use" => Some(AgentEvent::ToolUse {
                    id: s(&b["id"]),
                    name: short_tool_name(b["name"].as_str().unwrap_or_default()).to_string(),
                    input: b["input"].clone(),
                }),
                _ => None,
            })
            .collect(),
        "user" => blocks(&m["message"]["content"])
            .filter(|b| b["type"] == "tool_result")
            .map(|b| AgentEvent::ToolResult {
                id: s(&b["tool_use_id"]),
                is_error: b["is_error"].as_bool().unwrap_or(false),
                text: match &b["content"] {
                    Value::String(t) => t.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                },
            })
            .collect(),
        "result" => vec![AgentEvent::TurnDone {
            is_error: m["is_error"].as_bool().unwrap_or(false),
            text: match &m["result"] {
                Value::String(t) => t.clone(),
                _ => s(&m["subtype"]),
            },
            cost_usd: m["total_cost_usd"].as_f64(),
            session_id: s(&m["session_id"]),
        }],
        _ => Vec::new(),
    }
}

fn blocks(content: &Value) -> impl Iterator<Item = &Value> {
    content.as_array().into_iter().flatten()
}

/// `mcp__postpilot__run_query` vira `run_query`.
pub fn short_tool_name(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

/// A mensagem do usuário como linha de entrada do `stream-json`.
pub fn user_message(text: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    })
    .to_string()
}

/// Instruções do painel, somadas às do Claude Code.
pub const PANEL_PROMPT: &str = "You are the Claude panel inside PostPilot, the user's desktop \
database manager for PostgreSQL and SQL Server. list_servers says the engine of each server: \
write PostgreSQL SQL or T-SQL to match. The user is looking at PostPilot right now and sees every tool you call as \
a card in the panel. Use the postpilot tools to look at their servers, schema, query windows \
and results; you have no shell or file tools here. Reading is free; anything that changes data \
or schema shows them a confirmation dialog. When they ask for a query, write it into their \
editor with edit_query_window unless they ask you to run it. Each message starts with a short \
[PostPilot context] line saying which query window is active; read the window with \
get_query_window when you need its text. Answer in the user's language, briefly, with SQL in \
```sql blocks.";

/// Argumentos do `claude` para o painel. `mcp_config` é o JSON do servidor
/// MCP do PostPilot; `resume`, a sessão a continuar.
pub fn panel_args(mcp_config: &str, resume: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        mcp_config,
        "--allowedTools",
        "mcp__postpilot__*",
        "--append-system-prompt",
        PANEL_PROMPT,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(id) = resume {
        args.push("--resume".into());
        args.push(id.into());
    }
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args
}

/// JSON do `--mcp-config`: o próprio PostPilot (`exe mcp`) falando com o
/// canal local do painel em `endpoint`.
pub fn mcp_config(exe: &Path, endpoint: &Path) -> String {
    json!({
        "mcpServers": {
            "postpilot": {
                "command": exe,
                "args": ["mcp"],
                "env": { (crate::endpoint::ENV): endpoint },
            }
        }
    })
    .to_string()
}

/// Uma pergunta só ao Claude Code, sem ferramentas e sem os servidores MCP
/// do usuário (para a mensagem de commit): `claude -p`, com a pergunta em
/// stdin e a resposta em texto.
pub fn ask(program: &Path, system: &str, user: &str, cwd: &Path) -> Result<String, String> {
    std::fs::create_dir_all(cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let mut command = Command::new(program);
    command
        .args([
            "-p",
            "--output-format",
            "text",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--append-system-prompt",
            system,
        ])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start Claude Code ({}): {e}", program.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        // Um `claude` que não lê a entrada fecha o cano; a resposta (ou o
        // erro) dele é o que importa.
        let _ = stdin.write_all(user.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Claude Code did not finish: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !text.is_empty() {
        return Ok(text);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = [stderr.trim(), text.as_str()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("no answer")
        .chars()
        .take(400)
        .collect::<String>();
    Err(format!("Claude Code: {detail}"))
}

/// Onde está o `claude`: `PGSTUDIO_CLAUDE`, o PATH ou os lugares em que o
/// instalador costuma pôr.
pub fn find_program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(PROGRAM_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    let home = PathBuf::from(home);
    [
        home.join(".claude").join("local").join(names[0]),
        home.join(".local").join("bin").join(names[0]),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Um `claude` rodando.
pub struct Agent {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<AgentEvent>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("pid", &self.child.id())
            .finish()
    }
}

impl Agent {
    /// Abre `program args` em `cwd`. Os eventos chegam por
    /// [`Agent::try_recv`]; `waker` é chamado a cada um.
    pub fn start(
        program: &Path,
        args: &[String],
        cwd: &Path,
        waker: Waker,
    ) -> std::io::Result<Agent> {
        std::fs::create_dir_all(cwd)?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Sem janela de console piscando.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel();
        if let Some(out) = stdout {
            let (tx, waker) = (tx.clone(), waker.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    for e in parse_line(&line) {
                        let _ = tx.send(e);
                    }
                    waker();
                }
                let _ = tx.send(AgentEvent::Exited(None));
                waker();
            });
        }
        if let Some(err) = stderr {
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines() {
                    let Ok(line) = line else { break };
                    if !line.trim().is_empty() {
                        let _ = tx.send(AgentEvent::Stderr(line));
                        waker();
                    }
                }
            });
        }
        Ok(Agent { child, stdin, rx })
    }

    /// Manda uma mensagem do usuário.
    pub fn send(&mut self, text: &str) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("Claude Code is not running"))?;
        let mut line = user_message(text);
        line.push('\n');
        stdin.write_all(line.as_bytes())?;
        stdin.flush()
    }

    /// O próximo evento, sem esperar.
    pub fn try_recv(&self) -> Option<AgentEvent> {
        self.rx.try_recv().ok()
    }

    /// Se o processo ainda roda.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Termina o processo (o botão Parar e fechar o painel).
    pub fn stop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_json_lines() {
        assert_eq!(
            parse_line(
                r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-x","tools":[]}"#
            ),
            vec![AgentEvent::Started {
                session_id: "s1".into(),
                model: "claude-x".into()
            }]
        );
        assert_eq!(
            parse_line(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Olá"}}}"#
            ),
            vec![AgentEvent::TextDelta("Olá".into())]
        );
        assert!(
            parse_line(r#"{"type":"stream_event","event":{"type":"message_start"}}"#).is_empty()
        );
        let e = parse_line(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Vou ver."},{"type":"tool_use","id":"t1","name":"mcp__postpilot__describe_table","input":{"table":"pedido"}}]}}"#,
        );
        assert_eq!(e[0], AgentEvent::Text("Vou ver.".into()));
        assert_eq!(
            e[1],
            AgentEvent::ToolUse {
                id: "t1".into(),
                name: "describe_table".into(),
                input: json!({"table": "pedido"})
            }
        );
        assert_eq!(
            parse_line(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"declined"}]}]}}"#
            ),
            vec![AgentEvent::ToolResult {
                id: "t1".into(),
                is_error: true,
                text: "declined".into()
            }]
        );
        assert_eq!(
            parse_line(
                r#"{"type":"result","subtype":"success","is_error":false,"result":"pronto","total_cost_usd":0.01,"session_id":"s1"}"#
            ),
            vec![AgentEvent::TurnDone {
                is_error: false,
                text: "pronto".into(),
                cost_usd: Some(0.01),
                session_id: "s1".into()
            }]
        );
        assert!(parse_line("not json").is_empty());
        assert!(parse_line(r#"{"type":"rate_limit_event"}"#).is_empty());
    }

    #[test]
    fn args_and_config() {
        let a = panel_args("{}", Some("abc"), None);
        assert_eq!(a[0], "-p");
        let i = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[i + 1], "");
        assert!(a.ends_with(&["--resume".into(), "abc".into()]));
        let c: Value = serde_json::from_str(&mcp_config(
            Path::new("/x/postpilot"),
            Path::new("/t/e.json"),
        ))
        .unwrap();
        assert_eq!(c["mcpServers"]["postpilot"]["args"], json!(["mcp"]));
        assert_eq!(
            c["mcpServers"]["postpilot"]["env"][crate::endpoint::ENV],
            "/t/e.json"
        );
        let m: Value = serde_json::from_str(&user_message("oi \"x\"")).unwrap();
        assert_eq!(m["message"]["content"], "oi \"x\"");
    }

    #[cfg(unix)]
    #[test]
    fn talks_to_a_process() {
        let dir = std::env::temp_dir().join(format!("pgstudio-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-claude");
        std::fs::write(
            &script,
            "#!/bin/sh\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\",\"model\":\"m\"}'\n\
             while read line; do echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s\"}'; done\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut agent = Agent::start(&script, &[], &dir, std::sync::Arc::new(|| {})).unwrap();
        agent.send("oi").unwrap();
        let mut seen = Vec::new();
        let start = std::time::Instant::now();
        while seen.len() < 2 && start.elapsed() < std::time::Duration::from_secs(5) {
            if let Some(e) = agent.try_recv() {
                seen.push(e);
            }
        }
        assert!(matches!(seen[0], AgentEvent::Started { .. }));
        assert!(matches!(seen[1], AgentEvent::TurnDone { ref text, .. } if text == "ok"));
        agent.stop();
        assert!(!agent.is_running());
    }
}
