//! Git do workspace pelo `git` instalado no sistema.
//!
//! Usar o executável (e não uma biblioteca) aproveita o que já está
//! configurado na máquina: chaves SSH e o ssh-agent, o credential helper,
//! o `.gitconfig` com nome e e-mail. O PostPilot nunca pede nem guarda
//! credencial do git. `GIT_TERMINAL_PROMPT=0` impede que o git espere uma
//! senha num terminal que não existe: sem credencial, a operação falha com
//! a mensagem do git.
//!
//! Todas as chamadas podem demorar (rede), então a interface roda cada uma
//! numa thread separada com [`Job`].

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// O git falhou ou não existe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitError {
    /// Não há `git` no PATH.
    NotInstalled,
    /// A pasta não é um repositório git.
    NotRepo,
    /// O git rodou e falhou; a mensagem é a dele.
    Failed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotInstalled => f.write_str(
                "Git is not installed. Install git (on Windows, Git for Windows) to use source control.",
            ),
            GitError::NotRepo => f.write_str("The workspace is not under source control."),
            GitError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for GitError {}

/// Um arquivo alterado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Caminho relativo à raiz do repositório.
    pub path: String,
    /// As duas letras do `git status --porcelain` (`M `, ` M`, `??`...).
    pub code: String,
}

impl Change {
    /// Descrição curta, como no Team Explorer.
    pub fn kind(&self) -> &'static str {
        match self.code.as_str() {
            "??" => "Added",
            c if c.contains('D') => "Deleted",
            c if c.contains('A') => "Added",
            c if c.contains('R') => "Renamed",
            _ => "Modified",
        }
    }
}

/// Estado do repositório.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Branch atual (vazio com HEAD destacado).
    pub branch: String,
    /// Branch remoto seguido, se houver.
    pub upstream: Option<String>,
    /// Commits locais que ainda não foram enviados.
    pub ahead: u32,
    /// Commits do remoto que ainda não foram trazidos.
    pub behind: u32,
    /// Arquivos alterados, sem os em conflito.
    pub changes: Vec<Change>,
    /// Arquivos com conflito de merge.
    pub conflicts: Vec<String>,
    /// URL do remoto `origin`, se houver.
    pub remote: Option<String>,
}

impl Status {
    /// Se não há nada a commitar.
    pub fn is_clean(&self) -> bool {
        self.changes.is_empty() && self.conflicts.is_empty()
    }

    /// Lê a saída de `git status --porcelain=v1 --branch -z`.
    pub fn parse(output: &str) -> Status {
        let mut status = Status::default();
        let mut entries = output.split('\0').filter(|e| !e.is_empty());
        while let Some(entry) = entries.next() {
            if let Some(branch) = entry.strip_prefix("## ") {
                status.read_branch(branch);
                continue;
            }
            if entry.len() < 4 {
                continue;
            }
            let (code, path) = entry.split_at(2);
            let path = path[1..].to_string();
            if code.starts_with('R') || code.starts_with('C') {
                // Renomeado: a entrada seguinte é o nome antigo.
                entries.next();
            }
            let conflict = matches!(code, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU");
            if conflict {
                status.conflicts.push(path);
            } else {
                status.changes.push(Change {
                    path,
                    code: code.to_string(),
                });
            }
        }
        status
    }

    fn read_branch(&mut self, text: &str) {
        // "main...origin/main [ahead 1, behind 2]", "main", "No commits yet on main",
        // "HEAD (no branch)".
        let text = text
            .strip_prefix("No commits yet on ")
            .or_else(|| text.strip_prefix("Initial commit on "))
            .unwrap_or(text);
        let (names, counts) = match text.find(" [") {
            Some(i) => (&text[..i], &text[i + 2..text.len().saturating_sub(1)]),
            None => (text, ""),
        };
        match names.split_once("...") {
            Some((branch, upstream)) => {
                self.branch = branch.to_string();
                self.upstream = Some(upstream.to_string());
            }
            None if names.starts_with("HEAD ") => self.branch.clear(),
            None => self.branch = names.to_string(),
        }
        for part in counts.split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                self.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                self.behind = n.parse().unwrap_or(0);
            }
        }
    }
}

/// O git de uma pasta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Git {
    root: PathBuf,
}

