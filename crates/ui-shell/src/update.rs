//! "Check for Updates": avisa de versão nova e, onde dá, se atualiza.
//!
//! A checagem roda numa thread e manda o resultado por um canal; a UI só lê
//! o canal a cada frame. Veja `pgstudio_updater` para o que é enviado e como
//! cada tipo de instalação se atualiza.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use pgstudio_core::{APP_NAME, CommandId, VERSION};
use pgstudio_updater::{
    InstallKind, Installed, Progress, REPOSITORY, Release, UpdateSettings, aur_helper,
};

/// Em que pé está a atualização.
#[derive(Debug)]
pub enum State {
    /// Nada consultado nesta sessão.
    Idle,
    Checking,
    UpToDate,
    /// O repositório ainda não publicou nenhum Release.
    NoRelease,
    Available(Release),
    Downloading(Release),
    /// A versão nova está instalada; falta reabrir o app.
    Installed(Release, Installed),
    Failed(String),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    Installed(Result<Installed, String>),
}

/// Estado da atualização do app.
pub struct Updater {
    settings: UpdateSettings,
    kind: InstallKind,
    helper: Option<&'static str>,
    state: State,
    /// Se a checagem automática foi pedida nesta abertura (a manual ignora).
    network: bool,
    /// Se a checagem atual foi pedida no menu: aí o resultado abre a janela.
    manual: bool,
    rx: Option<Receiver<Msg>>,
    progress: Arc<Progress>,
    open: bool,
    /// Programa a abrir quando o app fechar ("Restart now").
    restart: Option<PathBuf>,
}

impl Default for Updater {
    /// Sem arquivo de preferências e sem checagem ao abrir (testes,
    /// screenshots, medição). "Check for Updates…" ainda funciona.
    fn default() -> Self {
        Updater::new(UpdateSettings::in_memory(), InstallKind::Other)
    }
}

fn user_agent() -> String {
    format!("{APP_NAME}/{VERSION}")
}

impl Updater {
    pub fn new(settings: UpdateSettings, kind: InstallKind) -> Self {
        let helper = matches!(kind, InstallKind::Pacman(_))
            .then(aur_helper)
            .flatten();
        Updater {
            settings,
            kind,
            helper,
            state: State::Idle,
            network: true,
            manual: false,
            rx: None,
            progress: Arc::new(Progress::default()),
            open: false,
            restart: None,
        }
    }

