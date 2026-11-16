//! Ponto de entrada do PostPilot (o binário `postpilot`).
//!
//! Uso: `postpilot [--renderer wgpu|glow] [arquivo.sql ...]` abre os arquivos
//! em janelas de consulta.
//!
//! `postpilot mcp` não abre janela: é o servidor MCP do Claude Code, por
//! stdio, que repassa as chamadas para o PostPilot aberto (veja
//! `pgstudio_automation::mcp`). Uso: `claude mcp add postpilot -- postpilot mcp`.
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
//! - `PGSTUDIO_THEME=light|dark`: abre com esse tema (sem ela, o salvo em
//!   Tools > Options, ou o escuro).
//! - `PGSTUDIO_LANG=pt-BR|en`: idioma da interface, no lugar do escolhido
//!   em Tools > Options (ou do idioma do sistema).
//! - `PGSTUDIO_OPTIONS=1`: abre Tools > Options (para screenshots).
//! - `PGSTUDIO_EXPAND=srv;srv/Databases`: expande esses nós do Object Explorer
//!   ao abrir (para demonstrações e screenshots).
//! - `PGSTUDIO_HISTORY=arquivo.jsonl`: usa outro arquivo para o histórico de
//!   consultas executadas no lugar de `<config>/PostPilot/history.jsonl`
//!   (também nos modos de screenshot e de medição, que sem ela não gravam).
//! - `PGSTUDIO_LAYOUT=arquivo.json`: usa outro arquivo de layout no lugar de
//!   `<config>/PostPilot/layout.json`. Sem ela, o modo screenshot e o de
//!   medição usam sempre o layout padrão.
//! - `PGSTUDIO_SESSION=arquivo.json`: usa outro arquivo de sessão no lugar de
//!   `<config>/PostPilot/session.json` (também nos modos de screenshot e de
//!   medição, que sem ela não reabrem nem salvam sessão).
//! - `PGSTUDIO_KEYMAP=arquivo.txt`: usa outro arquivo de keymap no lugar de
//!   `<config>/PostPilot/keymap.txt`.
//! - `PGSTUDIO_RUN=1`: executa a janela de consulta ativa (o primeiro
//!   arquivo aberto) assim que conectar, como um F5.
//! - `PGSTUDIO_LIST_MEMBERS=1`: põe o cursor no fim da janela de consulta
//!   ativa e abre o IntelliSense assim que o catálogo chegar (para
//!   screenshots).
//! - `PGSTUDIO_DATABASES=1`: abre a lista de bancos da toolbar (Ctrl+U)
//!   quando a janela de consulta ativa estiver pronta.
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
//! - `PGSTUDIO_JSON=view[:caminho][:text]|guide:N|edit:linha,coluna`: abre
//!   o JSON Viewer, o JSON Query Guide ou o editor de JSON do Edit Top Rows
//!   assim que der (para screenshots; veja `ShellApp::with_json_demo`).
//! - `PGSTUDIO_CONNECT`: sem ela, o app abre o "Connect to Server" (e, nos
//!   modos de screenshot e medição, usa o servidor de demonstração). Com
//!   `dialog`, abre o diálogo também nesses modos (`dialog:sqlserver`: já
//!   com o SQL Server escolhido). Com um nome de servidor
//!   (`host` ou `host,porta`), conecta direto usando `PGUSER`, `PGDATABASE` e
//!   `PGPASSWORD`. Vários servidores separados por `;` aparecem juntos no
//!   Object Explorer.
//!
//! - `PGSTUDIO_REGISTERED=arquivo.json`: usa outra árvore de Registered
//!   Servers no lugar de `<config>/PostPilot/registered_servers.json` e já
//!   abre a janela (para screenshots).
//! - `PGSTUDIO_WORKSPACE=pasta`: abre este workspace (veja
//!   `pgstudio_ui_shell::repo`). Sem ela, o app reabre o último workspace,
//!   lembrado em `<config>/PostPilot/workspace.txt`.
//!
//! O app escreve um log em `<config>/PostPilot/postpilot.log` (início, erros,
//! panics e, com o vigia da interface, quando a janela para de desenhar e
//! em que etapa; veja `pgstudio_ui_shell::applog`).
//!
//! As conexões recentes ficam em `<config>/PostPilot/connections.json` e os
//! servidores registrados em `<config>/PostPilot/registered_servers.json`,
//! sem senha. As senhas com "Remember password" ficam no cofre do sistema;
//! sem cofre (Linux sem D-Bus), o app pede a senha. Os modos de screenshot e
//! de medição não usam o cofre.
//!
//! O que estava aberto (servidores do Object Explorer, janelas de consulta
//! com o texto não salvo) fica em `<config>/PostPilot/session.json` e volta
//! ao abrir, sem senha: veja `pgstudio_ui_shell::session`. Com
//! `PGSTUDIO_CONNECT`, a sessão não é usada.
//!
//! Ao abrir, o app procura versão nova nos Releases do GitHub (no máximo a
//! cada 6 horas; desligável em Help > Check for Updates, guardado em
//! `<config>/PostPilot/update.json`). Os modos de screenshot e de medição não
//! consultam a rede.

