//! `pg_dump` e `pg_restore`: achar os binários, montar a linha de comando
//! e rodar fora da thread da interface, com o progresso linha a linha.
//!
//! O PGStudio não reimplementa os formatos de backup: chama os binários do
//! PostgreSQL. Eles são procurados em `PGSTUDIO_PG_BIN` (pasta ou arquivo),
//! no PATH e nas pastas padrão de instalação; entre os encontrados, vale o
//! de versão mais nova. Um `pg_dump` mais antigo que o servidor é recusado
//! pelo próprio `pg_dump`, então o PGStudio avisa antes.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};

/// Um binário encontrado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub path: PathBuf,
    /// Versão (maior, menor), como `(16, 4)`.
    pub version: (u32, u32),
}

impl Tool {
    pub fn version_text(&self) -> String {
        format!("{}.{}", self.version.0, self.version.1)
    }
}

/// Lê a versão de `pg_dump --version` (`pg_dump (PostgreSQL) 16.4 (Ubuntu
/// 16.4-1)`, `pg_restore (PostgreSQL) 17beta1`).
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let after = text.split(')').nth(1).unwrap_or(text);
    let word = after.split_whitespace().next()?;
    let mut parts = word.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// Maior e menor da versão do servidor (`16.15`, `17beta1`).
pub fn server_major(version: &str) -> u32 {
    version
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0)
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Pastas onde procurar, na ordem: a configurada, o PATH e as padrão.
pub fn search_dirs(configured: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(c) = configured {
        dirs.push(if c.is_file() {
            c.parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            c.to_path_buf()
        });
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    let globs: &[(&str, &str)] = if cfg!(windows) {
        &[
            ("C:\\Program Files\\PostgreSQL", "bin"),
            ("C:\\Program Files (x86)\\PostgreSQL", "bin"),
        ]
    } else {
        &[
            ("/usr/lib/postgresql", "bin"),
            ("/usr/pgsql-", ""),
            ("/opt/homebrew/opt", "bin"),
            ("/usr/local/opt", "bin"),
            ("/Library/PostgreSQL", "bin"),
        ]
    };
    for (base, sub) in globs {
        // `/usr/pgsql-16/bin`: o prefixo é parte do nome da pasta.
        let (parent, prefix) = match base.strip_suffix('-') {
            Some(_) => (Path::new("/usr"), "pgsql-"),
            None => (Path::new(*base), ""),
        };
        let Ok(entries) = std::fs::read_dir(parent) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with(prefix) {
                continue;
            }
            if parent.ends_with("opt")
                && !name.starts_with("postgresql")
                && !name.starts_with("libpq")
            {
                continue;
            }
            let dir = if sub.is_empty() {
                e.path().join("bin")
            } else {
                e.path().join(sub)
            };
            dirs.push(dir);
        }
    }
    dirs.extend(
        ["/usr/local/pgsql/bin", "/usr/local/bin", "/usr/bin"]
            .iter()
            .filter(|_| !cfg!(windows))
            .map(PathBuf::from),
    );
    dirs
}

/// A versão de um binário, rodando `--version`.
pub fn probe(path: &Path) -> Option<Tool> {
    let out = Command::new(path).arg("--version").output().ok()?;
    let version = parse_version(&String::from_utf8_lossy(&out.stdout))?;
    Some(Tool {
        path: path.to_path_buf(),
        version,
    })
}

/// Todos os `name` encontrados, do mais novo para o mais antigo.
pub fn discover(name: &str, configured: Option<&Path>) -> Vec<Tool> {
    let file = exe(name);
    let mut seen = std::collections::HashSet::new();
    let mut tools: Vec<Tool> = search_dirs(configured)
        .into_iter()
        .map(|d| d.join(&file))
        .filter(|p| p.is_file())
        .filter(|p| seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())))
        .filter_map(|p| probe(&p))
        .collect();
    tools.sort_by_key(|t| std::cmp::Reverse(t.version));
    tools
}

