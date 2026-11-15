//! Testes contra um SQL Server de verdade. Sem `MSSQL_TEST_HOST` eles só
//! avisam. Para rodar (veja `docs/sqlserver.md`):
//!
//! ```text
//! docker run -d -e ACCEPT_EULA=Y -e 'MSSQL_SA_PASSWORD=PostPilot!2026' \
//!     -p 14330:1433 mcr.microsoft.com/mssql/server:2022-latest
//! MSSQL_TEST_HOST=127.0.0.1 MSSQL_TEST_PORT=14330 \
//!     MSSQL_TEST_PASSWORD='PostPilot!2026' cargo test -p pgstudio-mssql
//! ```

use std::sync::mpsc;
use std::time::{Duration, Instant};

use pgstudio_core::connection::{ConnectError, ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::metadata::NodeId;
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_mssql::MssqlConnector;
use pgstudio_pg_client::DbRuntime;

fn settings() -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut s = ConnectionSettings::for_engine(Engine::SqlServer);
    s.host = host;
    s.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    s.user = var("MSSQL_TEST_USER", "sa");
    s.ssl = SslMode::Prefer;
    Some((s, var("MSSQL_TEST_PASSWORD", "")))
}

fn connector() -> MssqlConnector {
    MssqlConnector::new(DbRuntime::new().unwrap())
}

