//! Restore num PostgreSQL de verdade, com `pg_dump` e `pg_restore`
//! instalados (com `PGSTUDIO_TEST_HOST`; sem ele ou sem as ferramentas,
//! avisa e passa): o backup da 4.6 restaurado devolve o banco original.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::backup::BackupDialog;
use pgstudio_ui_shell::pgtools;
use pgstudio_ui_shell::restore::{Request, RestoreDialog, Target};

struct Server {
    service: pgstudio_pg_client::PgQueryService,
    settings: ConnectionSettings,
    password: String,
}

fn server() -> Option<Server> {
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
    let password = var("PGSTUDIO_TEST_PASSWORD", "");
    Some(Server {
        service: pgstudio_pg_client::PgQueryService::new(
            settings.clone(),
            password.clone(),
            pgstudio_pg_client::DbRuntime::new().unwrap(),
        ),
        settings,
        password,
    })
}

fn value(session: &Arc<dyn QuerySession>, sql: &str) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut first = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Row(r) if first.is_none() => first = r.into_iter().next().flatten(),
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return first,
            _ => {}
        }
    }
}

/// Tabelas, colunas, restrições e índices, para comparar dois bancos.
const SHAPE: &str = "SELECT string_agg(x, ',' ORDER BY x) FROM ( \
    SELECT c.relname || ':' || c.relkind::text || ':' || \
           (SELECT string_agg(a.attname || ' ' || format_type(a.atttypid, a.atttypmod), ';' \
                              ORDER BY a.attnum) \
              FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attnum > 0) x \
      FROM pg_class c WHERE c.relnamespace = 'public'::regnamespace \
    UNION ALL SELECT conname || ':' || contype::text FROM pg_constraint \
      WHERE connamespace = 'public'::regnamespace) s";

const DATA: &str = "SELECT (SELECT count(*) FROM cliente) || '/' || \
    (SELECT count(*) FROM \"Pedido\") || '/' || (SELECT sum(total) FROM \"Pedido\")";

