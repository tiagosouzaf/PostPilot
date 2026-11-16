//! Schema Compare num PostgreSQL de verdade (com `PGSTUDIO_TEST_HOST`; sem
//! ele, avisa e passa): dois bancos diferentes, aplicar tudo e comparar de
//! novo não mostra diferenças. O segundo teste compara um snapshot
//! `.pgsschema` com um banco aberto como "outro servidor", com tipos,
//! sequências, triggers e procedures.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::QuerySession;
use pgstudio_core::schema::compare::Status;
use pgstudio_ui_shell::schema_compare::{Kind, Place, Request, SchemaCompare, Which};

fn connected() -> Option<pgstudio_core::connection::Connected> {
    connection().map(|(c, _, _)| c)
}

/// A conexão, o conector e a senha.
fn connection() -> Option<(
    pgstudio_core::connection::Connected,
    Arc<dyn Connector>,
    String,
)> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
        ssh: None,
    };
    let connector: Arc<dyn Connector> = Arc::new(pgstudio_pg_metadata::PgConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ));
    let password = var("PGSTUDIO_TEST_PASSWORD", "");
    let c = connector.connect(&settings, &password).unwrap();
    Some((c, connector, password))
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
    d.target.database = src.clone();
    d.compare(&ctx);
    assert!(d.error().unwrap().contains("same database"));
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
    // As cinco de antes e a sequência do serial de cliente.
    assert_eq!(o.equal, 6);

    drop(d);
    for db in [&src, &dst] {
        run(
            &admin,
            &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        );
    }
}

const SOURCE_OBJECTS: &str = "
CREATE TYPE status AS ENUM ('novo', 'pago', 'enviado');
CREATE DOMAIN cep AS text CHECK (VALUE ~ '^[0-9]{8}$');
CREATE TYPE par AS (a int, b text);
CREATE SEQUENCE nota_seq START 100 INCREMENT 5;
CREATE TABLE pedido (id serial PRIMARY KEY, st status, cep cep,
    nota int DEFAULT nextval('nota_seq'));
CREATE FUNCTION audita() RETURNS trigger LANGUAGE plpgsql
    AS $$ BEGIN RETURN NEW; END $$;
CREATE TRIGGER audita BEFORE INSERT ON pedido FOR EACH ROW EXECUTE FUNCTION audita();
CREATE PROCEDURE limpa() LANGUAGE sql AS $$ DELETE FROM pedido WHERE false $$;
";

const TARGET_OBJECTS: &str = "
CREATE TYPE status AS ENUM ('novo', 'pago');
CREATE TYPE velho AS RANGE (subtype = int);
CREATE SEQUENCE nota_seq START 1;
CREATE TABLE pedido (id serial PRIMARY KEY, st status);
INSERT INTO pedido (st) VALUES ('pago');
CREATE FUNCTION audita() RETURNS trigger LANGUAGE plpgsql
    AS $$ BEGIN RETURN NULL; END $$;
CREATE TRIGGER audita AFTER INSERT ON pedido FOR EACH ROW EXECUTE FUNCTION audita();
CREATE PROCEDURE sobra() LANGUAGE sql AS $$ SELECT 1 $$;
";

