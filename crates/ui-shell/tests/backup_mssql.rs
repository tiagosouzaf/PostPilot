//! Backup e restore contra um SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, o teste só avisa; veja `docs/sqlserver.md`):
//! backup completo e diferencial no mesmo arquivo do servidor, leitura do
//! arquivo, restore num banco novo e por cima de um existente.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::backup_mssql::{BackupDialog, BackupKind, Request, RestoreDialog};

fn service() -> Option<pgstudio_mssql::MssqlQueryService> {
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
    Some(pgstudio_mssql::MssqlQueryService::new(
        settings,
        var("MSSQL_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

/// A primeira coluna da primeira linha de `sql`.
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

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(90), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Roda o backup até o fim; devolve o que foi para o Output.
fn back_up(d: &mut BackupDialog, ctx: &egui::Context) -> Vec<String> {
    let mut log = match d.start(ctx) {
        Some(Request::Log(l)) => l,
        other => panic!("{other:?} {:?}", d.error()),
    };
    wait("backup", || {
        if let Some(Request::Log(l)) = d.step() {
            log.extend(l);
        }
        !d.is_running()
    });
    log
}

fn restore(d: &mut RestoreDialog, ctx: &egui::Context) -> Vec<String> {
    let mut log = match d.start(ctx) {
        Some(Request::Log(l)) => l,
        other => panic!("{other:?} {:?}", d.error()),
    };
    wait("restore", || {
        match d.step(ctx) {
            Some(Request::Log(l) | Request::Restored(l)) => log.extend(l),
            Some(other) => panic!("{other:?}"),
            None => {}
        }
        !d.is_running()
    });
    log
}

#[test]
fn back_up_and_restore_on_the_server() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("master");
    let id = std::process::id();
    let db = format!("pp_bak_{id}");
    let copy = format!("pp_bak_{id}_copia");
    value(&admin, &format!("CREATE DATABASE [{db}]"));
    value(
        &admin,
        &format!(
            "CREATE TABLE [{db}].dbo.t (id int PRIMARY KEY, nome nvarchar(20)); \
             INSERT INTO [{db}].dbo.t VALUES (1, N'um')"
        ),
    );

    // Back Up: o arquivo sugerido fica na pasta de backups do servidor.
    let mut d = BackupDialog::open(db.clone(), service.open("master"), &ctx);
    wait("defaults", || {
        d.step();
        !d.options.file.is_empty()
    });
    assert!(
        d.options.file.ends_with(&format!("{db}.bak")),
        "{}",
        d.options.file
    );
    let file = d.options.file.clone();
    let log = back_up(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:?}");
    assert_eq!(d.percent(), Some(100), "{log:?}");
    assert!(
        log.iter()
            .any(|l| l.contains("BACKUP DATABASE successfully processed")),
        "{log:?}"
    );

    // Um diferencial acrescentado ao mesmo arquivo.
    value(
        &admin,
        &format!("INSERT INTO [{db}].dbo.t VALUES (2, N'dois')"),
    );
    let mut d = BackupDialog::open(db.clone(), service.open("master"), &ctx);
    d.options.file = file.clone();
    d.options.kind = BackupKind::Differential;
    d.options.overwrite = false;
    let log = back_up(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:?}");

    // Um caminho impossível (dentro de um arquivo): o erro do servidor
    // aparece. Uma pasta que só não existe o SQL Server 2017 no Linux cria.
    let mut d = BackupDialog::open(db.clone(), service.open("master"), &ctx);
    d.options.file = "/etc/hostname/x.bak".into();
    back_up(&mut d, &ctx);
    let error = d.result().unwrap().clone().unwrap_err();
    assert!(error.contains("/etc/hostname/x.bak"), "{error}");

    // Restore: lê o arquivo, sugere o completo e o nome do banco.
    let mut d = RestoreDialog::open(None, service.open("master"), &ctx);
    d.options.file = file.clone();
    d.list(&ctx);
    wait("list", || {
        d.step(&ctx);
        !d.is_listing()
    });
    assert_eq!(d.error(), None);
    let kinds: Vec<_> = d.sets().iter().map(|s| (s.position, s.kind)).collect();
    assert_eq!(
        kinds,
        [(1, BackupKind::Full), (2, BackupKind::Differential)]
    );
    assert_eq!(d.options.position, 1);
    assert_eq!(d.options.database, db);
    assert_eq!(d.files().len(), 2);
    // Num banco novo: os arquivos vão para as pastas padrão com o nome dele.
    d.options.database = copy.clone();
    let moves = d.moves();
    assert!(moves[0].1.ends_with(&format!("{copy}.mdf")), "{moves:?}");
    assert!(
        moves[1].1.ends_with(&format!("{copy}_log.ldf")),
        "{moves:?}"
    );
    let log = restore(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:?}");
    assert_eq!(
        value(&admin, &format!("SELECT COUNT(*) FROM [{copy}].dbo.t")).as_deref(),
        Some("1")
    );

    // Por cima de um banco em uso, sem REPLACE: o servidor recusa e o
    // banco continua aberto a todos.
    let busy = service.open(&copy);
    value(&busy, "SELECT 1");
    let mut d = RestoreDialog::open(Some(copy.clone()), service.open("master"), &ctx);
    assert!(d.options.replace && d.options.close_connections);
    d.options.file = file.clone();
    d.options.replace = false;
    d.list(&ctx);
    wait("list", || {
        d.step(&ctx);
        !d.is_listing()
    });
    assert_eq!(d.options.database, copy);
    let log = restore(&mut d, &ctx);
    assert!(matches!(d.result(), Some(Err(_))), "{log:?}");
    wait("multi user", || {
        value(
            &admin,
            &format!("SELECT CAST(DATABASEPROPERTYEX(N'{copy}', 'UserAccess') AS varchar(20))"),
        )
        .as_deref()
            == Some("MULTI_USER")
    });

    // Com REPLACE e derrubando as conexões, entra.
    value(
        &admin,
        &format!("INSERT INTO [{copy}].dbo.t VALUES (9, N'nove')"),
    );
    let mut d = RestoreDialog::open(Some(copy.clone()), service.open("master"), &ctx);
    d.options.file = file.clone();
    d.list(&ctx);
    wait("list", || {
        d.step(&ctx);
        !d.is_listing()
    });
    let log = restore(&mut d, &ctx);
    assert_eq!(d.result(), Some(&Ok(())), "{log:?}");
    assert_eq!(
        value(&admin, &format!("SELECT COUNT(*) FROM [{copy}].dbo.t")).as_deref(),
        Some("1")
    );

    for name in [&db, &copy] {
        value(
            &admin,
            &format!(
                "ALTER DATABASE [{name}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; \
                 DROP DATABASE [{name}]"
            ),
        );
    }
}
