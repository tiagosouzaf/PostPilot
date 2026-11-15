//! Recursos por motor: num SQL Server, o que ainda não existe para ele
//! avisa na status bar em vez de mandar SQL do PostgreSQL.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, Engine};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;

/// Conecta qualquer senha; o host "mssql" faz de conta que é um SQL Server.
struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _password: &str) -> Result<Connected, ConnectError> {
        let mut settings = s.clone();
        if s.host == "mssql" {
            settings.engine = Engine::SqlServer;
        }
        Ok(Connected {
            settings,
            server_version: "16.0".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn connected(host: &str) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    {
        let dialog = harness.state_mut().connect_dialog_mut().unwrap();
        dialog.server = host.into();
        dialog.password = "x".into();
    }
    harness.get_by_label("Connect").click();
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    harness
}

fn run(harness: &mut Harness<'static, ShellApp>, cmd: CommandId) {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, cmd);
    harness.run();
}

#[test]
fn sql_server_refuses_what_it_does_not_have_yet() {
    let mut harness = connected("mssql");
    assert_eq!(
        harness.state().connection().unwrap().settings.engine,
        Engine::SqlServer
    );
    let cmd = CommandId::SchemaCompare;
    run(&mut harness, cmd);
    let status = harness.state().status().to_string();
    assert!(
        status.contains("is not available for SQL Server yet"),
        "{cmd:?}: {status}"
    );
    assert!(
        harness
            .state()
            .output()
            .iter()
            .any(|l| l.contains("not available for this server"))
    );
}

#[test]
fn postgres_keeps_every_command() {
    let mut harness = connected("localhost");
    assert_eq!(
        harness.state().connection().unwrap().settings.engine,
        Engine::PostgreSql
    );
    run(&mut harness, CommandId::ActivityMonitor);
    assert!(!harness.state().status().contains("not available"));
}

#[test]
fn query_windows_use_the_dialect_of_their_server() {
    use pgstudio_sql_editor::lexer::Dialect;
    for (host, dialect) in [("mssql", Dialect::TSql), ("localhost", Dialect::Postgres)] {
        let mut harness = connected(host);
        run(&mut harness, CommandId::NewQuery);
        let id = harness.state().active_document().expect("query window");
        let doc = harness.state().documents().get(id).unwrap();
        assert_eq!(doc.editor.dialect(), dialect, "{host}");
    }
}
