//! Query Designer num SQL Server de verdade (com `MSSQL_TEST_HOST`; sem
//! ele, o teste só avisa; veja `docs/sqlserver.md`): cada T-SQL escrito à
//! mão é lido pelo designer (sem diferenciar maiúsculas, com `TOP` e
//! colchetes), e o SQL que ele gera devolve as mesmas linhas.

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::query_designer::read::read_for;

fn connected() -> Option<pgstudio_core::connection::Connected> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::SqlServer);
    settings.host = host;
    settings.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    settings.user = var("MSSQL_TEST_USER", "sa");
    settings.ssl = SslMode::Require;
    let connector =
        pgstudio_mssql::MssqlConnector::new(pgstudio_pg_client::DbRuntime::new().unwrap());
    Some(
        connector
            .connect(&settings, &var("MSSQL_TEST_PASSWORD", ""))
            .unwrap(),
    )
}

/// As linhas do último resultado de `sql`.
fn rows(session: &Arc<dyn QuerySession>, sql: &str) -> Vec<Vec<Option<String>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut out = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Columns(_) => out.clear(),
            QueryEvent::Row(r) => out.push(r),
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return out,
            _ => {}
        }
    }
}

const SCHEMA: &str = "
CREATE TABLE dbo.Cliente (Id int PRIMARY KEY, Nome nvarchar(40) NOT NULL, Cidade nvarchar(40));
CREATE TABLE dbo.Pedido (Id int PRIMARY KEY,
    ClienteId int REFERENCES dbo.Cliente (Id), [Valor Total] decimal(10,2));
INSERT INTO dbo.Cliente VALUES (1, N'Ana', N'Recife'), (2, N'Bia', N'Natal'),
    (3, N'Caio', NULL), (4, N'Davi', N'Recife');
INSERT INTO dbo.Pedido VALUES (10, 1, 50), (11, 1, 120), (12, 2, 80), (13, NULL, 5), (14, 4, 300);
";

const QUERIES: &[&str] = &[
    "select c.NOME, p.[valor total] from CLIENTE c left join pedido p on p.clienteid = c.id",
    "select top (3) nome from cliente order by id",
    "select TOP 2 c.nome, p.[Valor Total] as total from dbo.Cliente c \
       join dbo.Pedido p on p.ClienteId = c.Id order by p.[Valor Total] desc",
    "select distinct cidade from cliente",
    "select c.nome, count(*) as pedidos, sum(p.[Valor Total]) from cliente c \
       join pedido p on p.clienteid = c.id group by c.nome having count(*) > 1",
    "select * from [dbo].[Cliente] where id > 1 and (nome like 'B%' or cidade is null)",
    "select c.id, p.id from cliente c full join pedido p on p.clienteid = c.id",
];

#[test]
fn hand_written_tsql_returns_the_same_rows_after_the_designer() {
    let Some(connected) = connected() else {
        return;
    };
    let db = format!("pp_qd_{}", std::process::id());
    let admin = connected.queries.open("master");
    rows(&admin, &format!("CREATE DATABASE [{db}]"));
    let session = connected.queries.open(&db);
    rows(&session, SCHEMA);
    let model = connected.metadata.load_model(&db).unwrap();
    for sql in QUERIES {
        let query =
            read_for(sql, &model, Engine::SqlServer).unwrap_or_else(|e| panic!("{sql}: {e}"));
        let designed = query.sql(&model);
        let back = read_for(&designed, &model, Engine::SqlServer)
            .unwrap_or_else(|e| panic!("{designed}: {e}"));
        assert_eq!(back.sql(&model), designed, "round trip of {sql}");
        let mut expected = rows(&session, sql);
        let mut got = rows(&session, &designed);
        if !sql.contains("order by") {
            expected.sort();
            got.sort();
        }
        assert!(!expected.is_empty(), "{sql} returns nothing");
        assert_eq!(got, expected, "{sql}\n--- designed:\n{designed}");
    }
    drop(session);
    rows(
        &admin,
        &format!(
            "ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]"
        ),
    );
}
