//! "Check for Updates": avisa de versão nova e, onde dá, se atualiza.
//!
//! Uma versão nova aparece sozinha numa faixa no alto da janela
//! ([`Updater::banner`]), com o botão "Update now", e na status bar.
//!
//! A checagem roda numa thread e manda o resultado por um canal; a UI só lê
//! o canal a cada frame. Veja `pgstudio_updater` para o que é enviado e como
//! cada tipo de instalação se atualiza.

use pgstudio_i18n::{tr, trf};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, channel};
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use pgstudio_core::{APP_NAME, CommandId, VERSION};
use pgstudio_updater::{
    CANCELLED, CHECK_INTERVAL, NOT_AUTHORIZED, Progress, REPOSITORY, aur_helper, find_pkexec,
};

pub use pgstudio_updater::{
    InstallKind, Installed, Release, UpdateSettings, cleanup_old_binary, parse_release,
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
    /// A checagem falhou.
    Failed(String),
    /// O download ou a instalação falhou; dá para tentar de novo.
    InstallFailed(Release, String),
}

/// O que um clique na faixa ou na janela pede.
enum Action {
    Install(Release),
    Skip(String),
    Restart(PathBuf),
    CloseApp,
    OpenWindow,
    Later,
    CheckNow,
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
    /// O `pkexec`, para instalar o pacote do sistema com a senha de
    /// administrador.
    pkexec: Option<PathBuf>,
    /// "Later": a faixa some até o app reabrir (a status bar continua
    /// avisando).
    banner_dismissed: bool,
    state: State,
    /// Se pode consultar a rede. Desligado no padrão (testes, screenshots e
    /// medição); o app de verdade usa [`Updater::detect`].
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
    /// Sem arquivo de preferências e sem rede (testes, screenshots,
    /// medição).
    fn default() -> Self {
        Updater::new(UpdateSettings::in_memory(), InstallKind::Other).without_network()
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
            pkexec: None,
            banner_dismissed: false,
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
        let kind = InstallKind::detect();
        let pkexec = kind.is_release_package().then(find_pkexec).flatten();
        Updater::new(UpdateSettings::open(settings_file), kind).with_pkexec(pkexec)
    }

    /// O `pkexec` que instala o pacote do sistema (`None`: só mostra o
    /// comando).
    pub fn with_pkexec(mut self, pkexec: Option<PathBuf>) -> Self {
        self.pkexec = pkexec;
        self
    }

