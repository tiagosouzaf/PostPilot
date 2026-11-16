//! O SQL Server por um túnel SSH de verdade. Sem `SSH_TEST_HOST` e
//! `SSH_TEST_MSSQL` (o `host:porta` do SQL Server visto do servidor SSH) o
//! teste só avisa; as outras variáveis são as de
//! `crates/ssh/tests/tunnel.rs`, mais `SSH_TEST_MSSQL_PASSWORD`.

use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::connection::{
    ConnectionSettings, Connector, Engine, SshAuth, SshSettings, SslMode,
};
use pgstudio_core::query::QueryEvent;
use pgstudio_mssql::MssqlConnector;
use pgstudio_pg_client::DbRuntime;

#[test]
fn connects_and_queries_through_the_tunnel() {
    let (Ok(host), Ok(target)) = (
        std::env::var("SSH_TEST_HOST"),
        std::env::var("SSH_TEST_MSSQL"),
    ) else {
        eprintln!("SSH_TEST_HOST or SSH_TEST_MSSQL not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let (db_host, db_port) = target.rsplit_once(':').expect("host:port");
    let mut s = ConnectionSettings::for_engine(Engine::SqlServer);
    // Com instância nomeada no nome: pelo túnel vale a porta.
    s.host = format!("{db_host}\\MSSQLSERVER");
    s.port = db_port.parse().unwrap();
    s.ssl = SslMode::Require;
    s.ssh = Some(SshSettings {
        host,
        port: var("SSH_TEST_PORT", "22").parse().unwrap(),
        user: var("SSH_TEST_USER", "tunnel"),
        auth: SshAuth::KeyFile,
        key_file: format!("{}/id_plain", var("SSH_TEST_KEYS", ".")),
    });
    let password = var("SSH_TEST_MSSQL_PASSWORD", "");
    let connected = MssqlConnector::new(DbRuntime::new().unwrap())
        .connect(&s, &password)
        .expect("connect through the tunnel");
    assert_eq!(connected.settings, s);
    assert!(!connected.server_version.is_empty());

    // A janela de consulta abre a conexão dela pelo mesmo túnel.
    let session = connected.queries.open("master");
    let (tx, rx) = mpsc::channel();
    session.execute(
        "SELECT name FROM sys.databases WHERE name = 'master'; PRINT 'ok';".to_string(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    let mut rows = 0;
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).expect("event") {
            QueryEvent::Row(_) => rows += 1,
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(rows, 1);
}
