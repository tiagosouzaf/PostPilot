//! Capturas de tela das conexões em segundo plano: as bolinhas de estado no
//! Pesquisador de Objetos e a opção em Ferramentas > Opções. Só com
//! `--features screenshots`; os PNGs vão para `PGSTUDIO_SHOTS`.
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::{PRODUCTION, RegisteredServer, RegisteredServers, ServerGroup};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};

#[derive(Default)]
struct FakeConnector {
    release: AtomicBool,
}

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        while s.host.starts_with("vpn") && !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if s.host.starts_with("antigo") {
            return Err(ConnectError::Network(
                "connection refused (10.20.0.8:5432)".into(),
            ));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "16.4".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    h.run_steps(3);
    let image = h.render().expect("render");
    image.save(out(name)).expect("save png");
}

fn settings(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: "tiago".into(),
        ..ConnectionSettings::default()
    }
}

/// Nome, host, ambiente e se a senha está lembrada.
type Saved = (&'static str, &'static str, Option<&'static str>, bool);

fn start(theme: pgstudio_core::Theme, connector: Arc<FakeConnector>) -> Harness<'static, ShellApp> {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut servers = RegisteredServers::default();
    let store = Arc::new(MemoryStore::default());
    let clients: [(&str, &[Saved]); 2] = [
        (
            "Cliente Acme",
            &[
                ("acme-prd", "db.acme.com.br", Some(PRODUCTION), true),
                ("acme-hml", "hml.acme.com.br", None, true),
                ("acme-antigo", "antigo.acme.com.br", None, true),
            ],
        ),
        (
            "Cliente Borealis",
            &[
                ("bore-prd", "vpn.borealis.io", None, true),
                ("bore-bi", "bi.borealis.io", None, false),
            ],
        ),
    ];
    for (client, list) in clients {
        let g = servers
            .add_group(
                &[],
                ServerGroup {
                    name: client.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        for (name, host, environment, remembered) in list {
            let settings = settings(host);
            if *remembered {
                store.set(&password_key(&settings), "senha").unwrap();
            }
            servers
                .add_server(
                    &g,
                    RegisteredServer {
                        name: (*name).into(),
                        description: String::new(),
                        environment: environment.map(str::to_string),
                        settings,
                    },
                )
                .unwrap();
        }
    }
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(servers)
        .with_connector(connector)
        .with_secret_store(store)
        .with_background_connect();
    let mut h = Harness::builder()
        .with_size([1100.0, 620.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    let ctx = h.ctx.clone();
    h.state_mut().set_theme(&ctx, theme);
    h
}

fn wait(h: &mut Harness<'static, ShellApp>, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    loop {
        h.step();
        if done(h.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn status_dots_in_the_object_explorer() {
    for (theme, name) in [
        (pgstudio_core::Theme::Dark, "bolinhas-escuro.png"),
        (pgstudio_core::Theme::Light, "bolinhas-claro.png"),
    ] {
        let connector = Arc::new(FakeConnector::default());
        let mut h = start(theme, connector.clone());
        // Dois conectados, um ainda conectando (VPN lenta), um que falhou e
        // um sem senha guardada.
        wait(&mut h, |app| app.connected_servers().len() == 2);
        save(&mut h, name);
        connector.release.store(true, Ordering::SeqCst);
        wait(&mut h, |app| !app.background().is_busy());
    }
}

#[test]
fn the_option_in_tools_options() {
    let connector = Arc::new(FakeConnector::default());
    connector.release.store(true, Ordering::SeqCst);
    let mut h = start(pgstudio_core::Theme::Dark, connector);
    wait(&mut h, |app| !app.background().is_busy());
    let ctx = h.ctx.clone();
    h.state_mut()
        .execute(&ctx, pgstudio_core::CommandId::Options);
    h.run_steps(3);
    h.get_by_label("Geral").click();
    save(&mut h, "opcao.png");
}
