//! A faixa "Source Control" do Workspace Explorer: branch, o que mudou e os
//! botões Pull, Commit e Push, como o Team Explorer do Visual Studio.

use pgstudio_i18n::{tr, tr_noop, trf};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc;

use pgstudio_automation::llm::Backend;

use eframe::egui;
use egui_phosphor::regular as icon;

use super::git::{self, Done, Git, GitError, Job, Status};

/// "Commit" esperando a mensagem.
struct CommitDialog {
    message: String,
    error: Option<String>,
    /// A IA escrevendo a mensagem; a resposta chega aqui.
    generating: Option<mpsc::Receiver<Result<String, String>>>,
}

/// Idioma da interface, como a IA deve escrever a mensagem.
fn language_for_ai() -> &'static str {
    match pgstudio_i18n::language() {
        pgstudio_i18n::Language::PortugueseBrazil => "Brazilian Portuguese",
        _ => "English",
    }
}

/// "Remote..." esperando a URL.
struct RemoteDialog {
    url: String,
    error: Option<String>,
}

type Work = Box<dyn FnOnce(&Git) -> Result<String, GitError> + Send>;

/// Git do workspace aberto.
/// O que o "Changes" está pedindo para confirmar.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Discard {
    One(git::Change),
    All,
}

/// "Changes": os arquivos alterados, com descartar por arquivo e tudo.
#[derive(Default)]
struct ChangesDialog {
    /// Por que abriu sozinho (o Pull parou em alterações locais).
    note: Option<String>,
    confirm: Option<Discard>,
}

pub struct SourceControl {
    git: Git,
    /// Último estado lido; `None` antes da primeira leitura.
    state: Option<Result<Status, GitError>>,
    job: Option<Job>,
    /// Ações pedidas enquanto outra rodava, em ordem.
    queue: VecDeque<(&'static str, Work)>,
    /// Ler o estado de novo quando a operação atual terminar.
    stale: bool,
    /// Depois desta ação, fazer Push (Commit and Push).
    push_after_commit: bool,
    commit: Option<CommitDialog>,
    remote: Option<RemoteDialog>,
    changes: Option<ChangesDialog>,
    message: Option<String>,
    /// Arquivos do workspace podem ter mudado (pull): o painel relê.
    files_changed: bool,
}

impl SourceControl {
    /// Git da pasta do workspace; já começa a ler o estado.
    pub fn new(root: &Path) -> Self {
        let mut sc = SourceControl {
            git: Git::new(root),
            state: None,
            job: None,
            queue: VecDeque::new(),
            stale: false,
            push_after_commit: false,
            commit: None,
            remote: None,
            changes: None,
            message: None,
            files_changed: false,
        };
        sc.refresh();
        sc
    }

    /// Lê o estado de novo (se nada estiver rodando).
    pub fn refresh(&mut self) {
        if self.job.is_some() {
            self.stale = true;
            return;
        }
        self.stale = false;
        let git = self.git.clone();
        self.job = Some(Job::spawn(tr_noop("Reading status..."), move || {
            Done::Status(git.status())
        }));
    }

    /// O último estado lido.
    pub fn state(&self) -> Option<&Result<Status, GitError>> {
        self.state.as_ref()
    }

    /// O estado, se a pasta é um repositório.
    pub fn status(&self) -> Option<&Status> {
        self.state.as_ref().and_then(|s| s.as_ref().ok())
    }

    /// Se há uma operação rodando.
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// Se uma operação mudou os arquivos (o painel deve reler). Zera o aviso.
    pub fn take_files_changed(&mut self) -> bool {
        std::mem::take(&mut self.files_changed)
    }

    /// Mensagem para a status bar, se houver.
    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// Roda a ação agora ou, se outra estiver rodando, logo depois dela.
    fn start(
        &mut self,
        what: &'static str,
        f: impl FnOnce(&Git) -> Result<String, GitError> + Send + 'static,
    ) {
        if self.job.is_some() {
            self.queue.push_back((what, Box::new(f)));
            return;
        }
        self.job = Some(git::action(self.git.clone(), what, f));
    }

