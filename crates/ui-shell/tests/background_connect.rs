//! Ao abrir o app, os servidores guardados com a senha lembrada conectam em
//! segundo plano: sem diálogo, poucos de cada vez, e um servidor lento ou
//! fora do ar não segura os outros nem a janela.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::autoconnect::{LinkState, MAX_PARALLEL};
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::preferences::Preferences;
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};
use pgstudio_ui_shell::session::SessionStore;

const PASSWORD: &str = "s3cr3t";

/// Conector falso: `down*` recusa, `slow*` espera a liberação; conta quantas
/// conexões estão em andamento ao mesmo tempo.
#[derive(Default)]
struct FakeConnector {
    running: AtomicUsize,
    peak: AtomicUsize,
    attempts: AtomicUsize,
    release: AtomicBool,
    /// Sem rede: toda conexão é recusada.
    offline: AtomicBool,
    /// Quanto cada conexão demora.
    delay: Duration,
}

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        while s.host.starts_with("slow") && !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.running.fetch_sub(1, Ordering::SeqCst);
        if s.host.starts_with("down") || self.offline.load(Ordering::SeqCst) {
            return Err(ConnectError::Other("connection refused".into()));
        }
        if password != PASSWORD {
            return Err(ConnectError::Authentication("Login failed.".into()));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn settings(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        ..ConnectionSettings::default()
    }
}

/// A chave da conexão (com o banco): a do estado de cada servidor.
fn key(host: &str) -> String {
    pgstudio_ui_shell::servers::server_key(&settings(host))
}

/// Servidores registrados com estes hosts; os de `remembered` têm a senha
/// no cofre.
fn start(
    hosts: &[&str],
    remembered: &[&str],
    connector: Arc<FakeConnector>,
    prefs: Preferences,
) -> Harness<'static, ShellApp> {
    start_with_session(hosts, remembered, connector, prefs, None)
}

fn start_with_session(
    hosts: &[&str],
    remembered: &[&str],
    connector: Arc<FakeConnector>,
    prefs: Preferences,
    session: Option<&std::path::Path>,
) -> Harness<'static, ShellApp> {
    let mut servers = RegisteredServers::default();
    for host in hosts {
        servers
            .add_server(
                &[],
                RegisteredServer {
                    name: format!("srv {host}"),
                    description: String::new(),
                    settings: settings(host),
                    environment: None,
                },
            )
            .unwrap();
    }
    let store = Arc::new(MemoryStore::default());
    for host in remembered {
        store.set(&password_key(&settings(host)), PASSWORD).unwrap();
    }
    let mut app = ShellApp::default()
        .with_preferences(None, prefs)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(servers)
        .with_connector(connector)
        .with_secret_store(store);
    if let Some(file) = session {
        app = app.with_session(SessionStore::open(file));
    }
    let app = app.with_background_connect();
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

