//! O canal local entre o app aberto e o `pgstudio mcp` (marco 7.2).
//!
//! Com "Allow Claude Code (MCP)" ligado, o app escuta em `127.0.0.1`, numa
//! porta qualquer, e grava a porta e um segredo aleatório em
//! `<config>/PGStudio/mcp-endpoint.json`, legível só pelo usuário. O
//! `pgstudio mcp` lê esse arquivo e manda cada chamada numa conexão: uma
//! linha JSON `{"token", "tool", "args"}` e uma linha de volta
//! `{"text", "is_error"}`. Sem o segredo certo, o app fecha a conexão sem
//! responder.
//!
//! O mesmo caminho serve no Linux e no Windows, e dá para testar sem tela.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hub::{Client, Origin, Reply};

/// Nome do arquivo do canal, na pasta de configuração do PGStudio.
pub const FILE_NAME: &str = "mcp-endpoint.json";

/// Variável que troca o arquivo do canal (testes e mais de uma
/// instalação).
pub const ENV: &str = "PGSTUDIO_MCP_ENDPOINT";

/// Maior linha aceita (um SQL grande cabe com folga).
const MAX_LINE: u64 = 16 * 1024 * 1024;

/// Resposta quando o app não está aberto ou não aceita o Claude Code.
pub const NOT_RUNNING: &str = "PostPilot is not open, or \"Allow Claude Code (MCP)\" is off \
in its Tools menu. Ask the user to open PostPilot and turn it on, then try again.";

/// O que fica no arquivo do canal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

/// `<config>/PGStudio/mcp-endpoint.json`, ou o arquivo de
/// `PGSTUDIO_MCP_ENDPOINT`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    config_dir().map(|d| d.join(pgstudio_core::CONFIG_DIR_NAME).join(FILE_NAME))
}

/// Pasta de configuração do usuário: `%APPDATA%` no Windows,
/// `$XDG_CONFIG_HOME` ou `~/.config` nos demais (a mesma do app).
fn config_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    }
}

/// 32 bytes aleatórios do sistema, em hexadecimal.
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Grava o arquivo só para o usuário (0600 no Unix; no Windows, o perfil
/// do usuário já é só dele).
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Comparação que não para no primeiro byte diferente.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[derive(Deserialize)]
struct Request {
    token: String,
    tool: String,
    #[serde(default)]
    args: Value,
}

#[derive(Serialize, Deserialize)]
struct Response {
    text: String,
    is_error: bool,
}

/// O app escutando. Parar (ou soltar) fecha a porta e apaga o arquivo.
pub struct Listener {
    path: PathBuf,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    running: Arc<AtomicUsize>,
    pid: u32,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener")
            .field("addr", &self.addr)
            .finish()
    }
}

impl Listener {
    /// Começa a escutar e grava o arquivo do canal em `path`. As chamadas
    /// vão para `client`, com origem [`Origin::Mcp`].
    pub fn start(client: Client, path: PathBuf) -> std::io::Result<Listener> {
        Self::start_as(client, path, Origin::Mcp)
    }

