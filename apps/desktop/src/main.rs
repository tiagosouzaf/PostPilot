//! Ponto de entrada do PostPilot (o binário `postpilot`).
//!
//! Uso: `postpilot [--renderer wgpu|glow] [arquivo.sql ...]` abre os arquivos
//! em janelas de consulta.
//!
//! `--renderer` escolhe o renderizador gráfico (veja o módulo `renderer`):
//! `wgpu` (o padrão) ou `glow` (OpenGL). Sem escolha, o app tenta o wgpu e
//! reabre com o glow se ele falhar.
//!
//! Variáveis de ambiente opcionais:
//! - `PGSTUDIO_RENDERER=wgpu|glow`: o mesmo que `--renderer`.
//! - `PGSTUDIO_SCREENSHOT=arquivo.png`: salva um PNG da janela depois dos
//!   primeiros frames e fecha. Usado pelo CI.
//! - `PGSTUDIO_BENCH=1`: modo de medição usado por `cargo xtask bench`; avisa
//!   quando o primeiro frame aparece e ignora o layout salvo. Com `grid` ou
//!   `intellisense`, roda também esse cenário e escreve os números (veja
//!   `pgstudio_ui_shell::bench`).
//! - `PGSTUDIO_THEME=dark`: abre com o tema escuro.
//! - `PGSTUDIO_EXPAND=srv;srv/Databases`: expande esses nós do Object Explorer
//!   ao abrir (para demonstrações e screenshots).
//! - `PGSTUDIO_LAYOUT=arquivo.json`: usa outro arquivo de layout no lugar de
//!   `<config>/PostPilot/layout.json`. Sem ela, o modo screenshot e o de
//!   medição usam sempre o layout padrão.
//! - `PGSTUDIO_KEYMAP=arquivo.txt`: usa outro arquivo de keymap no lugar de
//!   `<config>/PostPilot/keymap.txt`.
//! - `PGSTUDIO_RUN=1`: executa a janela de consulta ativa (o primeiro
//!   arquivo aberto) assim que conectar, como um F5.
//! - `PGSTUDIO_LIST_MEMBERS=1`: põe o cursor no fim da janela de consulta
//!   ativa e abre o IntelliSense assim que o catálogo chegar (para
//!   screenshots).
//! - `PGSTUDIO_ACTION=select|edit|create|drop|design|newtable|diagram|querydesign|plan|actualplan|info|activity|properties|backup`:
//!   depois de
//!   `PGSTUDIO_EXPAND`, faz no nó selecionado (o último expandido) "Select
//!   Top 1000 Rows", "Edit Top 200 Rows", "Script as CREATE/DROP", "Design"
//!   "New Table..." ou "New Database Diagram" (para screenshots; com
//!   `PGSTUDIO_DIAGRAM_ALL=1`, o diagrama já abre com todas as tabelas).
//!   `querydesign` abre uma janela de consulta no banco e o Query Designer
//!   nela (com `PGSTUDIO_DIAGRAM_ALL=1`, com todas as tabelas; com
//!   `PGSTUDIO_QUERY`, lendo esse SELECT). `plan` abre
//!   uma janela de consulta no banco com o texto de `PGSTUDIO_QUERY` e pede
//!   o plano estimado (Ctrl+L); `actualplan` faz o mesmo com o plano real
//!   (Ctrl+M e F5) e `info` com o Alt+F1 no começo do texto. `activity`
//!   abre o Activity Monitor e `properties`, as propriedades de segurança
//!   (Permissions ou Role Properties), `backup`, o "Back Up Database", e
//!   `restore`, o "Restore Database" (com `PGSTUDIO_RESTORE_FILE`, já
//!   listando o arquivo). `compare` abre o "Schema Compare" com o banco
//!   como origem (com `PGSTUDIO_COMPARE=origem,destino`, já comparando os dois).
//! - `PGSTUDIO_CONNECT`: sem ela, o app abre o "Connect to Server" (e, nos
//!   modos de screenshot e medição, usa o servidor de demonstração). Com
//!   `dialog`, abre o diálogo também nesses modos. Com um nome de servidor
//!   (`host` ou `host,porta`), conecta direto usando `PGUSER`, `PGDATABASE` e
//!   `PGPASSWORD`. Vários servidores separados por `;` aparecem juntos no
//!   Object Explorer.
//!
//! - `PGSTUDIO_REGISTERED=arquivo.json`: usa outra árvore de Registered
//!   Servers no lugar de `<config>/PostPilot/registered_servers.json` e já
//!   abre a janela (para screenshots).
//! - `PGSTUDIO_WORKSPACE=pasta`: abre este workspace (veja
//!   `pgstudio_ui_shell::repo`). Sem ela, o app reabre o último workspace,
//!   lembrado em `<config>/PGStudio/workspace.txt`.
//!
//! As conexões recentes ficam em `<config>/PostPilot/connections.json` e os
//! servidores registrados em `<config>/PostPilot/registered_servers.json`,
//! sem senha. As senhas com "Remember password" ficam no cofre do sistema;
//! sem cofre (Linux sem D-Bus), o app pede a senha. Os modos de screenshot e
//! de medição não usam o cofre.

