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
                environment: None,
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

/// A pesquisa da janela Registered Servers: filtra a árvore, mantém a
/// pasta do que achou e Enter conecta o primeiro.
#[test]
fn search_filters_the_tree_and_enter_connects_the_first() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    store.set("alice@hr-db:5432", "secret").unwrap();
    let mut harness = harness(store, Vec::new());
    harness.get_by_label("Cancel").click();
    harness.run();
    harness.key_press_modifiers(Modifiers::CTRL | Modifiers::ALT, Key::G);
    harness.run();
    let shown = |h: &Harness<'static, ShellApp>, label: &str| {
        h.query_all_by_label_contains(label).count() > 0
    };
    let field = eframe::egui::accesskit::Role::TextInput;
    harness.get_by_role(field).focus();
    harness.run();
    harness.get_by_role(field).type_text("nada disso");
    harness.run();
    assert!(shown(&harness, "No servers found."));
    assert!(!shown(&harness, "sales-db"));
    assert!(!shown(&harness, "Production"));

    // Pelo usuário (a árvore só mostra o nome): os dois servidores.
    harness.state_mut().registered_mut().filter = "ALICE".into();
    harness.run();
    assert!(shown(&harness, "2 servers found."));
    assert!(shown(&harness, "user: alice"));

    // Pasta + nome, em qualquer ordem.
    harness.state_mut().registered_mut().filter.clear();
    harness.run();
    harness.get_by_role(field).focus();
    harness.run();
    harness.get_by_role(field).type_text("hr prod");
    harness.run();
    assert!(shown(&harness, "1 server found."));
    assert!(shown(&harness, "hr-db"));
    assert!(shown(&harness, "Production"));
    assert!(!shown(&harness, "sales-db"));

    harness.key_press(Key::Enter);
    harness.step();
    settle(&mut harness);
    assert!(harness.state().connect_dialog().is_none());
    assert_eq!(harness.state().connection().unwrap().settings.host, "hr-db");
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
    // As dos servidores registrados mostram o nome do registro; a do
    // web-db, que não está registrado, o nome do servidor.
    let roots = harness
        .query_all_by_label_contains(pgstudio_object_explorer::fake::SERVER_NAME)
        .count();
    assert_eq!(roots, 1);
    for name in [" sales-db", " hr-db"] {
        harness.get_by(|n| n.label().is_some_and(|l| l.ends_with(name)));
    }

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
    // (Registrado, o nó mostra o nome do registro.)
    let first = harness.get_by(|n| n.label().is_some_and(|l| l.ends_with(" sales-db")));
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
        engine: Default::default(),
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

// --- Marco 6.3: arrastar, Register..., Import/Export e a faixa de status ---

/// Diálogos de arquivo com respostas prontas.
struct Answer(std::path::PathBuf);

impl pgstudio_ui_shell::documents::FileDialogs for Answer {
    fn save_path(&mut self, _: &str, _: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        Some(self.0.clone())
    }

    fn open_path(&mut self) -> Option<std::path::PathBuf> {
        Some(self.0.clone())
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pgstudio-6.3-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Abre a janela Registered Servers, com a árvore deste arquivo.
fn registered_harness(file: &std::path::Path, dialogs: Answer) -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::open(file))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_file_dialogs(dialogs);
    app.show_registered_servers();
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    harness.get_by_label("Cancel").click();
    harness.run();
    harness
}

