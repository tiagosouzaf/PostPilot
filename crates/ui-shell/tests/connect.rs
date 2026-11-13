//! Testes do "Connect to Server" com um conector falso (sem banco).

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;

/// Aceita a senha "secret"; o host "down" simula servidor fora do ar.
struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        std::thread::sleep(Duration::from_millis(20));
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
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
        })
    }
}

fn harness_with(recent: Vec<ConnectionSettings>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(recent))
        .with_connector(Arc::new(FakeConnector));
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

fn harness() -> Harness<'static, ShellApp> {
    harness_with(Vec::new())
}

/// Roda frames até a conexão terminar (com ou sem sucesso).
fn wait_connect(harness: &mut Harness<'static, ShellApp>) {
    let start = Instant::now();
    loop {
        harness.step();
        let busy = harness
            .state()
            .connect_dialog()
            .is_some_and(|d| d.is_connecting());
        if !busy {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "connect never finished"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
}

fn type_password(harness: &mut Harness<'static, ShellApp>, password: &str) {
    harness.state_mut().connect_dialog_mut().unwrap().password = password.into();
    harness.run();
}

#[test]
fn opens_the_dialog_at_startup_without_the_demo_server() {
    let harness = harness();
    assert!(harness.state().connect_dialog().is_some());
    assert!(!harness.state().explorer().is_connected());
    harness.get_by_label("Connect to Server");
    for label in [
        "Server name:",
        "Login:",
        "Password:",
        "Database:",
        "Encryption:",
    ] {
        harness.get_by_label(label);
    }
    harness.get(by().label("Not connected"));
}

#[test]
fn wrong_password_keeps_the_dialog_open_with_the_reason() {
    let mut harness = harness();
    type_password(&mut harness, "wrong");
    harness.get_by_label("Connect").click();
    wait_connect(&mut harness);
    let dialog = harness.state().connect_dialog().expect("dialog stays open");
    assert!(dialog.error().unwrap().contains("Login failed"));
    assert!(
        harness
            .query_all(by().label_contains("Login failed for user \"postgres\""))
            .next()
            .is_some()
    );
    assert!(harness.state().connection().is_none());
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l.starts_with("Connection failed"))
    );
}

#[test]
fn connecting_fills_the_object_explorer_and_the_status_bar() {
    let mut harness = harness();
    type_password(&mut harness, "secret");
    harness.get_by_label("Connect").click();
    wait_connect(&mut harness);
    assert!(harness.state().connect_dialog().is_none());
    let c = harness.state().connection().expect("connected");
    assert_eq!(c.server_version, "17.2");
    assert!(harness.state().explorer().is_connected());
    harness.get(by().label("localhost (postgres)"));
    assert_eq!(
        harness.state().recent_connections().list()[0].host,
        "localhost"
    );
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l.contains("Connected to localhost (PostgreSQL 17.2)"))
    );
}

#[test]
fn enter_connects_and_bad_server_names_are_explained() {
    let mut harness = harness();
    harness.state_mut().connect_dialog_mut().unwrap().server = "db,abc".into();
    harness.run();
    harness.get_by_label("Connect").click();
    harness.run();
    let error = harness.state().connect_dialog().unwrap().error().unwrap();
    assert!(error.contains("not a valid port"), "{error}");

    let dialog = harness.state_mut().connect_dialog_mut().unwrap();
    dialog.server = "down,6000".into();
    harness.run();
    harness.key_press(eframe::egui::Key::Enter);
    wait_connect(&mut harness);
    let error = harness.state().connect_dialog().unwrap().error().unwrap();
    assert_eq!(error, "Could not connect to down,6000.");
}

#[test]
fn cancel_closes_and_file_connect_reopens_with_the_last_server() {
    let recent = vec![ConnectionSettings {
        host: "db.example.com".into(),
        port: 5433,
        user: "alice".into(),
        database: "sales".into(),
        ..Default::default()
    }];
    let mut harness = harness_with(recent);
    let dialog = harness.state().connect_dialog().unwrap();
    assert_eq!(dialog.server, "db.example.com,5433");
    assert_eq!(dialog.user, "alice");
    assert_eq!(dialog.database, "sales");
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.state().connect_dialog().is_none());

    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .execute(&ctx, CommandId::ConnectObjectExplorer);
    harness.run();
    assert_eq!(
        harness.state().connect_dialog().unwrap().server,
        "db.example.com,5433"
    );
}

#[test]
fn recent_menu_fills_every_field() {
    let recent = vec![
        ConnectionSettings::default(),
        ConnectionSettings {
            host: "other".into(),
            user: "bob".into(),
            database: "hr".into(),
            ssl: pgstudio_core::connection::SslMode::Require,
            ..Default::default()
        },
    ];
    let mut harness = harness_with(recent);
    // O botão do menu de recentes é a seta ao lado do "Server name".
    harness
        .get_by_label(egui_phosphor::regular::CARET_DOWN)
        .click();
    harness.run();
    harness.get_by_label("other (bob)").click();
    harness.run();
    let d = harness.state().connect_dialog().unwrap();
    assert_eq!(
        (d.server.as_str(), d.user.as_str(), d.database.as_str()),
        ("other", "bob", "hr")
    );
    assert_eq!(d.ssl, pgstudio_core::connection::SslMode::Require);
}

#[test]
fn shortcuts_wait_while_the_dialog_is_open_and_disconnect_clears() {
    let mut harness = harness();
    harness.key_press_modifiers(eframe::egui::Modifiers::CTRL, eframe::egui::Key::N);
    harness.run();
    assert!(harness.state().documents().ids().is_empty());

    type_password(&mut harness, "secret");
    harness.get_by_label("Connect").click();
    wait_connect(&mut harness);
    assert!(harness.state().connection().is_some());
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .execute(&ctx, CommandId::DisconnectObjectExplorer);
    harness.run();
    assert!(harness.state().connection().is_none());
    assert!(!harness.state().explorer().is_connected());
    harness.get(by().label("Not connected"));
}
