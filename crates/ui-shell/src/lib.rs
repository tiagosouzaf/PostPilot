//! Casca da interface do PGStudio: janela principal, barra de menus, toolbar,
//! status bar, tema e atalhos de teclado.
//!
//! Só esta camada conhece o egui. Menus, toolbar e atalhos viram
//! [`CommandId`] e passam por [`ShellApp::execute`]. O docking (marco 1.4) e o
//! Object Explorer (marco 1.5) entram depois na área central.

pub mod keys;
pub mod menus;
pub mod theme;

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use pgstudio_core::keymap::{ChordMatcher, Keymap, Match};
use pgstudio_core::{APP_NAME, CommandId, Theme, VERSION};

use menus::{Entry, MENUS, TOOLBAR, Tool};

const READY: &str = "Ready";

/// O aplicativo: estado da casca e desenho de cada frame.
pub struct ShellApp {
    theme: Theme,
    about_open: bool,
    keymap: Keymap,
    chords: ChordMatcher,
    status: String,
    last_command: Option<CommandId>,
    screenshot: Option<Screenshot>,
}

struct Screenshot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

impl Default for ShellApp {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            about_open: false,
            keymap: Keymap::ssms(),
            chords: ChordMatcher::default(),
            status: READY.to_string(),
            last_command: None,
            screenshot: None,
        }
    }
}

impl ShellApp {
    /// Cria o aplicativo, instala a fonte de ícones e aplica o tema padrão.
    pub fn new(ctx: &egui::Context) -> Self {
        install_fonts(ctx);
        let app = Self::default();
        theme::apply(ctx, app.theme);
        app
    }

    /// Usa outro keymap (ex.: o padrão com o arquivo do usuário aplicado).
    pub fn with_keymap(mut self, keymap: Keymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Mostra uma mensagem inicial na status bar no lugar de "Ready".
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    /// Salva um PNG da janela em `path` assim que ela estiver desenhada e fecha
    /// o aplicativo. Usado pelo CI para mostrar a janela em cada sistema.
    pub fn with_screenshot(mut self, path: PathBuf) -> Self {
        self.screenshot = Some(Screenshot {
            path,
            frames: 0,
            requested: false,
        });
        self
    }

    /// Tema em uso.
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// Troca o tema e aplica no contexto.
    pub fn set_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.theme = theme;
        theme::apply(ctx, theme);
    }

    /// Se a janela "About" está aberta.
    pub fn about_open(&self) -> bool {
        self.about_open
    }

    /// Texto atual da status bar.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Último comando executado, por qualquer caminho.
    pub fn last_command(&self) -> Option<CommandId> {
        self.last_command
    }

    /// Se o comando já tem implementação. Os demais aparecem desabilitados
    /// nos menus e, pelo atalho, só avisam na status bar.
    pub fn is_available(cmd: CommandId) -> bool {
        use CommandId::*;
        matches!(cmd, Exit | ThemeLight | ThemeDark | FullScreen | About)
    }

    /// Executa um comando, venha ele de menu, toolbar ou atalho.
    pub fn execute(&mut self, ctx: &egui::Context, cmd: CommandId) {
        self.last_command = Some(cmd);
        self.status = READY.to_string();
        match cmd {
            CommandId::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            CommandId::ThemeLight => self.set_theme(ctx, Theme::Light),
            CommandId::ThemeDark => self.set_theme(ctx, Theme::Dark),
            CommandId::FullScreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            CommandId::About => self.about_open = true,
            _ => {
                self.status = format!("\"{}\" is not available yet.", cmd.label());
            }
        }
    }