fn command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .args(["-c", "core.quotepath=false", "-c", "color.ui=false"]);
    // Sem janela de console a cada chamada no Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn run(mut cmd: Command) -> Result<String, GitError> {
    let output: Output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Failed(format!("Could not run git: {e}"))
        }
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut message = stderr.trim().to_string();
    if message.is_empty() {
        message = stdout.trim().to_string();
    }
    if message.is_empty() {
        message = format!("git failed ({})", output.status);
    }
    Err(GitError::Failed(message))
}

impl Git {
    /// O git da pasta `root`.
    pub fn new(root: impl Into<PathBuf>) -> Git {
        Git { root: root.into() }
    }

    /// Pasta do repositório.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Result<String, GitError> {
        let mut cmd = command();
        cmd.arg("-C").arg(&self.root).args(args);
        run(cmd)
    }

    /// Se há git instalado.
    pub fn is_installed() -> bool {
        let mut cmd = command();
        cmd.arg("--version");
        run(cmd).is_ok()
    }

    /// Se a pasta é a raiz de um repositório git (ou fica dentro de um).
    pub fn is_repo(&self) -> bool {
        self.git(&["rev-parse", "--is-inside-work-tree"])
            .is_ok_and(|o| o.trim() == "true")
    }

    /// "Initialize Git Repository": `git init` com o branch `main`.
    pub fn init(&self) -> Result<(), GitError> {
        self.git(&["init"])?;
        // `git init -b` só existe a partir do 2.28; isto funciona em todos.
        self.git(&["symbolic-ref", "HEAD", "refs/heads/main"])?;
        Ok(())
    }

    /// Estado do repositório, com a URL do remoto.
    pub fn status(&self) -> Result<Status, GitError> {
        if !self.is_repo() {
            return Err(if Git::is_installed() {
                GitError::NotRepo
            } else {
                GitError::NotInstalled
            });
        }
        let mut status = self
            .git(&[
                "status",
                "--porcelain=v1",
                "--branch",
                "-z",
                "--untracked-files=all",
            ])
            .map(|o| Status::parse(&o))?;
        status.remote = self.remote_url();
        Ok(status)
    }

    /// Commita tudo o que mudou na pasta (`git add -A` e `git commit`).
    pub fn commit_all(&self, message: &str) -> Result<String, GitError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(GitError::Failed("Enter a commit message.".into()));
        }
        self.git(&["add", "-A"])?;
        self.git(&["commit", "-m", message])
            .map(|o| o.lines().next().unwrap_or_default().to_string())
    }

    /// Traz o remoto: `git pull` com merge (nunca rebase, para não reescrever
    /// commits locais). Um conflito volta como erro, e [`Self::status`]
    /// lista os arquivos.
    pub fn pull(&self) -> Result<String, GitError> {
        match self.git(&["pull", "--no-rebase", "--no-edit"]) {
            Ok(o) => Ok(o.trim().lines().last().unwrap_or_default().to_string()),
            Err(e) => {
                let conflicts = self.status().map(|s| s.conflicts).unwrap_or_default();
                if conflicts.is_empty() {
                    Err(e)
                } else {
                    Err(GitError::Failed(format!(
                        "Pull found conflicts in {}. Fix the files (look for <<<<<<<), then Commit.",
                        conflicts.join(", ")
                    )))
                }
            }
        }
    }

    /// Envia os commits. Sem branch remoto ainda, cria em `origin`.
    pub fn push(&self) -> Result<String, GitError> {
        let status = self.status()?;
        if !status.conflicts.is_empty() {
            return Err(GitError::Failed(
                "Fix the conflicts and Commit before pushing.".into(),
            ));
        }
        if status.upstream.is_some() {
            self.git(&["push"])?;
        } else {
            if self.remote_url().is_none() {
                return Err(GitError::Failed(
                    "The workspace has no remote. Use Source Control > Remote... to set the repository URL."
                        .into(),
                ));
            }
            self.git(&["push", "-u", "origin", "HEAD"])?;
        }
        Ok("Pushed.".into())
    }

    /// URL do remoto `origin`, se houver.
    pub fn remote_url(&self) -> Option<String> {
        self.git(&["remote", "get-url", "origin"])
            .ok()
            .map(|o| o.trim().to_string())
            .filter(|u| !u.is_empty())
    }

    /// Define (ou troca) a URL do remoto `origin`.
    pub fn set_remote(&self, url: &str) -> Result<(), GitError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(GitError::Failed("Enter the repository URL.".into()));
        }
        if self.remote_url().is_some() {
            self.git(&["remote", "set-url", "origin", url])?;
        } else {
            self.git(&["remote", "add", "origin", url])?;
        }
        Ok(())
    }

    /// Clona `url` em `dest` (que não pode existir ou deve estar vazia).
    pub fn clone(url: &str, dest: &Path) -> Result<Git, GitError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(GitError::Failed("Enter the repository URL.".into()));
        }
        let mut cmd = command();
        cmd.arg("clone").arg("--").arg(url).arg(dest);
        run(cmd)?;
        Ok(Git::new(dest))
    }
}