/// O binário para um servidor `server_version`: o mais novo encontrado, ou
/// por que não dá.
pub fn choose(name: &str, configured: Option<&Path>, server_version: &str) -> Result<Tool, String> {
    let tools = discover(name, configured);
    let Some(best) = tools.into_iter().next() else {
        return Err(format!(
            "{name} was not found. Install the PostgreSQL client tools or set PGSTUDIO_PG_BIN \
             to the folder that has {name}."
        ));
    };
    let server = server_major(server_version);
    if best.version.0 < server {
        return Err(format!(
            "{name} {} is older than the server (PostgreSQL {server}). Install {name} {server} \
             or newer, or set PGSTUDIO_PG_BIN to it.",
            best.version_text()
        ));
    }
    Ok(best)
}

/// Pasta configurada em `PGSTUDIO_PG_BIN`.
pub fn configured_dir() -> Option<PathBuf> {
    std::env::var_os("PGSTUDIO_PG_BIN").map(PathBuf::from)
}

/// Variáveis de ambiente da conexão (a senha nunca vai na linha de
/// comando).
pub fn connection_env(settings: &ConnectionSettings, password: &str) -> Vec<(String, String)> {
    let ssl = match settings.ssl {
        SslMode::Disable => "disable",
        SslMode::Prefer => "prefer",
        SslMode::Require => "require",
        SslMode::VerifyFull => "verify-full",
    };
    let mut env = vec![
        ("PGSSLMODE".to_string(), ssl.to_string()),
        ("PGAPPNAME".to_string(), "PGStudio".to_string()),
    ];
    if !password.is_empty() {
        env.push(("PGPASSWORD".to_string(), password.to_string()));
    }
    env
}

/// `--host`, `--port`, `--username` e `--no-password`.
pub fn connection_args(settings: &ConnectionSettings) -> Vec<String> {
    vec![
        format!("--host={}", settings.host),
        format!("--port={}", settings.port),
        format!("--username={}", settings.user),
        "--no-password".to_string(),
    ]
}

