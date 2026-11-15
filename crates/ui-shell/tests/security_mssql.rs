//! Segurança contra um SQL Server de verdade (com `MSSQL_TEST_HOST`; sem
//! ele, o teste só avisa; veja `docs/sqlserver.md`): criar e alterar um
//! login, um usuário e uma database role pela janela de propriedades,
//! conferindo em `sys.server_principals` e `sys.database_principals`.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::metadata::{PrincipalKind, Securable};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::properties::{Content, Request, SecurityDialog};
use pgstudio_ui_shell::security_mssql::PermState;

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
        match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
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
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(d.error(), None);
}

/// Aplica e espera; devolve o log, ou o erro do servidor.
fn apply(d: &mut SecurityDialog, ctx: &egui::Context) -> Result<Vec<String>, String> {
    assert_eq!(d.apply(ctx), None, "{:?}", d.error());
    let start = Instant::now();
    loop {
        if let Some(r) = d.step() {
            match r {
                Request::Applied(log) => return Ok(log),
                other => panic!("{other:?}"),
            }
        }
        if !d.is_applying() {
            return Err(d.error().unwrap_or_default().to_string());
        }
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn principal(kind: PrincipalKind, database: Option<&str>, name: Option<&str>) -> Securable {
    Securable::Principal {
        kind,
        database: database.map(str::to_string),
        name: name.map(str::to_string),
    }
}

#[test]
fn logins_users_and_roles_reach_the_server() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("tempdb");
    let id = std::process::id();
    let login = format!("pp_sec_{id}");
    let renamed = format!("pp_sec_{id}_b");
    let role = format!("pp_sec_role_{id}");

    // New Login: senha, banco padrão e uma server role.
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::Login, None, None),
        service.open("master"),
        &ctx,
    );
    wait_loaded(&mut d);
    assert_eq!(d.title(), "Login - New");
    let Some(Content::Principal { edit, lists, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert!(lists.databases.iter().any(|d| d == "tempdb"), "{lists:?}");
    assert!(lists.roles.iter().any(|r| r == "sysadmin"), "{lists:?}");
    assert!(!lists.roles.iter().any(|r| r == "public"), "{lists:?}");
    edit.name = login.clone();
    edit.default_database = "tempdb".into();
    edit.member_of.insert("dbcreator".into());
    // Sem senha, a janela não deixa seguir.
    assert_eq!(d.apply(&ctx), None);
    assert_eq!(d.error(), Some("A new login needs a password."));
    let Some(Content::Principal {
        edit,
        confirm_password,
        ..
    }) = d.content_mut()
    else {
        panic!("not a principal");
    };
    edit.password = "Pp!Sec#2026x".into();
    *confirm_password = "Pp!Sec#2026x".into();
    let log = apply(&mut d, &ctx).expect("create login");
    assert!(
        log.iter().any(|l| l.contains("PASSWORD = N'********'")),
        "{log:?}"
    );
    assert_eq!(
        value(
            &admin,
            &format!(
                "SELECT default_database_name + '|' + CAST(is_disabled AS varchar(1)) + '|' + \
                 CAST(IS_SRVROLEMEMBER('dbcreator', name) AS varchar(1)) \
                 FROM sys.server_principals WHERE name = N'{login}'"
            )
        )
        .as_deref(),
        Some("tempdb|0|1")
    );

    // Login Properties: renomear, desabilitar, trocar a role e a política.
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::Login, None, Some(&login)),
        service.open("master"),
        &ctx,
    );
    wait_loaded(&mut d);
    assert_eq!(d.title(), format!("Login Properties - {login}"));
    let Some(Content::Principal { edit, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert_eq!(edit.default_database, "tempdb");
    assert!(edit.sql_login && edit.check_policy && !edit.disabled);
    assert!(edit.member_of.contains("dbcreator"), "{edit:?}");
    edit.name = renamed.clone();
    edit.disabled = true;
    edit.check_policy = false;
    edit.password = "Outra!Senha#9".into();
    edit.member_of.clear();
    edit.member_of.insert("processadmin".into());
    // A confirmação da senha não bate.
    assert_eq!(d.apply(&ctx), None);
    assert_eq!(
        d.error(),
        Some("The password and its confirmation do not match.")
    );
    let Some(Content::Principal {
        confirm_password, ..
    }) = d.content_mut()
    else {
        panic!("not a principal");
    };
    *confirm_password = "Outra!Senha#9".into();
    apply(&mut d, &ctx).expect("alter login");
    assert_eq!(
        value(
            &admin,
            &format!(
                "SELECT CAST(p.is_disabled AS varchar(1)) + \
                 CAST(l.is_policy_checked AS varchar(1)) + \
                 CAST(IS_SRVROLEMEMBER('dbcreator', p.name) AS varchar(1)) + \
                 CAST(IS_SRVROLEMEMBER('processadmin', p.name) AS varchar(1)) + \
                 CAST(PWDCOMPARE(N'Outra!Senha#9', l.password_hash) AS varchar(1)) \
                 FROM sys.server_principals p \
                 JOIN sys.sql_logins l ON l.principal_id = p.principal_id \
                 WHERE p.name = N'{renamed}'"
            )
        )
        .as_deref(),
        Some("10011")
    );

    // New User no tempdb, para o login, com schema e role.
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::User, Some("tempdb"), None),
        service.open("tempdb"),
        &ctx,
    );
    wait_loaded(&mut d);
    assert_eq!(d.title(), "Database User - New");
    let Some(Content::Principal { edit, lists, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert!(lists.logins.contains(&renamed), "{lists:?}");
    assert!(lists.schemas.iter().any(|s| s == "dbo"), "{lists:?}");
    assert!(lists.roles.iter().any(|r| r == "db_datareader"));
    edit.name = login.clone();
    edit.login = renamed.clone();
    edit.default_schema = "dbo".into();
    edit.member_of.insert("db_datareader".into());
    apply(&mut d, &ctx).expect("create user");
    let user_state = format!(
        "SELECT ISNULL(SUSER_SNAME(p.sid), '') + '|' + ISNULL(p.default_schema_name, '') + '|' + \
         (SELECT ISNULL(STRING_AGG(r.name, ','), '') FROM sys.database_role_members m \
           JOIN sys.database_principals r ON r.principal_id = m.role_principal_id \
          WHERE m.member_principal_id = p.principal_id) \
         FROM sys.database_principals p WHERE p.name = N'{login}'"
    );
    assert_eq!(
        value(&admin, &user_state),
        Some(format!("{renamed}|dbo|db_datareader"))
    );

    // New Database Role com o usuário de membro.
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::DatabaseRole, Some("tempdb"), None),
        service.open("tempdb"),
        &ctx,
    );
    wait_loaded(&mut d);
    let Some(Content::Principal { edit, lists, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert!(lists.candidates.contains(&login), "{lists:?}");
    edit.name = role.clone();
    edit.members.insert(login.clone());
    apply(&mut d, &ctx).expect("create role");

    // Database User: sai da db_datareader; a role nova aparece.
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::User, Some("tempdb"), Some(&login)),
        service.open("tempdb"),
        &ctx,
    );
    wait_loaded(&mut d);
    assert_eq!(d.title(), format!("Database User - {login}"));
    let Some(Content::Principal { edit, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert_eq!(edit.login, renamed);
    assert_eq!(
        edit.member_of.iter().cloned().collect::<Vec<_>>(),
        ["db_datareader".to_string(), role.clone()]
    );
    edit.member_of.remove("db_datareader");
    apply(&mut d, &ctx).expect("alter user");
    assert_eq!(
        value(&admin, &user_state),
        Some(format!("{renamed}|dbo|{role}"))
    );

    // Um erro do servidor desfaz tudo: a role some da lista, mas o
    // segundo comando falha (membro que não existe).
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::DatabaseRole, Some("tempdb"), Some(&role)),
        service.open("tempdb"),
        &ctx,
    );
    wait_loaded(&mut d);
    let Some(Content::Principal { edit, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert!(!edit.fixed);
    assert!(edit.members.contains(&login));
    edit.name = format!("{role}_x");
    edit.members.insert("pp_ninguem".into());
    let error = apply(&mut d, &ctx).expect_err("missing member");
    assert!(error.contains("pp_ninguem"), "{error}");
    assert_eq!(
        value(
            &admin,
            &format!(
                "SELECT CAST(COUNT(*) AS varchar(5)) + '|' + CAST(@@TRANCOUNT AS varchar(5)) \
                 FROM sys.database_principals WHERE name = N'{role}'"
            )
        )
        .as_deref(),
        Some("1|0")
    );

    // A role fixa só muda de membros.
    let mut d = SecurityDialog::open(
        principal(PrincipalKind::ServerRole, None, Some("sysadmin")),
        service.open("master"),
        &ctx,
    );
    wait_loaded(&mut d);
    let Some(Content::Principal { edit, .. }) = d.content_mut() else {
        panic!("not a principal");
    };
    assert!(edit.fixed);
    assert!(edit.members.contains("sa"), "{edit:?}");

    value(
        &admin,
        &format!("DROP USER [{login}]; DROP ROLE [{role}]; DROP LOGIN [{renamed}]"),
    );
}

#[test]
fn object_and_database_permissions_reach_the_server() {
    let Some(service) = service() else { return };
    let ctx = egui::Context::default();
    let admin = service.open("tempdb");
    let id = std::process::id();
    let user = format!("pp_perm_{id}");
    let table = format!("pp_perm_t_{id}");
    value(
        &admin,
        &format!(
            "CREATE USER [{user}] WITHOUT LOGIN; CREATE TABLE dbo.[{table}] (id int); \
             GRANT DELETE ON OBJECT::dbo.[{table}] TO [{user}]"
        ),
    );
    let oid: u32 = value(&admin, &format!("SELECT OBJECT_ID('dbo.{table}')"))
        .unwrap()
        .parse()
        .unwrap();
    let key = |p: &str| (user.clone(), p.to_string());
    // O que o usuário pode na tabela: SELECT, INSERT, UPDATE, DELETE.
    let can = format!(
        "EXECUTE AS USER = N'{user}'; \
         SELECT CAST(HAS_PERMS_BY_NAME('dbo.{table}', 'OBJECT', 'SELECT') AS varchar(1)) + \
                CAST(HAS_PERMS_BY_NAME('dbo.{table}', 'OBJECT', 'INSERT') AS varchar(1)) + \
                CAST(HAS_PERMS_BY_NAME('dbo.{table}', 'OBJECT', 'UPDATE') AS varchar(1)) + \
                CAST(HAS_PERMS_BY_NAME('dbo.{table}', 'OBJECT', 'DELETE') AS varchar(1)); \
         REVERT"
    );
    assert_eq!(value(&admin, &can).as_deref(), Some("0001"));

    let target = Securable::Relation {
        database: "tempdb".into(),
        oid,
    };
    let mut d = SecurityDialog::open(target.clone(), service.open("tempdb"), &ctx);
    wait_loaded(&mut d);
    assert_eq!(d.title(), format!("Table Properties - [dbo].[{table}]"));
    let Some(Content::Permissions {
        target: t,
        edit,
        listed,
        principals,
        ..
    }) = d.content_mut()
    else {
        panic!("not permissions");
    };
    assert_eq!(t.owner, "dbo");
    assert!(t.privileges.iter().any(|p| p == "SELECT"));
    assert!(principals.contains(&user) && principals.iter().any(|p| p == "public"));
    assert!(listed.contains(&user));
    assert_eq!(edit.get(&key("DELETE")), Some(&PermState::Grant));
    // SELECT com grant option, INSERT, UPDATE negado, DELETE revogado.
    edit.insert(key("SELECT"), PermState::WithGrant);
    edit.insert(key("INSERT"), PermState::Grant);
    edit.insert(key("UPDATE"), PermState::Deny);
    edit.remove(&key("DELETE"));
    let log = apply(&mut d, &ctx).expect("grant");
    assert!(
        log.iter().any(|l| l.contains(&format!(
            "GRANT SELECT ON OBJECT::[dbo].[{table}] TO [{user}] WITH GRANT OPTION"
        ))),
        "{log:?}"
    );
    assert_eq!(value(&admin, &can).as_deref(), Some("1100"));
    let states = format!(
        "SELECT STRING_AGG(p.permission_name + '=' + p.state, ',') \
           WITHIN GROUP (ORDER BY p.permission_name) \
         FROM sys.database_permissions p \
         WHERE p.class = 1 AND p.major_id = {oid} \
           AND p.grantee_principal_id = USER_ID(N'{user}')"
    );
    assert_eq!(
        value(&admin, &states).as_deref(),
        Some("INSERT=G,SELECT=W,UPDATE=D")
    );

    // Reabrir mostra os três estados; tirar o grant option e o DENY.
    let mut d = SecurityDialog::open(target, service.open("tempdb"), &ctx);
    wait_loaded(&mut d);
    let Some(Content::Permissions { edit, .. }) = d.content_mut() else {
        panic!("not permissions");
    };
    assert_eq!(edit.get(&key("SELECT")), Some(&PermState::WithGrant));
    assert_eq!(edit.get(&key("UPDATE")), Some(&PermState::Deny));
    edit.insert(key("SELECT"), PermState::Grant);
    edit.insert(key("UPDATE"), PermState::Grant);
    edit.remove(&key("INSERT"));
    apply(&mut d, &ctx).expect("revoke");
    assert_eq!(value(&admin, &states).as_deref(), Some("SELECT=G,UPDATE=G"));

    // Permissões do banco: CREATE TABLE.
    let mut d = SecurityDialog::open(
        Securable::Database("tempdb".into()),
        service.open("tempdb"),
        &ctx,
    );
    wait_loaded(&mut d);
    assert_eq!(d.title(), "Database Properties - tempdb");
    let Some(Content::Permissions { edit, .. }) = d.content_mut() else {
        panic!("not permissions");
    };
    edit.insert(key("CREATE TABLE"), PermState::Grant);
    let log = apply(&mut d, &ctx).expect("database grant");
    assert!(
        log.iter()
            .any(|l| l.contains(&format!("GRANT CREATE TABLE TO [{user}]"))),
        "{log:?}"
    );
    assert_eq!(
        value(
            &admin,
            &format!(
                "SELECT p.state FROM sys.database_permissions p WHERE p.class = 0 \
                 AND p.permission_name = 'CREATE TABLE' \
                 AND p.grantee_principal_id = USER_ID(N'{user}')"
            )
        )
        .as_deref()
        .map(str::trim),
        Some("G")
    );

    // Um objeto sem permissões próprias (uma constraint) é recusado.
    value(
        &admin,
        &format!("ALTER TABLE dbo.[{table}] ADD CONSTRAINT [ck_{table}] CHECK (id > 0)"),
    );
    let ck: u32 = value(&admin, &format!("SELECT OBJECT_ID('dbo.ck_{table}')"))
        .unwrap()
        .parse()
        .unwrap();
    let mut d = SecurityDialog::open(
        Securable::Function {
            database: "tempdb".into(),
            oid: ck,
        },
        service.open("tempdb"),
        &ctx,
    );
    let start = Instant::now();
    while d.is_loading() {
        d.step();
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        d.error(),
        Some("This object has no permissions of its own.")
    );

    value(
        &admin,
        &format!("DROP TABLE dbo.[{table}]; DROP USER [{user}]"),
    );
}
