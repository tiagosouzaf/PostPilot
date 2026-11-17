//! Vários registros do mesmo servidor e usuário, um por banco (como os
//! bancos atrás de um túnel em `127.0.0.1`): cada registro é uma conexão.
//! Antes o banco não entrava na identidade do servidor: o primeiro conectava
//! e os outros ficavam "sem conexão" para sempre, mesmo com o "Test
//! connection" passando. E salvar o registro (novo, ou corrigido depois de
//! uma falha) conecta na hora.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::autoconnect::LinkState;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::preferences::Preferences;
use pgstudio_ui_shell::registered::{NodePath, RegisteredServer, RegisteredServers};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};
use pgstudio_ui_shell::servers::server_key;

const PASSWORD: &str = "s3cr3t";

/// Conector falso: guarda os bancos em que conectou, na ordem.
#[derive(Default)]
struct FakeConnector {
    databases: Mutex<Vec<String>>,
}

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if password != PASSWORD {
            return Err(ConnectError::Authentication("Login failed.".into()));
        }
        self.databases.lock().unwrap().push(s.database.clone());
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.9".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

/// O mesmo usuário no mesmo servidor, no banco `database`.
fn on(database: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: "127.0.0.1".into(),
        user: "webged_novo_user".into(),
        database: database.into(),
        ..ConnectionSettings::default()
    }
}

fn registered(databases: &[&str]) -> RegisteredServers {
    let mut servers = RegisteredServers::default();
    for database in databases {
        servers
            .add_server(
                &[],
                RegisteredServer {
                    name: format!("{database}@Jucesp_127.0.0.1"),
                    description: String::new(),
                    settings: on(database),
                    environment: None,
                },
            )
            .unwrap();
    }
    servers
}

fn start(
    databases: &[&str],
    store: Arc<MemoryStore>,
    connector: Arc<FakeConnector>,
    background: bool,
) -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default()
        .with_preferences(None, Preferences::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(registered(databases))
        .with_connector(connector)
        .with_secret_store(store);
    if background {
        app = app.with_background_connect();
    }
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

fn remembered() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::default());
    // Uma senha só: ela é do usuário no servidor, não de cada banco.
    store.set(&password_key(&on("webged")), PASSWORD).unwrap();
    store
}

#[test]
fn each_registration_of_the_same_login_connects_to_its_own_database() {
    let connector = Arc::new(FakeConnector::default());
    let databases = ["webged", "controle_migracao", "controle_sincronizacao"];
    let mut h = start(&databases, remembered(), connector.clone(), true);
    wait(&mut h, "background connections", |app| {
        !app.background().is_busy()
    });
    let app = h.state();
    // Os três conectaram, cada um no seu banco.
    assert_eq!(app.connected_servers().len(), 3);
    let mut connected = connector.databases.lock().unwrap().clone();
    connected.sort();
    assert_eq!(
        connected,
        ["controle_migracao", "controle_sincronizacao", "webged"]
    );
    for database in databases {
        let key = server_key(&on(database));
        // Conectado: sem estado pendente, e com o nome do registro dele.
        assert_eq!(app.background().state(&key), None);
        assert_eq!(
            app.registered_name(&key),
            Some(format!("{database}@Jucesp_127.0.0.1"))
        );
    }
}

#[test]
fn double_click_connects_the_other_database_instead_of_showing_the_first() {
    let connector = Arc::new(FakeConnector::default());
    // Sem a conexão em segundo plano: o usuário conecta um por um.
    let mut h = start(
        &["webged", "controle_migracao"],
        remembered(),
        connector.clone(),
        false,
    );
    h.run();
    h.state_mut().connect_registered(&on("webged"), false);
    wait(&mut h, "the first one", |app| {
        app.connected_servers().len() == 1
    });
    // O segundo registro é outra conexão: não basta trazer a primeira para
    // a frente.
    h.state_mut()
        .connect_registered(&on("controle_migracao"), false);
    wait(&mut h, "the second one", |app| {
        app.connected_servers().len() == 2
    });
    assert_eq!(
        *connector.databases.lock().unwrap(),
        ["webged", "controle_migracao"]
    );
    assert_eq!(
        h.state().connection().map(|c| c.settings.database.as_str()),
        Some("controle_migracao")
    );
    // De novo no primeiro: já está conectado, só volta a ser o ativo.
    h.state_mut().connect_registered(&on("webged"), false);
    h.run();
    assert_eq!(connector.databases.lock().unwrap().len(), 2);
    assert_eq!(
        h.state().connection().map(|c| c.settings.database.as_str()),
        Some("webged")
    );
}

#[test]
fn saving_the_registration_connects_it_right_away() {
    let connector = Arc::new(FakeConnector::default());
    // Nenhuma senha guardada: ao abrir, o servidor fica sem conexão.
    let store = Arc::new(MemoryStore::default());
    let mut h = start(
        &["controle_migracao"],
        store.clone(),
        connector.clone(),
        true,
    );
    let key = server_key(&on("controle_migracao"));
    wait(&mut h, "the scan", |app| {
        app.background().state(&key) == Some(&LinkState::NoPassword)
    });
    assert!(h.state().connected_servers().is_empty());

    // "Properties" do registro: o usuário digita a senha, testa e salva.
    let store: Arc<dyn SecretStore> = store;
    h.state_mut()
        .registered_mut()
        .edit_server(&NodePath::server(&[], 0), Some(&store));
    h.run();
    let d = h.state_mut().registered_mut().server_dialog_mut().unwrap();
    d.wait_lookup();
    d.password = PASSWORD.into();
    d.remember = true;
    h.run();
    h.get_by_label("Save").click();
    // Sem duplo clique: salvar já conecta, com a senha do diálogo (o cofre
    // ainda pode estar gravando).
    wait(&mut h, "the saved server", |app| {
        app.connected_servers().len() == 1
    });
    assert_eq!(h.state().background().state(&key), None);
    assert_eq!(*connector.databases.lock().unwrap(), ["controle_migracao"]);
}

#[test]
fn saving_with_the_wrong_password_shows_the_error_on_the_server() {
    let connector = Arc::new(FakeConnector::default());
    let store = Arc::new(MemoryStore::default());
    let mut h = start(&["controle_migracao"], store.clone(), connector, true);
    let key = server_key(&on("controle_migracao"));
    wait(&mut h, "the scan", |app| {
        app.background().state(&key) == Some(&LinkState::NoPassword)
    });
    let store: Arc<dyn SecretStore> = store;
    h.state_mut()
        .registered_mut()
        .edit_server(&NodePath::server(&[], 0), Some(&store));
    h.run();
    let d = h.state_mut().registered_mut().server_dialog_mut().unwrap();
    d.wait_lookup();
    d.password = "errada".into();
    d.remember = true;
    h.run();
    h.get_by_label("Save").click();
    wait(&mut h, "the failure", |app| {
        matches!(app.background().state(&key), Some(LinkState::Failed(_)))
    });
    assert!(h.state().connected_servers().is_empty());
}