/// A linha de comando como texto, para o Script e o Output.
pub fn command_line(program: &Path, args: &[String]) -> String {
    let quote = |s: &str| {
        if !s.is_empty()
            && s.chars()
                .all(|c| c.is_alphanumeric() || "-_=./:\\,".contains(c))
        {
            s.to_string()
        } else if cfg!(windows) {
            format!("\"{}\"", s.replace('"', "\\\""))
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    std::iter::once(program.display().to_string())
        .chain(args.iter().map(|a| quote(a)))
        .map(|s| {
            if s.contains(' ') && !s.starts_with(['\'', '"']) {
                quote(&s)
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// O que um processo conta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEvent {
    /// Uma linha de stderr (o progresso com `--verbose`) ou de stdout.
    Line(String),
    /// Terminou: Ok, ou o código de saída.
    Exit(Result<(), String>),
}

/// Um `pg_dump` ou `pg_restore` rodando.
pub struct ToolRun {
    child: Arc<Mutex<Child>>,
    rx: Receiver<ToolEvent>,
    canceled: Arc<std::sync::atomic::AtomicBool>,
    /// Saída de stdout, quando `capture_stdout`.
    pub stdout: Arc<Mutex<String>>,
}

impl ToolRun {
    /// Começa `program args`. As linhas de stderr viram eventos; stdout
    /// vira eventos ou, com `capture_stdout`, fica em [`ToolRun::stdout`].
    pub fn spawn(
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        capture_stdout: bool,
        ctx: &egui::Context,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Sem janela de console.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn()?;
        let (tx, rx) = mpsc::channel();
        let stdout_text = Arc::new(Mutex::new(String::new()));
        let stderr = child.stderr.take();
        let stdout = child.stdout.take();
        let mut readers = Vec::new();
        if let Some(err) = stderr {
            let tx = tx.clone();
            let ctx = ctx.clone();
            readers.push(std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send(ToolEvent::Line(line));
                    ctx.request_repaint();
                }
            }));
        }
        if let Some(mut out) = stdout {
            let tx = tx.clone();
            let ctx = ctx.clone();
            let text = stdout_text.clone();
            readers.push(std::thread::spawn(move || {
                if capture_stdout {
                    let mut s = String::new();
                    let _ = out.read_to_string(&mut s);
                    if let Ok(mut t) = text.lock() {
                        *t = s;
                    }
                } else {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        let _ = tx.send(ToolEvent::Line(line));
                        ctx.request_repaint();
                    }
                }
            }));
        }
        let child = Arc::new(Mutex::new(child));
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let child = child.clone();
            let ctx = ctx.clone();
            let canceled = canceled.clone();
            std::thread::spawn(move || {
                let status = loop {
                    let done = child.lock().ok().and_then(|mut c| c.try_wait().transpose());
                    match done {
                        Some(s) => break s,
                        None => std::thread::sleep(Duration::from_millis(30)),
                    }
                };
                for r in readers {
                    let _ = r.join();
                }
                let result = match status {
                    _ if canceled.load(std::sync::atomic::Ordering::SeqCst) => {
                        Err("canceled".to_string())
                    }
                    Ok(s) if s.success() => Ok(()),
                    Ok(s) => Err(match s.code() {
                        Some(c) => format!("exit code {c}"),
                        None => "stopped".to_string(),
                    }),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(ToolEvent::Exit(result));
                ctx.request_repaint();
            });
        }
        Ok(Self {
            child,
            rx,
            canceled,
            stdout: stdout_text,
        })
    }

    /// Para o processo.
    pub fn cancel(&self) {
        self.canceled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }

    /// Eventos que chegaram desde a última chamada.
    pub fn events(&self) -> Vec<ToolEvent> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read() {
        assert_eq!(
            parse_version("pg_dump (PostgreSQL) 16.4 (Ubuntu 16.4-1.pgdg22.04+1)\n"),
            Some((16, 4))
        );
        assert_eq!(
            parse_version("pg_restore (PostgreSQL) 17beta1"),
            Some((17, 0))
        );
        assert_eq!(parse_version("pg_dump (PostgreSQL) 9.6.24"), Some((9, 6)));
        assert_eq!(parse_version("nothing"), None);
        assert_eq!(server_major("16.15"), 16);
        assert_eq!(server_major("17beta1"), 17);
    }

    #[test]
    fn the_password_goes_in_the_environment() {
        let s = ConnectionSettings {
            host: "db.local".into(),
            port: 5433,
            user: "app".into(),
            ssl: SslMode::Require,
            ..Default::default()
        };
        let args = connection_args(&s);
        assert_eq!(
            args,
            [
                "--host=db.local",
                "--port=5433",
                "--username=app",
                "--no-password"
            ]
        );
        let env = connection_env(&s, "s3cret");
        assert!(env.contains(&("PGPASSWORD".into(), "s3cret".into())));
        assert!(env.contains(&("PGSSLMODE".into(), "require".into())));
        assert!(!command_line(Path::new("pg_dump"), &args).contains("s3cret"));
    }

    #[test]
    fn command_lines_are_quoted() {
        let line = command_line(
            Path::new("/usr/bin/pg_dump"),
            &[
                "--file=/tmp/my backup.sql".into(),
                "--table=public.\"T\"".into(),
            ],
        );
        if !cfg!(windows) {
            assert_eq!(
                line,
                "/usr/bin/pg_dump '--file=/tmp/my backup.sql' '--table=public.\"T\"'"
            );
        }
    }

    #[test]
    fn missing_tools_are_explained() {
        let e = choose("pgstudio_no_such_tool", None, "16.1").unwrap_err();
        assert!(e.contains("was not found"), "{e}");
    }
}
