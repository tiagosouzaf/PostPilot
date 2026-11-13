//! Ponto de entrada do PGStudio.
//!
//! Uso: `pgstudio [arquivo.sql ...]` abre os arquivos em janelas de consulta.
//!
//! Variáveis de ambiente opcionais:
//! - `PGSTUDIO_SCREENSHOT=arquivo.png`: salva um PNG da janela depois dos
//!   primeiros frames e fecha. Usado pelo CI.
//! - `PGSTUDIO_BENCH=1`: modo de medição usado por `cargo xtask bench`; avisa
//!   quando o primeiro frame aparece e ignora o layout salvo.
//! - `PGSTUDIO_THEME=dark`: abre com o tema escuro.
//! - `PGSTUDIO_EXPAND=srv;srv/Databases`: expande esses nós do Object Explorer
//!   ao abrir (para demonstrações e screenshots).
//! - `PGSTUDIO_LAYOUT=arquivo.json`: usa outro arquivo de layout no lugar de
//!   `<config>/PGStudio/layout.json`. Sem ela, o modo screenshot e o de
//!   medição usam sempre o layout padrão.
//! - `PGSTUDIO_KEYMAP=arquivo.txt`: usa outro arquivo de keymap no lugar de
//!   `<config>/PGStudio/keymap.txt`.
//! - `PGSTUDIO_CONNECT`: sem ela, o app abre o "Connect to Server" (e, nos
//!   modos de screenshot e medição, usa o servidor de demonstração). Com
//!   `dialog`, abre o diálogo também nesses modos. Com um nome de servidor
//!   (`host` ou `host,porta`), conecta direto usando `PGUSER`, `PGDATABASE` e
//!   `PGPASSWORD`.
//!
//! As conexões recentes ficam em `<config>/PGStudio/connections.json`, sem
//! senha.

use eframe::egui;
use std::path::PathBuf;

use std::path::Path;
use std::sync::Arc;

use pgstudio_core::Theme;
use pgstudio_core::connection::{ConnectionSettings, DEFAULT_DATABASE};
use pgstudio_core::keymap::Keymap;
use pgstudio_pg_client::{DbRuntime, PgConnector};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::layout::{LayoutStore, MIN_SIZE};

fn main() -> eframe::Result {
    let screenshot: Option<PathBuf> = std::env::var_os("PGSTUDIO_SCREENSHOT").map(Into::into);
    let bench = std::env::var_os("PGSTUDIO_BENCH").is_some();
    // O screenshot e a medição usam sempre o layout padrão.
    // Um PGSTUDIO_LAYOUT explícito vale mesmo nesses modos.
    let explicit_layout = std::env::var_os("PGSTUDIO_LAYOUT").is_some();
    let layout = if (screenshot.is_some() || bench) && !explicit_layout {
        None
    } else {
        layout_path().map(LayoutStore::open)
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
    let connect = std::env::var("PGSTUDIO_CONNECT").ok();
    let demo = (screenshot.is_some() || bench) && connect.is_none();
    let recent = if screenshot.is_some() || bench {
        RecentConnections::in_memory(Vec::new())
    } else {
        config_dir()
            .map(|d| RecentConnections::open(d.join("PGStudio").join("connections.json")))
            .unwrap_or_default()
    };
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
            if !demo {
                match DbRuntime::new() {
                    Ok(runtime) => {
                        app = app
                            .with_recent_connections(recent)
                            .with_connector(Arc::new(PgConnector::new(runtime)));
                    }
                    Err(e) => {
                        eprintln!("could not start the database runtime: {e}");
                        app = app.with_status("Could not start the database runtime.");
                    }
                }
                if let Some(server) = connect.as_deref().filter(|c| *c != "dialog") {
                    connect_from_env(&mut app, server);
                }
            }
            for file in &files {
                app.open_file(file);
            }
            if bench {
                app = app.with_bench();
            }
            if let Some(path) = screenshot {
                app = app.with_screenshot(path);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Conecta direto ao servidor de `PGSTUDIO_CONNECT`, com as variáveis do
/// libpq para o resto.
fn connect_from_env(app: &mut ShellApp, server: &str) {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut settings = ConnectionSettings {
        user: env("PGUSER").unwrap_or_else(|| "postgres".into()),
        database: env("PGDATABASE").unwrap_or_else(|| DEFAULT_DATABASE.into()),
        ..Default::default()
    };
    if let Err(e) = settings.set_server_name(server) {
        eprintln!("PGSTUDIO_CONNECT: {e}");
        return;
    }
    app.connect_to(settings, env("PGPASSWORD").unwrap_or_default());
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
