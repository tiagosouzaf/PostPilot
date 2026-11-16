//! Data Compare num SQL Server de verdade (com `MSSQL_TEST_HOST`; sem ele,
//! avisa e passa): chaves inteiras com identidade, de texto, compostas,
//! uniqueidentifier e de data, colunas binárias, calculadas e rowversion.
//! O script gerado, rodado no destino, não deixa diferença nenhuma.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::data_compare::{DataCompare, Request, Server, Side};

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

/// Roda um script com `GO` até o fim; qualquer erro derruba o teste.
fn run(session: &Arc<dyn QuerySession>, sql: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    loop {
        match rx.recv_timeout(Duration::from_secs(120)).unwrap() {
            QueryEvent::Error(e) => panic!("{}: {}", &sql[..sql.len().min(300)], e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

const SCHEMA: &str = "
CREATE TABLE dbo.cliente (id int IDENTITY PRIMARY KEY, nome nvarchar(80) NOT NULL,
    saldo decimal(12,2) NULL, ativo bit NULL, criado datetime NULL, quando datetime2(7) NULL,
    foto varbinary(max) NULL, guia uniqueidentifier NULL, preco money NULL, nota float NULL,
    dobro AS (saldo * 2), versao rowversion);
CREATE TABLE dbo.[Item Pedido] (pedido int NOT NULL, sigla nvarchar(10) NOT NULL, qtd int NULL,
    obs varchar(50) NULL, PRIMARY KEY (sigla, pedido));
CREATE TABLE dbo.evento (id uniqueidentifier PRIMARY KEY, texto nvarchar(20) NULL);
CREATE TABLE dbo.dia (dia date NOT NULL, hora datetime2(3) NOT NULL, v int NULL,
    PRIMARY KEY (dia, hora));
CREATE TABLE dbo.sem_chave (x int NULL);
CREATE TABLE dbo.grande (id bigint PRIMARY KEY, v varchar(40) NULL);
";

const SOURCE: &str = "
SET IDENTITY_INSERT dbo.cliente ON;
INSERT INTO dbo.cliente (id, nome, saldo, ativo, criado, quando, foto, guia, preco, nota) VALUES
    (1, N'Ana', 10.50, 1, '2026-01-02T03:04:05.123', '2026-01-02T03:04:05.1234567', 0x00FF10,
     'A0000000-0000-0000-0000-000000000002', 12.3456, 0.1),
    (2, N'd''água \\ barra', NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL),
    (3, N'duas' + NCHAR(10) + N'linhas', 0, NULL, '2026-05-05T12:00:00', NULL, 0x, NULL, 1, 1e300),
    (10, N'só na origem', 1, 1, '2026-12-31T23:59:59.997', '2026-12-31T23:59:59.9999999', 0xDEADBEEF,
     NEWID(), 9.99, 2.5),
    (11, N'', 2, 1, NULL, NULL, NULL, NULL, NULL, NULL);
SET IDENTITY_INSERT dbo.cliente OFF;
INSERT INTO dbo.[Item Pedido] VALUES (1, N'Z', 1, 'a'), (1, N'a', 2, 'b'), (2, N'a', 3, NULL),
    (1, N'á', 4, 'c'), (1, N'A b', 5, 'd'), (10, N'a', 6, 'e');
INSERT INTO dbo.evento VALUES ('00000000-0000-0000-0000-000000000001', N'um'),
    ('A0000000-0000-0000-0000-000000000002', N'dois'),
    ('F0000000-0000-0000-0000-000000000003', N'três');
INSERT INTO dbo.dia VALUES ('2026-01-01', '2026-01-01T10:00:00.123', 1),
    ('2026-01-01', '2026-01-01T10:00:00.124', 2), ('2026-02-01', '2026-01-01T00:00:00', 3);
INSERT INTO dbo.grande SELECT n, CONCAT('linha ', n) FROM (SELECT TOP (120000)
    ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n
    FROM sys.all_columns a CROSS JOIN sys.all_columns b) g;
";

const TARGET: &str = "
SET IDENTITY_INSERT dbo.cliente ON;
INSERT INTO dbo.cliente (id, nome, saldo, ativo, criado, quando, foto, guia, preco, nota) VALUES
    (1, N'Ana', 10.50, 1, '2026-01-02T03:04:05.123', '2026-01-02T03:04:05.1234567', 0x00FF10,
     'A0000000-0000-0000-0000-000000000002', 12.3456, 0.1),
    (2, N'outro nome', 5, 0, '2020-01-01', NULL, 0x01, NULL, 3, 4),
    (3, N'duas' + NCHAR(10) + N'linhas', 0, NULL, '2026-05-05T12:00:00', NULL, 0x, NULL, 1, 1e300),
    (7, N'só no destino', 1, 1, NULL, NULL, NULL, NULL, NULL, NULL);
SET IDENTITY_INSERT dbo.cliente OFF;
INSERT INTO dbo.[Item Pedido] VALUES (1, N'Z', 1, 'a'), (1, N'a', 2, 'mudou'), (3, N'a', 3, NULL),
    (1, N'á', 4, 'c'), (1, N'B', 5, 'd');
INSERT INTO dbo.evento VALUES ('00000000-0000-0000-0000-000000000001', N'um'),
    ('A0000000-0000-0000-0000-000000000002', N'mudou'),
    ('B0000000-0000-0000-0000-000000000009', N'sobra');
INSERT INTO dbo.dia VALUES ('2026-01-01', '2026-01-01T10:00:00.123', 1),
    ('2026-01-01', '2026-01-01T10:00:00.124', 9), ('2026-03-01', '2026-01-01T00:00:00', 3);
INSERT INTO dbo.grande SELECT n, CONCAT('linha ', n) FROM (SELECT TOP (120500)
    ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n
    FROM sys.all_columns a CROSS JOIN sys.all_columns b) g WHERE n % 1000 <> 0;
UPDATE dbo.grande SET v = 'mudou' WHERE id % 777 = 0;
";

fn wait(d: &mut DataCompare, ctx: &egui::Context) -> Vec<Request> {
    let mut out = Vec::new();
    let start = Instant::now();
    loop {
        out.extend(d.step(ctx));
        if !d.is_busy() {
            return out;
        }
        assert!(start.elapsed() < Duration::from_secs(180));
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn counts(d: &DataCompare, table: &str) -> (u64, u64, u64, u64) {
    let r = d
        .results()
        .iter()
        .find(|r| r.plan.label() == table)
        .unwrap_or_else(|| panic!("{table} was not compared"));
    assert_eq!(r.error, None, "{table}");
    let c = r.counts;
    (c.only_source, c.different, c.only_target, c.equal)
}

#[test]
fn the_script_leaves_the_target_like_the_source() {
    let Some(c) = connected() else { return };
    let ctx = egui::Context::default();
    let admin = c.queries.open("master");
    let pid = std::process::id();
    let (src, dst) = (format!("pp_data_src_{pid}"), format!("pp_data_dst_{pid}"));
    for db in [&src, &dst] {
        run(&admin, &format!("CREATE DATABASE [{db}]"));
    }
    let (source, target) = (c.queries.open(&src), c.queries.open(&dst));
    run(&source, SCHEMA);
    run(&source, SOURCE);
    run(&target, SCHEMA);
    run(&target, TARGET);

    let server = Server {
        key: "mssql".into(),
        label: "mssql".into(),
        engine: c.settings.engine,
        queries: c.queries.clone(),
        home: "master".into(),
    };
    let mut d = DataCompare::open(vec![server]);
    d.source = Side {
        server: "mssql".into(),
        database: src.clone(),
    };
    d.target = Side {
        server: "mssql".into(),
        database: dst.clone(),
    };
    d.compare(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.error(), None);

    // (só na origem, diferentes, só no destino, iguais)
    assert_eq!(counts(&d, "dbo.cliente"), (2, 1, 1, 2));
    assert_eq!(counts(&d, "dbo.Item Pedido"), (3, 1, 2, 2));
    assert_eq!(counts(&d, "dbo.evento"), (1, 1, 1, 1));
    assert_eq!(counts(&d, "dbo.dia"), (1, 1, 1, 1));
    assert_eq!(
        counts(&d, "dbo.grande"),
        (120, 154, 500, 120_000 - 120 - 154)
    );
    let cliente = d
        .results()
        .iter()
        .find(|r| r.plan.label() == "dbo.cliente")
        .unwrap();
    assert_eq!(cliente.plan.ignored, ["dobro", "versao"]);

    let Some(Request::Script { sql, database, .. }) = d.generate() else {
        panic!("no script: {:?}", d.error());
    };
    assert_eq!(database, dst);
    assert!(sql.contains("SET XACT_ABORT ON;") && sql.contains("COMMIT TRANSACTION;"));
    assert!(sql.contains("SET IDENTITY_INSERT dbo.cliente ON;"));
    run(&target, &sql);

    d.compare(&ctx);
    wait(&mut d, &ctx);
    for r in d.results() {
        assert_eq!(r.error, None);
        assert_eq!(r.counts.differences(), 0, "{}", r.plan.label());
    }
    assert_eq!(counts(&d, "dbo.grande").3, 120_000);

    drop(d);
    drop((source, target));
    std::thread::sleep(Duration::from_millis(300));
    for db in [&src, &dst] {
        run(
            &admin,
            &format!(
                "ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]"
            ),
        );
    }
}