    /// Como [`Listener::start`], com as chamadas marcadas com `origin` (o
    /// painel do Claude usa um canal próprio, com [`Origin::Panel`]).
    pub fn start_as(client: Client, path: PathBuf, origin: Origin) -> std::io::Result<Listener> {
        let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = socket.local_addr()?;
        let token = new_token()?;
        let pid = std::process::id();
        let endpoint = Endpoint {
            port: addr.port(),
            token: token.clone(),
            pid,
        };
        write_private(&path, &serde_json::to_string_pretty(&endpoint)?)?;
        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicUsize::new(0));
        let (s, c, r) = (stop.clone(), calls.clone(), running.clone());
        std::thread::Builder::new()
            .name("pgstudio-mcp".into())
            .spawn(move || {
                for stream in socket.incoming() {
                    if s.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (client, token, calls, running) =
                        (client.clone(), token.clone(), c.clone(), r.clone());
                    std::thread::spawn(move || {
                        serve(stream, &client, &token, origin, &calls, &running)
                    });
                }
            })?;
        Ok(Listener {
            path,
            addr,
            stop,
            calls,
            running,
            pid,
        })
    }

    /// Porta em que escuta.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Arquivo do canal.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Chamadas atendidas até agora.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Chamadas em andamento.
    pub fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Acorda o accept para a thread terminar.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        // Só apaga o arquivo se ainda é deste app (outro pode ter aberto
        // depois).
        let mine = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Endpoint>(&t).ok())
            .is_some_and(|e| e.pid == self.pid && e.port == self.addr.port());
        if mine {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn serve(
    stream: TcpStream,
    client: &Client,
    token: &str,
    origin: Origin,
    calls: &AtomicUsize,
    running: &AtomicUsize,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(mut write) = stream.try_clone() else {
        return;
    };
    let mut line = String::new();
    let mut reader = BufReader::new(stream).take(MAX_LINE);
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let Ok(request) = serde_json::from_str::<Request>(&line) else {
        return;
    };
    if !same(&request.token, token) {
        return;
    }
    calls.fetch_add(1, Ordering::SeqCst);
    running.fetch_add(1, Ordering::SeqCst);
    let reply = client.call(&request.tool, request.args, origin);
    running.fetch_sub(1, Ordering::SeqCst);
    let response = Response {
        text: reply.text,
        is_error: reply.is_error,
    };
    if let Ok(mut text) = serde_json::to_string(&response) {
        text.push('\n');
        let _ = write.write_all(text.as_bytes());
    }
}

/// Lado do `pgstudio mcp`: manda a chamada para o app pelo arquivo do
/// canal. Sem app, responde com [`NOT_RUNNING`].
pub fn call(path: &Path, tool: &str, args: Value) -> Reply {
    let Some(endpoint) = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Endpoint>(&t).ok())
    else {
        return Reply::error(NOT_RUNNING);
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return Reply::error(NOT_RUNNING);
    };
    let request = serde_json::json!({ "token": endpoint.token, "tool": tool, "args": args });
    let mut text = request.to_string();
    text.push('\n');
    if stream.write_all(text.as_bytes()).is_err() {
        return Reply::error(NOT_RUNNING);
    }
    let mut line = String::new();
    let mut reader = BufReader::new(stream).take(MAX_LINE);
    match reader.read_line(&mut line) {
        Ok(n) if n > 0 => match serde_json::from_str::<Response>(&line) {
            Ok(r) => Reply {
                text: r.text,
                is_error: r.is_error,
            },
            Err(e) => Reply::error(format!(
                "PostPilot sent an answer that could not be read: {e}"
            )),
        },
        _ => Reply::error(
            "PostPilot closed the connection without answering (it may have been closed, \
             or Claude Code access was turned off and on again; try again).",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::channel;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pgstudio-endpoint-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE_NAME)
    }

    #[test]
    fn calls_reach_the_app_and_the_file_goes_away() {
        let path = temp("ok");
        let (client, inbox) = channel(Arc::new(|| {}));
        let listener = Listener::start(client, path.clone()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let app = std::thread::spawn(move || {
            loop {
                if let Some(p) = inbox.try_next() {
                    assert_eq!(p.origin, Origin::Mcp);
                    p.responder.send(Reply::ok(
                        &serde_json::json!({ "tool": p.tool, "args": p.args }),
                    ));
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let reply = call(&path, "list_servers", serde_json::json!({ "x": 1 }));
        app.join().unwrap();
        assert!(!reply.is_error, "{}", reply.text);
        let v: Value = serde_json::from_str(&reply.text).unwrap();
        assert_eq!(v["tool"], "list_servers");
        assert_eq!(v["args"]["x"], 1);
        assert_eq!(listener.calls(), 1);
        drop(listener);
        assert!(!path.exists());
        assert_eq!(call(&path, "list_servers", Value::Null).text, NOT_RUNNING);
    }

    #[test]
    fn wrong_token_gets_nothing() {
        let path = temp("token");
        let (client, inbox) = channel(Arc::new(|| {}));
        let listener = Listener::start(client, path.clone()).unwrap();
        let mut e: Endpoint =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(e.token.len(), 64);
        e.token = "0".repeat(64);
        std::fs::write(&path, serde_json::to_string(&e).unwrap()).unwrap();
        let reply = call(&path, "list_servers", Value::Null);
        assert!(reply.is_error);
        assert!(inbox.try_next().is_none());
        assert_eq!(listener.calls(), 0);
    }

    #[test]
    fn stale_file_means_not_running() {
        let path = temp("stale");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let e = Endpoint {
            port,
            token: "x".into(),
            pid: 1,
        };
        std::fs::write(&path, serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(call(&path, "list_servers", Value::Null).text, NOT_RUNNING);
        assert_eq!(
            call(&temp("none"), "list_servers", Value::Null).text,
            NOT_RUNNING
        );
    }
}
