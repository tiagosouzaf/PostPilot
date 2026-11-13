//! Ponto de entrada do PGStudio.
//!
//! Uso: `pgstudio [arquivo.sql ...]` abre os arquivos em janelas de consulta.
//!
//! Variáveis de ambiente opcionais:
//! - `PGSTUDIO_SCREENSHOT=arquivo.png`: salva um PNG da janela depois dos
//!   primeiros frames e fecha. Usado pelo CI.
//! - `PGSTUDIO_THEME=dark`: abre com o tema escuro.
//! - `PGSTUDIO_EXPAND=srv;srv/Databases`: expande esses nós do Object Explorer
//!   ao abrir (para demonstrações e screenshots).
//! - `PGSTUDIO_LAYOUT=arquivo.json`: usa outro arquivo de layout no lugar de
//!   `<config>/PGStudio/layout.json`. O layout não é lido nem salvo no modo
//!   screenshot.
//! - `PGSTUDIO_KEYMAP=arquivo.txt`: usa outro arquivo de keymap no lugar de
//!   `<config>/PGStudio/keymap.txt`.

use eframe::egui;
use std::path::PathBuf;

use std::path::Path;

use pgstudio_core::Theme;
use pgstudio_core::keymap::Keymap;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::layout::{LayoutStore, MIN_SIZE};

fn main() -> eframe::Result {
    let screenshot: Option<PathBuf> = std::env::var_os("PGSTUDIO_SCREENSHOT").map(Into::into);
    // O screenshot do CI usa sempre o layout padrão.
    let layout = match screenshot {
        Some(_) => None,
        None => layout_path().map(LayoutStore::open),
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(pgstudio_core::window_title())
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size(MIN_SIZE);
    if let Some(window) = layout.as_ref().and_then(|l| l.saved.as_ref()?.window) {
        viewport = viewport
            .with_inner_size(window.size)
            .with_maximized(window.maximized);
        if let Some(position) = window.position {
            viewport = viewport.with_position(position);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let theme = match std::env::var("PGSTUDIO_THEME").as_deref() {
        Ok("dark") => Theme::Dark,
        _ => Theme::Light,
    };
    let (keymap, keymap_status) = load_keymap();
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    eframe::run_native(
        "pgstudio",
        options,
        Box::new(move |cc| {
            let mut app = ShellApp::new(&cc.egui_ctx)
                .with_keymap(keymap)
                .with_file_dialogs(NativeDialogs::default());
            if let Some(status) = keymap_status {
                app = app.with_status(status);
            }
            app.set_theme(&cc.egui_ctx, theme);
            if let Ok(paths) = std::env::var("PGSTUDIO_EXPAND") {
                app = app.with_expanded(paths.split(';').map(str::to_string));
            }
            if let Some(store) = layout {
                app = app.with_layout_store(store);
            }
            for file in &files {
                app.open_file(file);
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

/// Arquivo de layout: `PGSTUDIO_LAYOUT` ou a pasta de configuração.
fn layout_path() -> Option<PathBuf> {
    std::env::var_os("PGSTUDIO_LAYOUT")
        .map(PathBuf::from)
        .or_else(|| config_dir().map(|d| d.join("PGStudio").join("layout.json")))
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

/// Diálogos de arquivo nativos do sistema (rfd). Lembra a última pasta.
#[derive(Default)]
struct NativeDialogs {
    last_dir: Option<PathBuf>,
}

impl NativeDialogs {
    fn dialog(&self) -> rfd::FileDialog {
        let dialog = rfd::FileDialog::new()
            .add_filter("SQL Files (*.sql)", &["sql"])
            .add_filter("All Files (*.*)", &["*"]);
        match &self.last_dir {
            Some(dir) => dialog.set_directory(dir),
            None => dialog,
        }
    }

    fn remember(&mut self, path: Option<PathBuf>) -> Option<PathBuf> {
        if let Some(dir) = path.as_deref().and_then(Path::parent) {
            self.last_dir = Some(dir.to_path_buf());
        }
        path
    }
}

impl FileDialogs for NativeDialogs {
    fn save_path(&mut self, name: &str, current: Option<&Path>) -> Option<PathBuf> {
        let mut dialog = self.dialog().set_title("Save File As").set_file_name(name);
        if let Some(dir) = current.and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.save_file();
        self.remember(path)
    }

    fn open_path(&mut self) -> Option<PathBuf> {
        let path = self.dialog().set_title("Open File").pick_file();
        self.remember(path)
    }
}