    /// Se o botão "Update now" faz tudo: baixa, confere e instala.
    pub fn can_install(&self) -> bool {
        self.kind.can_self_update() || (self.kind.is_release_package() && self.pkexec.is_some())
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
        if !self.network {
            self.state = State::Failed(tr("Update checks are off in this mode.").into());
            return;
        }
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
        let pkexec = self.pkexec.clone();
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
            let result =
                pgstudio_updater::install(&kind, &r, pkexec.as_deref(), &user_agent(), &progress);
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
                let skipped =
                    self.settings.skipped_version.as_deref() == Some(&r.version.to_string());
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
            (Msg::Installed(Err(e)), State::Downloading(r)) => State::InstallFailed(r, e),
            (Msg::Installed(Err(e)), _) => State::Failed(e),
        };
    }

    /// Com o app aberto por horas ou dias, repete a checagem automática no
    /// mesmo intervalo da de abertura.
    fn recheck_if_due(&mut self, ctx: &egui::Context) {
        if !self.network || !self.settings.check_on_startup || self.rx.is_some() {
            return;
        }
        if matches!(self.state, State::Idle | State::UpToDate | State::NoRelease) {
            if self.settings.due(SystemTime::now()) {
                self.check(ctx, false);
            }
            // A janela parada não redesenha sozinha: acorda para checar.
            ctx.request_repaint_after(CHECK_INTERVAL);
        }
    }

    /// Versão nova a anunciar na status bar.
    pub fn available(&self) -> Option<&Release> {
        match &self.state {
            State::Available(r)
            | State::Downloading(r)
            | State::Installed(r, _)
            | State::InstallFailed(r, _) => Some(r),
            _ => None,
        }
    }

    /// Se a faixa de versão nova deve aparecer no alto da janela.
    pub fn banner_visible(&mut self) -> bool {
        self.poll();
        match &self.state {
            State::Available(_) => !self.banner_dismissed,
            State::Downloading(_) | State::Installed(..) | State::InstallFailed(..) => true,
            _ => false,
        }
    }

    /// A faixa "há uma versão nova", com o botão que atualiza. `color` é a
    /// cor do texto sobre o fundo da faixa. Devolve um comando quando o app
    /// precisa fechar.
    pub fn banner(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        color: egui::Color32,
    ) -> Option<CommandId> {
        let mut action = None;
        let text = |t: String| RichText::new(t).color(color).strong();
        ui.horizontal(|ui| match &self.state {
            State::Available(r) => {
                ui.label(text(trf!(
                    "{app} {version} is available.",
                    app = APP_NAME,
                    version = r.version
                )));
                if self.can_install() && self.kind.asset(r).is_some() {
                    if ui.button(tr("Update now")).clicked() {
                        action = Some(Action::Install(r.clone()));
                    }
                    if ui.button(tr("What's new")).clicked() {
                        action = Some(Action::OpenWindow);
                    }
                } else if ui.button(tr("How to update")).clicked() {
                    action = Some(Action::OpenWindow);
                }
                if ui.button(tr("Later")).clicked() {
                    action = Some(Action::Later);
                }
            }
            State::Downloading(r) => {
                if self.progress.installing.load(Ordering::Relaxed) {
                    ui.spinner();
                    ui.label(text(self.installing_text(r)));
                } else {
                    ui.label(text(trf!("Downloading {version}…", version = r.version)));
                    let bar = match self.progress.fraction() {
                        Some(f) => egui::ProgressBar::new(f).show_percentage(),
                        None => egui::ProgressBar::new(0.0).animate(true),
                    };
                    ui.add(bar.desired_width(220.0));
                }
            }
            State::Installed(r, Installed::Restart(program)) => {
                ui.label(text(trf!(
                    "Version {version} is installed.",
                    version = r.version
                )));
                if ui.button(tr("Restart now")).clicked() {
                    action = Some(Action::Restart(program.clone()));
                }
            }
            State::Installed(r, Installed::InstallerStarted) => {
                ui.label(text(trf!(
                    "The installer for {version} is running. Close {app} so it can replace the files.",
                    version = r.version,
                    app = APP_NAME
                )));
                if ui.button(trf!("Close {app}", app = APP_NAME)).clicked() {
                    action = Some(Action::CloseApp);
                }
            }
            State::InstallFailed(r, e) => {
                ui.label(text(trf!(
                    "The update to {version} failed: {error}",
                    version = r.version,
                    error = error_text(e)
                )));
                if ui.button(tr("Try again")).clicked() {
                    action = Some(Action::Install(r.clone()));
                }
                if ui.button(tr("Details")).clicked() {
                    action = Some(Action::OpenWindow);
                }
            }
            _ => {}
        });
        action.and_then(|a| self.apply(ctx, a))
    }

    fn installing_text(&self, r: &Release) -> String {
        if self.kind.is_release_package() {
            trf!(
                "Installing {version}… Enter your password in the system prompt.",
                version = r.version
            )
        } else {
            trf!("Installing {version}…", version = r.version)
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) -> Option<CommandId> {
        match action {
            Action::Install(r) => self.start_install(ctx, r),
            Action::Skip(v) => {
                self.settings.skipped_version = Some(v);
                let _ = self.settings.save();
                self.state = State::UpToDate;
                self.open = false;
            }
            Action::Restart(program) => {
                self.restart = Some(program);
                return Some(CommandId::Exit);
            }
            Action::CloseApp => return Some(CommandId::Exit),
            Action::OpenWindow => self.open = true,
            Action::Later => self.banner_dismissed = true,
            Action::CheckNow => self.check(ctx, true),
        }
        None
    }

    /// O aviso na status bar, à direita da versão. Clicar abre a janela.
    pub fn status_badge(&mut self, ui: &mut egui::Ui, color: egui::Color32) {
        let text = match &self.state {
            State::Available(r) => trf!("Update available: {version}", version = r.version),
            State::Downloading(_) => tr("Downloading update…").to_string(),
            State::Installed(..) => tr("Restart to update").to_string(),
            State::InstallFailed(..) => tr("Update failed").to_string(),
            _ => return,
        };
        let badge = egui::Button::new(RichText::new(text).color(color).strong()).frame(false);
        if ui
            .add(badge)
            .on_hover_text(tr("Check for Updates"))
            .clicked()
        {
            self.open = true;
        }
        ui.separator();
    }

    /// Linha do "About" sobre a atualização.
    pub fn about_line(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            match &self.state {
                State::Available(r) => {
                    ui.label(trf!("Version {version} is available.", version = r.version));
                }
                State::UpToDate => {
                    ui.label(tr("This is the latest version."));
                }
                _ => {}
            }
            if ui.link(tr("Check for Updates…")).clicked() {
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
        if let Some(program) = self.take_restart()
            && let Err(e) = std::process::Command::new(&program).spawn()
        {
            eprintln!("could not restart {}: {e}", program.display());
        }
    }

    /// A janela "Check for Updates". Devolve um comando quando o app precisa
    /// fechar (para reabrir na versão nova ou deixar o instalador trabalhar).
    pub fn window(&mut self, ctx: &egui::Context) -> Option<CommandId> {
        self.poll();
        self.recheck_if_due(ctx);
        // Só em build de desenvolvimento, para o teste de ponta a ponta sem
        // mouse: instala a versão encontrada como se o botão fosse clicado.
        #[cfg(debug_assertions)]
        if std::env::var_os("PGSTUDIO_UPDATE_AUTO").is_some()
            && let State::Available(r) = &self.state
            && !self.banner_dismissed
        {
            let r = r.clone();
            self.banner_dismissed = true;
            self.start_install(ctx, r);
        }
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut command = None;
        egui::Window::new(tr("Check for Updates"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{APP_NAME} {VERSION} ({})", self.kind.describe())); // i18n-ignore
                ui.add_space(6.0);
                command = self.window_body(ui, ctx);
                ui.add_space(6.0);
                ui.separator();
                let mut on = self.settings.check_on_startup;
                if ui
                    .checkbox(&mut on, tr("Check for updates when the app starts"))
                    .on_hover_text(trf!(
                        "Asks api.github.com for the latest release of {repository}. \
                         Only the app name and version are sent.",
                        repository = REPOSITORY
                    ))
                    .changed()
                {
                    self.settings.check_on_startup = on;
                    let _ = self.settings.save();
                }
            });
        self.open &= open;
        command
    }

    fn window_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<CommandId> {
        let mut action = None;
        match &self.state {
            State::Idle => {
                if ui.button(tr("Check now")).clicked() {
                    action = Some(Action::CheckNow);
                }
            }
            State::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("Checking GitHub for a new version…"));
                });
            }
            State::UpToDate => {
                ui.label(tr("You have the latest version."));
            }
            State::NoRelease => {
                ui.label(tr("No release has been published yet."));
                if ui.link(tr("Open the releases page")).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(pgstudio_updater::releases_page()));
                }
            }
            State::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                if ui.button(tr("Try again")).clicked() {
                    action = Some(Action::CheckNow);
                }
            }
            State::Available(r) => {
                ui.label(
                    RichText::new(trf!("Version {version} is available.", version = r.version))
                        .strong(),
                );
                notes(ui, r);
                if ui.link(tr("Release page")).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&r.page));
                }
                ui.add_space(6.0);
                if self.can_install() {
                    if self.kind.asset(r).is_some() {
                        ui.horizontal(|ui| {
                            if ui.button(tr("Download and install")).clicked() {
                                action = Some(Action::Install(r.clone()));
                            }
                            if ui.button(tr("Skip this version")).clicked() {
                                action = Some(Action::Skip(r.version.to_string()));
                            }
                        });
                        ui.small(tr(
                            "The download is checked against the release's SHA256SUMS.",
                        ));
                        if self.kind.is_release_package() {
                            ui.small(tr(
                                "The system asks for your password to install the package.",
                            ));
                        }
                    } else {
                        ui.label(tr("This release has no package for this installation."));
                    }
                } else if let Some(cmd) = self.kind.update_command(r, self.helper) {
                    ui.label(tr(
                        "This copy is managed by your package manager. Update it with:",
                    ));
                    command_line(ui, ctx, &cmd);
                    if ui.button(tr("Skip this version")).clicked() {
                        action = Some(Action::Skip(r.version.to_string()));
                    }
                } else {
                    ui.label(tr("Download the new version from the release page."));
                    if ui.button(tr("Skip this version")).clicked() {
                        action = Some(Action::Skip(r.version.to_string()));
                    }
                }
            }
            State::Downloading(r) => {
                if self.progress.installing.load(Ordering::Relaxed) {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.installing_text(r));
                    });
                } else {
                    ui.label(trf!("Downloading {version}…", version = r.version));
                    let bar = match self.progress.fraction() {
                        Some(f) => egui::ProgressBar::new(f).show_percentage(),
                        None => egui::ProgressBar::new(0.0).animate(true),
                    };
                    ui.add(bar);
                }
            }
            State::Installed(r, Installed::Restart(program)) => {
                ui.label(trf!("Version {version} is installed.", version = r.version));
                if ui.button(tr("Restart now")).clicked() {
                    action = Some(Action::Restart(program.clone()));
                }
            }
            State::Installed(r, Installed::InstallerStarted) => {
                ui.label(trf!(
                    "The installer for {version} is running. Close {app} so it can replace the files.",
                    version = r.version,
                    app = APP_NAME
                ));
                if ui.button(trf!("Close {app}", app = APP_NAME)).clicked() {
                    action = Some(Action::CloseApp);
                }
            }
            State::InstallFailed(r, e) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    trf!(
                        "The update to {version} failed: {error}",
                        version = r.version,
                        error = error_text(e)
                    ),
                );
                if ui.button(tr("Try again")).clicked() {
                    action = Some(Action::Install(r.clone()));
                }
                if let Some(cmd) = self.kind.update_command(r, self.helper) {
                    ui.add_space(6.0);
                    ui.label(tr("Or update it in a terminal with:"));
                    command_line(ui, ctx, &cmd);
                }
            }
        }
        action.and_then(|a| self.apply(ctx, a))
    }

    /// Para testes: põe a janela num estado sem passar pela rede.
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Para testes: as preferências em uso.
    pub fn settings(&self) -> &UpdateSettings {
        &self.settings
    }

    /// Para testes e screenshots: nunca consulta a rede sozinho.
    pub fn without_network(mut self) -> Self {
        self.network = false;
        self
    }
}

/// O erro da instalação, traduzido quando é um dos conhecidos.
fn error_text(e: &str) -> &str {
    if e == CANCELLED {
        tr("The update was cancelled at the password prompt.")
    } else if e == NOT_AUTHORIZED {
        tr(
            "The system did not authorize the installation (wrong password, or no polkit agent is running).",
        )
    } else {
        e
    }
}

/// Um comando para copiar e rodar no terminal.
fn command_line(ui: &mut egui::Ui, ctx: &egui::Context, cmd: &str) {
    ui.horizontal(|ui| {
        ui.code(cmd);
        if ui.small_button(tr("Copy")).clicked() {
            ctx.copy_text(cmd.to_string());
        }
    });
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
