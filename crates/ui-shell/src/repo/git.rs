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

use pgstudio_i18n::{tr, tr_noop, trf};
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
    /// O Pull parou porque o remoto mudou arquivos que têm alterações
    /// locais ainda não commitadas (os arquivos, relativos à raiz).
    LocalChanges(Vec<String>),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotInstalled => f.write_str(tr(
                "Git is not installed. Install git (on Windows, Git for Windows) to use source control.",
            )),
            GitError::NotRepo => f.write_str(tr("The workspace is not under source control.")),
            GitError::Failed(m) => f.write_str(m),
            GitError::LocalChanges(files) => f.write_str(&trf!(
                "Pull stopped: {files} changed on this computer and in the repository. Commit your changes or discard them, then Pull again.",
                files = files.join(", ")
            )),
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
            "??" => tr_noop("Added"),
            c if c.contains('D') => tr_noop("Deleted"),
            c if c.contains('A') => tr_noop("Added"),
            c if c.contains('R') => tr_noop("Renamed"),
            _ => tr_noop("Modified"),
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
    // Mensagens do git em inglês, qualquer que seja o idioma do sistema:
    // o Pull reconhece pelo texto quando são alterações locais no caminho.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("LC_MESSAGES", "C")
        .env_remove("LANGUAGE")
        .env_remove("LC_ALL")
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
            GitError::Failed(trf!("Could not run git: {e}", e = e))
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

    /// O que o Commit vai gravar, em texto, para a IA escrever a mensagem:
    /// a lista de arquivos e o diff. Não mexe no índice. Arquivos novos
    /// entram com o começo do conteúdo; o cofre de senhas, só com o nome.
    pub fn commit_context(&self) -> Result<(String, String), GitError> {
        /// Quanto de um arquivo novo entra.
        const NEW_FILE: usize = 4000;
        let status = self.status()?;
        let mut changes = String::new();
        let mut diff = String::new();
        // Sem commit ainda (repositório novo), o diff contra HEAD falha:
        // tudo entra como arquivo novo, abaixo.
        if let Ok(tracked) = self.git(&[
            "diff",
            "HEAD",
            "--no-ext-diff",
            "--",
            ".",
            &format!(":(exclude){}", super::vault::FILE),
        ]) {
            diff.push_str(&tracked);
        }
        for c in &status.changes {
            changes.push_str(&format!("{} {}\n", c.kind(), c.path));
            if c.kind() != "Added" || c.path == super::vault::FILE {
                continue;
            }
            // Novo e ainda fora do índice: o diff acima não mostra.
            if diff.contains(&format!("b/{}\n", c.path)) {
                continue;
            }
            let Ok(bytes) = std::fs::read(self.root.join(&c.path)) else {
                continue;
            };
            if bytes.contains(&0) {
                diff.push_str(&format!("\nnew file {} (binary)\n", c.path));
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let mut cut: String = text.chars().take(NEW_FILE).collect();
            if cut.len() < text.len() {
                cut.push_str("\n[...]");
            }
            diff.push_str(&format!("\nnew file {}:\n{cut}\n", c.path));
        }
        for c in &status.conflicts {
            changes.push_str(&format!("Resolved {c}\n"));
        }
        Ok((changes, diff))
    }

    /// Commita tudo o que mudou na pasta (`git add -A` e `git commit`).
    pub fn commit_all(&self, message: &str) -> Result<String, GitError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(GitError::Failed(tr("Enter a commit message.").into()));
        }
        self.git(&["add", "-A"])?;
        self.git(&["commit", "-m", message])
            .map(|o| o.lines().next().unwrap_or_default().to_string())
    }

    /// Traz o remoto: `git pull` com merge (nunca rebase, para não reescrever
    /// commits locais). Um conflito só no `secrets.json` (as duas máquinas
    /// salvaram senhas) é resolvido sozinho, juntando as senhas. Outro
    /// conflito volta como erro, e [`Self::status`] lista os arquivos.
    /// Alterações locais no caminho voltam como [`GitError::LocalChanges`].
    pub fn pull(&self) -> Result<String, GitError> {
        match self.git(&["pull", "--no-rebase", "--no-edit"]) {
            Ok(o) => Ok(o.trim().lines().last().unwrap_or_default().to_string()),
            Err(e) => {
                if let GitError::Failed(m) = &e
                    && let Some(files) = overwritten_files(m)
                {
                    return Err(GitError::LocalChanges(files));
                }
                let mut conflicts = self.status().map(|s| s.conflicts).unwrap_or_default();
                let merged_passwords = conflicts.iter().any(|c| c == super::vault::FILE)
                    && self.merge_passwords().is_ok();
                if merged_passwords {
                    conflicts.retain(|c| c != super::vault::FILE);
                    if conflicts.is_empty() {
                        self.git(&["commit", "--no-edit"])?;
                        return Ok(tr(
                            "merged the passwords saved on this computer with the ones from the repository",
                        )
                        .into());
                    }
                }
                if conflicts.is_empty() {
                    Err(e)
                } else {
                    Err(GitError::Failed(trf!(
                        "Pull found conflicts in {files}. Fix the files (look for <<<<<<<), then Commit.",
                        files = conflicts.join(", ")
                    )))
                }
            }
        }
    }

    /// Resolve o conflito do `secrets.json` juntando as senhas dos dois
    /// lados (veja [`super::vault::merge`]) e marca o arquivo como resolvido.
    fn merge_passwords(&self) -> Result<(), GitError> {
        let file = super::vault::FILE;
        let stage = |n: u8| self.git(&["show", &format!(":{n}:{file}")]);
        let base = stage(1).ok();
        let (ours, theirs) = (stage(2)?, stage(3)?);
        let merged = super::vault::merge(base.as_deref(), &ours, &theirs).ok_or_else(|| {
            GitError::Failed(
                tr("The passwords of the two sides use different master passwords.").into(),
            )
        })?;
        std::fs::write(self.root.join(file), merged).map_err(|e| {
            GitError::Failed(trf!("Could not save {file}: {e}", file = file, e = e))
        })?;
        self.git(&["add", "--", file]).map(|_| ())
    }

    /// Descarta as alterações locais destes arquivos: os alterados e os
    /// apagados voltam ao último commit; os novos são apagados do disco.
    /// Devolve quantos arquivos foram descartados.
    pub fn discard(&self, paths: &[String]) -> Result<usize, GitError> {
        // Tira tudo do index antes (o PostPilot sempre commita tudo): assim
        // só há dois casos, arquivo do último commit ou arquivo novo. Num
        // repositório sem commits o reset falha e tudo é arquivo novo.
        let _ = self.git(&["reset", "-q"]);
        let status = self.status()?;
        let mut done = 0;
        for change in status.changes.iter().filter(|c| paths.contains(&c.path)) {
            if change.code == "??" {
                let target = self.root.join(&change.path);
                let removed = if target.is_dir() {
                    std::fs::remove_dir_all(&target)
                } else {
                    std::fs::remove_file(&target)
                };
                removed.map_err(|e| {
                    GitError::Failed(trf!(
                        "Could not delete {path}: {e}",
                        path = change.path,
                        e = e
                    ))
                })?;
            } else {
                self.git(&["checkout", "HEAD", "--", &change.path])?;
            }
            done += 1;
        }
        Ok(done)
    }

    /// Envia os commits. Sem branch remoto ainda, cria em `origin`.
    pub fn push(&self) -> Result<String, GitError> {
        let status = self.status()?;
        if !status.conflicts.is_empty() {
            return Err(GitError::Failed(
                tr("Fix the conflicts and Commit before pushing.").into(),
            ));
        }
        if status.upstream.is_some() {
            self.git(&["push"])?;
        } else {
            if self.remote_url().is_none() {
                return Err(GitError::Failed(
                    tr(
                        "The workspace has no remote. Use Source Control > Remote... to set the repository URL.",
                    )
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
            return Err(GitError::Failed(tr("Enter the repository URL.").into()));
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
            return Err(GitError::Failed(tr("Enter the repository URL.").into()));
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

/// Os arquivos de "Your local changes to the following files would be
/// overwritten by merge" (ou a variante dos arquivos novos), se a mensagem
/// do git é essa.
fn overwritten_files(message: &str) -> Option<Vec<String>> {
    if !message.contains("would be overwritten by") {
        return None;
    }
    let files: Vec<String> = message
        .lines()
        .filter(|l| l.starts_with('\t') || l.starts_with("    "))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    (!files.is_empty()).then_some(files)
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
                tr("The git operation stopped unexpectedly.").into(),
            )))),
        }
    }

    /// Espera o resultado (testes).
    pub fn wait(&self) -> Done {
        self.rx.recv().unwrap_or(Done::Status(Err(GitError::Failed(
            tr("The git operation stopped unexpectedly.").into(),
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
    fn reads_the_files_that_block_a_pull() {
        let m = "error: Your local changes to the following files would be overwritten by merge:\n\tsecrets.json\n\tqueries/a b.sql\nPlease commit your changes or stash them before you merge.\nAborting";
        assert_eq!(
            overwritten_files(m).unwrap(),
            ["secrets.json", "queries/a b.sql"]
        );
        assert_eq!(overwritten_files("fatal: unable to access"), None);
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
