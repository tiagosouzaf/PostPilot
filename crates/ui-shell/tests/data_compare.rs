//! Data Compare num PostgreSQL de verdade (com `PGSTUDIO_TEST_HOST`; sem
//! ele, avisa e passa): dois bancos com as mesmas tabelas e dados
//! diferentes, chaves inteiras, de texto, compostas e uuid. O script
//! gerado, rodado no destino, não deixa diferença nenhuma.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::data_compare::{DataCompare, Request, Server, Side};

fn connected() -> Option<pgstudio_core::connection::Connected> {
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
        match rx.recv_timeout(Duration::from_secs(120)).unwrap() {
            QueryEvent::Error(e) => panic!("{}: {}", &sql[..sql.len().min(300)], e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

const SCHEMA: &str = r#"
CREATE TABLE cliente (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, nome text NOT NULL,
    saldo numeric(12,2), ativo boolean, criado timestamptz, dados jsonb, foto bytea,
    dobro numeric GENERATED ALWAYS AS (saldo * 2) STORED);
CREATE TABLE "Item Pedido" (pedido int, sigla varchar(10), qtd int, obs text,
    PRIMARY KEY (sigla, pedido));
CREATE TABLE evento (id uuid PRIMARY KEY, quando date, texto text);
CREATE TABLE sem_chave (x int);
CREATE TABLE grande (id bigint PRIMARY KEY, v text);
"#;

const SOURCE: &str = r#"
CREATE TABLE so_origem (id int PRIMARY KEY);
INSERT INTO cliente (id, nome, saldo, ativo, criado, dados, foto) OVERRIDING SYSTEM VALUE VALUES
    (1, 'Ana', 10.50, true, '2026-01-02 03:04:05+00', '{"a": [1, 2]}', '\x00ff10'),
    (2, 'd''água \ barra', NULL, false, NULL, NULL, NULL),
    (3, E'duas\nlinhas', 0, NULL, '2026-05-05 12:00:00-03', '"x"', ''),
    (10, 'só na origem', 1, true, NULL, NULL, NULL),
    (11, '', 2, true, NULL, NULL, NULL);
INSERT INTO "Item Pedido" VALUES (1, 'Z', 1, 'a'), (1, 'a', 2, 'b'), (2, 'a', 3, NULL),
    (1, 'á', 4, 'c'), (1, 'A b', 5, 'd'), (10, 'a', 6, 'e');
INSERT INTO evento VALUES ('00000000-0000-0000-0000-000000000001', '2026-01-01', 'um'),
    ('a0000000-0000-0000-0000-000000000002', '2026-01-02', 'dois'),
    ('f0000000-0000-0000-0000-000000000003', NULL, 'três');
INSERT INTO sem_chave VALUES (1);
INSERT INTO grande SELECT g, 'linha ' || g FROM generate_series(1, 300000) g;
"#;

const TARGET: &str = r#"
INSERT INTO cliente (id, nome, saldo, ativo, criado, dados, foto) OVERRIDING SYSTEM VALUE VALUES
    (1, 'Ana', 10.50, true, '2026-01-02 03:04:05+00', '{"a": [1, 2]}', '\x00ff10'),
    (2, 'outro nome', 5, false, NULL, '{}', '\x01'),
    (3, E'duas\nlinhas', 0, NULL, '2026-05-05 12:00:00-03', '"x"', ''),
    (7, 'só no destino', 1, true, NULL, NULL, NULL);
INSERT INTO "Item Pedido" VALUES (1, 'Z', 1, 'a'), (1, 'a', 2, 'mudou'), (3, 'a', 3, NULL),
    (1, 'á', 4, 'c'), (1, 'B', 5, 'd');
INSERT INTO evento VALUES ('00000000-0000-0000-0000-000000000001', '2026-01-01', 'um'),
    ('a0000000-0000-0000-0000-000000000002', '2026-01-03', 'dois'),
    ('b0000000-0000-0000-0000-000000000009', NULL, 'sobra');
INSERT INTO grande SELECT g, 'linha ' || g FROM generate_series(1, 300500) g
    WHERE g % 1000 <> 0;
UPDATE grande SET v = 'mudou' WHERE id % 777 = 0;
"#;

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
    let admin = c.queries.open("postgres");
    let pid = std::process::id();
    let (src, dst) = (format!("pp_data_src_{pid}"), format!("pp_data_dst_{pid}"));
    for db in [&src, &dst] {
        run(&admin, &format!("DROP DATABASE IF EXISTS {db}"));
        run(&admin, &format!("CREATE DATABASE {db}"));
    }
    let (source, target) = (c.queries.open(&src), c.queries.open(&dst));
    run(&source, SCHEMA);
    run(&source, SOURCE);
    run(&target, SCHEMA);
    run(&target, TARGET);

    let server = Server {
        key: "pg".into(),
        label: "pg".into(),
        engine: c.settings.engine,
        queries: c.queries.clone(),
        home: "postgres".into(),
    };
    let mut d = DataCompare::open(vec![server]);
    d.source = Side {
        server: "pg".into(),
        database: src.clone(),
    };
    d.target = d.source.clone();
    d.compare(&ctx);
    assert_eq!(
        d.error(),
        Some("The source and the target are the same database.")
    );
    d.target.database = dst.clone();
    d.compare(&ctx);
    let requests = wait(&mut d, &ctx);
    assert_eq!(d.error(), None);
    assert!(matches!(requests.last(), Some(Request::Log(_))));

    // As tabelas sem chave ou sem par aparecem, com o motivo.
    let reason = |label: &str| {
        let pair = d.pairs().iter().find(|p| p.label == label).unwrap();
        pair.plan.clone().unwrap_err()
    };
    assert_eq!(reason("public.sem_chave"), "no primary key in the source");
    assert_eq!(reason("public.so_origem"), "not in the target");

    // (só na origem, diferentes, só no destino, iguais)
    assert_eq!(counts(&d, "public.cliente"), (2, 1, 1, 2));
    assert_eq!(counts(&d, "public.Item Pedido"), (3, 1, 2, 2));
    assert_eq!(counts(&d, "public.evento"), (1, 1, 1, 1));
    // 300 múltiplos de 1000 faltam no destino, 386 múltiplos de 777
    // mudaram e as linhas de 300001 a 300500 sobram.
    let (only_source, different, only_target, equal) = counts(&d, "public.grande");
    assert_eq!(only_source, 300);
    assert_eq!(only_target, 500);
    assert_eq!(different, 386);
    assert_eq!(equal, 300_000 - 300 - 386);
    let grande = d
        .results()
        .iter()
        .find(|r| r.plan.label() == "public.grande")
        .unwrap();
    assert_eq!(grande.sample.len(), 500);
    let cliente = d
        .results()
        .iter()
        .find(|r| r.plan.label() == "public.cliente")
        .unwrap();
    assert_eq!(cliente.plan.ignored, ["dobro"]);

    // Só as inserções: nenhum UPDATE nem DELETE no script.
    d.wanted.update = false;
    d.wanted.delete = false;
    let only_inserts = d.script_text().unwrap();
    assert!(!only_inserts.contains("UPDATE ") && !only_inserts.contains("DELETE "));
    assert_eq!(d.statements(), 2 + 3 + 1 + 300);
    d.wanted.update = true;
    d.wanted.delete = true;

    let Some(Request::Script {
        sql,
        server,
        database,
    }) = d.generate()
    else {
        panic!("no script: {:?}", d.error());
    };
    assert_eq!((server.as_str(), database.as_str()), ("pg", dst.as_str()));
    assert!(sql.contains("BEGIN;") && sql.trim_end().ends_with("COMMIT;"));
    assert!(sql.contains("OVERRIDING SYSTEM VALUE"));
    assert!(sql.contains("DELETE FROM public.\"Item Pedido\" WHERE sigla = 'B' AND pedido = '1';"));
    run(&target, &sql);

    d.compare(&ctx);
    wait(&mut d, &ctx);
    for r in d.results() {
        assert_eq!(r.error, None);
        assert_eq!(r.counts.differences(), 0, "{}", r.plan.label());
    }
    assert_eq!(counts(&d, "public.grande").3, 300_000);
    assert_eq!(d.statements(), 0);
    assert_eq!(d.generate(), None);

    drop(d);
    drop((source, target));
    std::thread::sleep(Duration::from_millis(300));
    for db in [&src, &dst] {
        run(
            &admin,
            &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        );
    }
}

#[test]
fn canceling_stops_the_comparison() {
    let Some(c) = connected() else { return };
    let ctx = egui::Context::default();
    let admin = c.queries.open("postgres");
    let pid = std::process::id();
    let (src, dst) = (format!("pp_data_csrc_{pid}"), format!("pp_data_cdst_{pid}"));
    for db in [&src, &dst] {
        run(&admin, &format!("DROP DATABASE IF EXISTS {db}"));
        run(&admin, &format!("CREATE DATABASE {db}"));
        let s = c.queries.open(db);
        run(
            &s,
            "CREATE TABLE grande (id bigint PRIMARY KEY, v text);
             INSERT INTO grande SELECT g, repeat('x', 200) FROM generate_series(1, 400000) g;",
        );
    }
    let server = Server {
        key: "pg".into(),
        label: "pg".into(),
        engine: c.settings.engine,
        queries: c.queries.clone(),
        home: "postgres".into(),
    };
    let mut d = DataCompare::open(vec![server]);
    d.source = Side {
        server: "pg".into(),
        database: src.clone(),
    };
    d.target = Side {
        server: "pg".into(),
        database: dst.clone(),
    };
    d.list(&ctx);
    wait(&mut d, &ctx);
    assert_eq!(d.pairs().len(), 1);
    d.compare(&ctx);
    d.cancel();
    wait(&mut d, &ctx);
    assert_eq!(d.status(), Some("The comparison was canceled."));
    assert!(d.results().iter().all(|r| r.counts.equal < 400_000));

    // Outra consulta no mesmo servidor continua respondendo depois.
    run(&admin, "SELECT 1");
    drop(d);
    std::thread::sleep(Duration::from_millis(500));
    for db in [&src, &dst] {
        run(
            &admin,
            &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        );
    }
}
