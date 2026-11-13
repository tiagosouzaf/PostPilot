//! Casca da interface do PGStudio: janela principal, barra de menus, toolbar,
//! status bar e tema.
//!
//! Só esta camada conhece o egui. O docking (marco 1.4) e o Object Explorer
//! (marco 1.5) entram depois na área central.

pub mod menus;
pub mod theme;

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use pgstudio_core::{APP_NAME, Theme, VERSION};

use menus::{Action, Entry, MENUS};

/// O aplicativo: estado da casca e desenho de cada frame.
#[derive(Default)]
pub struct ShellApp {
    theme: Theme,
    about_open: bool,
    screenshot: Option<Screenshot>,
}

struct Screenshot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

impl ShellApp {
    /// Cria o aplicativo e aplica o tema padrão.
    pub fn new(ctx: &egui::Context) -> Self {
        let app = Self::default();
        theme::apply(ctx, app.theme);
        app
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

    /// Desenha a casca inteira num `Ui` que ocupa a janela.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut action = None;

        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                for menu in MENUS {
                    ui.menu_button(menu.title, |ui| {
                        if let Some(a) = menu_entries(ui, menu.entries, self.theme) {
                            action = Some(a);
                        }
                    });
                }
            });
        });
        egui::Panel::top("toolbar").show(ui, toolbar);
        egui::Panel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::palette(self.theme).accent)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| status_bar(ui, self.theme));
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

        if let Some(a) = action {
            self.run(&ctx, a);
        }
    }

    fn run(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::SetTheme(t) => self.set_theme(ctx, t),
            Action::About => self.about_open = true,
        }
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

/// Desenha as entradas de um menu e devolve a ação clicada, se houver.
fn menu_entries(ui: &mut egui::Ui, entries: &[Entry], current: Theme) -> Option<Action> {
    let mut clicked = None;
    for entry in entries {
        match *entry {
            Entry::Item {
                label,
                shortcut,
                action,
            } => {
                let selected = matches!(action, Some(Action::SetTheme(t)) if t == current);
                let mut button = egui::Button::new(label).selected(selected);
                if let Some(s) = shortcut {
                    button = button.shortcut_text(s);
                }
                if ui.add_enabled(action.is_some(), button).clicked() {
                    clicked = action;
                    ui.close();
                }
            }
            Entry::Submenu { label, entries } => {
                ui.menu_button(label, |ui| {
                    if let Some(a) = menu_entries(ui, entries, current) {
                        clicked = Some(a);
                    }
                });
            }
            Entry::Separator => {
                ui.separator();
            }
        }
    }
    clicked
}

/// Toolbar padrão do SSMS. Os botões ganham comandos no marco 1.3.
fn toolbar(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_enabled_ui(false, |ui| {
            let _ = ui.button("New Query");
            ui.separator();
            let _ = ui.button("Open File");
            let _ = ui.button("Save");
            let _ = ui.button("Save All");
            ui.separator();
            egui::ComboBox::from_id_salt("database")
                .selected_text("(no database)")
                .width(160.0)
                .show_ui(ui, |_| {});
            let _ = ui.button("Execute");
            let _ = ui.button("Cancel");
            let _ = ui.button("Parse");
            ui.separator();
            let _ = ui.button("Estimated Plan");
            let _ = ui.button("Actual Plan");
        });
    });
}

fn status_bar(ui: &mut egui::Ui, theme: Theme) {
    let color = theme::palette(theme).on_accent;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ready").color(color));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{APP_NAME} {VERSION}")).color(color));
            ui.separator();
            ui.label(RichText::new("Not connected").color(color));
        });
    });
}