/// Roda `sql` numa sessão e devolve todos os eventos.
fn run(session: &dyn QuerySession, sql: &str) -> Vec<QueryEvent> {
    let (tx, rx) = mpsc::channel();
    session.execute(
        sql.to_string(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    let mut events = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("event");
        let end = matches!(e, QueryEvent::Finished { .. });
        events.push(e);
        if end {
            return events;
        }
    }
}

fn session(database: &str) -> Option<std::sync::Arc<dyn QuerySession>> {
    let (s, pw) = settings()?;
    let connected = connector().connect(&s, &pw).expect("connect");
    Some(connected.queries.open(database))
}

#[test]
fn connects_and_reports_the_version() {
    let Some((s, pw)) = settings() else { return };
    let c = connector().connect(&s, &pw).expect("connect");
    assert_eq!(c.settings.engine, Engine::SqlServer);
    assert!(c.server_version.starts_with("16."), "{}", c.server_version);
    // Sem TLS nenhum além do login também conecta.
    let mut plain = s.clone();
    plain.ssl = SslMode::Disable;
    connector()
        .connect(&plain, &pw)
        .expect("connect without TLS");
}

#[test]
fn wrong_password_and_missing_database_are_explained() {
    let Some((s, _)) = settings() else { return };
    match connector().connect(&s, "errada") {
        Err(ConnectError::Authentication(m)) => assert!(m.contains("Login failed"), "{m}"),
        other => panic!("{other:?}"),
    }
    let (s, pw) = settings().unwrap();
    let mut s = s;
    s.database = "nao_existe".into();
    match connector().connect(&s, &pw) {
        Err(ConnectError::Database(m)) => assert!(m.contains("nao_existe"), "{m}"),
        other => panic!("{other:?}"),
    }
    let mut down = settings().unwrap().0;
    down.port = 1;
    assert!(matches!(
        connector().connect(&down, "x"),
        Err(ConnectError::Network(_))
    ));
}

#[test]
fn batches_results_counts_prints_and_errors_in_order() {
    let Some(session) = session("tempdb") else {
        return;
    };
    let script = "\
CREATE TABLE #t (id int PRIMARY KEY, nome nvarchar(20));
INSERT #t VALUES (1, N'um'), (2, N'dois');
PRINT 'depois do insert';
SELECT id, nome FROM #t ORDER BY id;
GO
SELECT 1 AS a;
SELECT * FROM tabela_que_nao_existe;
GO
SELECT 'segue' AS b
GO 2
";
    let events = run(&*session, script);
    let mut kinds = Vec::new();
    for e in &events {
        kinds.push(match e {
            QueryEvent::Columns(c) => format!("cols {}", c.join(",")),
            QueryEvent::Row(r) => format!(
                "row {}",
                r.iter()
                    .map(|v| v.clone().unwrap_or("NULL".into()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            QueryEvent::CommandComplete { rows } => format!("done {rows}"),
            QueryEvent::Notice(n) => format!("notice {}", n.message),
            QueryEvent::Error(e) => format!("error {} line {:?}", e.code, e.position),
            QueryEvent::Finished { canceled } => format!("finished {canceled}"),
        });
    }
    let error_pos = script.find("SELECT * FROM tabela").unwrap() + 1;
    let expected = [
        "done 2".to_string(),
        "notice depois do insert".into(),
        "cols id,nome".into(),
        "row 1,um".into(),
        "row 2,dois".into(),
        "done 2".into(),
        "cols a".into(),
        "row 1".into(),
        "done 1".into(),
        format!("error 208 line Some({error_pos})"),
        "cols b".into(),
        "row segue".into(),
        "done 1".into(),
        "cols b".into(),
        "row segue".into(),
        "done 1".into(),
        "finished false".into(),
    ];
    assert_eq!(kinds, expected);
}

#[test]
fn values_look_like_ssms() {
    let Some(session) = session("master") else {
        return;
    };
    let events = run(
        &*session,
        "SELECT CAST(1 AS bit), CAST(255 AS tinyint), CAST(-5 AS smallint), \
         CAST(2147483647 AS int), CAST(9007199254740993 AS bigint), \
         CAST(12.345 AS decimal(10,3)), CAST(1.5 AS float), CAST(2.25 AS real), \
         CAST(12.5 AS money), N'ção', 'abc', CAST(0xCAFE AS varbinary(4)), \
         CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS uniqueidentifier), \
         CAST('2025-11-15' AS date), CAST('13:45:30.1234567' AS time), \
         CAST('2025-11-15 13:45:30.123' AS datetime), \
         CAST('2025-11-15 13:45:00' AS smalldatetime), \
         CAST('2025-11-15 13:45:30.1234567' AS datetime2), \
         CAST('2025-11-15 13:45:30.12 -03:00' AS datetimeoffset(2)), \
         CAST('<a>1</a>' AS xml), CAST(NULL AS int)",
    );
    let row = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Row(r) => Some(r.clone()),
            _ => None,
        })
        .expect("row");
    let row: Vec<_> = row
        .into_iter()
        .map(|v| v.unwrap_or("NULL".into()))
        .collect();
    assert_eq!(
        row,
        [
            "1",
            "255",
            "-5",
            "2147483647",
            "9007199254740993",
            "12.345",
            "1.5",
            "2.25",
            "12.5000",
            "ção",
            "abc",
            "0xCAFE",
            "6F9619FF-8B86-D011-B42D-00C04FC964FF",
            "2025-11-15",
            "13:45:30.1234567",
            "2025-11-15 13:45:30.123",
            "2025-11-15 13:45:00",
            "2025-11-15 13:45:30.1234567",
            "2025-11-15 13:45:30.12 -03:00",
            "<a>1</a>",
            "NULL",
        ]
    );
}

#[test]
fn errors_in_procedures_and_raiserror() {
    let Some(session) = session("tempdb") else {
        return;
    };
    let events = run(
        &*session,
        "RAISERROR('aviso %d', 10, 1, 7);\nRAISERROR('falhou', 16, 1);\nSELECT 2 AS depois",
    );
    let notices: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Notice(n) => Some(n.message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(notices, ["aviso 7"]);
    let error = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Error(e) => Some(e.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("50000", "falhou")
    );
    // O lote segue depois de um erro de nível 16.
    assert!(events.contains(&QueryEvent::Columns(vec!["depois".into()])));
}

/// Cancela um WAITFOR longo, com e sem TLS, e a sessão continua boa.
#[test]
fn cancel_stops_the_batch_and_the_session_stays_usable() {
    let Some((s, pw)) = settings() else { return };
    for ssl in [SslMode::Prefer, SslMode::Disable] {
        let mut s = s.clone();
        s.ssl = ssl;
        let connected = connector().connect(&s, &pw).expect("connect");
        let session = connected.queries.open("master");
        // Abre a conexão.
        run(&*session, "SELECT 1");
        let (tx, rx) = mpsc::channel();
        session.execute(
            "SELECT 1 AS antes; WAITFOR DELAY '00:00:30'; SELECT 2 AS depois".into(),
            Box::new(move |e| {
                let _ = tx.send(e);
            }),
        );
        let started = Instant::now();
        // O servidor segura o primeiro resultado no pacote até o WAITFOR
        // acabar; cancela com o lote parado nele.
        std::thread::sleep(Duration::from_millis(500));
        session.cancel();
        let mut finished = None;
        while finished.is_none() {
            match rx.recv_timeout(Duration::from_secs(10)).expect("finish") {
                QueryEvent::Finished { canceled } => finished = Some(canceled),
                QueryEvent::Columns(c) if c[0] == "depois" => {
                    panic!("ran past the cancel: {c:?}")
                }
                _ => {}
            }
        }
        assert_eq!(finished, Some(true), "{ssl:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "{ssl:?}");
        let again = run(&*session, "SELECT 3 AS x");
        assert!(
            again.contains(&QueryEvent::Row(vec![Some("3".into())])),
            "{again:?}"
        );
    }
}

#[test]
fn cancel_during_a_large_result() {
    let Some(session) = session("master") else {
        return;
    };
    run(&*session, "SELECT 1");
    let (tx, rx) = mpsc::channel();
    session.execute(
        "SELECT TOP (2000000) a.object_id, REPLICATE(N'x', 200) AS texto \
         FROM sys.all_columns a CROSS JOIN sys.all_columns b"
            .into(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    let mut rows = 0;
    loop {
        if let QueryEvent::Row(_) = rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            rows += 1;
            if rows == 1000 {
                break;
            }
        }
    }
    session.cancel();
    loop {
        if let QueryEvent::Finished { canceled } =
            rx.recv_timeout(Duration::from_secs(30)).expect("finish")
        {
            assert!(canceled);
            break;
        }
    }
    let again = run(&*session, "SELECT 4 AS x");
    assert!(
        again.contains(&QueryEvent::Row(vec![Some("4".into())])),
        "{again:?}"
    );
}

#[test]
fn explorer_lists_the_databases() {
    let Some((s, pw)) = settings() else { return };
    let c = connector().connect(&s, &pw).expect("connect");
    let roots = c.metadata.roots().unwrap();
    assert!(
        roots[0].name.contains("SQL Server 16."),
        "{}",
        roots[0].name
    );
    let folders = c.metadata.children(&roots[0].id).unwrap();
    let dbs = c.metadata.children(&folders[0].id).unwrap();
    let names: Vec<_> = dbs.iter().map(|d| d.name.as_str()).collect();
    for db in ["master", "tempdb", "msdb"] {
        assert!(names.contains(&db), "{names:?}");
    }
    let tempdb = dbs.iter().find(|d| d.name == "tempdb").unwrap();
    assert_eq!(
        c.metadata.database_of(&tempdb.id).as_deref(),
        Some("tempdb")
    );
    assert_eq!(c.metadata.database_of(&NodeId("server".into())), None);
}
