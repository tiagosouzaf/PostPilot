//! Ponto de entrada do PGStudio.
//!
//! Variável de ambiente opcional:
//! - `PGSTUDIO_SCREENSHOT=arquivo.png`: salva um PNG da janela depois dos
//!   primeiros frames e fecha. Usado pelo CI.
//! - `PGSTUDIO_THEME=dark`: abre com o tema escuro.

use eframe::egui;
use pgstudio_core::Theme;
use pgstudio_ui_shell::ShellApp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(pgstudio_core::window_title())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    let screenshot = std::env::var_os("PGSTUDIO_SCREENSHOT").map(Into::into);
    let theme = match std::env::var("PGSTUDIO_THEME").as_deref() {
        Ok("dark") => Theme::Dark,
        _ => Theme::Light,
    };
    eframe::run_native(
        "pgstudio",
        options,
        Box::new(move |cc| {
            let mut app = ShellApp::new(&cc.egui_ctx);
            app.set_theme(&cc.egui_ctx, theme);
            if let Some(path) = screenshot {
                app = app.with_screenshot(path);
            }
            Ok(Box::new(app))
        }),
    )
}
