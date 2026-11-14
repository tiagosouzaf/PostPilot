//! Testes do marco 5.4: Registered Servers, vários servidores no Object
//! Explorer e "Remember password", com um conector falso (sem banco).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::panels::Panel;
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers, ServerGroup};
use pgstudio_ui_shell::secrets::{MemoryStore, NoStore, SecretStore};

/// Aceita a senha "secret".
struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        std::thread::sleep(Duration::from_millis(10));
        if password != "secret" {
            return Err(ConnectError::Authentication(format!(
                "Login failed for user \"{}\".",
                s.user
            )));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "16.4".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn settings(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: "alice".into(),
        ..Default::default()
    }
}

/// Árvore com o grupo "Production" e dois servidores nele.
fn tree() -> RegisteredServers {
    let mut r = RegisteredServers::default();
    let g = r
        .add_group(
            &[],
            ServerGroup {
                name: "Production".into(),
                ..Default::default()
            },
        )
        .unwrap();
    for host in ["sales-db", "hr-db"] {
        r.add_server(
            &g,
            RegisteredServer {
                name: host.into(),
                description: String::new(),
                settings: settings(host),
            },
        )
        .unwrap();
    }
    r
}

fn harness(
    store: Arc<dyn SecretStore>,
    recent: Vec<ConnectionSettings>,
) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_secret_store(store)
        .with_registered_servers(tree())
        .with_recent_connections(RecentConnections::in_memory(recent))
        .with_connector(Arc::new(FakeConnector));
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

