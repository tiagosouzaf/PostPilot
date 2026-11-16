//! O ChatGPT pela conta do usuário, sem chave de API.
//!
//! É o mesmo caminho do Claude Code ([`crate::agent`]): o PostPilot não
//! fala com a API da OpenAI nem guarda credencial nenhuma. Ele abre o
//! Codex CLI da máquina (`codex`, o programa oficial da OpenAI), em que o
//! usuário entrou com "Sign in with ChatGPT", e conversa com ele em modo
//! não interativo (`codex exec --json`). O login, a assinatura e o modelo
//! são os do Codex; quem guarda o login é o Codex, não o PostPilot.
//!
//! As ferramentas chegam pelo MCP do PostPilot (`postpilot mcp`), no canal
//! local do painel. O Codex roda com o sandbox só de leitura e sem a
//! ferramenta de shell: o que ele faz no banco passa pelas ferramentas do
//! PostPilot, com a confirmação de sempre.
//!
//! Cada mensagem do usuário é um processo `codex exec`; as seguintes
//! continuam a mesma conversa com `codex exec resume <id>`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use serde_json::Value;

use crate::agent::AgentEvent;
use crate::hub::Waker;

/// Variável que aponta outro programa no lugar do `codex` (testes, ou uma
/// instalação fora do PATH).
pub const PROGRAM_ENV: &str = "PGSTUDIO_CODEX";

/// Nome do servidor MCP do PostPilot dentro do Codex.
const MCP_SERVER: &str = "postpilot";

/// Quantas linhas do stderr guardar para explicar um erro.
const STDERR_LINES: usize = 12;