    /// Depois que uma operação termina: a próxima da fila ou a releitura.
    fn next(&mut self) {
        if self.job.is_some() {
            return;
        }
        if let Some((what, f)) = self.queue.pop_front() {
            self.stale = false;
            self.job = Some(git::action(self.git.clone(), what, f));
        } else if self.stale {
            self.refresh();
        }
    }

    /// "Initialize Git Repository".
    pub fn init(&mut self) {
        self.start(tr_noop("Initializing..."), |g| {
            g.init()
                .map(|()| tr("Initialized the git repository.").into())
        });
    }

    /// Pull.
    pub fn pull(&mut self) {
        self.start(tr_noop("Pulling..."), |g| {
            g.pull().map(|m| {
                if m.is_empty() {
                    tr("Pulled.").into()
                } else {
                    trf!("Pulled: {m}", m = m)
                }
            })
        });
    }

    /// Push.
    pub fn push(&mut self) {
        self.start(tr_noop("Pushing..."), Git::push);
    }

    /// Commit de tudo o que mudou, e Push depois se `push`.
    pub fn commit(&mut self, message: &str, push: bool) {
        let message = message.to_string();
        self.push_after_commit = push;
        self.start(tr_noop("Committing..."), move |g| {
            g.commit_all(&message)
                .map(|m| trf!("Committed: {m}", m = m))
        });
    }

    /// Abre "Changes" (o botão das alterações e os testes).
    pub fn open_changes(&mut self) {
        self.changes = Some(ChangesDialog::default());
    }

    /// Se "Changes" está aberto.
    pub fn is_showing_changes(&self) -> bool {
        self.changes.is_some()
    }

    /// Descarta as alterações locais destes arquivos (os novos são
    /// apagados).
    pub fn discard(&mut self, paths: Vec<String>) {
        self.start(tr_noop("Discarding..."), move |g| {
            g.discard(&paths).map(|n| {
                if n == 1 {
                    tr("Discarded the changes in 1 file.").to_string()
                } else {
                    trf!("Discarded the changes in {n} files.", n = n)
                }
            })
        });
    }

