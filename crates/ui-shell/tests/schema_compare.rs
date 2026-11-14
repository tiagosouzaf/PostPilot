//! Schema Compare num PostgreSQL de verdade (com `PGSTUDIO_TEST_HOST`; sem
//! ele, avisa e passa): dois bancos diferentes, aplicar tudo e comparar de
//! novo não mostra diferenças.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::QuerySession;
use pgstudio_core::schema::compare::Status;
use pgstudio_ui_shell::schema_compare::{Kind, Request, SchemaCompare};

fn connected() -> Option<pgstudio_core::connection::Connected> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
    };
    let connector =
        pgstudio_pg_metadata::PgConnector::new(pgstudio_pg_client::DbRuntime::new().unwrap());
    Some(
        connector
            .connect(&settings, &var("PGSTUDIO_TEST_PASSWORD", ""))
            .unwrap(),
    )
}

fn run(session: &Arc<dyn QuerySession>, sql: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            pgstudio_core::query::QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            pgstudio_core::query::QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

const SOURCE: &str = "
CREATE TABLE cliente (id serial PRIMARY KEY, nome text NOT NULL, email text,
    CONSTRAINT cliente_email_key UNIQUE (email));
CREATE TABLE pedido (id int PRIMARY KEY, cliente_id int REFERENCES cliente,
    total numeric(10,2) CHECK (total >= 0));
CREATE INDEX pedido_cliente_idx ON pedido (cliente_id);
CREATE TABLE \"Novo Item\" (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, pedido_id int
    REFERENCES pedido ON DELETE CASCADE);
CREATE VIEW grandes AS SELECT * FROM pedido WHERE total > 100;
CREATE FUNCTION total_cliente(c int) RETURNS numeric LANGUAGE sql
    AS $$ SELECT sum(total) FROM pedido WHERE cliente_id = c $$;
";

const TARGET: &str = "
CREATE TABLE cliente (id serial PRIMARY KEY, nome varchar(50), legado int);
CREATE TABLE pedido (id int PRIMARY KEY, cliente_id int, total numeric(10,2));
CREATE TABLE velha (x int);
INSERT INTO cliente (nome) VALUES ('a'), ('b');
CREATE VIEW grandes AS SELECT * FROM pedido WHERE total > 50;
CREATE VIEW sobra AS SELECT 1 AS um;
CREATE FUNCTION total_cliente(c int) RETURNS numeric LANGUAGE sql
    AS $$ SELECT 0::numeric $$;
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
    let admin = c.queries.open("postgres");
    let pid = std::process::id();
    let (src, dst) = (
        format!("pgstudio_cmp_src_{pid}"),
        format!("pgstudio_cmp_dst_{pid}"),
    );
    for db in [&src, &dst] {
        run(
            &admin,
            &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        );
        run(&admin, &format!("CREATE DATABASE {db}"));
    }
    run(&c.queries.open(&src), SOURCE);
    run(&c.queries.open(&dst), TARGET);

    let mut d = SchemaCompare::open(
        Some(src.clone()),
        "postgres",
        c.metadata.clone(),
        c.queries.clone(),
        &ctx,
    );
    wait(&mut d, &ctx);
    d.target = src.clone();
    d.compare(&ctx);
    assert!(d.error().unwrap().contains("same database"));
    d.target = dst.clone();
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
            (
                Kind::Function,
                "total_cliente(c integer)",
                Status::Different
            ),
        ]
    );
    let cliente = &o.items[1].statements;
    assert!(
        cliente
            .iter()
            .any(|s| s == "ALTER TABLE public.cliente DROP COLUMN legado;"),
        "{cliente:#?}"
    );
    assert!(
        cliente
            .iter()
            .any(|s| s.contains("ALTER COLUMN nome TYPE text"))
    );

    // Só a tabela velha: as outras diferenças continuam.
    d.chosen = BTreeSet::from([(Kind::Table, "public".into(), "velha".into())]);
    assert_eq!(d.script(), ["DROP TABLE public.velha;"]);
    d.apply(&ctx);
    let r = wait(&mut d, &ctx);
    assert!(
        r.iter()
            .any(|r| matches!(r, Request::Applied(db, _) if *db == dst)),
        "{r:?} {:?}",
        d.error()
    );
    assert_eq!(d.outcome().unwrap().items.len(), 6);

    // Um erro desfaz tudo: o nome NOT NULL com uma linha nula.
    run(
        &c.queries.open(&dst),
        "INSERT INTO cliente (nome) VALUES (NULL)",
    );
    d.chosen = d
        .outcome()
        .unwrap()
        .items
        .iter()
        .map(|i| i.key.clone())
        .collect();
    d.apply(&ctx);
    wait(&mut d, &ctx);
    assert!(
        d.error().unwrap().contains("Nothing was changed"),
        "{:?}",
        d.error()
    );
    d.compare(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.outcome().unwrap().items.len(), 6);

    // Tudo: comparar de novo não mostra diferenças.
    run(
        &c.queries.open(&dst),
        "DELETE FROM cliente WHERE nome IS NULL",
    );
    d.chosen = d
        .outcome()
        .unwrap()
        .items
        .iter()
        .map(|i| i.key.clone())
        .collect();
    let script = d.script();
    d.apply(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.error(), None, "{script:#?}");
    let o = d.outcome().unwrap();
    assert!(o.items.is_empty(), "{:#?}\n{script:#?}", o.items);
    assert_eq!(o.equal, 5);

    drop(d);
    for db in [&src, &dst] {
        run(
            &admin,
            &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        );
    }
}