// Sem console no Windows nos builds de release (os pacotes de `cargo xtask
// dist`); em debug o console fica, para ver os logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::path::PathBuf;

use std::path::Path;
use std::sync::Arc;

use pgstudio_core::Theme;
use pgstudio_core::connection::{ConnectionSettings, DEFAULT_DATABASE};
use pgstudio_core::keymap::Keymap;
use pgstudio_pg_metadata::{PgConnector, Runtime as DbRuntime};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::layout::{LayoutStore, MIN_SIZE};
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::secrets::KeyringStore;

mod migrate;
mod renderer;

/// Identificador do app no Wayland e no X11 (`WM_CLASS`).
const APP_ID: &str = "postpilot";
/// Ícone da janela e da barra de tarefas (o mesmo dos pacotes).
const ICON_PNG: &[u8] = include_bytes!("../../../packaging/icons/postpilot-256.png");

fn main() -> std::process::ExitCode {
    let launch = renderer::Launch::parse(
        std::env::args_os().skip(1),
        std::env::var(renderer::ENV).ok().as_deref(),
    );
    for warning in &launch.warnings {
        eprintln!("postpilot: {warning}");
    }
    // Até a 0.1 o app se chamava PGStudio: traz a configuração antiga.
    if let Some(config) = config_dir() {
        match migrate::legacy_config(&config) {
            Ok(true) => eprintln!("postpilot: settings copied from the PGStudio folder"),
            Ok(false) => {}
            Err(e) => eprintln!("postpilot: could not copy the PGStudio settings: {e}"),
        }
    }
    match run(&launch) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) if launch.should_fall_back(&e) => {
            eprintln!("postpilot: the wgpu renderer failed ({e}); trying again with glow (OpenGL)");
            relaunch_with_glow()
        }
        Err(e) => {
            eprintln!("postpilot: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Abre o app de novo com `PGSTUDIO_RENDERER=glow` e sai com o código dele.
/// Um processo novo porque o winit não deixa criar um segundo event loop no
/// mesmo processo.
fn relaunch_with_glow() -> std::process::ExitCode {
    let status = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(renderer::ENV, "glow")
            .status()
    });
    match status {
        Ok(s) if s.success() => std::process::ExitCode::SUCCESS,
        Ok(s) => {
            let code = s.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1);
            std::process::ExitCode::from(code)
        }
        Err(e) => {
            eprintln!("postpilot: could not restart with glow: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(launch: &renderer::Launch) -> eframe::Result {
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
        .with_min_inner_size(MIN_SIZE)
        // Igual ao `StartupWMClass` e ao nome do .desktop dos pacotes Linux:
        // é como o Wayland e o X11 ligam a janela ao ícone do menu.
        .with_app_id(APP_ID);
    match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => viewport = viewport.with_icon(Arc::new(icon)),
        Err(e) => eprintln!("could not load the window icon: {e}"),
    }
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
        renderer: launch.eframe_renderer(),
        ..Default::default()
    };
    let theme = match std::env::var("PGSTUDIO_THEME").as_deref() {
        Ok("dark") => Theme::Dark,
        _ => Theme::Light,
    };
    let (keymap, keymap_status) = load_keymap();
    let files = launch.files.clone();
    let connect = std::env::var("PGSTUDIO_CONNECT").ok();
    let demo = (screenshot.is_some() || bench) && connect.is_none();
    let recent = if screenshot.is_some() || bench {
        RecentConnections::in_memory(Vec::new())
    } else {
        config_dir()
            .map(|d| {
                RecentConnections::open(
                    d.join(pgstudio_core::CONFIG_DIR_NAME)
                        .join("connections.json"),
                )
            })
            .unwrap_or_default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            let mut app = ShellApp::new(&cc.egui_ctx)
                .with_keymap(keymap)
                .with_file_dialogs(NativeDialogs::default())
                .with_renderer(renderer::describe(cc));
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
            let registered_file = std::env::var_os("PGSTUDIO_REGISTERED").map(PathBuf::from);
            if let Some(file) = &registered_file {
                app = app.with_registered_servers(RegisteredServers::open(file));
                app.show_registered_servers();
            } else if screenshot.is_none() && !bench {
                if let Some(dir) = config_dir() {
                    let file = dir
                        .join(pgstudio_core::CONFIG_DIR_NAME)
                        .join("registered_servers.json");
                    app = app.with_registered_servers(RegisteredServers::open(file));
                }
                app = app.with_secret_store(Arc::new(KeyringStore::default()));
            }
            if let Some(root) = std::env::var_os("PGSTUDIO_WORKSPACE").map(PathBuf::from) {
                app.open_workspace(&root);
            } else if screenshot.is_none()
                && !bench
                && let Some(dir) = config_dir()
            {
                app = app.with_workspace_memory(dir.join("PGStudio").join("workspace.txt"));
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
                if let Some(servers) = connect.as_deref().filter(|c| *c != "dialog") {
                    app = connect_from_env(app, servers);
                }
            }
            for file in &files {
                app.open_file(file);
            }
            if std::env::var_os("PGSTUDIO_RUN").is_some() {
                app = app.with_run_on_connect();
            }
            if std::env::var_os("PGSTUDIO_LIST_MEMBERS").is_some() {
                app = app.with_list_members_on_load();
            }
            if let Ok(action) = std::env::var("PGSTUDIO_ACTION") {
                use pgstudio_core::metadata::ScriptAction;
                use pgstudio_ui_shell::ExplorerEvent as Event;
                let make: fn(_) -> Event = match action.as_str() {
                    "select" => Event::SelectTop,
                    "edit" => Event::EditTop,
                    "create" => |n| Event::Script(n, ScriptAction::Create),
                    "design" => Event::Design,
                    "newtable" => Event::NewTable,
                    "diagram" => Event::NewDiagram,
                    "activity" => Event::ActivityMonitor,
                    "register" => Event::Register,
                    "properties" => Event::Properties,
                    "backup" => Event::BackUp,
                    "restore" => Event::Restore,
                    "compare" => Event::CompareSchema,
                    "querydesign" | "plan" | "actualplan" | "info" => Event::NewQuery,
                    _ => |n| Event::Script(n, ScriptAction::Drop),
                };
                app = app.with_selected_action(make);
                if action == "querydesign" {
                    // Com PGSTUDIO_QUERY, o designer abre lendo esse SELECT.
                    match std::env::var("PGSTUDIO_QUERY") {
                        Ok(text) => {
                            app =
                                app.with_query_on_open(text, pgstudio_core::CommandId::DesignQuery);
                        }
                        Err(_) => app = app.with_design_query_on_open(),
                    }
                }
                if let Ok(pair) = std::env::var("PGSTUDIO_COMPARE")
                    && let Some((source, target)) = pair.split_once(',')
                {
                    app = app.with_compare(source.into(), target.into());
                }
                if let Ok(file) = std::env::var("PGSTUDIO_RESTORE_FILE") {
                    app = app.with_restore_file(file);
                }
                if matches!(action.as_str(), "plan" | "actualplan" | "info") {
                    let text = std::env::var("PGSTUDIO_QUERY").unwrap_or_default();
                    let cmd = match action.as_str() {
                        "plan" => pgstudio_core::CommandId::EstimatedPlan,
                        "actualplan" => pgstudio_core::CommandId::ActualPlan,
                        _ => pgstudio_core::CommandId::ObjectInfo,
                    };
                    app = app.with_query_on_open(text, cmd);
                }
                if std::env::var_os("PGSTUDIO_DIAGRAM_ALL").is_some() {
                    app = app.with_diagram_all_tables();
                }
            }
            if bench {
                app = app.with_bench();
                let scenario = std::env::var("PGSTUDIO_BENCH").unwrap_or_default();
                if let Some(s) = pgstudio_ui_shell::bench::Scenario::parse(&scenario) {
                    app = app.with_bench_scenario(s);
                }
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
fn connect_from_env(mut app: ShellApp, servers: &str) -> ShellApp {
    let mut servers = servers.split(';').filter(|s| !s.trim().is_empty());
    let Some(first) = servers.next() else {
        return app;
    };
    let password = std::env::var("PGPASSWORD").unwrap_or_default();
    let mut rest = Vec::new();
    for server in servers {
        if let Some(s) = env_settings(server) {
            rest.push(s);
        }
    }
    if !rest.is_empty() {
        // Os outros servidores conectam como servidores registrados, com a
        // senha num cofre só da memória.
        let store = pgstudio_ui_shell::secrets::MemoryStore::default();
        for s in &rest {
            let key = pgstudio_ui_shell::secrets::password_key(s);
            pgstudio_ui_shell::secrets::SecretStore::set(&store, &key, &password)
                .expect("memory store");
        }
        app = app.with_secret_store(Arc::new(store));
        for s in rest {
            app.queue_registered_connect(s);
        }
    }
    if let Some(settings) = env_settings(first) {
        app.connect_to(settings, password);
    }
    app
}

/// Conexão para um servidor de `PGSTUDIO_CONNECT`, com `PGUSER` e
/// `PGDATABASE`.
fn env_settings(server: &str) -> Option<ConnectionSettings> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut settings = ConnectionSettings {
        user: env("PGUSER").unwrap_or_else(|| "postgres".into()),
        database: env("PGDATABASE").unwrap_or_else(|| DEFAULT_DATABASE.into()),
        ..Default::default()
    };
    if let Err(e) = settings.set_server_name(server) {
        eprintln!("PGSTUDIO_CONNECT: {e}");
        return None;
    }
    Some(settings)
}

/// Arquivo de keymap do usuário: `PGSTUDIO_KEYMAP` ou a pasta de configuração.
fn keymap_path() -> Option<PathBuf> {
    std::env::var_os("PGSTUDIO_KEYMAP")
        .map(PathBuf::from)
        .or_else(|| config_dir().map(|d| d.join(pgstudio_core::CONFIG_DIR_NAME).join("keymap.txt")))
}

/// Arquivo de layout: `PGSTUDIO_LAYOUT` ou a pasta de configuração.
fn layout_path() -> Option<PathBuf> {
    std::env::var_os("PGSTUDIO_LAYOUT")
        .map(PathBuf::from)
        .or_else(|| {
            config_dir().map(|d| d.join(pgstudio_core::CONFIG_DIR_NAME).join("layout.json"))
        })
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

    fn save_results_path(&mut self, name: &str) -> Option<PathBuf> {
        let path = self
            .dialog()
            .set_title("Save Grid Results")
            .set_file_name(name)
            .add_filter("CSV (Comma delimited)", &["csv"])
            .add_filter("Text (Tab delimited)", &["tsv", "txt"])
            .add_filter("JSON", &["json"])
            .add_filter("SQL INSERT statements", &["sql"])
            .save_file();
        self.remember(path)
    }

    fn folder_path(&mut self, title: &str) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = &self.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.pick_folder();
        if let Some(p) = &path {
            self.last_dir = Some(p.clone());
        }
        path
    }
}