/// Nome da pasta para o clone de `url`: o último trecho, sem `.git`.
pub fn repo_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':', '\\']).next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        "workspace".into()
    } else {
        name.into()
    }
}

/// O que uma operação do git devolve à interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// Estado lido.
    Status(Result<Status, GitError>),
    /// Uma ação terminou: a mensagem para a status bar e o estado novo.
    Action {
        what: &'static str,
        result: Result<String, GitError>,
        status: Option<Status>,
    },
    /// Clone terminou.
    Cloned(Result<PathBuf, GitError>),
}

/// Uma operação do git em andamento numa thread separada.
pub struct Job {
    rx: Receiver<Done>,
    /// O que está rodando, para o painel ("Pulling...").
    pub label: &'static str,
}

impl Job {
    /// Roda `work` numa thread.
    pub fn spawn(label: &'static str, work: impl FnOnce() -> Done + Send + 'static) -> Job {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("postpilot-git".into())
            .spawn(move || {
                let _ = tx.send(work());
            })
            .expect("spawn git thread");
        Job { rx, label }
    }

    /// O resultado, quando terminar.
    pub fn poll(&self) -> Option<Done> {
        match self.rx.try_recv() {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Done::Status(Err(GitError::Failed(
                "The git operation stopped unexpectedly.".into(),
            )))),
        }
    }

    /// Espera o resultado (testes).
    pub fn wait(&self) -> Done {
        self.rx.recv().unwrap_or(Done::Status(Err(GitError::Failed(
            "The git operation stopped unexpectedly.".into(),
        ))))
    }
}

/// Roda uma ação e lê o estado depois dela.
pub fn action(
    git: Git,
    what: &'static str,
    f: impl FnOnce(&Git) -> Result<String, GitError> + Send + 'static,
) -> Job {
    Job::spawn(what, move || {
        let result = f(&git);
        let status = git.status().ok();
        Done::Action {
            what,
            result,
            status,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_counts() {
        let s = Status::parse(
            "## main...origin/main [ahead 2, behind 1]\0 M queries/a.sql\0?? queries/b.sql\0",
        );
        assert_eq!(s.branch, "main");
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.changes.len(), 2);
        assert_eq!(s.changes[0].kind(), "Modified");
        assert_eq!(s.changes[1].kind(), "Added");
        assert!(!s.is_clean());
    }

    #[test]
    fn parses_new_repo_renames_and_conflicts() {
        let s = Status::parse("## No commits yet on main\0A  servers.json\0");
        assert_eq!(s.branch, "main");
        assert_eq!(s.upstream, None);
        let s = Status::parse(
            "## main...origin/main [behind 3]\0R  novo.sql\0velho.sql\0UU queries/x.sql\0",
        );
        assert_eq!(s.behind, 3);
        assert_eq!(s.changes.len(), 1);
        assert_eq!(s.changes[0].path, "novo.sql");
        assert_eq!(s.changes[0].kind(), "Renamed");
        assert_eq!(s.conflicts, ["queries/x.sql"]);
        let s = Status::parse("## main\0");
        assert!(s.is_clean());
        assert_eq!((s.ahead, s.behind), (0, 0));
        let s = Status::parse("## main...origin/main [gone]\0");
        assert_eq!((s.ahead, s.behind), (0, 0));
        let s = Status::parse("## Ação ünicode\0 M queries/relatório.sql\0");
        assert_eq!(s.branch, "Ação ünicode");
        assert_eq!(s.changes[0].path, "queries/relatório.sql");
    }

    #[test]
    fn repo_names_from_urls() {
        assert_eq!(
            repo_name("git@github.com:tiago/sql-vendas.git"),
            "sql-vendas"
        );
        assert_eq!(
            repo_name("https://github.com/tiago/sql-vendas"),
            "sql-vendas"
        );
        assert_eq!(
            repo_name("https://github.com/tiago/sql-vendas.git/"),
            "sql-vendas"
        );
        assert_eq!(repo_name("/srv/git/vendas.git"), "vendas");
        assert_eq!(repo_name(""), "workspace");
    }
}