// Sem console no Windows nos builds de release (os pacotes de `cargo xtask
// dist`); em debug o console fica, para ver os logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::path::PathBuf;

use std::path::Path;
use std::sync::Arc;

use pgstudio_core::Theme;
use pgstudio_core::connection::{ConnectionSettings, Engine};
use pgstudio_core::keymap::Keymap;
use pgstudio_i18n::{tr, trf};
use pgstudio_mssql::{EngineConnector, MssqlConnector};
use pgstudio_pg_metadata::{PgConnector, Runtime as DbRuntime};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::layout::{LayoutStore, MIN_SIZE};
use pgstudio_ui_shell::preferences::{Preferences, PreferencesStore};
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::secrets::KeyringStore;
use pgstudio_ui_shell::update::Updater;

mod migrate;
mod renderer;

/// Identificador do app no Wayland e no X11 (`WM_CLASS`).
const APP_ID: &str = "postpilot";
/// Ícone da janela e da barra de tarefas (o mesmo dos pacotes).
const ICON_PNG: &[u8] = include_bytes!("../../../packaging/icons/postpilot-256.png");

fn main() -> std::process::ExitCode {
    // O .exe antigo que uma atualização da cópia portátil deixou para trás.
    pgstudio_ui_shell::update::cleanup_old_binary();
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        return pgstudio_automation::mcp::serve_stdio();
    }
    let launch = renderer::Launch::parse(
        std::env::args_os().skip(1),
        std::env::var(renderer::ENV).ok().as_deref(),
    );
    if let Some(config) = config_dir() {
        pgstudio_ui_shell::applog::init(
            &config
                .join(pgstudio_core::CONFIG_DIR_NAME)
                .join("postpilot.log"),
        );
    }
    log_line(&format!(
        "start: PostPilot {} on {} ({}), renderer {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        session_kind(),
        match launch.renderer {
            Some(r) => format!("{r:?}"),
            None => "wgpu (glow as fallback)".into(),
        }
    ));
    for warning in &launch.warnings {
        log_line(warning);
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
        Ok(()) => {
            log_line("exit");
            std::process::ExitCode::SUCCESS
        }
        Err(e) if launch.should_fall_back(&e) => {
            log_line(&format!(
                "the wgpu renderer failed ({e}); trying again with glow (OpenGL)"
            ));
            relaunch_with_glow()
        }
        Err(e) => {
            log_line(&format!("error: {e}"));
            std::process::ExitCode::FAILURE
        }
    }
}

fn log_line(message: &str) {
    pgstudio_ui_shell::applog::line(message);
}