#[test]
fn a_snapshot_against_another_server_with_types_sequences_and_triggers() {
    let Some((c, connector, password)) = connection() else {
        return;
    };
    let ctx = egui::Context::default();
    let admin = c.queries.open("postgres");
    let pid = std::process::id();
    let (src, dst) = (
        format!("pgstudio_cmp2_src_{pid}"),
        format!("pgstudio_cmp2_dst_{pid}"),
    );
    for db in [&src, &dst] {
        run(
            &admin,
            &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        );
        run(&admin, &format!("CREATE DATABASE {db}"));
    }
    run(&c.queries.open(&src), SOURCE_OBJECTS);
    run(&c.queries.open(&dst), TARGET_OBJECTS);

    let mut d = SchemaCompare::open(
        Some(src.clone()),
        "postgres",
        c.metadata.clone(),
        c.queries.clone(),
        &ctx,
    )
    .with_connector(Some(connector), &c.settings);
    wait(&mut d, &ctx);

    // O snapshot da origem.
    let file = std::env::temp_dir().join(format!("pgstudio-cmp-{pid}.pgsschema"));
    d.save_snapshot(Which::Source, file.clone(), &ctx);
    let r = wait(&mut d, &ctx);
    assert_eq!(d.error(), None);
    assert!(
        r.iter()
            .any(|r| matches!(r, Request::Log(l) if l[0].contains("saved the snapshot"))),
        "{r:?}"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"format\": 1"), "{text}");
    // Tirado o snapshot, a origem pode até sumir.
    run(
        &admin,
        &format!("DROP DATABASE IF EXISTS {src} WITH (FORCE)"),
    );

    // Snapshot contra o destino aberto como outro servidor.
    d.source.place = Place::Snapshot;
    d.source.file = file.display().to_string();
    d.target.place = Place::Server;
    d.target.database = dst.clone();
    d.target.password = password.clone();
    d.compare(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.error(), None);
    assert!(d.can_apply());
    let o = d.outcome().unwrap();
    let summary: Vec<(Kind, &str, Status)> = o
        .items
        .iter()
        .map(|i| (i.key.0, i.key.2.as_str(), i.status))
        .collect();
    assert_eq!(
        summary,
        [
            (Kind::Table, "pedido", Status::Different),
            (Kind::Function, "audita()", Status::Different),
            (Kind::Procedure, "limpa()", Status::OnlyInSource),
            (Kind::Procedure, "sobra()", Status::OnlyInTarget),
            (Kind::Trigger, "pedido.audita", Status::Different),
            (Kind::Sequence, "nota_seq", Status::Different),
            (Kind::Type, "cep", Status::OnlyInSource),
            (Kind::Type, "par", Status::OnlyInSource),
            (Kind::Type, "status", Status::Different),
            (Kind::Type, "velho", Status::OnlyInTarget),
        ]
    );
    assert_eq!(o.equal, 1, "a sequência do serial é igual");
    let status = &o.items[8].statements;
    assert_eq!(
        status,
        &["ALTER TYPE public.status ADD VALUE 'enviado' AFTER 'pago';"]
    );
    // A função do trigger muda sem DROP (o trigger depende dela).
    assert_eq!(o.items[1].statements.len(), 1);
    assert!(o.items[1].statements[0].starts_with("CREATE OR REPLACE FUNCTION"));

    // Tudo, no outro servidor: comparar de novo não mostra diferenças.
    d.chosen = o.items.iter().map(|i| i.key.clone()).collect();
    let script = d.script();
    d.apply(&ctx);
    let r = wait(&mut d, &ctx);
    assert_eq!(d.error(), None, "{script:#?}");
    // Em outro servidor não há nó do Object Explorer para atualizar.
    assert!(
        r.iter()
            .any(|r| matches!(r, Request::Log(l) if l[0].starts_with("Schema Compare: applied"))),
        "{r:?}"
    );
    let o = d.outcome().unwrap();
    assert!(o.items.is_empty(), "{:#?}\n{script:#?}", o.items);
    assert_eq!(o.equal, 9);

    // Ao contrário, o snapshot é o destino: dá para gerar o script, não
    // para aplicar.
    d.source.place = Place::Database;
    d.source.database = dst.clone();
    d.target.place = Place::Snapshot;
    d.target.file = file.display().to_string();
    d.compare(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.error(), None);
    assert!(!d.can_apply());

    drop(d);
    let _ = std::fs::remove_file(&file);
    run(
        &admin,
        &format!("DROP DATABASE IF EXISTS {dst} WITH (FORCE)"),
    );
}