/// Onde está o `codex`: `PGSTUDIO_CODEX`, o PATH ou os lugares em que os
/// instaladores costumam pôr.
pub fn find_program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(PROGRAM_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    // No Windows o `.exe` vem antes: um `.cmd` do npm passa mal os
    // argumentos.
    let names: &[&str] = if cfg!(windows) {
        &["codex.exe", "codex.cmd"]
    } else {
        &["codex"]
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    for name in names {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    let home = PathBuf::from(home);
    [
        home.join(".local").join("bin").join(names[0]),
        home.join(".npm-global").join("bin").join(names[0]),
        home.join(".bun").join("bin").join(names[0]),
        PathBuf::from("/opt/homebrew/bin").join(names[0]),
        PathBuf::from("/usr/local/bin").join(names[0]),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn command(program: &Path) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Sem janela de console piscando.
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Como o Codex está autenticado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    /// Com a conta do ChatGPT (a assinatura).
    ChatGpt,
    /// Com uma chave de API guardada no próprio Codex.
    ApiKey,
    /// Ninguém entrou ainda.
    SignedOut,
    /// O `codex login status` não rodou.
    Unknown(String),
}

impl Login {
    /// Se dá para usar.
    pub fn is_signed_in(&self) -> bool {
        matches!(self, Login::ChatGpt | Login::ApiKey)
    }
}

/// Lê o que o `codex login status` respondeu.
pub fn parse_login_status(success: bool, output: &str) -> Login {
    let text = output.to_lowercase();
    if text.contains("not logged in") || text.contains("not signed in") {
        Login::SignedOut
    } else if !success {
        match output.trim() {
            "" => Login::SignedOut,
            other => Login::Unknown(other.chars().take(300).collect()),
        }
    } else if text.contains("api key") {
        Login::ApiKey
    } else {
        Login::ChatGpt
    }
}

/// Pergunta ao Codex se o usuário já entrou (`codex login status`).
pub fn login_status(program: &Path) -> Login {
    let output = command(program)
        .args(["login", "status"])
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            parse_login_status(o.status.success(), &text)
        }
        Err(e) => Login::Unknown(e.to_string()),
    }
}

/// Um valor de texto para o `-c chave=valor` do Codex (TOML). Prefere as
/// aspas simples, que passam inteiras por qualquer shell ou `.cmd`.
fn toml_string(value: &str) -> String {
    if value.contains('\'') || value.contains('\n') {
        serde_json::to_string(value).unwrap_or_default()
    } else {
        format!("'{value}'")
    }
}

/// Os `-c` que ligam o MCP do PostPilot (`exe mcp`, falando com o canal
/// local em `endpoint`).
pub fn mcp_overrides(exe: &Path, endpoint: &Path) -> Vec<String> {
    let key = format!("mcp_servers.{MCP_SERVER}");
    [
        format!("{key}.command={}", toml_string(&exe.to_string_lossy())),
        format!("{key}.args=['mcp']"),
        format!(
            "{key}.env={{{}={}}}",
            crate::endpoint::ENV,
            toml_string(&endpoint.to_string_lossy())
        ),
        // Quem pede confirmação é o PostPilot, com o SQL na tela; sem
        // isto o `codex exec`, que não tem a quem perguntar, cancela as
        // ferramentas.
        format!("{key}.default_tools_approval_mode='approve'"),
        format!("{key}.tool_timeout_sec=600"),
    ]
    .into_iter()
    .flat_map(|v| ["-c".to_string(), v])
    .collect()
}

/// Argumentos do `codex exec`. A pergunta vai por stdin. `resume` é a
/// conversa a continuar; `extra` são os `-c` do chamador.
pub fn exec_args(json: bool, resume: Option<&str>, extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--color",
        "never",
        "-c",
        "features.shell_tool=false",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if json {
        args.push("--json".into());
    }
    args.extend(extra.iter().cloned());
    if let Some(id) = resume {
        args.push("resume".into());
        args.push(id.into());
    }
    args
}

/// Junta as instruções do PostPilot à primeira mensagem da conversa.
pub fn first_prompt(system: &str, user: &str) -> String {
    format!("<postpilot_instructions>\n{system}\n</postpilot_instructions>\n\n{user}")
}

/// O que dizer quando o Codex falha por falta de login.
fn explain(detail: &str) -> String {
    let lower = detail.to_lowercase();
    let detail: String = detail.chars().take(600).collect();
    if lower.contains("not logged in")
        || lower.contains("login")
        || lower.contains("401")
        || lower.contains("unauthorized")
    {
        format!("ChatGPT (Codex) is not signed in: {detail}")
    } else {
        format!("ChatGPT (Codex): {detail}")
    }
}

/// Uma pergunta só, sem ferramentas (mensagem de commit, nome de arquivo):
/// a pergunta em stdin e a resposta em texto.
pub fn ask(program: &Path, system: &str, user: &str, cwd: &Path) -> Result<String, String> {
    std::fs::create_dir_all(cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let extra = [
        "--ephemeral".to_string(),
        "-c".to_string(),
        "model_reasoning_effort='low'".to_string(),
    ];
    let mut child = command(program)
        .args(exec_args(false, None, &extra))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start Codex ({}): {e}", program.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(first_prompt(system, user).as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Codex did not finish: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !text.is_empty() {
        return Ok(text);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // O fim do stderr é o erro; o começo é o cabeçalho da sessão.
    let tail: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(4)
        .collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join(" ");
    let detail = [tail.as_str(), text.as_str()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("no answer")
        .to_string();
    Err(explain(&detail))
}

/// Leitor do JSONL do `codex exec --json`, com o que precisa lembrar entre
/// as linhas.
#[derive(Debug, Default)]
pub struct Parser {
    /// A conversa (`thread_id`), para o `resume`.
    pub thread_id: Option<String>,
    /// Ferramentas já anunciadas ao painel.
    announced: Vec<String>,
    /// A vez acabou (bem ou mal) e o painel já sabe.
    pub done: bool,
    /// O último erro que o Codex relatou.
    pub last_error: Option<String>,
}

impl Parser {
    /// Lê uma linha. Linhas que o painel não usa voltam vazias.
    pub fn line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(m) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match m["type"].as_str().unwrap_or_default() {
            "thread.started" => {
                self.thread_id = m["thread_id"].as_str().map(str::to_string);
                Vec::new()
            }
            "item.started" | "item.updated" | "item.completed" => {
                let completed = m["type"] == "item.completed";
                let item = &m["item"];
                let id = s(&item["id"]);
                match item["type"].as_str().unwrap_or_default() {
                    "agent_message" if completed => vec![AgentEvent::Text(s(&item["text"]))],
                    "mcp_tool_call" => {
                        let mut events = Vec::new();
                        if !self.announced.contains(&id) {
                            self.announced.push(id.clone());
                            events.push(AgentEvent::ToolUse {
                                id: id.clone(),
                                name: s(&item["tool"]),
                                input: tool_arguments(&item["arguments"]),
                            });
                        }
                        if completed {
                            let error = item["error"]["message"].as_str();
                            events.push(AgentEvent::ToolResult {
                                id,
                                is_error: error.is_some() || item["status"] == "failed",
                                text: match error {
                                    Some(e) => e.to_string(),
                                    None => tool_result_text(&item["result"]),
                                },
                            });
                        }
                        events
                    }
                    "error" if completed => {
                        self.last_error = Some(s(&item["message"]));
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "turn.completed" => {
                self.done = true;
                vec![AgentEvent::TurnDone {
                    is_error: false,
                    text: String::new(),
                    cost_usd: None,
                    session_id: String::new(),
                }]
            }
            "turn.failed" => {
                self.done = true;
                let text = m["error"]["message"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| self.last_error.clone())
                    .unwrap_or_else(|| "the turn failed".into());
                vec![AgentEvent::TurnDone {
                    is_error: true,
                    text: explain(&text),
                    cost_usd: None,
                    session_id: String::new(),
                }]
            }
            // Também vem a cada nova tentativa de conexão: só vira erro
            // se a vez acabar sem resposta.
            "error" => {
                self.last_error = Some(s(&m["message"]));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// Os argumentos da ferramenta: objeto, ou JSON em texto.
fn tool_arguments(arguments: &Value) -> Value {
    match arguments {
        Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

/// O texto da resposta de uma ferramenta MCP.
fn tool_result_text(result: &Value) -> String {
    match &result["content"] {
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::String(text) => text.clone(),
        _ => match result {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        },
    }
}

/// Uma conversa do painel com o Codex.
pub struct CodexAgent {
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<AgentEvent>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl std::fmt::Debug for CodexAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CodexAgent")
    }
}

impl CodexAgent {
    /// Começa uma conversa. `overrides` são os `-c` do MCP do PostPilot
    /// ([`mcp_overrides`]); `system`, as instruções do painel. Os eventos
    /// chegam por [`CodexAgent::try_recv`]; `waker` é chamado a cada um.
    pub fn start(
        program: PathBuf,
        overrides: Vec<String>,
        system: &str,
        cwd: PathBuf,
        waker: Waker,
    ) -> CodexAgent {
        let (tx, user_rx) = mpsc::channel::<String>();
        let (event_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let (stop, running) = (cancel.clone(), child.clone());
        let system = system.to_string();
        std::thread::spawn(move || {
            let emit = |e: AgentEvent| {
                let _ = event_tx.send(e);
                waker();
            };
            emit(AgentEvent::Started {
                session_id: String::new(),
                model: String::new(),
            });
            let mut thread_id: Option<String> = None;
            while let Ok(text) = user_rx.recv() {
                stop.store(false, Ordering::SeqCst);
                let prompt = match thread_id {
                    Some(_) => text,
                    None => first_prompt(&system, &text),
                };
                let args = exec_args(true, thread_id.as_deref(), &overrides);
                let outcome = run_turn(&program, &args, &cwd, &prompt, &running, &emit);
                if stop.load(Ordering::SeqCst) {
                    // Parar: o painel já avisou o usuário. A conversa
                    // continua de onde o Codex gravou.
                    if let Ok(parser) = outcome {
                        thread_id = parser.thread_id.or(thread_id);
                    }
                    continue;
                }
                match outcome {
                    Ok(parser) => thread_id = parser.thread_id.or(thread_id),
                    Err(e) => emit(AgentEvent::TurnDone {
                        is_error: true,
                        text: e,
                        cost_usd: None,
                        session_id: String::new(),
                    }),
                }
            }
        });
        CodexAgent {
            tx,
            rx,
            cancel,
            child,
        }
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
        if let Some(child) = self.child.lock().expect("codex child").as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for CodexAgent {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Roda um `codex exec` até o fim, mandando os eventos. Devolve o leitor
/// (com a conversa) ou o erro, quando a vez acabou sem o Codex dizer
/// como.
fn run_turn(
    program: &Path,
    args: &[String],
    cwd: &Path,
    prompt: &str,
    running: &Arc<Mutex<Option<Child>>>,
    emit: &dyn Fn(AgentEvent),
) -> Result<Parser, String> {
    std::fs::create_dir_all(cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let mut child = command(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start Codex ({}): {e}", program.display()))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *running.lock().expect("codex child") = Some(child);
    if let Some(mut stdin) = stdin {
        // Fechar a entrada é o que diz ao Codex que a pergunta acabou.
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let errors = std::thread::spawn(move || {
        let mut lines: Vec<String> = Vec::new();
        if let Some(err) = stderr {
            for line in BufReader::new(err).lines() {
                let Ok(line) = line else { break };
                if !line.trim().is_empty() {
                    lines.push(line);
                    if lines.len() > STDERR_LINES {
                        lines.remove(0);
                    }
                }
            }
        }
        lines
    });
    let mut parser = Parser::default();
    if let Some(out) = stdout {
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            for event in parser.line(&line) {
                emit(event);
            }
        }
    }
    let status = running
        .lock()
        .expect("codex child")
        .take()
        .and_then(|mut c| c.wait().ok());
    let stderr = errors.join().unwrap_or_default();
    if parser.done {
        return Ok(parser);
    }
    let detail = parser
        .last_error
        .clone()
        .or_else(|| (!stderr.is_empty()).then(|| stderr.join(" ")))
        .unwrap_or_else(|| match status.and_then(|s| s.code()) {
            Some(code) => format!("it stopped before answering (exit code {code})"),
            None => "it stopped before answering".into(),
        });
    Err(explain(&detail))
}

/// O `codex login` rodando: abre o navegador para o usuário entrar com a
/// conta do ChatGPT. O PostPilot só acompanha; o login fica no Codex.
pub struct LoginFlow {
    child: Child,
    rx: mpsc::Receiver<String>,
    /// O que o Codex escreveu (o endereço para abrir à mão, se o navegador
    /// não abrir).
    pub lines: Vec<String>,
}

impl std::fmt::Debug for LoginFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoginFlow")
    }
}

impl LoginFlow {
    /// Abre o `codex login`. `waker` é chamado a cada linha e no fim.
    pub fn start(program: &Path, waker: Waker) -> std::io::Result<LoginFlow> {
        let mut child = command(program)
            .arg("login")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (tx, rx) = mpsc::channel();
        let pipe = |reader: Box<dyn std::io::Read + Send>| {
            let (tx, waker) = (tx.clone(), waker.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(reader).lines() {
                    let Ok(line) = line else { break };
                    let _ = tx.send(line);
                    waker();
                }
                waker();
            });
        };
        if let Some(out) = child.stdout.take() {
            pipe(Box::new(out));
        }
        if let Some(err) = child.stderr.take() {
            pipe(Box::new(err));
        }
        Ok(LoginFlow {
            child,
            rx,
            lines: Vec::new(),
        })
    }

    /// Lê o que chegou. Devolve `Some(deu certo)` quando o `codex login`
    /// termina.
    pub fn poll(&mut self) -> Option<bool> {
        while let Ok(line) = self.rx.try_recv() {
            let line = strip_ansi(&line);
            if !line.trim().is_empty() {
                self.lines.push(line);
            }
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                while let Ok(line) = self.rx.try_recv() {
                    self.lines.push(strip_ansi(&line));
                }
                Some(status.success())
            }
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }

    /// O endereço de login que o Codex mostrou, para abrir à mão.
    pub fn url(&self) -> Option<&str> {
        self.lines.iter().find_map(|l| {
            l.split_whitespace()
                .find(|w| w.starts_with("https://") || w.starts_with("http://"))
        })
    }
}

impl Drop for LoginFlow {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Tira as cores do terminal de uma linha.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... letra
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn login_status_text() {
        assert_eq!(
            parse_login_status(true, "Logged in using ChatGPT\n"),
            Login::ChatGpt
        );
        assert_eq!(
            parse_login_status(true, "API key configured - sk-proj-***abcde"),
            Login::ApiKey
        );
        assert_eq!(parse_login_status(false, "Not logged in"), Login::SignedOut);
        assert_eq!(parse_login_status(false, ""), Login::SignedOut);
        assert!(matches!(
            parse_login_status(false, "error: unrecognized subcommand"),
            Login::Unknown(_)
        ));
        assert!(Login::ChatGpt.is_signed_in());
        assert!(!Login::SignedOut.is_signed_in());
    }

    #[test]
    fn exec_jsonl() {
        let mut p = Parser::default();
        assert!(
            p.line(r#"{"type":"thread.started","thread_id":"0199a213-81c0"}"#)
                .is_empty()
        );
        assert_eq!(p.thread_id.as_deref(), Some("0199a213-81c0"));
        assert!(p.line(r#"{"type":"turn.started"}"#).is_empty());
        let started = p.line(
            r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"postpilot","tool":"describe_table","arguments":{"table":"pedido"},"status":"in_progress"}}"#,
        );
        assert_eq!(
            started,
            vec![AgentEvent::ToolUse {
                id: "item_1".into(),
                name: "describe_table".into(),
                input: json!({"table": "pedido"}),
            }]
        );
        // A ferramenta já foi anunciada: só a resposta.
        assert_eq!(
            p.line(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"postpilot","tool":"describe_table","arguments":{"table":"pedido"},"result":{"content":[{"type":"text","text":"{\"columns\":[]}"}],"structured_content":null},"error":null,"status":"completed"}}"#
            ),
            vec![AgentEvent::ToolResult {
                id: "item_1".into(),
                is_error: false,
                text: "{\"columns\":[]}".into(),
            }]
        );
        // Sem o `item.started`: anuncia e responde.
        let failed = p.line(
            r#"{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"postpilot","tool":"run_query","arguments":"{\"sql\":\"delete from t\"}","result":null,"error":{"message":"declined"},"status":"failed"}}"#,
        );
        assert_eq!(failed.len(), 2);
        assert_eq!(
            failed[0],
            AgentEvent::ToolUse {
                id: "item_2".into(),
                name: "run_query".into(),
                input: json!({"sql": "delete from t"}),
            }
        );
        assert_eq!(
            failed[1],
            AgentEvent::ToolResult {
                id: "item_2".into(),
                is_error: true,
                text: "declined".into(),
            }
        );
        assert!(
            p.line(r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"..."}}"#)
                .is_empty()
        );
        assert_eq!(
            p.line(
                r#"{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"Pronto."}}"#
            ),
            vec![AgentEvent::Text("Pronto.".into())]
        );
        assert!(!p.done);
        let done = p.line(
            r#"{"type":"turn.completed","usage":{"input_tokens":24763,"cached_input_tokens":24448,"output_tokens":122}}"#,
        );
        assert!(matches!(
            done[0],
            AgentEvent::TurnDone {
                is_error: false,
                ..
            }
        ));
        assert!(p.done);
        assert!(p.line("not json").is_empty());
    }

    #[test]
    fn failures_explain_themselves() {
        let mut p = Parser::default();
        assert!(
            p.line(r#"{"type":"error","message":"Reconnecting... 1/5"}"#)
                .is_empty()
        );
        let failed = p.line(r#"{"type":"turn.failed","error":{"message":"401 Unauthorized"}}"#);
        let AgentEvent::TurnDone { is_error, text, .. } = &failed[0] else {
            panic!()
        };
        assert!(is_error);
        assert!(text.contains("not signed in"), "{text}");
        assert!(text.contains("401"), "{text}");
    }

    #[test]
    fn args_and_overrides() {
        let a = exec_args(true, None, &[]);
        assert_eq!(a[0], "exec");
        assert!(a.contains(&"--json".to_string()));
        let i = a.iter().position(|x| x == "--sandbox").unwrap();
        assert_eq!(a[i + 1], "read-only");
        assert!(a.contains(&"features.shell_tool=false".to_string()));
        let a = exec_args(true, Some("abc"), &["-c".into(), "x=1".into()]);
        assert!(a.ends_with(&["resume".into(), "abc".into()]));
        let m = mcp_overrides(Path::new("/x/postpilot"), Path::new("/t/e.json"));
        assert_eq!(m[0], "-c");
        assert_eq!(m[1], "mcp_servers.postpilot.command='/x/postpilot'");
        assert_eq!(m[3], "mcp_servers.postpilot.args=['mcp']");
        assert_eq!(
            m[5],
            format!(
                "mcp_servers.postpilot.env={{{}='/t/e.json'}}",
                crate::endpoint::ENV
            )
        );
        // Aspa simples no caminho: texto TOML com escape.
        assert_eq!(toml_string("C:\\it's"), "\"C:\\\\it's\"");
        assert_eq!(toml_string("C:\\Users\\t"), "'C:\\Users\\t'");
        assert_eq!(strip_ansi("\u{1b}[1mhttps://x\u{1b}[0m"), "https://x");
    }

    /// Um `codex` de mentira: responde ao `login status`, ao `login` e ao
    /// `exec`, gravando os argumentos e a pergunta.
    #[cfg(unix)]
    pub(crate) fn fake_codex(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("fake-codex");
        let log = dir.join("calls.log");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
LOG='{log}'
if [ "$1" = login ] && [ "$2" = status ]; then
  if [ -f '{dir}/signed-in' ]; then printf 'Logged in using ChatGPT\n'; exit 0; fi
  printf 'Not logged in\n' >&2; exit 1
fi
if [ "$1" = login ]; then
  printf 'Open this link: https://auth.example/login?x=1\n' >&2
  : > '{dir}/signed-in'
  printf 'Successfully logged in\n' >&2
  exit 0
fi
printf 'ARGS %s\n' "$*" >> "$LOG"
PROMPT=$(cat)
printf 'PROMPT %s\n' "$PROMPT" >> "$LOG"
case "$*" in
  *--json*)
    printf '%s\n' '{{"type":"thread.started","thread_id":"th-1"}}'
    printf '%s\n' '{{"type":"turn.started"}}'
    printf '%s\n' '{{"type":"item.completed","item":{{"id":"item_0","type":"agent_message","text":"resposta"}}}}'
    printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":1}}}}'
    ;;
  *) printf 'texto final\n' ;;
esac
"#,
                log = log.display(),
                dir = dir.display(),
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("postpilot-codex-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn signs_in_asks_and_chats_with_a_process() {
        let dir = temp("flow");
        let codex = fake_codex(&dir);
        assert_eq!(login_status(&codex), Login::SignedOut);

        let mut login = LoginFlow::start(&codex, Arc::new(|| {})).unwrap();
        let start = std::time::Instant::now();
        let ok = loop {
            if let Some(ok) = login.poll() {
                break ok;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(ok);
        // As últimas linhas podem chegar logo depois do fim do processo.
        let start = std::time::Instant::now();
        while login.url().is_none() && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            login.poll();
        }
        assert_eq!(login.url(), Some("https://auth.example/login?x=1"));
        assert_eq!(login_status(&codex), Login::ChatGpt);

        assert_eq!(
            ask(&codex, "regras", "pergunta", &dir.join("cwd")).as_deref(),
            Ok("texto final")
        );

        let mut agent = CodexAgent::start(
            codex,
            mcp_overrides(Path::new("/x/postpilot"), Path::new("/t/e.json")),
            "instruções do painel",
            dir.join("cwd"),
            Arc::new(|| {}),
        );
        let wait_turn = |agent: &CodexAgent| {
            let mut seen = Vec::new();
            let start = std::time::Instant::now();
            loop {
                match agent.try_recv() {
                    Some(e @ AgentEvent::TurnDone { .. }) => {
                        seen.push(e);
                        return seen;
                    }
                    Some(e) => seen.push(e),
                    None => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
                assert!(start.elapsed() < std::time::Duration::from_secs(10));
            }
        };
        agent.send("oi").unwrap();
        let seen = wait_turn(&agent);
        assert!(
            seen.contains(&AgentEvent::Text("resposta".into())),
            "{seen:?}"
        );
        assert!(matches!(
            seen.last(),
            Some(AgentEvent::TurnDone {
                is_error: false,
                ..
            })
        ));
        agent.send("e agora?").unwrap();
        wait_turn(&agent);

        let log = std::fs::read_to_string(dir.join("calls.log")).unwrap();
        let args: Vec<&str> = log.lines().filter(|l| l.starts_with("ARGS")).collect();
        assert_eq!(args.len(), 3, "{log}");
        assert!(args[0].contains("--ephemeral") && !args[0].contains("--json"));
        assert!(args[1].contains("--json") && !args[1].contains("resume"));
        assert!(args[1].contains("mcp_servers.postpilot.command='/x/postpilot'"));
        // A segunda mensagem continua a conversa, sem repetir as instruções.
        assert!(args[2].ends_with("resume th-1"), "{}", args[2]);
        assert!(log.contains("instruções do painel"));
        assert_eq!(log.matches("instruções do painel").count(), 1, "{log}");
        assert!(log.contains("PROMPT e agora?"), "{log}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_that_dies_says_why() {
        let dir = temp("dies");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("codex");
        std::fs::write(
            &script,
            "#!/bin/sh\ncat > /dev/null\nprintf 'Not logged in. Run codex login.\\n' >&2\nexit 1\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = ask(&script, "s", "u", &dir).unwrap_err();
        assert!(err.contains("not signed in"), "{err}");
        let mut agent = CodexAgent::start(script, Vec::new(), "s", dir.clone(), Arc::new(|| {}));
        agent.send("oi").unwrap();
        let start = std::time::Instant::now();
        let text = loop {
            if let Some(AgentEvent::TurnDone { is_error, text, .. }) = agent.try_recv() {
                assert!(is_error);
                break text;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(text.contains("codex login"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