/// Wayland ou X11 (o que o ambiente indica), para o log.
fn session_kind() -> String {
    if cfg!(windows) {
        return "windows".into();
    }
    let var = |n| std::env::var(n).unwrap_or_default();
    let desktop = var("XDG_CURRENT_DESKTOP");
    let kind = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "wayland"
    } else if std::env::var_os("DISPLAY").is_some() {
        "x11"
    } else {
        "no display"
    };
    if desktop.is_empty() {
        kind.into()
    } else {
        format!("{kind}, {desktop}")
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
    // Preferências (Tools > Options). O screenshot e a medição usam as
    // padrão, como fazem com o layout.
    let (prefs_store, prefs) = match config_dir() {
        Some(dir) if screenshot.is_none() && !bench => {
            let (store, prefs) = PreferencesStore::open(
                dir.join(pgstudio_core::CONFIG_DIR_NAME)
                    .join("settings.json"),
            );
            (Some(store), prefs)
        }
        _ => (None, Preferences::default()),
    };
    prefs.apply();
    // PGSTUDIO_THEME vale mais que o tema salvo (e que o do sistema).
    let theme = match std::env::var("PGSTUDIO_THEME").as_deref() {
        Ok("light") => Some(Theme::Light),
        Ok("dark") => Some(Theme::Dark),
        _ => None,
    };
    // Sem olhar o sistema (screenshot e medição): o tema salvo ou o padrão.
    let offline_theme = prefs.theme(None);
    if let Some(language) = std::env::var("PGSTUDIO_LANG")
        .ok()
        .and_then(|l| pgstudio_i18n::Language::from_locale(&l))
    {
        pgstudio_i18n::set_language(language);
    }
    let (keymap, keymap_status) = load_keymap();
    let files = launch.files.clone();
    let connect = std::env::var("PGSTUDIO_CONNECT").ok();
    let demo = (screenshot.is_some() || bench) && connect.is_none();
    let recent = if connect.as_deref() == Some("dialog:sqlserver") {
        // O "Connect to Server" já no SQL Server (screenshots).
        RecentConnections::in_memory(vec![ConnectionSettings::for_engine(Engine::SqlServer)])
    } else if screenshot.is_some() || bench {
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
                .with_preferences(prefs_store, prefs)
                .with_keymap(keymap)
                .with_file_dialogs(NativeDialogs::default())
                .with_renderer(renderer::describe(cc));
            log_line(&format!("window: {}", renderer::describe(cc)));
            if screenshot.is_none() && !bench {
                app = app.with_watchdog(&cc.egui_ctx);
            }
            if let Some(status) = keymap_status {
                app = app.with_status(status);
            }
            app = match theme {
                Some(theme) => {
                    app.set_theme(&cc.egui_ctx, theme);
                    app
                }
                // Os modos de screenshot e medição não olham o sistema.
                None if screenshot.is_some() || bench => {
                    app.set_theme(&cc.egui_ctx, offline_theme);
                    app
                }
                None => app.with_system_theme(&cc.egui_ctx),
            };
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
            // Provedores de IA (Tools > AI Providers): as chaves vão direto
            // para o cofre do sistema, nunca para o do workspace.
            app = app.with_ai(match config_dir() {
                Some(dir) if screenshot.is_none() && !bench => pgstudio_ui_shell::ai::Ai::open(
                    &dir.join(pgstudio_core::CONFIG_DIR_NAME),
                    Some(Arc::new(KeyringStore::default())),
                ),
                _ => pgstudio_ui_shell::ai::Ai::system(),
            });
            if screenshot.is_none()
                && !bench
                && let Some(dir) = config_dir()
            {
                let file = dir.join(pgstudio_core::CONFIG_DIR_NAME).join("update.json");
                app = app.with_updater(&cc.egui_ctx, Updater::detect(file));
            }
            // Claude Code pelo MCP (Tools > Allow Claude Code), fora dos
            // modos de screenshot e medição, a menos que o canal seja dado.
            let explicit_endpoint = std::env::var_os(pgstudio_automation::endpoint::ENV).is_some();
            if ((screenshot.is_none() && !bench) || explicit_endpoint)
                && let (Some(endpoint), Some(dir)) =
                    (pgstudio_automation::endpoint::default_path(), config_dir())
            {
                let settings = dir.join(pgstudio_core::CONFIG_DIR_NAME).join("claude.json");
                app = app.with_claude_code(&cc.egui_ctx, endpoint, settings);
            }
            if let Some(root) = std::env::var_os("PGSTUDIO_WORKSPACE").map(PathBuf::from) {
                app.open_workspace(&root);
            } else if screenshot.is_none()
                && !bench
                && let Some(dir) = config_dir()
            {
                app = app.with_workspace_memory(
                    dir.join(pgstudio_core::CONFIG_DIR_NAME)
                        .join("workspace.txt"),
                );
            }
            // Os trechos do usuário que ficam só neste computador (os do
            // workspace ficam na pasta dele).
            if screenshot.is_none()
                && !bench
                && let Some(dir) = config_dir()
            {
                app = app
                    .with_snippets_dir(dir.join(pgstudio_core::CONFIG_DIR_NAME).join("snippets"));
            }
            if !demo {
                match DbRuntime::new() {
                    Ok(runtime) => {
                        app = app.with_recent_connections(recent).with_connector(Arc::new(
                            EngineConnector {
                                postgres: Arc::new(PgConnector::new(runtime.clone())),
                                sql_server: Arc::new(MssqlConnector::new(runtime)),
                            },
                        ));
                    }
                    Err(e) => {
                        eprintln!("could not start the database runtime: {e}");
                        app = app.with_status(tr("Could not start the database runtime."));
                    }
                }
                if let Some(servers) = connect.as_deref().filter(|c| !c.starts_with("dialog")) {
                    app = connect_from_env(app, servers);
                }
            }
            // A sessão anterior, depois do conector, do cofre e do workspace
            // (as senhas dos servidores vêm deles).
            let session_file = match std::env::var_os("PGSTUDIO_SESSION") {
                Some(file) => Some(PathBuf::from(file)),
                None if screenshot.is_none() && !bench && connect.is_none() => config_dir()
                    .map(|d| d.join(pgstudio_core::CONFIG_DIR_NAME).join("session.json")),
                None => None,
            };
            if let Some(file) = session_file {
                app = app.with_session(pgstudio_ui_shell::session::SessionStore::open(file));
            }
            // O histórico de consultas executadas, só neste computador.
            let history_file = match std::env::var_os("PGSTUDIO_HISTORY") {
                Some(file) => Some(PathBuf::from(file)),
                None if screenshot.is_none() && !bench => config_dir()
                    .map(|d| d.join(pgstudio_core::CONFIG_DIR_NAME).join("history.jsonl")),
                None => None,
            };
            if let Some(file) = history_file {
                app = app.with_history(pgstudio_ui_shell::history::HistoryStore::open(file));
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
            if std::env::var_os("PGSTUDIO_OPTIONS").is_some() {
                app.execute(&cc.egui_ctx, pgstudio_core::CommandId::Options);
            }
            if std::env::var_os("PGSTUDIO_DATABASES").is_some() {
                app = app.with_database_list_on_load();
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
            if let Ok(spec) = std::env::var("PGSTUDIO_JSON") {
                app = app.with_json_demo(spec);
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
/// `PGDATABASE`. O prefixo `sqlserver:` (`sqlserver:db,1433`) conecta num
/// SQL Server.
fn env_settings(server: &str) -> Option<ConnectionSettings> {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (engine, server) = match server.trim().strip_prefix("sqlserver:") {
        Some(rest) => (Engine::SqlServer, rest),
        None => (Engine::PostgreSql, server),
    };
    let mut settings = ConnectionSettings::for_engine(engine);
    if let Some(user) = env("PGUSER") {
        settings.user = user;
    }
    if let Some(db) = env("PGDATABASE") {
        settings.database = db;
    }
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
            return (
                keymap,
                Some(trf!("Could not read {path}.", path = path.display())),
            );
        }
    };
    let errors = keymap.apply_overrides(&text);
    for e in &errors {
        eprintln!("keymap: {}: {e}", path.display());
    }
    let status = (!errors.is_empty()).then(|| {
        trf!(
            "{n} invalid line(s) ignored in {path}.",
            n = errors.len(),
            path = path.display()
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
            .add_filter(tr("SQL Files (*.sql)"), &["sql"])
            .add_filter(tr("All Files (*.*)"), &["*"]);
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
    /// Os diálogos do sistema esperam a pessoa: a casca pergunta noutra
    /// thread para a janela não travar.
    fn blocks(&self) -> bool {
        true
    }

    fn save_path(&mut self, name: &str, current: Option<&Path>) -> Option<PathBuf> {
        let mut dialog = self
            .dialog()
            .set_title(tr("Save File As"))
            .set_file_name(name);
        if let Some(dir) = current.and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.save_file();
        self.remember(path)
    }

    fn open_path(&mut self) -> Option<PathBuf> {
        let path = self.dialog().set_title(tr("Open File")).pick_file();
        self.remember(path)
    }

    fn open_data_path(&mut self) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(tr("Import CSV"))
            .add_filter(tr("CSV and text files"), &["csv", "txt", "tsv", "tab"])
            .add_filter(tr("All Files (*.*)"), &["*"]);
        if let Some(dir) = &self.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.pick_file();
        self.remember(path)
    }

    fn save_results_path(&mut self, name: &str) -> Option<PathBuf> {
        let path = self
            .dialog()
            .set_title(tr("Save Grid Results"))
            .set_file_name(name)
            .add_filter(tr("CSV (Comma delimited)"), &["csv"])
            .add_filter(tr("Text (Tab delimited)"), &["tsv", "txt"])
            .add_filter("JSON", &["json"])
            .add_filter(tr("SQL INSERT statements"), &["sql"])
            .add_filter(tr("Excel Workbook"), &["xlsx"])
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