    /// As preferências salvas em `<config>/.../update.json` e a instalação
    /// detectada.
    pub fn detect(settings_file: PathBuf) -> Self {
        Updater::new(UpdateSettings::open(settings_file), InstallKind::detect())
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn install_kind(&self) -> &InstallKind {
        &self.kind
    }

    /// Checa ao abrir o app, se a preferência estiver ligada e já tiver
    /// passado o intervalo desde a última checagem.
    pub fn check_if_due(&mut self, ctx: &egui::Context) {
        if self.network && self.settings.due(SystemTime::now()) {
            self.check(ctx, false);
        }
    }

    /// "Check for Updates…": abre a janela e consulta agora.
    pub fn check_now(&mut self, ctx: &egui::Context) {
        self.open = true;
        if !matches!(self.state, State::Checking | State::Downloading(_)) {
            self.check(ctx, true);
        }
    }

    fn check(&mut self, ctx: &egui::Context, manual: bool) {
        self.manual = manual;
        self.state = State::Checking;
        self.settings.mark_checked(SystemTime::now());
        let _ = self.settings.save();
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = pgstudio_updater::fetch_latest(REPOSITORY, &user_agent());
            let _ = tx.send(Msg::Checked(result));
            ctx.request_repaint();
        });
    }

    fn start_install(&mut self, ctx: &egui::Context, release: Release) {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.progress = Arc::new(Progress::default());
        let progress = self.progress.clone();
        let kind = self.kind.clone();
        let ctx2 = ctx.clone();
        let r = release.clone();
        std::thread::spawn(move || {
            // Repinta durante o download para a barra de progresso andar.
            let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let ticker = {
                let (done, ctx) = (done.clone(), ctx2.clone());
                std::thread::spawn(move || {
                    while !done.load(std::sync::atomic::Ordering::Relaxed) {
                        ctx.request_repaint();
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                })
            };
            let result = pgstudio_updater::install(&kind, &r, &user_agent(), &progress);
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = ticker.join();
            let _ = tx.send(Msg::Installed(result));
            ctx2.request_repaint();
        });
        self.state = State::Downloading(release);
    }

    /// Lê o resultado da thread, se já chegou. Chame a cada frame.
    pub fn poll(&mut self) {
        let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.rx = None;
        let state = std::mem::replace(&mut self.state, State::Idle);
        self.state = match (msg, state) {
            (Msg::Checked(Ok(Some(r))), _) if r.is_newer_than(VERSION) => {
                let skipped = self.settings.skipped_version.as_deref() == Some(&r.version.to_string());
                if skipped && !self.manual {
                    State::UpToDate
                } else {
                    State::Available(r)
                }
            }
            (Msg::Checked(Ok(Some(_))), _) => State::UpToDate,
            (Msg::Checked(Ok(None)), _) => State::NoRelease,
            // Uma checagem automática que falhou (sem rede, por exemplo)
            // não incomoda; a manual mostra o erro.
            (Msg::Checked(Err(_)), _) if !self.manual => State::Idle,
            (Msg::Checked(Err(e)), _) => State::Failed(e),
            (Msg::Installed(Ok(done)), State::Downloading(r)) => State::Installed(r, done),
            (Msg::Installed(Ok(_)), other) => other,
            (Msg::Installed(Err(e)), _) => State::Failed(e),
        };
    }

    /// Versão nova a anunciar na status bar.
    pub fn available(&self) -> Option<&Release> {
        match &self.state {
            State::Available(r) | State::Downloading(r) | State::Installed(r, _) => Some(r),
            _ => None,
        }
    }

    /// O aviso na status bar, à direita da versão. Clicar abre a janela.
    pub fn status_badge(&mut self, ui: &mut egui::Ui, color: egui::Color32) {
        let text = match &self.state {
            State::Available(r) => format!("Update available: {}", r.version),
            State::Downloading(_) => "Downloading update…".to_string(),
            State::Installed(..) => "Restart to update".to_string(),
            _ => return,
        };
        let badge = egui::Button::new(RichText::new(text).color(color).strong()).frame(false);
        if ui.add(badge).on_hover_text("Check for Updates").clicked() {
            self.open = true;
        }
        ui.separator();
    }

    /// Linha do "About" sobre a atualização.
    pub fn about_line(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            match &self.state {
                State::Available(r) => {
                    ui.label(format!("Version {} is available.", r.version));
                }
                State::UpToDate => {
                    ui.label("This is the latest version.");
                }
                _ => {}
            }
            if ui.link("Check for Updates…").clicked() {
                self.check_now(ctx);
            }
        });
    }

    /// Programa a reabrir depois que o app fechar.
    pub fn take_restart(&mut self) -> Option<PathBuf> {
        self.restart.take()
    }

    /// Ao fechar o app: reabre a versão nova, se o usuário pediu.
    pub fn on_close(&mut self) {
        if let Some(program) = self.take_restart() {
            if let Err(e) = std::process::Command::new(&program).spawn() {
                eprintln!("could not restart {}: {e}", program.display());
            }
        }
    }

    /// A janela "Check for Updates". Devolve um comando quando o app precisa
    /// fechar (para reabrir na versão nova ou deixar o instalador trabalhar).
    pub fn window(&mut self, ctx: &egui::Context) -> Option<CommandId> {
        self.poll();
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut command = None;
        egui::Window::new("Check for Updates")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{APP_NAME} {VERSION} ({})", self.kind.describe()));
                ui.add_space(6.0);
                command = self.window_body(ui, ctx);
                ui.add_space(6.0);
                ui.separator();
                let mut on = self.settings.check_on_startup;
                if ui
                    .checkbox(&mut on, "Check for updates when the app starts")
                    .on_hover_text(format!(
                        "Asks api.github.com for the latest release of {REPOSITORY}. \
                         Only the app name and version are sent."
                    ))
                    .changed()
                {
                    self.settings.check_on_startup = on;
                    let _ = self.settings.save();
                }
            });
        self.open = open;
        command
    }

    fn window_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<CommandId> {
        let mut command = None;
        let mut install = None;
        let mut skip = None;
        match &self.state {
            State::Idle => {
                if ui.button("Check now").clicked() {
                    self.check(ctx, true);
                }
            }
            State::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking GitHub for a new version…");
                });
            }
            State::UpToDate => {
                ui.label("You have the latest version.");
            }
            State::NoRelease => {
                ui.label("No release has been published yet.");
                if ui.link("Open the releases page").clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(pgstudio_updater::releases_page()));
                }
            }
            State::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                if ui.button("Try again").clicked() {
                    self.check(ctx, true);
                }
            }
            State::Available(r) => {
                ui.label(RichText::new(format!("Version {} is available.", r.version)).strong());
                notes(ui, r);
                if ui.link("Release page").clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                }
                ui.add_space(6.0);
                if self.kind.can_self_update() {
                    if self.kind.asset(r).is_some() {
                        ui.horizontal(|ui| {
                            if ui.button("Download and install").clicked() {
                                install = Some(r.clone());
                            }
                            if ui.button("Skip this version").clicked() {
                                skip = Some(r.version.to_string());
                            }
                        });
                        ui.small("The download is checked against the release's SHA256SUMS.");
                    } else {
                        ui.label("This release has no package for this installation.");
                    }
                } else if let Some(cmd) = self.kind.update_command(r, self.helper) {
                    ui.label("This copy is managed by your package manager. Update it with:");
                    ui.horizontal(|ui| {
                        ui.code(&cmd);
                        if ui.small_button("Copy").clicked() {
                            ctx.copy_text(cmd.clone());
                        }
                    });
                    if matches!(self.kind, InstallKind::Dpkg) {
                        ui.small("Download the .deb from the release page first.");
                    }
                    if ui.button("Skip this version").clicked() {
                        skip = Some(r.version.to_string());
                    }
                } else {
                    ui.label("Download the new version from the release page.");
                    if ui.button("Skip this version").clicked() {
                        skip = Some(r.version.to_string());
                    }
                }
            }
            State::Downloading(r) => {
                ui.label(format!("Downloading {}…", r.version));
                let bar = match self.progress.fraction() {
                    Some(f) => egui::ProgressBar::new(f).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar);
            }
            State::Installed(r, Installed::Restart(program)) => {
                ui.label(format!("Version {} is installed.", r.version));
                if ui.button("Restart now").clicked() {
                    self.restart = Some(program.clone());
                    command = Some(CommandId::Exit);
                }
            }
            State::Installed(r, Installed::InstallerStarted) => {
                ui.label(format!(
                    "The installer for {} is running. Close {APP_NAME} so it can replace the files.",
                    r.version
                ));
                if ui.button(format!("Close {APP_NAME}")).clicked() {
                    command = Some(CommandId::Exit);
                }
            }
        }
        if let Some(r) = install {
            self.start_install(ctx, r);
        }
        if let Some(v) = skip {
            self.settings.skipped_version = Some(v);
            let _ = self.settings.save();
            self.state = State::UpToDate;
            self.open = false;
        }
        command
    }

    /// Para testes: põe a janela num estado sem passar pela rede.
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Para testes e screenshots: nunca consulta a rede sozinho.
    pub fn without_network(mut self) -> Self {
        self.network = false;
        self
    }
}

/// As notas do Release, numa área com rolagem.
fn notes(ui: &mut egui::Ui, r: &Release) {
    let text = r.notes.trim();
    if text.is_empty() {
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(160.0)
        .show(ui, |ui| ui.label(text));
}