/// Roda frames até o diálogo parar de procurar a senha e de conectar.
fn settle(harness: &mut Harness<'static, ShellApp>) {
    let start = Instant::now();
    loop {
        harness.step();
        let busy = harness
            .state()
            .connect_dialog()
            .is_some_and(|d| d.is_connecting() || d.is_looking_up());
        if !busy {
            // Um frame a mais: a senha achada vira um pedido de conexão.
            harness.step();
            let busy = harness
                .state()
                .connect_dialog()
                .is_some_and(|d| d.is_connecting() || d.is_looking_up());
            if !busy {
                break;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(5), "never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn connect_with_dialog(harness: &mut Harness<'static, ShellApp>, host: &str, remember: bool) {
    let ctx = harness.ctx.clone();
    if harness.state().connect_dialog().is_none() {
        harness
            .state_mut()
            .execute(&ctx, CommandId::ConnectObjectExplorer);
    }
    settle(harness);
    let d = harness.state_mut().connect_dialog_mut().unwrap();
    d.server = host.into();
    d.user = "alice".into();
    d.password = "secret".into();
    d.remember = remember;
    harness.run();
    harness.get_by_label("Connect").click();
    settle(harness);
    assert!(
        harness.state().connect_dialog().is_none(),
        "connected to {host}"
    );
}

#[test]
fn ctrl_alt_g_opens_registered_servers_with_groups() {
    let mut harness = harness(Arc::new(MemoryStore::default()), Vec::new());
    harness.get_by_label("Cancel").click();
    harness.run();
    harness.key_press_modifiers(Modifiers::CTRL | Modifiers::ALT, Key::G);
    harness.run();
    assert!(harness.state().is_panel_open(Panel::RegisteredServers));
    assert!(
        harness
            .state()
            .panels_grouped(Panel::RegisteredServers, Panel::ObjectExplorer)
    );
    harness.get_by_label_contains("Local Server Groups");
    harness.get_by_label_contains("Production");
    harness.get_by_label_contains("sales-db");
    harness.get_by_label_contains("hr-db");
}

#[test]
fn remembered_password_reconnects_after_a_restart_without_asking() {
    // A "máquina": o cofre sobrevive ao reinício do app.
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let mut first = harness(store.clone(), Vec::new());
    connect_with_dialog(&mut first, "sales-db", true);
    wait_for("password saved", || {
        store.get("alice@sales-db:5432").unwrap().as_deref() == Some("secret")
    });
    drop(first);

    // Reinício: o "Connect to Server" já vem com a senha.
    let mut second = harness(store.clone(), vec![settings("sales-db")]);
    settle(&mut second);
    let d = second.state().connect_dialog().unwrap();
    assert_eq!(d.password, "secret");
    assert!(d.remember && d.password_was_saved());
    second.get_by_label("Cancel").click();
    second.run();

    // E o servidor registrado conecta sem diálogo para preencher.
    second
        .state_mut()
        .connect_registered(&settings("sales-db"), false);
    settle(&mut second);
    assert!(second.state().connect_dialog().is_none());
    let c = second.state().connection().expect("connected");
    assert_eq!(c.settings.host, "sales-db");
}

#[test]
fn without_a_keyring_the_password_is_asked_without_errors() {
    let mut harness = harness(Arc::new(NoStore), Vec::new());
    harness.get_by_label("Cancel").click();
    harness.run();
    harness
        .state_mut()
        .connect_registered(&settings("hr-db"), false);
    settle(&mut harness);
    let d = harness
        .state()
        .connect_dialog()
        .expect("asks for the password");
    assert_eq!(d.server, "hr-db");
    assert!(d.password.is_empty());
    assert!(d.error().is_none());
    assert!(harness.state().connection().is_none());

    // Com "Remember password" marcado, conectar funciona mesmo sem cofre.
    let d = harness.state_mut().connect_dialog_mut().unwrap();
    d.password = "secret".into();
    d.remember = true;
    harness.run();
    harness.get_by_label("Connect").click();
    settle(&mut harness);
    assert_eq!(harness.state().connection().unwrap().settings.host, "hr-db");
}

#[test]
fn unchecking_remember_password_forgets_it() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    store.set("alice@sales-db:5432", "secret").unwrap();
    let mut harness = harness(store.clone(), vec![settings("sales-db")]);
    settle(&mut harness);
    let d = harness.state_mut().connect_dialog_mut().unwrap();
    assert_eq!(d.password, "secret");
    d.remember = false;
    harness.run();
    harness.get_by_label("Connect").click();
    settle(&mut harness);
    assert!(harness.state().connection().is_some());
    wait_for("password forgotten", || {
        store.get("alice@sales-db:5432").unwrap().is_none()
    });
}

#[test]
fn several_servers_share_the_object_explorer() {
    let mut harness = harness(Arc::new(MemoryStore::default()), Vec::new());
    connect_with_dialog(&mut harness, "sales-db", false);
    connect_with_dialog(&mut harness, "hr-db", false);
    connect_with_dialog(&mut harness, "web-db", false);
    let servers = harness.state().connected_servers();
    assert_eq!(
        servers,
        [
            ("sales-db".to_string(), false),
            ("hr-db".to_string(), false),
            ("web-db".to_string(), true)
        ]
    );
    // Uma árvore por servidor.
    let roots = harness
        .query_all_by_label_contains(pgstudio_object_explorer::fake::SERVER_NAME)
        .count();
    assert_eq!(roots, 3);

    // Conectar de novo um servidor já aberto não duplica, só o traz para a
    // frente.
    harness
        .state_mut()
        .connect_registered(&settings("sales-db"), false);
    settle(&mut harness);
    assert!(harness.state().connect_dialog().is_none());
    assert_eq!(harness.state().connected_servers().len(), 3);
    assert_eq!(
        harness.state().connection().unwrap().settings.host,
        "sales-db"
    );

    // Uma janela de consulta fica com o servidor em que foi aberta.
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    assert!(harness.state_mut().activate_key("alice@hr-db:5432"));
    harness.run();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert_eq!(
        harness.state().connection().unwrap().settings.host,
        "sales-db",
        "F5 brings the window's server back"
    );

    // Desconectar o ativo deixa os outros.
    harness
        .state_mut()
        .execute(&ctx, CommandId::DisconnectObjectExplorer);
    harness.run();
    let servers = harness.state().connected_servers();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0], ("hr-db".to_string(), true));
    assert!(harness.state().explorer().is_connected());
}