    /// Descarta as alterações de todos os arquivos que já estavam no
    /// repositório. Os arquivos novos ficam.
    pub fn discard_all(&mut self) {
        let paths = self
            .status()
            .map(|s| {
                s.changes
                    .iter()
                    .filter(|c| c.code != "??")
                    .map(|c| c.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.discard(paths);
    }

    /// Define a URL do remoto `origin`.
    pub fn set_remote(&mut self, url: &str) {
        let url = url.to_string();
        self.start(tr_noop("Setting remote..."), move |g| {
            g.set_remote(&url)
                .map(|()| trf!("Remote set to {url}.", url = url))
        });
    }

    /// Abre "Commit..." (testes e o botão).
    pub fn open_commit(&mut self) {
        self.commit = Some(CommitDialog {
            message: String::new(),
            error: None,
            generating: None,
        });
    }

    /// O botão "Generate with AI" do Commit: pede a `backend` uma mensagem
    /// para o que mudou. Roda numa thread; a resposta preenche o campo.
    pub fn generate_commit_message(&mut self, ctx: &egui::Context, backend: &Backend) {
        let Some(dialog) = &mut self.commit else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (git, backend, ctx) = (self.git.clone(), backend.clone(), ctx.clone());
        let language = language_for_ai();
        std::thread::spawn(move || {
            let result = git
                .commit_context()
                .map_err(|e| e.to_string())
                .and_then(|(changes, diff)| {
                    backend.ask(
                        &pgstudio_automation::commit::system_prompt(language),
                        &pgstudio_automation::commit::user_prompt(&changes, &diff),
                        &std::env::temp_dir().join("postpilot-ai"),
                    )
                })
                .map(|answer| pgstudio_automation::commit::clean(&answer));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        dialog.error = None;
        dialog.generating = Some(rx);
    }

    /// A mensagem no diálogo de Commit aberto (testes).
    pub fn commit_message(&self) -> Option<&str> {
        self.commit.as_ref().map(|d| d.message.as_str())
    }

    /// O erro mostrado no diálogo de Commit (testes).
    pub fn commit_error(&self) -> Option<&str> {
        self.commit.as_ref().and_then(|d| d.error.as_deref())
    }

    /// Se a IA ainda está escrevendo a mensagem.
    pub fn is_generating(&self) -> bool {
        self.commit.as_ref().is_some_and(|d| d.generating.is_some())
    }

    /// Recebe a mensagem da IA, se chegou.
    fn poll_generated(&mut self) {
        let Some(dialog) = &mut self.commit else {
            return;
        };
        let Some(rx) = &dialog.generating else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(message)) if !message.is_empty() => {
                dialog.message = message;
                dialog.generating = None;
            }
            Ok(Ok(_)) => {
                dialog.error = Some(tr("The AI answered with an empty message.").into());
                dialog.generating = None;
            }
            Ok(Err(e)) => {
                dialog.error = Some(trf!(
                    "The AI could not write the message: {error}",
                    error = e
                ));
                dialog.generating = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => dialog.generating = None,
        }
    }

    /// Abre "Remote..." (testes e o menu).
    pub fn open_remote(&mut self) {
        let url = self
            .status()
            .and_then(|s| s.remote.clone())
            .unwrap_or_default();
        self.remote = Some(RemoteDialog { url, error: None });
    }

    /// Se algum diálogo está aberto.
    pub fn has_dialog(&self) -> bool {
        self.commit.is_some() || self.remote.is_some() || self.changes.is_some()
    }

    /// Trata o fim da operação em andamento. Devolve `true` quando algo
    /// terminou.
    pub fn poll(&mut self) -> bool {
        let Some(done) = self.job.as_ref().and_then(Job::poll) else {
            return false;
        };
        self.job = None;
        self.apply(done);
        self.next();
        true
    }

    /// Espera a operação em andamento (testes).
    pub fn wait(&mut self) {
        while let Some(job) = self.job.take() {
            let done = job.wait();
            self.apply(done);
            self.next();
        }
    }

    fn apply(&mut self, done: Done) {
        match done {
            Done::Status(state) => self.state = Some(state),
            Done::Action {
                what,
                result,
                status,
            } => {
                if let Some(s) = status {
                    self.state = Some(Ok(s));
                }
                match result {
                    Ok(m) => {
                        self.message = Some(m);
                        if what == "Committing..." && self.push_after_commit {
                            self.push_after_commit = false;
                            self.push();
                        }
                    }
                    Err(e) => {
                        self.push_after_commit = false;
                        self.message = Some(e.to_string());
                        // O Pull parou em alterações locais: mostra quais
                        // são, com o botão de descartar ao lado.
                        if matches!(e, GitError::LocalChanges(_)) {
                            self.changes = Some(ChangesDialog {
                                note: Some(e.to_string()),
                                confirm: None,
                            });
                        }
                    }
                }
                // Pull (mesmo com conflito), init e descartar mexem nos
                // arquivos.
                if what == "Pulling..." || what == "Initializing..." || what == "Discarding..." {
                    self.files_changed = true;
                }
            }
            Done::Cloned(_) => {}
        }
    }

    /// Desenha a faixa.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.poll() {
            ui.ctx().request_repaint();
        }
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(tr(job.label));
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        match &self.state {
            None => {}
            Some(Err(GitError::NotRepo)) => {
                ui.horizontal(|ui| {
                    ui.weak(tr("Not under source control."));
                    if ui.small_button(tr("Initialize Git")).clicked() {
                        self.init();
                    }
                });
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().warn_fg_color, e.to_string());
            }
            Some(Ok(status)) => {
                let status = status.clone();
                let mut open_changes = false;
                ui.horizontal_wrapped(|ui| {
                    let branch = if status.branch.is_empty() {
                        tr("(detached)").to_string()
                    } else {
                        status.branch.clone()
                    };
                    ui.label(format!("{} {branch}", icon::GIT_BRANCH)) // i18n-ignore
                        .on_hover_text(
                            status
                                .remote
                                .clone()
                                .unwrap_or_else(|| tr("No remote").into()),
                        );
                    if status.ahead > 0 || status.behind > 0 {
                        ui.label(format!(
                            "{}{} {}{}",
                            icon::ARROW_UP,
                            status.ahead,
                            icon::ARROW_DOWN,
                            status.behind
                        ))
                        .on_hover_text(tr("Commits to push / to pull"));
                    }
                    let n = status.changes.len();
                    if n > 0 {
                        let text = if n == 1 {
                            tr("1 change").to_string()
                        } else {
                            trf!("{n} changes", n = n)
                        };
                        let link = ui.link(text);
                        if link.clicked() {
                            open_changes = true;
                        }
                        link.on_hover_text(
                            status
                                .changes
                                .iter()
                                .map(|c| format!("{} {}", c.kind(), c.path))
                                .collect::<Vec<_>>()
                                .join("\n"),
                        );
                    }
                });
                if !status.conflicts.is_empty() {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        trf!("Conflicts: {files}", files = status.conflicts.join(", ")),
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .small_button(format!("{} {}", icon::ARROW_DOWN, tr("Pull")))
                        .on_hover_text(tr("Bring the changes from the remote"))
                        .clicked()
                    {
                        self.pull();
                    }
                    if ui
                        .add_enabled(
                            !status.is_clean(),
                            egui::Button::new(format!("{} {}", icon::CHECK, tr("Commit...")))
                                .small(),
                        )
                        .clicked()
                    {
                        self.open_commit();
                    }
                    if ui
                        .small_button(format!("{} {}", icon::ARROW_UP, tr("Push")))
                        .on_hover_text(tr("Send your commits to the remote"))
                        .clicked()
                    {
                        self.push();
                    }
                    ui.menu_button(icon::DOTS_THREE, |ui| {
                        if ui
                            .add_enabled(
                                !status.changes.is_empty(),
                                egui::Button::new(tr("Changes and Discard...")),
                            )
                            .clicked()
                        {
                            open_changes = true;
                            ui.close();
                        }
                        if ui.button(tr("Remote...")).clicked() {
                            self.open_remote();
                            ui.close();
                        }
                        if ui.button(tr("Refresh")).clicked() {
                            self.refresh();
                            ui.close();
                        }
                    });
                });
                if open_changes {
                    self.open_changes();
                }
            }
        }
    }

