//! "Test connection" nos diálogos de conexão: tenta conectar com os campos
//! preenchidos, sem abrir a conexão nem gravar nada, e não segura a janela.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, Engine};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::conn_test::{ConnectionTest, Outcome};
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::secrets::MemoryStore;

/// Aceita a senha "secret"; "down" está fora do ar; "slow" só responde
/// depois de `release`.
#[derive(Default)]
struct FakeConnector {
    release: AtomicBool,
}

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        std::thread::sleep(Duration::from_millis(10));
        while s.host == "slow" && !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if s.host == "down" {
            return Err(ConnectError::Network(format!(
                "Could not connect to {}.",
                s.server_name()
            )));
        }
        if password != "secret" {
            return Err(ConnectError::Authentication(format!(
                "Login failed for user \"{}\".",
                s.user
            )));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: match s.engine {
                Engine::SqlServer => "16.0.4135".into(),
                _ => "17.2".into(),
            },
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn harness(connector: Arc<FakeConnector>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(connector);
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.run();
    h
}

fn connect_test<'a>(h: &'a Harness<'static, ShellApp>) -> &'a ConnectionTest {
    &h.state().connect_dialog().expect("dialog open").test
}

/// Roda frames até `test` parar.
fn wait(h: &mut Harness<'static, ShellApp>, test: fn(&ShellApp) -> Option<&ConnectionTest>) {
    let start = Instant::now();
    loop {
        h.step();
        if !test(h.state()).is_some_and(ConnectionTest::is_running) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the connection test never finished"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run();
}

fn in_connect(app: &ShellApp) -> Option<&ConnectionTest> {
    app.connect_dialog().map(|d| &d.test)
}

fn in_registered(app: &ShellApp) -> Option<&ConnectionTest> {
    app.registered().server_dialog().map(|d| &d.test)
}

fn sees(h: &Harness<'static, ShellApp>, text: &str) -> bool {
    h.query_all(by().label_contains(text)).next().is_some()
}

#[test]
fn testing_tells_the_result_without_connecting_or_remembering() {
    let mut h = harness(Arc::new(FakeConnector::default()));
    h.state_mut().connect_dialog_mut().unwrap().password = "secret".into();
    h.run();
    h.get_by_label("Test connection").click();
    wait(&mut h, in_connect);
    match connect_test(&h).outcome() {
        Some(Outcome::Connected {
            server, product, ..
        }) => {
            assert_eq!(server, "localhost");
            assert_eq!(product, "PostgreSQL 17.2");
        }
        other => panic!("{other:?}"),
    }
    assert!(sees(
        &h,
        "Connection succeeded: localhost answered as PostgreSQL 17.2"
    ));
    // Nada foi aberto nem lembrado: o diálogo continua como estava.
    assert!(h.state().connect_dialog().is_some());
    assert!(h.state().connection().is_none());
    assert!(!h.state().explorer().is_connected());
    assert!(h.state().recent_connections().list().is_empty());

    // Mudou a senha: o resultado antigo não vale mais.
    h.state_mut().connect_dialog_mut().unwrap().password = "wrong".into();
    h.run();
    assert!(connect_test(&h).outcome().is_none());
    assert!(!sees(&h, "Connection succeeded"));
    h.get_by_label("Test connection").click();
    wait(&mut h, in_connect);
    assert!(sees(
        &h,
        "Connection test failed: Login failed for user \"postgres\"."
    ));
    assert!(h.state().connect_dialog().unwrap().error().is_none());

    // Servidor fora do ar e nome de servidor inválido.
    h.state_mut().connect_dialog_mut().unwrap().server = "down,6000".into();
    h.run();
    h.get_by_label("Test connection").click();
    wait(&mut h, in_connect);
    assert!(sees(
        &h,
        "Connection test failed: Could not connect to down,6000."
    ));
    h.state_mut().connect_dialog_mut().unwrap().server = "db,abc".into();
    h.run();
    h.get_by_label("Test connection").click();
    h.run();
    match connect_test(&h).outcome() {
        Some(Outcome::Invalid(e)) => assert!(e.contains("not a valid port"), "{e}"),
        other => panic!("{other:?}"),
    }

    // Depois do teste, conectar funciona como sempre.
    let d = h.state_mut().connect_dialog_mut().unwrap();
    d.server = "localhost".into();
    d.password = "secret".into();
    h.run();
    h.get_by_label("Connect").click();
    let start = Instant::now();
    while h.state().connect_dialog().is_some() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(5), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state().connection().is_some());
}

#[test]
fn every_server_type_is_tested_through_its_own_connector() {
    let mut h = harness(Arc::new(FakeConnector::default()));
    for engine in Engine::ALL {
        let d = h.state_mut().connect_dialog_mut().unwrap();
        d.set_engine(engine);
        d.server = "db".into();
        d.password = "secret".into();
        h.run();
        h.get_by_label("Test connection").click();
        wait(&mut h, in_connect);
        match connect_test(&h).outcome() {
            Some(Outcome::Connected { product, .. }) => {
                assert!(product.starts_with(engine.label()), "{product}");
            }
            other => panic!("{engine:?}: {other:?}"),
        }
    }
}

#[test]
fn a_server_that_never_answers_can_be_canceled_or_times_out() {
    let connector = Arc::new(FakeConnector::default());
    let mut h = harness(connector.clone());
    let d = h.state_mut().connect_dialog_mut().unwrap();
    d.server = "slow".into();
    d.password = "secret".into();
    h.run();
    h.get_by_label("Test connection").click();
    h.run_steps(3);
    assert!(connect_test(&h).is_running());
    // A janela continua respondendo: o botão virou "Cancel test".
    assert!(sees(&h, "Testing slow"));
    h.get_by_label("Cancel test").click();
    h.run();
    assert!(!connect_test(&h).is_running());
    assert_eq!(connect_test(&h).outcome(), Some(&Outcome::Canceled));
    assert!(sees(&h, "Connection test canceled."));
    h.get_by_label("Test connection");

    // Sem cancelar, o diálogo desiste sozinho no limite de tempo.
    let d = h.state_mut().connect_dialog_mut().unwrap();
    d.test.set_limit(Duration::from_millis(60));
    h.get_by_label("Test connection").click();
    wait(&mut h, in_connect);
    assert!(matches!(
        connect_test(&h).outcome(),
        Some(Outcome::TimedOut { .. })
    ));
    assert!(sees(&h, "No answer from slow"));
    // A resposta atrasada de uma tentativa abandonada não muda nada.
    connector.release.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(40));
    h.run();
    assert!(matches!(
        connect_test(&h).outcome(),
        Some(Outcome::TimedOut { .. })
    ));
    assert!(h.state().connection().is_none());
}