    /// Desenha a casca inteira num `Ui` que ocupa a janela.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut commands = self.keyboard(&ctx);

        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                for menu in MENUS {
                    ui.menu_button(menu.title, |ui| {
                        self.menu_entries(ui, menu.entries, &mut commands);
                    });
                }
            });
        });
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui, &mut commands));
        egui::Panel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::palette(self.theme).accent)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::palette(self.theme).document_well))
            .show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.weak("Object Explorer and query windows will appear here.");
                });
            });

        if self.about_open {
            let mut open = true;
            egui::Window::new(format!("About {APP_NAME}"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    ui.heading(APP_NAME);
                    ui.label(format!("Version {VERSION}"));
                    ui.label("PostgreSQL management studio.");
                    ui.label("Licensed under MIT or Apache-2.0.");
                });
            self.about_open = open;
        }

        for cmd in commands {
            self.execute(&ctx, cmd);
        }
    }

    /// Lê as teclas do frame e devolve os comandos completados.
    fn keyboard(&mut self, ctx: &egui::Context) -> Vec<CommandId> {
        let chords: Vec<_> = ctx.input(|i| i.events.iter().filter_map(keys::chord).collect());
        let mut commands = Vec::new();
        for chord in chords {
            match self.chords.feed(&self.keymap, chord) {
                Match::Command(cmd) => commands.push(cmd),
                Match::Pending(first) => {
                    self.status =
                        format!("({first}) was pressed. Waiting for second key of chord...");
                }
                Match::Unbound(first, second) => {
                    self.status =
                        format!("The key combination ({first}, {second}) is not a command.");
                }
                Match::None => {}
            }
        }
        commands
    }

    /// Desenha as entradas de um menu e guarda o comando clicado.
    fn menu_entries(&self, ui: &mut egui::Ui, entries: &[Entry], clicked: &mut Vec<CommandId>) {
        for entry in entries {
            match *entry {
                Entry::Item(cmd) => {
                    let selected = matches!(
                        (cmd, self.theme),
                        (CommandId::ThemeLight, Theme::Light) | (CommandId::ThemeDark, Theme::Dark)
                    );
                    let mut button = egui::Button::new(cmd.label()).selected(selected);
                    if let Some(s) = self.keymap.shortcut_for(cmd) {
                        button = button.shortcut_text(s.to_string());
                    }
                    if ui.add_enabled(Self::is_available(cmd), button).clicked() {
                        clicked.push(cmd);
                        ui.close();
                    }
                }
                Entry::Submenu { label, entries } => {
                    ui.menu_button(label, |ui| self.menu_entries(ui, entries, clicked));
                }
                Entry::Separator => {
                    ui.separator();
                }
            }
        }
    }

    /// Toolbar padrão do SSMS.
    fn toolbar(&self, ui: &mut egui::Ui, clicked: &mut Vec<CommandId>) {
        ui.horizontal(|ui| {
            for tool in TOOLBAR {
                match *tool {
                    Tool::Button {
                        icon,
                        text,
                        command,
                    } => {
                        let caption = if text.is_empty() {
                            icon.to_string()
                        } else {
                            format!("{icon} {text}")
                        };
                        let tip = match self.keymap.shortcut_for(command) {
                            Some(s) => format!("{} ({s})", command.label()),
                            None => command.label().to_string(),
                        };
                        let button = egui::Button::new(caption);
                        let response = ui
                            .add_enabled(Self::is_available(command), button)
                            .on_hover_text(&tip)
                            .on_disabled_hover_text(&tip);
                        if response.clicked() {
                            clicked.push(command);
                        }
                    }
                    Tool::Database => {
                        ui.add_enabled_ui(false, |ui| {
                            egui::ComboBox::from_id_salt("database")
                                .selected_text("(no database)")
                                .width(160.0)
                                .show_ui(ui, |_| {});
                        });
                    }
                    Tool::Separator => {
                        ui.separator();
                    }
                }
            }
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let color = theme::palette(self.theme).on_accent;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.status).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{APP_NAME} {VERSION}")).color(color));
                ui.separator();
                ui.label(RichText::new("Not connected").color(color));
            });
        });
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        // Espera alguns frames para o layout assentar antes de capturar.
        shot.frames += 1;
        if !shot.requested && shot.frames >= 5 {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let result = image::RgbaImage::from_raw(w as u32, h as u32, image.as_raw().to_vec())
                .ok_or_else(|| "tamanho de imagem inválido".to_string())
                .and_then(|img| img.save(&shot.path).map_err(|e| e.to_string()));
            match result {
                Ok(()) => println!("screenshot={}", shot.path.display()),
                Err(e) => eprintln!("falha ao salvar o screenshot: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for ShellApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
        let ctx = ui.ctx().clone();
        self.screenshot_step(&ctx);
    }
}

/// Fontes padrão do egui mais os ícones Phosphor.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