fn restore_dialog(
    s: &Server,
    db: Option<&str>,
    version: &str,
    ctx: &egui::Context,
) -> RestoreDialog {
    let mut d = RestoreDialog::open(
        db.map(str::to_string),
        s.settings.clone(),
        s.password.clone(),
        version,
        s.service.open("postgres"),
        ctx,
    );
    let start = Instant::now();
    while d.databases().is_none() {
        d.step(ctx);
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    d
}

fn list(d: &mut RestoreDialog, file: &str, ctx: &egui::Context) {
    d.options.file = file.into();
    d.list(ctx);
    let start = Instant::now();
    while d.is_listing() {
        d.step(ctx);
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(d.error(), None);
}

fn run(d: &mut RestoreDialog, ctx: &egui::Context) -> Vec<String> {
    let mut log = match d.start(ctx) {
        Some(Request::Log(l)) => l,
        other => panic!("{other:?} {:?}", d.error()),
    };
    let start = Instant::now();
    while d.result().is_none() {
        match d.step(ctx) {
            Some(Request::Log(l)) | Some(Request::Restored(l)) => log.extend(l),
            _ => {}
        }
        assert!(start.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(10));
    }
    log
}

#[test]
fn the_backup_restores_into_new_and_existing_databases() {
    let Some(s) = server() else { return };
    let admin = s.service.open("postgres");
    let version = value(&admin, "SHOW server_version").unwrap();
    for tool in ["pg_dump", "pg_restore"] {
        if let Err(e) = pgtools::choose(tool, None, &version) {
            eprintln!("{e}; skipping");
            return;
        }
    }
    let ctx = egui::Context::default();
    let pid = std::process::id();
    let src = format!("pgstudio_rs_src_{pid}");
    let new = format!("pgstudio_rs_new_{pid}");
    let part = format!("pgstudio_rs_part_{pid}");
    for db in [&src, &new, &part] {
        value(&admin, &format!("DROP DATABASE IF EXISTS {db}"));
    }
    value(&admin, &format!("CREATE DATABASE {src}"));
    let db = s.service.open(&src);
    value(
        &db,
        "CREATE TABLE cliente (id int PRIMARY KEY, nome text NOT NULL); \
         CREATE TABLE \"Pedido\" (id int, cliente_id int REFERENCES cliente, total numeric(10,2)); \
         CREATE INDEX pedido_cliente ON \"Pedido\" (cliente_id); \
         CREATE VIEW grandes AS SELECT * FROM \"Pedido\" WHERE total > 100; \
         INSERT INTO cliente SELECT g, 'c' || g FROM generate_series(1, 500) g; \
         INSERT INTO \"Pedido\" SELECT g, 1 + g % 500, g * 1.5 FROM generate_series(1, 3000) g",
    );
    let dir = std::env::temp_dir().join(format!("pgstudio-restore-{pid}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("loja.backup").display().to_string();

    // O backup da 4.6 (custom, banco inteiro).
    let mut b = BackupDialog::open(
        src.clone(),
        s.settings.clone(),
        s.password.clone(),
        &version,
        s.service.open(&src),
        &ctx,
    );
    let start = Instant::now();
    while b.tables().is_none() {
        b.step();
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    b.options.file = file.clone();
    b.start(&ctx).unwrap();
    while b.result().is_none() {
        b.step();
        assert!(start.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(b.result(), Some(&Ok(())));

    // Sem listar não começa.
    let mut d = restore_dialog(&s, None, &version, &ctx);
    d.options.file = file.clone();
    d.options.target = Target::New(new.clone());
    assert!(d.start(&ctx).is_none());
    assert!(d.error().unwrap().contains("List the contents"));

    // Banco novo, tudo: devolve o original.
    list(&mut d, &file, &ctx);
    let entries = d.entries().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.kind == "TABLE DATA" && e.name == "Pedido"),
        "{entries:#?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.kind == "VIEW" && e.name == "grandes")
    );
    assert_eq!(d.chosen.len(), entries.len());
    let log = run(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:#?}");
    assert!(
        log.iter().any(|l| l.contains("CREATE DATABASE")),
        "{log:#?}"
    );
    assert!(log.iter().any(|l| l.contains("processing data for table")));
    assert!(!s.password.is_empty() || !log.is_empty());
    assert!(
        !log.iter()
            .any(|l| !s.password.is_empty() && l.contains(&s.password))
    );
    let restored = s.service.open(&new);
    assert_eq!(value(&restored, SHAPE), value(&db, SHAPE));
    assert_eq!(value(&restored, DATA), value(&db, DATA));
    assert_eq!(
        value(&restored, DATA).as_deref(),
        Some("500/3000/6752250.00")
    );

    // Banco existente com --clean: os dados mexidos voltam.
    value(
        &restored,
        "DELETE FROM \"Pedido\" WHERE id > 10; UPDATE cliente SET nome = 'x'",
    );
    let mut d = restore_dialog(&s, Some(&new), &version, &ctx);
    assert_eq!(d.options.target, Target::Existing(new.clone()));
    list(&mut d, &file, &ctx);
    d.options.clean = true;
    d.options.single_transaction = true;
    assert!(d.command_line().unwrap().contains("--clean --if-exists"));
    let log = run(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:#?}");
    assert_eq!(value(&restored, DATA), value(&db, DATA));
    assert_eq!(
        value(&restored, "SELECT count(*) FROM cliente WHERE nome = 'x'").as_deref(),
        Some("0")
    );

    // Só alguns itens: a tabela cliente e os dados dela.
    let mut d = restore_dialog(&s, None, &version, &ctx);
    d.options.target = Target::New(part.clone());
    list(&mut d, &file, &ctx);
    d.chosen = d
        .entries()
        .unwrap()
        .iter()
        .filter(|e| e.name == "cliente" && matches!(e.kind.as_str(), "TABLE" | "TABLE DATA"))
        .map(|e| e.id)
        .collect();
    assert_eq!(d.chosen.len(), 2);
    assert!(d.command_line().unwrap().contains("--use-list="));
    let log = run(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:#?}");
    let partial = s.service.open(&part);
    assert_eq!(
        value(
            &partial,
            "SELECT string_agg(relname, ',' ORDER BY relname) FROM pg_class \
             WHERE relnamespace = 'public'::regnamespace"
        )
        .as_deref(),
        Some("cliente")
    );
    assert_eq!(
        value(&partial, "SELECT count(*) FROM cliente").as_deref(),
        Some("500")
    );

    // Um banco novo que já existe falha antes do pg_restore.
    let mut d = restore_dialog(&s, None, &version, &ctx);
    d.options.target = Target::New(part.clone());
    list(&mut d, &file, &ctx);
    run(&mut d, &ctx);
    assert!(
        matches!(d.result(), Some(Err(e)) if e.contains("already exists")),
        "{:?}",
        d.result()
    );

    // Um arquivo que não é backup avisa ao listar.
    let junk = dir.join("junk.txt");
    std::fs::write(&junk, "not a backup").unwrap();
    let mut d = restore_dialog(&s, None, &version, &ctx);
    d.options.file = junk.display().to_string();
    d.list(&ctx);
    while d.is_listing() {
        d.step(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(d.entries().is_none());
    assert!(d.error().is_some());

    drop(db);
    drop(restored);
    drop(partial);
    value(
        &admin,
        &format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname IN ('{src}', '{new}', '{part}')"
        ),
    );
    std::thread::sleep(Duration::from_millis(200));
    for db in [&src, &new, &part] {
        value(&admin, &format!("DROP DATABASE IF EXISTS {db}"));
    }
    let _ = std::fs::remove_dir_all(dir);
}