fn wait(harness: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    loop {
        harness.step();
        if done(harness.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.step();
}

fn connected(app: &ShellApp) -> Vec<String> {
    app.connected_servers()
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

#[test]
fn saved_servers_with_a_password_connect_without_any_dialog() {
    let connector = Arc::new(FakeConnector::default());
    let mut h = start(
        &["alpha", "beta", "nopass", "down1"],
        &["alpha", "beta", "down1"],
        connector.clone(),
        Preferences::default(),
    );
    wait(&mut h, "background connections", |app| {
        !app.background().is_busy()
    });
    let app = h.state();
    // Nenhum diálogo: nem o "Connect to Server" de toda abertura.
    assert!(app.connect_dialog().is_none());
    let mut names = connected(app);
    names.sort();
    assert_eq!(names, ["alpha", "beta"]);
    // Os conectados não têm estado pendente; os outros dizem por quê.
    assert_eq!(app.background().state(&key("alpha")), None);
    assert_eq!(
        app.background().state(&key("nopass")),
        Some(&LinkState::NoPassword)
    );
    assert_eq!(
        app.background().state(&key("down1")),
        Some(&LinkState::Failed("connection refused".into()))
    );
    // Sem senha guardada, nem tenta.
    assert_eq!(connector.attempts.load(Ordering::SeqCst), 3);
    // O que falhou não fica tentando sozinho.
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(connector.attempts.load(Ordering::SeqCst), 3);
    assert!(h.state().connect_dialog().is_none());
}

#[test]
fn a_slow_server_does_not_hold_the_window_or_the_others() {
    let connector = Arc::new(FakeConnector::default());
    let mut h = start(
        &["slow1", "alpha", "beta"],
        &["slow1", "alpha", "beta"],
        connector.clone(),
        Preferences::default(),
    );
    // Os outros conectam com o lento ainda esperando; os frames continuam.
    wait(&mut h, "the fast ones", |app| connected(app).len() == 2);
    assert_eq!(
        h.state().background().state(&key("slow1")),
        Some(&LinkState::Connecting)
    );
    assert!(h.state().background().is_busy());
    connector.release.store(true, Ordering::SeqCst);
    wait(&mut h, "the slow one", |app| connected(app).len() == 3);
    assert!(!h.state().background().is_busy());
    // O primeiro que conectou continua sendo o ativo.
    assert_eq!(
        h.state()
            .connected_servers()
            .iter()
            .filter(|(_, active)| *active)
            .count(),
        1
    );
}

#[test]
fn only_a_few_connect_at_the_same_time() {
    let connector = Arc::new(FakeConnector {
        delay: Duration::from_millis(40),
        ..FakeConnector::default()
    });
    let hosts: Vec<String> = (0..11).map(|i| format!("host{i}")).collect();
    let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let mut h = start(&hosts, &hosts, connector.clone(), Preferences::default());
    wait(&mut h, "all of them", |app| connected(app).len() == 11);
    let peak = connector.peak.load(Ordering::SeqCst);
    assert!(peak > 1, "connections ran one at a time");
    assert!(peak <= MAX_PARALLEL, "{peak} connections at once");
}

#[test]
fn the_option_turns_it_off() {
    let connector = Arc::new(FakeConnector::default());
    let prefs = Preferences {
        auto_connect: false,
        ..Preferences::default()
    };
    let mut h = start(&["alpha"], &["alpha"], connector.clone(), prefs);
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(connected(h.state()).is_empty());
    assert_eq!(connector.attempts.load(Ordering::SeqCst), 0);
    // O "Connect to Server" de sempre continua lá.
    assert!(h.state().connect_dialog().is_some());
}

#[test]
fn a_failed_server_connects_with_a_double_click_later() {
    let connector = Arc::new(FakeConnector::default());
    let mut h = start(
        &["alpha", "down1"],
        &["alpha", "down1"],
        connector,
        Preferences::default(),
    );
    wait(&mut h, "background connections", |app| {
        !app.background().is_busy()
    });
    assert!(matches!(
        h.state().background().state(&key("down1")),
        Some(LinkState::Failed(_))
    ));
    // Um servidor desconectado à mão não reconecta sozinho.
    let ctx = h.ctx.clone();
    h.state_mut()
        .execute(&ctx, pgstudio_core::CommandId::DisconnectObjectExplorer);
    for _ in 0..20 {
        h.step();
    }
    assert!(connected(h.state()).is_empty());
    // O duplo clique (o mesmo pedido) conecta pelo caminho de sempre.
    h.state_mut().connect_registered(&settings("alpha"), false);
    wait(&mut h, "alpha again", |app| connected(app) == ["alpha"]);
}

#[test]
fn a_session_server_that_is_offline_stays_in_the_session() {
    let dir = std::env::temp_dir().join(format!("postpilot-bg-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("session.json");
    let run = |offline: bool| {
        let connector = Arc::new(FakeConnector::default());
        connector.offline.store(offline, Ordering::SeqCst);
        start_with_session(
            &["alpha", "beta"],
            &["alpha", "beta"],
            connector,
            Preferences::default(),
            Some(&file),
        )
    };
    // Primeira vez: os dois conectam e ficam na sessão.
    let mut h = run(false);
    wait(&mut h, "both", |app| connected(app).len() == 2);
    h.state_mut().save_session();
    assert_eq!(h.state().session_state().servers.len(), 2);
    drop(h);
    // Sem rede: ninguém conecta, nenhum diálogo aparece, e a sessão não
    // esquece os servidores.
    let mut h = run(true);
    wait(&mut h, "both failed", |app| !app.background().is_busy());
    for _ in 0..10 {
        h.step();
    }
    assert!(h.state().connect_dialog().is_none());
    assert!(connected(h.state()).is_empty());
    assert!(matches!(
        h.state().background().state(&key("alpha")),
        Some(LinkState::Failed(_))
    ));
    h.state_mut().save_session();
    assert_eq!(h.state().session_state().servers.len(), 2);
    drop(h);
    // A rede voltou: os dois voltam sozinhos.
    let mut h = run(false);
    wait(&mut h, "both again", |app| connected(app).len() == 2);
    let _ = std::fs::remove_dir_all(&dir);
}
