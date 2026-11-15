//! Schema Compare num SQL Server de verdade (com `MSSQL_TEST_HOST`; sem
//! ele, avisa e passa): dois bancos diferentes, com tabelas, view, função,
//! procedure, trigger, sequência e tipo; aplicar tudo e comparar de novo
//! não mostra diferenças, e um erro no meio não muda nada.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_core::schema::compare::Status;
use pgstudio_ui_shell::schema_compare::{Kind, Request, SchemaCompare};

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
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

const SOURCE: &str = "
CREATE TYPE dbo.cep FROM char(8) NOT NULL;
CREATE SEQUENCE dbo.numero AS int START WITH 1 INCREMENT BY 5;
CREATE TABLE dbo.cliente (id int IDENTITY CONSTRAINT pk_cliente PRIMARY KEY,
    nome nvarchar(80) NOT NULL, email varchar(120) NULL CONSTRAINT df_email DEFAULT 'x',
    CONSTRAINT uq_cliente_email UNIQUE (email));
CREATE TABLE dbo.pedido (id int CONSTRAINT pk_pedido PRIMARY KEY,
    cliente_id int CONSTRAINT fk_pedido_cliente REFERENCES dbo.cliente (id),
    total decimal(12,2) CONSTRAINT ck_pedido_total CHECK (total >= 0));
CREATE INDEX ix_pedido_cliente ON dbo.pedido (cliente_id);
CREATE TABLE dbo.[Novo Item] (id int IDENTITY CONSTRAINT pk_item PRIMARY KEY,
    pedido_id int CONSTRAINT fk_item_pedido REFERENCES dbo.pedido (id) ON DELETE CASCADE);
GO
CREATE VIEW dbo.grandes AS SELECT id, total FROM dbo.pedido WHERE total > 100
GO
CREATE FUNCTION dbo.total_cliente (@c int) RETURNS decimal(12,2) AS
BEGIN RETURN (SELECT SUM(total) FROM dbo.pedido WHERE cliente_id = @c) END
GO
CREATE PROCEDURE dbo.listar AS SELECT id, nome FROM dbo.cliente
GO
CREATE TRIGGER dbo.audita ON dbo.pedido AFTER INSERT AS SET NOCOUNT ON
GO
";

const TARGET: &str = "
CREATE SEQUENCE dbo.numero AS int START WITH 1 INCREMENT BY 1;
CREATE TABLE dbo.cliente (id int IDENTITY CONSTRAINT pk_cliente PRIMARY KEY,
    nome nvarchar(50) NULL, legado int NULL DEFAULT 0);
CREATE TABLE dbo.pedido (id int CONSTRAINT pk_pedido PRIMARY KEY, cliente_id int NULL,
    total decimal(10,2) NULL);
CREATE TABLE dbo.velha (x int NULL);
INSERT INTO dbo.cliente (nome) VALUES (N'a');
GO
CREATE VIEW dbo.grandes AS SELECT id, total FROM dbo.pedido WHERE total > 50
GO
CREATE VIEW dbo.sobra AS SELECT 1 AS um
GO
CREATE FUNCTION dbo.total_cliente (@c int) RETURNS decimal(12,2) AS
BEGIN RETURN 0 END
GO
";

fn wait(d: &mut SchemaCompare, ctx: &egui::Context) -> Vec<Request> {
    let mut out = Vec::new();
    let start = Instant::now();
    while d.is_busy() || d.databases().is_none() {
        out.extend(d.step(ctx));
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

#[test]
fn applying_the_comparison_leaves_no_differences() {
    let Some(c) = connected() else { return };
    let ctx = egui::Context::default();
    let admin = c.queries.open("master");
    let pid = std::process::id();
    let (src, dst) = (format!("pp_cmp_src_{pid}"), format!("pp_cmp_dst_{pid}"));
    for db in [&src, &dst] {
        run(&admin, &format!("CREATE DATABASE [{db}]"));
    }
    run(&c.queries.open(&src), SOURCE);
    run(&c.queries.open(&dst), TARGET);

    let mut d = SchemaCompare::open(
        Some(src.clone()),
        "master",
        c.metadata.clone(),
        c.queries.clone(),
        &ctx,
    );
    wait(&mut d, &ctx);
    assert!(d.databases().unwrap().contains(&dst));
    d.target.database = dst.clone();
    d.compare(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.error(), None);
    let o = d.outcome().unwrap();
    let summary: Vec<(Kind, &str, Status)> = o
        .items
        .iter()
        .map(|i| (i.key.0, i.key.2.as_str(), i.status))
        .collect();
    assert_eq!(
        summary,
        [
            (Kind::Table, "Novo Item", Status::OnlyInSource),
            (Kind::Table, "cliente", Status::Different),
            (Kind::Table, "pedido", Status::Different),
            (Kind::Table, "velha", Status::OnlyInTarget),
            (Kind::View, "grandes", Status::Different),
            (Kind::View, "sobra", Status::OnlyInTarget),
            (Kind::Function, "total_cliente", Status::Different),
            (Kind::Procedure, "listar", Status::OnlyInSource),
            (Kind::Trigger, "pedido.audita", Status::OnlyInSource),
            (Kind::Sequence, "numero", Status::Different),
            (Kind::Type, "cep", Status::OnlyInSource),
        ]
    );
    let script = d.script();
    assert!(
        script
            .iter()
            .any(|s| s == "ALTER TABLE [dbo].[cliente] DROP COLUMN [legado];"),
        "{script:#?}"
    );
    assert!(
        script
            .iter()
            .any(|s| s.starts_with("CREATE OR ALTER VIEW dbo.grandes")),
        "{script:#?}"
    );

    // Um erro desfaz tudo: o nome NOT NULL com uma linha nula.
    run(
        &c.queries.open(&dst),
        "INSERT INTO dbo.cliente (nome) VALUES (NULL)",
    );
    d.apply(&ctx);
    wait(&mut d, &ctx);
    assert!(
        d.error().unwrap().contains("Nothing was changed"),
        "{:?}",
        d.error()
    );
    d.compare(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.outcome().unwrap().items.len(), 11);

    // Sem a linha nula, tudo entra e não sobra diferença.
    run(
        &c.queries.open(&dst),
        "DELETE FROM dbo.cliente WHERE nome IS NULL",
    );
    d.apply(&ctx);
    let r = wait(&mut d, &ctx);
    assert!(
        r.iter()
            .any(|r| matches!(r, Request::Applied(db, _) if *db == dst)),
        "{r:?} {:?}\n{}",
        d.error(),
        script.join("\n")
    );
    let left: Vec<_> = d.outcome().unwrap().items.clone();
    assert!(left.is_empty(), "{left:#?}");

    for db in [&src, &dst] {
        run(
            &admin,
            &format!(
                "ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{db}]"
            ),
        );
    }
}