#[test]
fn dragging_a_server_to_another_group_survives_a_restart() {
    let dir = temp_dir("drag");
    let file = dir.join("registered_servers.json");
    let mut r = RegisteredServers::open(&file);
    r.add_group(
        &[],
        ServerGroup {
            name: "Production".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let dev = r
        .add_group(
            &[],
            ServerGroup {
                name: "Development".into(),
                ..Default::default()
            },
        )
        .unwrap();
    r.add_server(
        &[0],
        RegisteredServer {
            name: "sales-db".into(),
            description: String::new(),
            environment: None,
            settings: settings("sales-db"),
        },
    )
    .unwrap();
    drop(r);

    let mut harness = registered_harness(&file, Answer(dir.join("unused")));
    let from = harness.get_by_label_contains("sales-db").rect().center();
    let to = harness.get_by_label_contains("Development").rect().center();
    harness.drag_at(from);
    harness.run();
    // O arrasto começa depois de o ponteiro andar um pouco.
    for i in 1..=10 {
        harness.hover_at(from + (to - from) * (i as f32 / 10.0));
        harness.run();
    }
    harness.drop_at(to);
    harness.run();
    let r = harness.state().registered().servers();
    assert!(r.group(&[0]).unwrap().servers.is_empty(), "left Production");
    assert_eq!(r.group(&dev).unwrap().servers[0].name, "sales-db");
    assert!(
        harness.state().status().contains("Moved"),
        "{}",
        harness.state().status()
    );
    drop(harness);

    let again = RegisteredServers::open(&file);
    assert_eq!(again.group(&dev).unwrap().servers[0].name, "sales-db");
    assert!(again.group(&[0]).unwrap().servers.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn move_to_menu_and_folder_color_survive_a_restart() {
    let dir = temp_dir("move-to");
    let file = dir.join("registered_servers.json");
    let mut r = RegisteredServers::open(&file);
    for name in ["Production", "Development"] {
        r.add_group(
            &[],
            ServerGroup {
                name: name.into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    r.add_server(
        &[0],
        RegisteredServer {
            name: "sales-db".into(),
            description: String::new(),
            environment: None,
            settings: settings("sales-db"),
        },
    )
    .unwrap();
    drop(r);

    let mut harness = registered_harness(&file, Answer(dir.join("unused")));
    // Botão direito no servidor > Move To > Development.
    harness.get_by_label_contains("sales-db").click_secondary();
    harness.run();
    harness.get_by_label_contains("Move To").hover();
    harness.run();
    harness
        .query_all_by_label_contains("Development")
        .last()
        .unwrap()
        .click();
    harness.run();
    let r = harness.state().registered().servers();
    assert!(r.group(&[0]).unwrap().servers.is_empty(), "left Production");
    assert_eq!(r.group(&[1]).unwrap().servers[0].name, "sales-db");

    // Properties da pasta Production: cor vermelha.
    harness
        .query_all_by_label_contains("Production")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label("Properties").click();
    harness.run();
    harness.get_by_label("#e5534b").click();
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .registered()
            .servers()
            .group(&[0])
            .unwrap()
            .color,
        Some(pgstudio_ui_shell::registered::FOLDER_COLORS[0])
    );
    drop(harness);

    let again = RegisteredServers::open(&file);
    assert_eq!(again.group(&[1]).unwrap().servers[0].name, "sales-db");
    assert_eq!(
        again.group(&[0]).unwrap().color.map(|c| c.hex()),
        Some("#e5534b".to_string())
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn export_then_import_from_the_group_menu() {
    let dir = temp_dir("export");
    let exported = dir.join("RegisteredServers.json");
    let file = dir.join("registered_servers.json");
    {
        let mut r = RegisteredServers::open(&file);
        let t = tree();
        for g in &t.root().groups {
            r.add_group(&[], g.clone()).unwrap();
        }
    }
    let mut harness = registered_harness(&file, Answer(exported.clone()));
    // A primeira: a árvore vazia tem uma dica que cita o grupo raiz.
    harness
        .query_all_by_label_contains("Local Server Groups")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label_contains("Tasks").hover();
    harness.run();
    harness.get_by_label("Export...").click();
    harness.run();
    let text = std::fs::read_to_string(&exported).unwrap();
    assert!(text.contains("sales-db") && !text.to_lowercase().contains("password"));
    let original = harness.state().registered().servers().root().clone();
    drop(harness);

    // Outra máquina: árvore vazia, importa o arquivo.
    let other = dir.join("other.json");
    let mut harness = registered_harness(&other, Answer(exported));
    // A primeira: a árvore vazia tem uma dica que cita o grupo raiz.
    harness
        .query_all_by_label_contains("Local Server Groups")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label_contains("Tasks").hover();
    harness.run();
    harness.get_by_label("Import...").click();
    harness.run();
    assert_eq!(harness.state().status(), "Imported 2 registered servers.");
    assert_eq!(harness.state().registered().servers().root(), &original);
    harness.get_by_label_contains("hr-db");
    drop(harness);
    assert_eq!(RegisteredServers::open(&other).root(), &original);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn register_from_the_object_explorer_server_menu() {
    let dir = temp_dir("register");
    let file = dir.join("registered_servers.json");
    let mut harness = registered_harness(&file, Answer(dir.join("unused")));
    connect_with_dialog(&mut harness, "web-db", false);
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::ObjectExplorer);
    harness.run();
    harness
        .get_by_label_contains(pgstudio_object_explorer::fake::SERVER_NAME)
        .click_secondary();
    harness.run();
    harness.get_by_label("Register...").click();
    harness.run();
    assert!(harness.state().is_panel_open(Panel::RegisteredServers));
    let d = harness
        .state_mut()
        .registered_mut()
        .server_dialog_mut()
        .expect("New Server Registration");
    assert_eq!((d.server.as_str(), d.user.as_str()), ("web-db", "alice"));
    harness.get_by_label("Save").click();
    harness.run();
    let r = RegisteredServers::open(&file);
    assert_eq!(r.root().servers[0].settings, settings("web-db"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn query_status_strip_shows_the_window_server_not_the_active_one() {
    let mut harness = harness(Arc::new(MemoryStore::default()), Vec::new());
    connect_with_dialog(&mut harness, "sales-db", false);
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    connect_with_dialog(&mut harness, "hr-db", false);
    // O hr-db é o ativo, mas a janela aberta é do sales-db.
    harness.run();
    assert_eq!(harness.state().connection().unwrap().settings.host, "hr-db");
    harness.get_by_label("Connected.");
    harness.get_by_label("sales-db (alice)");
}

#[test]
fn query_windows_show_the_environment_of_their_registered_server() {
    use pgstudio_ui_shell::registered::{DEVELOPMENT, NodePath, PRODUCTION};
    // A pasta Production é do ambiente Production; o hr-db, dentro dela,
    // é Development.
    let mut r = tree();
    r.update_group(&[0], "Production", "", None, Some(PRODUCTION.into()))
        .unwrap();
    let hr_path = NodePath::server(&[0], 1);
    let mut hr = r.server(&hr_path).unwrap().clone();
    assert_eq!(hr.name, "hr-db");
    hr.environment = Some(DEVELOPMENT.into());
    r.update_server(&hr_path, hr).unwrap();
    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(r)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);

    connect_with_dialog(&mut harness, "sales-db", false);
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    let tag = harness.state().active_tag().expect("sales-db has a tag");
    assert_eq!(tag.environment.as_deref(), Some("Production"));
    // Na faixa da janela e na status bar (a faixa pode levar uns frames).
    let start = std::time::Instant::now();
    while harness.query_all_by_label("Production").count() < 2 {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "Production on the strip and the status bar"
        );
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    connect_with_dialog(&mut harness, "hr-db", false);
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    let tag = harness.state().active_tag().expect("hr-db has a tag");
    assert_eq!(tag.environment.as_deref(), Some("Development"));
    let mut envs: Vec<_> = harness
        .state()
        .document_tags()
        .into_values()
        .filter_map(|t| t.environment)
        .collect();
    envs.sort();
    assert_eq!(envs, ["Development", "Production"]);

    // Uma conexão que não está nos Registered Servers não tem etiqueta.
    connect_with_dialog(&mut harness, "web-db", false);
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    assert_eq!(harness.state().active_tag(), None);
}

#[test]
fn production_asks_before_running_sql_that_changes_the_database() {
    use pgstudio_ui_shell::registered::{NodePath, PRODUCTION};
    // sales-db é Production (pede confirmação); hr-db fica sem ambiente.
    let mut r = tree();
    let sales = NodePath::server(&[0], 0);
    let mut s = r.server(&sales).unwrap().clone();
    s.environment = Some(PRODUCTION.into());
    r.update_server(&sales, s).unwrap();
    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(r)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let ctx = harness.ctx.clone();
    connect_with_dialog(&mut harness, "sales-db", false);
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    let doc = harness.state().active_document().unwrap();
    let set_text = |h: &mut Harness<'static, ShellApp>, sql: &str| {
        h.state_mut().documents_mut().get_mut(doc).unwrap().text = sql.into();
    };

    // Leitura roda sem perguntar.
    set_text(&mut harness, "select 1");
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert!(!harness.state().is_confirming_environment());

    // UPDATE pergunta; Cancel não roda.
    set_text(&mut harness, "update pedidos set status = 'pago'");
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert!(harness.state().is_confirming_environment());
    harness.get_by_label("Confirm Changes");
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(!harness.state().is_confirming_environment());
    assert_eq!(harness.state().status(), "The query was not executed.");

    // Execute roda.
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    harness.get_by_label("Execute").click();
    harness.run();
    assert!(!harness.state().is_confirming_environment());
    assert_ne!(harness.state().status(), "The query was not executed.");

    // Fora de Production, DDL roda sem perguntar.
    connect_with_dialog(&mut harness, "hr-db", false);
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    let hr_doc = harness.state().active_document().unwrap();
    harness
        .state_mut()
        .documents_mut()
        .get_mut(hr_doc)
        .unwrap()
        .text = "drop table pedidos".into();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert!(!harness.state().is_confirming_environment());
}
