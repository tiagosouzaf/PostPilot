//! A faixa "Source Control" do Workspace Explorer: branch, o que mudou e os
//! botões Pull, Commit e Push, como o Team Explorer do Visual Studio.

use std::collections::VecDeque;
use std::path::Path;

use eframe::egui;
use egui_phosphor::regular as icon;

use super::git::{self, Done, Git, GitError, Job, Status};

/// "Commit" esperando a mensagem.
struct CommitDialog {
    message: String,
    error: Option<String>,
}

/// "Remote..." esperando a URL.
struct RemoteDialog {
    url: String,
    error: Option<String>,
}

type Work = Box<dyn FnOnce(&Git) -> Result<String, GitError> + Send>;

/// Git do workspace aberto.
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
        self.job = Some(Job::spawn("Reading status...", move || {
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
        self.start("Initializing...", |g| {
            g.init().map(|()| "Initialized the git repository.".into())
        });
    }

    /// Pull.
    pub fn pull(&mut self) {
        self.start("Pulling...", |g| {
            g.pull().map(|m| {
                if m.is_empty() {
                    "Pulled.".into()
                } else {
                    format!("Pulled: {m}")
                }
            })
        });
    }

    /// Push.
    pub fn push(&mut self) {
        self.start("Pushing...", Git::push);
    }

    /// Commit de tudo o que mudou, e Push depois se `push`.
    pub fn commit(&mut self, message: &str, push: bool) {
        let message = message.to_string();
        self.push_after_commit = push;
        self.start("Committing...", move |g| {
            g.commit_all(&message).map(|m| format!("Committed: {m}"))
        });
    }

    /// Define a URL do remoto `origin`.
    pub fn set_remote(&mut self, url: &str) {
        let url = url.to_string();
        self.start("Setting remote...", move |g| {
            g.set_remote(&url).map(|()| format!("Remote set to {url}."))
        });
    }

    /// Abre "Commit..." (testes e o botão).
    pub fn open_commit(&mut self) {
        self.commit = Some(CommitDialog {
            message: String::new(),
            error: None,
        });
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
        self.commit.is_some() || self.remote.is_some()
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
                    }
                }
                // Pull (mesmo com conflito) e init mexem nos arquivos.
                if what == "Pulling..." || what == "Initializing..." {
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
                ui.weak(job.label);
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        match &self.state {
            None => {}
            Some(Err(GitError::NotRepo)) => {
                ui.horizontal(|ui| {
                    ui.weak("Not under source control.");
                    if ui.small_button("Initialize Git").clicked() {
                        self.init();
                    }
                });
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().warn_fg_color, e.to_string());
            }
            Some(Ok(status)) => {
                let status = status.clone();
                ui.horizontal_wrapped(|ui| {
                    let branch = if status.branch.is_empty() {
                        "(detached)".to_string()
                    } else {
                        status.branch.clone()
                    };
                    ui.label(format!("{} {branch}", icon::GIT_BRANCH))
                        .on_hover_text(status.remote.clone().unwrap_or_else(|| "No remote".into()));
                    if status.ahead > 0 || status.behind > 0 {
                        ui.label(format!(
                            "{}{} {}{}",
                            icon::ARROW_UP,
                            status.ahead,
                            icon::ARROW_DOWN,
                            status.behind
                        ))
                        .on_hover_text("Commits to push / to pull");
                    }
                    let n = status.changes.len();
                    if n > 0 {
                        ui.weak(if n == 1 {
                            "1 change".to_string()
                        } else {
                            format!("{n} changes")
                        })
                        .on_hover_text(
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
                        format!("Conflicts: {}", status.conflicts.join(", ")),
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .small_button(format!("{} Pull", icon::ARROW_DOWN))
                        .on_hover_text("Bring the changes from the remote")
                        .clicked()
                    {
                        self.pull();
                    }
                    if ui
                        .add_enabled(
                            !status.is_clean(),
                            egui::Button::new(format!("{} Commit...", icon::CHECK)).small(),
                        )
                        .clicked()
                    {
                        self.open_commit();
                    }
                    if ui
                        .small_button(format!("{} Push", icon::ARROW_UP))
                        .on_hover_text("Send your commits to the remote")
                        .clicked()
                    {
                        self.push();
                    }
                    ui.menu_button(icon::DOTS_THREE, |ui| {
                        if ui.button("Remote...").clicked() {
                            self.open_remote();
                            ui.close();
                        }
                        if ui.button("Refresh").clicked() {
                            self.refresh();
                            ui.close();
                        }
                    });
                });
            }
        }
    }

    /// Desenha "Commit" e "Remote...". Devolve uma mensagem para a status
    /// bar quando há.
    pub fn show_windows(&mut self, ctx: &egui::Context) -> Option<String> {
        if self.poll() {
            ctx.request_repaint();
        }
        let changes = self.status().map(|s| s.changes.clone()).unwrap_or_default();
        let conflicts = self
            .status()
            .map(|s| s.conflicts.clone())
            .unwrap_or_default();
        let mut commit: Option<bool> = None;
        let mut cancel = false;
        if let Some(dialog) = &mut self.commit {
            egui::Modal::new(egui::Id::new("workspace_commit")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.heading("Commit");
                ui.label("Changes:");
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for c in &changes {
                                ui.label(format!("{:<9} {}", c.kind(), c.path));
                            }
                            for c in &conflicts {
                                ui.label(format!("Resolved  {c}"));
                            }
                        });
                });
                ui.label("Message:");
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.message)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                if let Some(e) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Commit and Push").clicked() {
                        commit = Some(true);
                    }
                    if ui.button("Commit").clicked() {
                        commit = Some(false);
                    }
                });
            });
        }
        if cancel {
            self.commit = None;
        } else if let Some(push) = commit
            && let Some(dialog) = &mut self.commit
        {
            if dialog.message.trim().is_empty() {
                dialog.error = Some("Enter a commit message.".into());
            } else {
                let message = dialog.message.clone();
                self.commit = None;
                self.commit(&message, push);
            }
        }

        let mut ok = false;
        let mut cancel = false;
        if let Some(dialog) = &mut self.remote {
            egui::Modal::new(egui::Id::new("workspace_remote")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.heading("Remote");
                ui.label("Repository URL (origin), for example git@github.com:you/sql.git:");
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
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("OK").clicked() {
                        ok = true;
                    }
                });
            });
        }
        if cancel {
            self.remote = None;
        } else if ok && let Some(dialog) = &mut self.remote {
            if dialog.url.trim().is_empty() {
                dialog.error = Some("Enter the repository URL.".into());
            } else {
                let url = dialog.url.trim().to_string();
                self.remote = None;
                self.set_remote(&url);
            }
        }
        self.message.take()
    }
}