    /// Desenha "Commit" e "Remote...". Devolve uma mensagem para a status
    /// bar quando há. `ai` é a IA conectada, se houver: com ela o Commit
    /// oferece escrever a mensagem.
    pub fn show_windows(&mut self, ctx: &egui::Context, ai: Option<&Backend>) -> Option<String> {
        if self.poll() {
            ctx.request_repaint();
        }
        self.poll_generated();
        let mut generate = false;
        let changes = self.status().map(|s| s.changes.clone()).unwrap_or_default();
        let conflicts = self
            .status()
            .map(|s| s.conflicts.clone())
            .unwrap_or_default();
        let mut commit: Option<bool> = None;
        let mut cancel = false;
        if let Some(dialog) = &mut self.commit {
            crate::theme::modal(egui::Id::new("workspace_commit")).show(ctx, |ui| {
                crate::fit::dialog_width(ui, 460.0);
                ui.heading(tr("Commit"));
                ui.label(tr("Changes:"));
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for c in &changes {
                                ui.label(format!("{:<11} {}", tr(c.kind()), c.path)); // i18n-ignore
                            }
                            for c in &conflicts {
                                ui.label(trf!("Resolved  {c}", c = c));
                            }
                        });
                });
                let generating = dialog.generating.is_some();
                ui.horizontal(|ui| {
                    ui.label(tr("Message:"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = ui.add_enabled(
                            ai.is_some() && !generating,
                            egui::Button::new(format!(
                                "{} {}",
                                egui_phosphor::regular::SPARKLE,
                                tr("Generate with AI")
                            ))
                            .small(),
                        );
                        let button = match ai {
                            Some(backend) => button.on_hover_text(trf!(
                                "{name} writes the message from the changes. The list of files and the diff are sent to it.",
                                name = backend.label()
                            )),
                            None => button.on_disabled_hover_text(tr(
                                "No AI is connected. Install Claude Code or add a provider in Tools > AI Providers.",
                            )),
                        };
                        if button.clicked() {
                            generate = true;
                        }
                        if generating {
                            ui.weak(tr("Writing the message…"));
                            ui.spinner();
                        }
                    });
                });
                ui.add_enabled(
                    !generating,
                    egui::TextEdit::multiline(&mut dialog.message)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("Commit and Push")).clicked() {
                        commit = Some(true);
                    }
                    if ui.button(tr("Commit")).clicked() {
                        commit = Some(false);
                    }
                });
            });
        }
        if generate && let Some(backend) = ai {
            self.generate_commit_message(ctx, backend);
        }
        if cancel {
            self.commit = None;
        } else if self.is_generating() {
            // Commit só depois que a mensagem chegar.
        } else if let Some(push) = commit
            && let Some(dialog) = &mut self.commit
        {
            if dialog.message.trim().is_empty() {
                dialog.error = Some(tr("Enter a commit message.").into());
            } else {
                let message = dialog.message.clone();
                self.commit = None;
                self.commit(&message, push);
            }
        }

        let mut close = false;
        let mut discard: Option<Discard> = None;
        if let Some(dialog) = &mut self.changes {
            let busy = self.job.is_some();
            crate::theme::modal(egui::Id::new("workspace_changes")).show(ctx, |ui| {
                crate::fit::dialog_width(ui, 480.0);
                ui.heading(tr("Changes"));
                if let Some(note) = &dialog.note {
                    ui.colored_label(ui.visuals().warn_fg_color, note);
                    ui.add_space(4.0);
                }
                if let Some(confirm) = dialog.confirm.clone() {
                    ui.label(match &confirm {
                        Discard::One(c) if c.code == "??" => trf!(
                            "Delete the new file {path}? This cannot be undone.",
                            path = c.path
                        ),
                        Discard::One(c) => trf!(
                            "Discard your changes in {path} and go back to the last commit? This cannot be undone.",
                            path = c.path
                        ),
                        Discard::All => tr("Discard your changes in all the files and go back to the last commit? New files are kept. This cannot be undone.").to_string(),
                    });
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            dialog.confirm = None;
                        }
                        if ui.button(tr("Discard")).clicked() {
                            discard = Some(confirm);
                            dialog.confirm = None;
                        }
                    });
                    return;
                }
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(tr("Discarding..."));
                    });
                } else if changes.is_empty() {
                    ui.weak(tr("No local changes."));
                }
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for c in &changes {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(!busy, egui::Button::new(tr("Discard")).small())
                                    .on_hover_text(if c.code == "??" {
                                        tr("Delete this new file")
                                    } else {
                                        tr("Go back to the last commit of this file")
                                    })
                                    .clicked()
                                {
                                    dialog.confirm = Some(Discard::One(c.clone()));
                                }
                                ui.weak(tr(c.kind()));
                                ui.label(&c.path);
                            });
                        }
                    });
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Close")).clicked() {
                        close = true;
                    }
                    let tracked = changes.iter().any(|c| c.code != "??");
                    if ui
                        .add_enabled(tracked && !busy, egui::Button::new(tr("Discard All...")))
                        .clicked()
                    {
                        dialog.confirm = Some(Discard::All);
                    }
                });
            });
        }
        if close {
            self.changes = None;
        }
        match discard {
            Some(Discard::One(c)) => self.discard(vec![c.path]),
            Some(Discard::All) => self.discard_all(),
            None => {}
        }

        let mut ok = false;
        let mut cancel = false;
        if let Some(dialog) = &mut self.remote {
            crate::theme::modal(egui::Id::new("workspace_remote")).show(ctx, |ui| {
                crate::fit::dialog_width(ui, 460.0);
                ui.heading(tr("Remote"));
                ui.label(tr(
                    "Repository URL (origin), for example git@github.com:you/sql.git:",
                ));
                let edit = ui
                    .add(egui::TextEdit::singleline(&mut dialog.url).desired_width(f32::INFINITY));
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(tr("OK")).clicked() {
                        ok = true;
                    }
                });
            });
        }
        if cancel {
            self.remote = None;
        } else if ok && let Some(dialog) = &mut self.remote {
            if dialog.url.trim().is_empty() {
                dialog.error = Some(tr("Enter the repository URL.").into());
            } else {
                let url = dialog.url.trim().to_string();
                self.remote = None;
                self.set_remote(&url);
            }
        }
        self.message.take()
    }
}