#[test]
fn clicking_another_tree_makes_its_server_active() {
    let mut harness = harness(Arc::new(MemoryStore::default()), Vec::new());
    connect_with_dialog(&mut harness, "sales-db", false);
    connect_with_dialog(&mut harness, "hr-db", false);
    assert_eq!(harness.state().connection().unwrap().settings.host, "hr-db");
    // A primeira árvore é a do sales-db.
    let first = harness
        .query_all_by_label_contains(pgstudio_object_explorer::fake::SERVER_NAME)
        .next()
        .unwrap();
    first.click();
    harness.run();
    assert_eq!(
        harness.state().connection().unwrap().settings.host,
        "sales-db"
    );
    assert_eq!(harness.state().connection_label(), "sales-db (alice)");
}

#[test]
fn registered_new_query_connects_and_opens_a_window() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    store.set("alice@hr-db:5432", "secret").unwrap();
    let mut harness = harness(store, Vec::new());
    harness.get_by_label("Cancel").click();
    harness.run();
    harness
        .state_mut()
        .connect_registered(&settings("hr-db"), true);
    settle(&mut harness);
    harness.run();
    assert_eq!(harness.state().connection().unwrap().settings.host, "hr-db");
    assert_eq!(harness.state().documents().ids().len(), 1);
}

// --- PostgreSQL de verdade ---

/// O servidor de teste com outro nome de host: `localhost` e `127.0.0.1`
/// são dois servidores para o Object Explorer.
fn real_servers() -> Option<(ConnectionSettings, ConnectionSettings, String)> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let a = ConnectionSettings {
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: pgstudio_core::connection::SslMode::Disable,
    };
    let other = if a.host == "127.0.0.1" {
        "localhost"
    } else {
        "127.0.0.1"
    };
    let b = ConnectionSettings {
        host: other.into(),
        ..a.clone()
    };
    Some((a, b, var("PGSTUDIO_TEST_PASSWORD", "")))
}

fn real_harness(store: Arc<dyn SecretStore>) -> Harness<'static, ShellApp> {
    let runtime = pgstudio_pg_metadata::Runtime::new().unwrap();
    let app = ShellApp::default()
        .with_secret_store(store)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(pgstudio_pg_metadata::PgConnector::new(runtime)));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    settle(&mut harness);
    harness.get_by_label("Cancel").click();
    harness.run();
    harness
}

#[test]
fn two_real_servers_and_a_restart_with_the_remembered_password() {
    let Some((a, b, password)) = real_servers() else {
        return;
    };
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let mut harness = real_harness(store.clone());
    for s in [&a, &b] {
        harness.state_mut().connect_registered(s, false);
        settle(&mut harness);
        // Primeira vez: o cofre está vazio e o diálogo pede a senha.
        let d = harness.state_mut().connect_dialog_mut().expect("asks");
        assert!(d.password.is_empty() && d.error().is_none());
        d.password = password.clone();
        d.remember = true;
        harness.run();
        harness.get_by_label("Connect").click();
        settle(&mut harness);
        assert!(
            harness.state().connect_dialog().is_none(),
            "{:?}",
            harness.state().output()
        );
        assert!(
            harness
                .state_mut()
                .explorer_mut()
                .wait_idle(Duration::from_secs(10))
        );
    }
    let names: Vec<_> = harness
        .state()
        .connected_servers()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, [a.server_name(), b.server_name()]);
    harness
        .state_mut()
        .activate_key(&format!("{}@{}:{}", a.user, a.host, a.port));
    harness.run();
    assert!(matches!(
        harness.state().explorer().roots(),
        pgstudio_object_explorer::Load::Loaded(r) if !r.is_empty()
    ));
    wait_for("passwords saved", || {
        store
            .get(&format!("{}@{}:{}", b.user, b.host, b.port))
            .unwrap()
            .is_some()
    });
    drop(harness);

    // Reinício: os dois reconectam sem pedir a senha.
    let mut harness = real_harness(store);
    for s in [&a, &b] {
        harness.state_mut().connect_registered(s, false);
        settle(&mut harness);
        assert!(harness.state().connect_dialog().is_none());
    }
    assert_eq!(harness.state().connected_servers().len(), 2);
}
