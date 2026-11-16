//! Propriedades de segurança num PostgreSQL de verdade (com
//! `PGSTUDIO_TEST_HOST`; sem ele, avisa e passa): criar e alterar uma role
//! e dar e tirar permissões de uma tabela, conferindo em `pg_roles` e
//! `has_table_privilege`.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_core::metadata::Securable;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::properties::{Content, Request, SecurityDialog};

fn service() -> Option<pgstudio_pg_client::PgQueryService> {
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
    Some(pgstudio_pg_client::PgQueryService::new(
        settings,
        var("PGSTUDIO_TEST_PASSWORD", ""),
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

/// A primeira coluna da primeira linha de `sql`.
fn value(session: &Arc<dyn QuerySession>, sql: &str) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut first = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QueryEvent::Row(r) if first.is_none() => first = r.into_iter().next().flatten(),
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return first,
            _ => {}
        }
    }
}

fn wait_loaded(d: &mut SecurityDialog) {
    let start = Instant::now();
    while d.is_loading() {
        assert!(d.step().is_none());
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(d.error(), None);
}

fn apply(d: &mut SecurityDialog, ctx: &egui::Context) -> Vec<String> {
    assert_eq!(d.apply(ctx), None);
    let start = Instant::now();
    loop {
        if let Some(r) = d.step() {
            match r {
                Request::Applied(log) => return log,
                other => panic!("{other:?}"),
            }
        }
        assert!(!d.is_applying() || start.elapsed() < Duration::from_secs(20));
        if !d.is_applying() {
            panic!("apply failed: {:?}", d.error());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn roles_and_table_permissions_reach_the_server() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("postgres");
    let role = format!("pgstudio_sec_{}", std::process::id());
    let table = format!("pgstudio_sec_t_{}", std::process::id());

    // New Role: login com senha e CREATEDB.
    let mut d = SecurityDialog::open(Securable::NewRole, service.open("postgres"), &ctx);
    wait_loaded(&mut d);
    let Some(Content::Role {
        edit,
        confirm_password,
        ..
    }) = d.content_mut()
    else {
        panic!("not a role");
    };
    edit.name = role.clone();
    edit.password = "s3nha".into();
    *confirm_password = "s3nha".into();
    edit.create_db = true;
    let log = apply(&mut d, &ctx);
    assert!(
        log.iter().any(|l| l.contains("PASSWORD '********'")),
        "{log:?}"
    );
    let attrs = value(
        &admin,
        &format!(
            "SELECT rolcanlogin::text || rolcreatedb::text || rolsuper::text || \
             (rolpassword IS NOT NULL)::text FROM pg_authid WHERE rolname = '{role}'"
        ),
    );
    assert_eq!(attrs.as_deref(), Some("truetruefalsetrue"));

    // Role Properties: limite de conexões e membro de pg_read_all_stats.
    let oid: u32 = value(
        &admin,
        &format!("SELECT oid FROM pg_roles WHERE rolname = '{role}'"),
    )
    .unwrap()
    .parse()
    .unwrap();
    let mut d = SecurityDialog::open(Securable::Role(oid), service.open("postgres"), &ctx);
    wait_loaded(&mut d);
    assert_eq!(d.title(), format!("Role Properties - {role}"));
    let Some(Content::Role { edit, .. }) = d.content_mut() else {
        panic!("not a role");
    };
    assert!(edit.create_db && edit.can_login);
    edit.connection_limit = 3;
    edit.create_db = false;
    edit.member_of.insert("pg_read_all_stats".into());
    apply(&mut d, &ctx);
    assert_eq!(
        value(
            &admin,
            &format!(
                "SELECT rolconnlimit::text || rolcreatedb::text FROM pg_roles \
                 WHERE rolname = '{role}'"
            )
        )
        .as_deref(),
        Some("3false")
    );
    assert_eq!(
        value(
            &admin,
            &format!("SELECT pg_has_role('{role}', 'pg_read_all_stats', 'MEMBER')::text")
        )
        .as_deref(),
        Some("true")
    );

    // Permissions de uma tabela: SELECT com grant option e INSERT.
    value(&admin, &format!("CREATE TABLE {table} (id int)"));
    let toid: u32 = value(&admin, &format!("SELECT '{table}'::regclass::oid"))
        .unwrap()
        .parse()
        .unwrap();
    let target = Securable::Relation {
        database: "postgres".into(),
        oid: toid,
    };
    let mut d = SecurityDialog::open(target.clone(), service.open("postgres"), &ctx);
    wait_loaded(&mut d);
    let Some(Content::Grants {
        edit, target: t, ..
    }) = d.content_mut()
    else {
        panic!("not grants");
    };
    assert_eq!(t.name, table);
    edit.insert((role.clone(), "SELECT".into()), true);
    edit.insert((role.clone(), "INSERT".into()), false);
    apply(&mut d, &ctx);
    let has = |p: &str| {
        value(
            &admin,
            &format!("SELECT has_table_privilege('{role}', '{table}', '{p}')::text"),
        )
    };
    assert_eq!(has("SELECT").as_deref(), Some("true"));
    assert_eq!(has("SELECT WITH GRANT OPTION").as_deref(), Some("true"));
    assert_eq!(has("INSERT").as_deref(), Some("true"));
    assert_eq!(has("DELETE").as_deref(), Some("false"));

    // Lido de novo, tira o INSERT e a grant option.
    let mut d = SecurityDialog::open(target, service.open("postgres"), &ctx);
    wait_loaded(&mut d);
    let Some(Content::Grants { edit, before, .. }) = d.content_mut() else {
        panic!("not grants");
    };
    assert_eq!(before.get(&(role.clone(), "SELECT".into())), Some(&true));
    edit.remove(&(role.clone(), "INSERT".into()));
    edit.insert((role.clone(), "SELECT".into()), false);
    apply(&mut d, &ctx);
    assert_eq!(has("INSERT").as_deref(), Some("false"));
    assert_eq!(has("SELECT").as_deref(), Some("true"));
    assert_eq!(has("SELECT WITH GRANT OPTION").as_deref(), Some("false"));

    // Um erro desfaz tudo: o nome repetido falha e nada muda.
    let mut d = SecurityDialog::open(Securable::NewRole, service.open("postgres"), &ctx);
    wait_loaded(&mut d);
    let Some(Content::Role { edit, .. }) = d.content_mut() else {
        panic!("not a role");
    };
    edit.name = role.clone();
    edit.member_of.insert("pg_monitor".into());
    assert_eq!(d.apply(&ctx), None);
    let start = Instant::now();
    while d.is_applying() {
        assert!(d.step().is_none());
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        d.error().unwrap().contains("already exists"),
        "{:?}",
        d.error()
    );
    assert_eq!(
        value(
            &admin,
            &format!("SELECT pg_has_role('{role}', 'pg_monitor', 'MEMBER')::text")
        )
        .as_deref(),
        Some("false")
    );

    value(&admin, &format!("DROP TABLE {table}"));
    value(&admin, &format!("DROP ROLE {role}"));
}