#[test]
fn the_registration_dialog_tests_without_saving() {
    let dir = std::env::temp_dir().join(format!("postpilot-conn-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("registered_servers.json");
    let mut app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_registered_servers(RegisteredServers::open(&file))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector::default()));
    app.show_registered_servers();
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.run();
    // Fecha o "Connect to Server" do início.
    h.get_by_label("Cancel").click();
    h.run();
    h.state_mut().registered_mut().new_server(&[], None);
    h.run();
    let d = h.state_mut().registered_mut().server_dialog_mut().unwrap();
    d.connection_string = "postgresql://app:secret@vendas-db:5433/vendas".into();
    d.apply_connection_string().unwrap();
    h.run();
    h.get_by_label("Test connection").click();
    wait(&mut h, in_registered);
    assert!(sees(
        &h,
        "Connection succeeded: vendas-db,5433 answered as PostgreSQL 17.2"
    ));
    // O teste não gravou o registro nem abriu a conexão.
    assert!(h.state().registered().server_dialog().is_some());
    assert!(h.state().registered().servers().root().servers.is_empty());
    assert!(!file.exists());
    assert!(h.state().connection().is_none());

    let d = h.state_mut().registered_mut().server_dialog_mut().unwrap();
    d.password = "wrong".into();
    h.run();
    h.get_by_label("Test connection").click();
    wait(&mut h, in_registered);
    assert!(sees(
        &h,
        "Connection test failed: Login failed for user \"app\"."
    ));

    // Salvar continua sendo o que grava.
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().registered().servers().root().servers.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

// --- PostgreSQL de verdade ---

/// Com o conector de verdade: senha certa, senha errada e porta fechada.
#[test]
fn a_real_server_is_tested_with_the_real_connector() {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: pgstudio_core::connection::SslMode::Disable,
        ..ConnectionSettings::default()
    };
    let password = var("PGSTUDIO_TEST_PASSWORD", "");
    let connector: Arc<dyn Connector> = Arc::new(pgstudio_pg_metadata::PgConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ));
    let ctx = eframe::egui::Context::default();
    let run = |settings: &ConnectionSettings, password: &str| {
        let mut test = ConnectionTest::default();
        test.request(settings.clone(), password.to_string());
        let start = Instant::now();
        while test.is_running() {
            test.step(Some(&connector), &ctx);
            assert!(start.elapsed() < Duration::from_secs(20), "never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
        test.outcome().cloned().expect("an outcome")
    };
    match run(&settings, &password) {
        Outcome::Connected { product, .. } => {
            assert!(product.starts_with("PostgreSQL "), "{product}");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        run(&settings, "not-the-password"),
        Outcome::Failed(_)
    ));
    let closed = ConnectionSettings {
        port: 1,
        ..settings.clone()
    };
    assert!(matches!(run(&closed, &password), Outcome::Failed(_)));
}
