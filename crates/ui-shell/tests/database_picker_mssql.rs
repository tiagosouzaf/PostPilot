//! A lista de bancos do seletor contra um SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, o teste só avisa; veja `docs/sqlserver.md`):
//! para um login comum ela chega em partes, só com os bancos que ele abre.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use pgstudio_core::connection::{ConnectionSettings, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession};
use pgstudio_ui_shell::databases::DatabaseLists;

fn service(user: Option<&str>) -> Option<Arc<dyn QueryService>> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut settings = ConnectionSettings::for_engine(Engine::SqlServer);
    settings.host = host;
    settings.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    settings.user = user.map_or_else(|| var("MSSQL_TEST_USER", "sa"), String::from);
    settings.ssl = SslMode::Require;
    let password = match user {
        Some(_) => "PostPilot!2026".to_string(),
        None => var("MSSQL_TEST_PASSWORD", ""),
    };
    Some(Arc::new(pgstudio_mssql::MssqlQueryService::new(
        settings,
        password,
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    )))
}

fn run(session: &dyn QuerySession, sql: &str) {
    let (tx, rx) = mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

/// Lê a lista e devolve cada estado diferente que ela mostrou enquanto
/// carregava, e a lista final.
fn read(service: &Arc<dyn QueryService>) -> (Vec<Vec<String>>, Vec<String>) {
    let ctx = eframe::egui::Context::default();
    let mut lists = DatabaseLists::default();
    lists.refresh("srv", service, "master", &ctx);
    let started = Instant::now();
    let mut partial: Vec<Vec<String>> = Vec::new();
    loop {
        lists.poll();
        if !lists.is_loading("srv") {
            assert_eq!(lists.error("srv"), None);
            return (partial, lists.get("srv").to_vec());
        }
        let now = lists.get("srv").to_vec();
        if !now.is_empty() && partial.last() != Some(&now) {
            partial.push(now);
        }
        assert!(started.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn the_list_arrives_as_the_access_to_each_database_is_confirmed() {
    let Some(admin) = service(None) else { return };
    let master = admin.open("master");
    run(
        &*master,
        "IF SUSER_ID('pp_picker') IS NULL \
         CREATE LOGIN pp_picker WITH PASSWORD = 'PostPilot!2026', CHECK_POLICY = OFF",
    );
    for i in 1..=40 {
        let db = format!("pp_pick_{i:02}");
        run(
            &*master,
            &format!("IF DB_ID('{db}') IS NULL CREATE DATABASE {db}"),
        );
        if [2, 20, 39].contains(&i) {
            run(
                &*admin.open(&db),
                "IF USER_ID('pp_picker') IS NULL CREATE USER pp_picker FOR LOGIN pp_picker",
            );
        }
    }
    let mine = |list: &[String]| -> Vec<String> {
        list.iter()
            .filter(|n| n.starts_with("pp_pick_"))
            .cloned()
            .collect()
    };

    // Quem é sysadmin recebe todos de uma vez, sem checagem.
    let (partial, all) = read(&admin);
    assert!(partial.is_empty(), "{partial:?}");
    assert_eq!(mine(&all).len(), 40);
    assert!(all.contains(&"master".to_string()));

    // Um login comum: só os bancos que ele abre, e a lista cresce.
    let reader = service(Some("pp_picker")).unwrap();
    let (partial, list) = read(&reader);
    assert_eq!(mine(&list), ["pp_pick_02", "pp_pick_20", "pp_pick_39"]);
    assert!(list.contains(&"master".to_string()));
    // Cada estado intermediário é um começo da lista final.
    for part in &partial {
        assert!(part.len() < list.len(), "{part:?}");
        assert_eq!(part[..], list[..part.len()], "{part:?}");
    }
}
