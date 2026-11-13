//! Ponto de entrada do PGStudio.
//!
//! Variável de ambiente opcional:
//! - `PGSTUDIO_SCREENSHOT=arquivo.png`: salva um PNG da janela depois dos
//!   primeiros frames e fecha. Usado pelo CI.
//! - `PGSTUDIO_THEME=dark`: abre com o tema escuro.
//! - `PGSTUDIO_EXPAND=srv;srv/Databases`: expande esses nós do Object Explorer
//!   ao abrir (para demonstrações e screenshots).
//! - `PGSTUDIO_KEYMAP=arquivo.txt`: usa outro arquivo de keymap no lugar de
//!   `<config>/PGStudio/keymap.txt`.

use eframe::egui;
use std::path::PathBuf;

use pgstudio_core::Theme;
use pgstudio_core::keymap::Keymap;
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
    let (keymap, keymap_status) = load_keymap();
    eframe::run_native(
        "pgstudio",
        options,
        Box::new(move |cc| {
            let mut app = ShellApp::new(&cc.egui_ctx).with_keymap(keymap);
            if let Some(status) = keymap_status {
                app = app.with_status(status);
            }
            app.set_theme(&cc.egui_ctx, theme);
            if let Ok(paths) = std::env::var("PGSTUDIO_EXPAND") {
                app = app.with_expanded(paths.split(';').map(str::to_string));
            }
            if let Some(path) = screenshot {
                app = app.with_screenshot(path);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Arquivo de keymap do usuário: `PGSTUDIO_KEYMAP` ou a pasta de configuração.
fn keymap_path() -> Option<PathBuf> {
    std::env::var_os("PGSTUDIO_KEYMAP")
        .map(PathBuf::from)
        .or_else(|| config_dir().map(|d| d.join("PGStudio").join("keymap.txt")))
}

/// Pasta de configuração do usuário: `%APPDATA%` no Windows,
/// `$XDG_CONFIG_HOME` ou `~/.config` nos demais.
fn config_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    }
}

/// Keymap do SSMS com o arquivo do usuário aplicado, se existir, e uma
/// mensagem para a status bar quando o arquivo tem problemas.
fn load_keymap() -> (Keymap, Option<String>) {
    let mut keymap = Keymap::ssms();
    let Some(path) = keymap_path() else {
        return (keymap, None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (keymap, None),
        Err(e) => {
            eprintln!("keymap: não foi possível ler {}: {e}", path.display());
            return (keymap, Some(format!("Could not read {}.", path.display())));
        }
    };
    let errors = keymap.apply_overrides(&text);
    for e in &errors {
        eprintln!("keymap: {}: {e}", path.display());
    }
    let status = (!errors.is_empty()).then(|| {
        format!(
            "{} invalid line(s) ignored in {}.",
            errors.len(),
            path.display()
        )
    });
    (keymap, status)
}
