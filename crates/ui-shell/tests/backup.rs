//! Back Up num PostgreSQL de verdade, com o `pg_dump` instalado (com
//! `PGSTUDIO_TEST_HOST`; sem ele ou sem `pg_dump`, avisa e passa): o
//! backup restaura num banco limpo com os mesmos dados.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::backup::{BackupDialog, Content, Format};
use pgstudio_ui_shell::pgtools;

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
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
        ssh: None,
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

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pgstudio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Abre a janela no banco, espera a lista de tabelas.
fn dialog(s: &Server, db: &str, version: &str, ctx: &egui::Context) -> BackupDialog {
    let d = BackupDialog::open(
        db.into(),
        s.settings.clone(),
        s.password.clone(),
        version,
        s.service.open(db),
        ctx,
    );
    let mut d = d;
    let start = Instant::now();
    while d.tables().is_none() {
        d.step();
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    d
}

fn run(d: &mut BackupDialog, ctx: &egui::Context) -> Vec<String> {
    let mut log = match d.start(ctx) {
        Some(pgstudio_ui_shell::backup::Request::Log(l)) => l,
        other => panic!("{other:?} {:?}", d.error()),
    };
    let start = Instant::now();
    while d.result().is_none() {
        if let Some(pgstudio_ui_shell::backup::Request::Log(l)) = d.step() {
            log.extend(l);
        }
        assert!(start.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(10));
    }
    log
}

fn pg_restore(s: &Server, version: &str, file: &Path, db: &str) {
    let tool = pgtools::choose("pg_restore", None, version).unwrap();
    let mut args = pgtools::connection_args(&s.settings);
    args.push(format!("--dbname={db}"));
    args.push(file.display().to_string());
    let out = std::process::Command::new(&tool.path)
        .args(&args)
        .envs(pgtools::connection_env(&s.settings, &s.password))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_backup_restores_into_a_clean_database() {
    let Some(s) = server() else { return };
    let admin = s.service.open("postgres");
    let version = value(&admin, "SHOW server_version").unwrap();
    if let Err(e) = pgtools::choose("pg_dump", None, &version) {
        eprintln!("{e}; skipping");
        return;
    }
    let ctx = egui::Context::default();
    let src = format!("pgstudio_bk_src_{}", std::process::id());
    let dst = format!("pgstudio_bk_dst_{}", std::process::id());
    value(&admin, &format!("DROP DATABASE IF EXISTS {src}"));
    value(&admin, &format!("DROP DATABASE IF EXISTS {dst}"));
    value(&admin, &format!("CREATE DATABASE {src}"));
    let db = s.service.open(&src);
    value(
        &db,
        "CREATE TABLE cliente (id int PRIMARY KEY, nome text); \
         CREATE TABLE \"Pedido\" (id int, cliente_id int REFERENCES cliente, total numeric); \
         INSERT INTO cliente SELECT g, 'c' || g FROM generate_series(1, 500) g; \
         INSERT INTO \"Pedido\" SELECT g, 1 + g % 500, g * 1.5 FROM generate_series(1, 3000) g",
    );
    let dir = temp_dir("backup");

    // Custom, banco inteiro.
    let mut d = dialog(&s, &src, &version, &ctx);
    assert_eq!(
        d.tables().unwrap(),
        [
            "public.\"Pedido\"".to_string(),
            "public.cliente".to_string()
        ]
    );
    let file = dir.join("full.backup");
    d.options.file = file.display().to_string();
    let log = run(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:#?}");
    assert!(
        log.iter().any(|l| l.contains("dumping contents of table")),
        "{log:#?}"
    );
    assert!(log.last().unwrap().starts_with("Back Up: completed"));
    assert!(
        !log.iter()
            .any(|l| l.contains(&s.password) && !s.password.is_empty())
    );

    value(&admin, &format!("CREATE DATABASE {dst}"));
    pg_restore(&s, &version, &file, &dst);
    let restored = s.service.open(&dst);
    let check = "SELECT (SELECT count(*) FROM cliente) || '/' || \
                 (SELECT count(*) FROM \"Pedido\") || '/' || (SELECT sum(total) FROM \"Pedido\")";
    assert_eq!(value(&restored, check), value(&db, check));
    assert_eq!(
        value(&restored, check).as_deref(),
        Some("500/3000/6752250.0")
    );

    // Plain, só schema, só uma tabela.
    let mut d = dialog(&s, &src, &version, &ctx);
    let file = dir.join("schema.sql");
    d.options.file = file.display().to_string();
    d.options.format = Format::Plain;
    d.options.content = Content::SchemaOnly;
    d.choose_tables = true;
    d.options.tables.insert("public.cliente".into());
    run(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())));
    let sql = std::fs::read_to_string(&file).unwrap();
    assert!(sql.contains("CREATE TABLE public.cliente"), "{sql}");
    assert!(!sql.contains("Pedido"));
    assert!(!sql.contains("COPY public.cliente"));

    // Sem tabela escolhida não começa.
    let mut d = dialog(&s, &src, &version, &ctx);
    d.choose_tables = true;
    assert!(d.start(&ctx).is_none());
    assert!(d.error().unwrap().contains("Select at least one table"));

    // Stop para o pg_dump.
    value(
        &db,
        "CREATE TABLE grande AS SELECT g, md5(g::text) m FROM generate_series(1, 2000000) g",
    );
    let mut d = dialog(&s, &src, &version, &ctx);
    d.options.file = dir.join("stopped.sql").display().to_string();
    d.options.format = Format::Plain;
    d.start(&ctx).unwrap();
    d.cancel();
    let start = Instant::now();
    while d.result().is_none() {
        d.step();
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(d.result(), Some(&Err("canceled".to_string())));

    drop(db);
    drop(restored);
    // As sessões fecham ao soltar o serviço; força para o DROP passar.
    value(
        &admin,
        &format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname IN ('{src}', '{dst}')"
        ),
    );
    std::thread::sleep(Duration::from_millis(200));
    value(&admin, &format!("DROP DATABASE IF EXISTS {src}"));
    value(&admin, &format!("DROP DATABASE IF EXISTS {dst}"));
    let _ = std::fs::remove_dir_all(dir);
}
