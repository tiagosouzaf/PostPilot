//! Testes contra um Redis de verdade. Sem `REDIS_TEST_HOST` eles só
//! avisam. Para rodar (veja `docs/redis.md`):
//!
//! ```text
//! docker run -d -p 16379:6379 redis:7 redis-server --requirepass 'PostPilot!2026'
//! REDIS_TEST_HOST=127.0.0.1 REDIS_TEST_PORT=16379 \
//!     REDIS_TEST_PASSWORD='PostPilot!2026' cargo test -p pgstudio-redis
//! ```
//!
//! Os testes usam o banco 15 e chaves com o prefixo `pptest:`.

use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectError, ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::metadata::NodeId;
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_pg_client::DbRuntime;
use pgstudio_redis::RedisConnector;

fn var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn settings() -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("REDIS_TEST_HOST") else {
        eprintln!("REDIS_TEST_HOST not set; skipping");
        return None;
    };
    let mut s = ConnectionSettings::for_engine(Engine::Redis);
    s.host = host;
    s.port = var("REDIS_TEST_PORT", "6379").parse().unwrap();
    s.user = var("REDIS_TEST_USER", "");
    Some((s, var("REDIS_TEST_PASSWORD", "")))
}

fn connector() -> RedisConnector {
    RedisConnector::new(DbRuntime::new().unwrap())
}

/// Roda `text` numa sessão e devolve todos os eventos.
fn run(session: &dyn QuerySession, text: &str) -> Vec<QueryEvent> {
    let (tx, rx) = mpsc::channel();
    session.execute(
        text.to_string(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    let mut events = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("event");
        let end = matches!(e, QueryEvent::Finished { .. });
        events.push(e);
        if end {
            return events;
        }
    }
}

fn session(database: &str) -> Option<std::sync::Arc<dyn QuerySession>> {
    let (s, pw) = settings()?;
    let connected = connector().connect(&s, &pw).expect("connect");
    Some(connected.queries.open(database))
}

fn rows(events: &[QueryEvent]) -> Vec<Vec<String>> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Row(r) => Some(
                r.iter()
                    .map(|c| c.clone().unwrap_or_else(|| "NULL".into()))
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn errors(events: &[QueryEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Error(m) => Some(m.message.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn connects_and_reports_the_version() {
    let Some((s, pw)) = settings() else { return };
    let connected = connector().connect(&s, &pw).expect("connect");
    assert_eq!(connected.settings.engine, Engine::Redis);
    assert_eq!(connected.settings.database, "db0");
    let major: u32 = connected
        .server_version
        .split('.')
        .next()
        .unwrap()
        .parse()
        .expect("version");
    assert!(major >= 2, "{}", connected.server_version);
    let roots = connected.metadata.roots().unwrap();
    assert!(roots[0].name.contains("Redis"), "{}", roots[0].name);
    let dbs = connected.metadata.children(&roots[0].id).unwrap();
    assert!(dbs.len() >= 16);
    assert!(dbs[0].name.starts_with("db0 ("), "{}", dbs[0].name);
    assert_eq!(
        connected.metadata.database_of(&NodeId("db/3".into())),
        Some("db3".into())
    );
}

#[test]
fn wrong_password_is_an_authentication_error() {
    let Some((s, pw)) = settings() else { return };
    if pw.is_empty() {
        return;
    }
    match connector().connect(&s, "errada") {
        Err(ConnectError::Authentication(m)) => assert!(!m.is_empty()),
        other => panic!("{other:?}"),
    }
    match connector().connect(&s, "") {
        Err(ConnectError::Authentication(m)) => assert!(!m.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn closed_port_is_a_network_error() {
    let Some((mut s, pw)) = settings() else {
        return;
    };
    s.port = 1;
    assert!(matches!(
        connector().connect(&s, &pw),
        Err(ConnectError::Network(_))
    ));
    let Some((mut s, pw)) = settings() else {
        return;
    };
    s.database = "vendas".into();
    assert!(matches!(
        connector().connect(&s, &pw),
        Err(ConnectError::Database(_))
    ));
}

#[test]
fn commands_return_grids_statuses_and_errors() {
    let Some(session) = session("db15") else {
        return;
    };
    let events = run(
        session.as_ref(),
        "# limpa\nDEL pptest:s pptest:h pptest:l pptest:z\n\
         SET pptest:s \"João da Silva\"\nGET pptest:s\n\
         HSET pptest:h nome Ana idade 30\nHGETALL pptest:h\n\
         RPUSH pptest:l a b c\nLRANGE pptest:l 0 -1\n\
         ZADD pptest:z 1.5 um 2 dois\nZRANGE pptest:z 0 -1 WITHSCORES\n\
         GET pptest:h\nTYPE pptest:z\nGET pptest:nada",
    );
    assert_eq!(
        events.last(),
        Some(&QueryEvent::Finished { canceled: false })
    );
    let columns: Vec<&Vec<String>> = events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Columns(c) => Some(c),
            _ => None,
        })
        .collect();
    assert!(columns.iter().any(|c| *c == &["field", "value"]));
    assert!(columns.iter().any(|c| *c == &["member", "score"]));
    let rows = rows(&events);
    assert!(rows.contains(&vec!["João da Silva".to_string()]));
    assert!(rows.contains(&vec!["nome".to_string(), "Ana".to_string()]));
    assert!(rows.contains(&vec!["b".to_string()]));
    assert!(rows.contains(&vec!["um".to_string(), "1.5".to_string()]));
    assert!(rows.contains(&vec!["NULL".to_string()]));
    // SET e TYPE respondem com status: mensagem, sem grade.
    let notices: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Notice(m) => Some(m.message.as_str()),
            _ => None,
        })
        .collect();
    assert!(notices.contains(&"OK"));
    assert!(notices.contains(&"zset"));
    // O GET num hash falha, com a posição do comando, e o resto roda.
    let error = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Error(m) => Some(m),
            _ => None,
        })
        .expect("error");
    assert_eq!(error.code, "WRONGTYPE");
    assert!(error.position.is_some());
    run(session.as_ref(), "DEL pptest:s pptest:h pptest:l pptest:z");
}

#[test]
fn each_window_keeps_its_database() {
    let Some(a) = session("db15") else { return };
    let Some(b) = session("db14") else { return };
    run(a.as_ref(), "SET pptest:db quinze");
    run(b.as_ref(), "SET pptest:db catorze");
    assert_eq!(rows(&run(a.as_ref(), "GET pptest:db")), [["quinze"]]);
    assert_eq!(rows(&run(b.as_ref(), "GET pptest:db")), [["catorze"]]);
    assert_eq!(a.database(), "db15");
    run(a.as_ref(), "DEL pptest:db");
    run(b.as_ref(), "DEL pptest:db");
}

#[test]
fn client_commands_list_databases_and_transactions() {
    let Some(session) = session("db15") else {
        return;
    };
    let dbs = rows(&run(session.as_ref(), Engine::Redis.list_databases_sql()));
    assert!(dbs.len() >= 16);
    assert_eq!(dbs[0], ["db0"]);
    let probe = Engine::Redis.in_transaction_sql();
    assert_eq!(rows(&run(session.as_ref(), probe)), [["0"]]);
    run(session.as_ref(), "MULTI\nSET pptest:tx 1");
    assert_eq!(rows(&run(session.as_ref(), probe)), [["1"]]);
    run(session.as_ref(), "DISCARD");
    assert_eq!(rows(&run(session.as_ref(), probe)), [["0"]]);
}

#[test]
fn unclosed_quote_is_reported_without_running() {
    let Some(session) = session("db15") else {
        return;
    };
    let events = run(session.as_ref(), "SET pptest:q \"sem fim");
    assert_eq!(errors(&events).len(), 1);
    assert_eq!(rows(&run(session.as_ref(), "EXISTS pptest:q")), [["0"]]);
}

#[test]
fn subscribe_streams_until_canceled() {
    let Some(session) = session("db15") else {
        return;
    };
    let Some(publisher) = self::session("db15") else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    session.execute(
        "SUBSCRIBE pptest:canal".into(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    let mut seen = Vec::new();
    let next = |seen: &mut Vec<QueryEvent>| {
        let e = rx.recv_timeout(Duration::from_secs(10)).expect("event");
        seen.push(e);
    };
    // Colunas, aviso e a confirmação da inscrição.
    while rows(&seen).is_empty() {
        next(&mut seen);
    }
    assert_eq!(rows(&seen)[0][..2], ["subscribe", "pptest:canal"]);
    run(publisher.as_ref(), "PUBLISH pptest:canal \"olá\"");
    while rows(&seen).len() < 2 {
        next(&mut seen);
    }
    assert_eq!(rows(&seen)[1], ["message", "pptest:canal", "olá"]);
    session.cancel();
    while !matches!(seen.last(), Some(QueryEvent::Finished { .. })) {
        next(&mut seen);
    }
    assert_eq!(seen.last(), Some(&QueryEvent::Finished { canceled: true }));
    // A sessão reabre a conexão e segue funcionando.
    assert_eq!(rows(&run(session.as_ref(), "ECHO volta")), [["volta"]]);
}

#[test]
fn acl_user_logs_in() {
    let Some((mut s, _)) = settings() else { return };
    let (Ok(user), Ok(pw)) = (
        std::env::var("REDIS_TEST_ACL_USER"),
        std::env::var("REDIS_TEST_ACL_PASSWORD"),
    ) else {
        return;
    };
    s.user = user.clone();
    let connected = connector().connect(&s, &pw).expect("connect");
    assert!(connected.metadata.roots().unwrap()[0].name.contains(&user));
    let session = connected.queries.open("db15");
    assert_eq!(rows(&run(session.as_ref(), "ACL WHOAMI")), [[user]]);
}

#[test]
fn tls_connects_without_verifying_and_refuses_when_verifying() {
    let Some((mut s, pw)) = settings() else {
        return;
    };
    let Ok(port) = std::env::var("REDIS_TEST_TLS_PORT") else {
        return;
    };
    s.port = port.parse().unwrap();
    s.ssl = SslMode::Require;
    let connected = connector().connect(&s, &pw).expect("connect over TLS");
    let session = connected.queries.open("db15");
    assert_eq!(rows(&run(session.as_ref(), "ECHO tls")), [["tls"]]);
    // Certificado autoassinado: com conferência, recusa.
    s.ssl = SslMode::VerifyFull;
    assert!(matches!(
        connector().connect(&s, &pw),
        Err(ConnectError::Tls(_))
    ));
}

/// O Object Explorer de um banco: grupos por prefixo, chaves com o tipo,
/// filtro, "Open Value", "Delete Key" e propriedades. Usa o banco 14, só
/// dele.
#[test]
fn explorer_groups_keys_and_opens_values() {
    use pgstudio_core::metadata::{NodeKind, ScriptAction};

    let Some((s, pw)) = settings() else { return };
    let connected = connector().connect(&s, &pw).expect("connect");
    let session = connected.queries.open("db11");
    let seed = "FLUSHDB\n\
        SET user:1:name Ana\n\
        HSET user:1:profile city Recife age 30\n\
        SET user:2:name Bia\n\
        RPUSH queue:mail a b c\n\
        SADD tags rust redis\n\
        ZADD rank 1 ana 2 bia\n\
        SET \"com espaco\" x\n\
        SET \"sess\\xff\\x00\" bin\n\
        EXPIRE tags 1000";
    assert_eq!(errors(&run(session.as_ref(), seed)), Vec::<String>::new());

    let meta = &connected.metadata;
    assert!(!meta.relational());
    let server = meta.roots().unwrap().remove(0);
    let dbs = meta.children(&server.id).unwrap();
    let db = dbs.iter().find(|d| d.id.0 == "db/11").unwrap();
    assert_eq!((db.name.as_str(), db.has_children), ("db11 (8)", true));
    assert!(!dbs.iter().find(|d| d.id.0 == "db/12").unwrap().has_children);

    let names = |nodes: &[pgstudio_core::metadata::ObjectNode]| -> Vec<String> {
        nodes.iter().map(|n| n.name.clone()).collect()
    };
    let top = meta.children(&db.id).unwrap();
    assert_eq!(
        names(&top),
        [
            "user:* (3)",
            "com espaco (string)",
            "queue:mail (list)",
            "rank (zset)",
            "sess\\xff\\x00 (string)",
            "tags (set)",
        ]
    );
    assert_eq!(top[0].kind, NodeKind::KeyGroup);
    assert_eq!(top[1].kind, NodeKind::Key);
    assert_eq!(meta.database_of(&top[1].id), Some("db11".into()));

    let users = meta.children(&top[0].id).unwrap();
    assert_eq!(names(&users), ["user:1:* (2)", "user:2:name (string)"]);
    let user1 = meta.children(&users[0].id).unwrap();
    assert_eq!(
        names(&user1),
        ["user:1:name (string)", "user:1:profile (hash)"]
    );

    // "Open Value": o comando do tipo, que roda como está.
    let open = |name: &str, level: &[pgstudio_core::metadata::ObjectNode]| -> String {
        let node = level.iter().find(|n| n.name.starts_with(name)).unwrap();
        meta.script(&node.id, ScriptAction::SelectTop(1000))
            .unwrap()
    };
    assert_eq!(open("user:1:profile", &user1), "HGETALL user:1:profile");
    assert_eq!(open("queue:mail", &top), "LRANGE queue:mail 0 999");
    assert_eq!(open("rank", &top), "ZRANGE rank 0 999 WITHSCORES");
    assert_eq!(open("tags", &top), "SMEMBERS tags");
    assert_eq!(open("com espaco", &top), "GET \"com espaco\"");
    let binary = open("sess", &top);
    assert_eq!(binary, "GET \"sess\\xff\\x00\"");
    assert_eq!(rows(&run(session.as_ref(), &binary)), [["bin"]]);
    assert_eq!(
        rows(&run(session.as_ref(), &open("user:1:profile", &user1))),
        [["city", "Recife"], ["age", "30"]]
    );

    // Propriedades de uma chave.
    let tags = top.iter().find(|n| n.name.starts_with("tags")).unwrap();
    let props = meta.properties(&tags.id).unwrap();
    let prop = |name: &str| {
        props
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.clone())
            .unwrap_or_else(|| panic!("{name} missing in {props:?}"))
    };
    assert_eq!(prop("Key"), "tags");
    assert_eq!(prop("Data type"), "set");
    assert_eq!(prop("Length"), "2");
    let ttl: i64 = prop("Expires in (TTL)")
        .trim_end_matches(" s")
        .parse()
        .unwrap();
    assert!((900..=1000).contains(&ttl), "{ttl}");
    let rank = top.iter().find(|n| n.name.starts_with("rank")).unwrap();
    let props = meta.properties(&rank.id).unwrap();
    assert!(props.iter().any(|p| p.value == "No expiry"), "{props:?}");

    // Filtro: texto solto é "contém"; com `*`, o padrão como está.
    assert_eq!(meta.filter(&db.id), Some(String::new()));
    assert_eq!(meta.filter(&top[0].id), None);
    meta.set_filter(&db.id, "name");
    let filtered = meta.children(&db.id).unwrap();
    assert_eq!(names(&filtered), ["Filter: name", "user:* (2)"]);
    assert_eq!(filtered[0].kind, NodeKind::Note);
    let users = meta.children(&filtered[1].id).unwrap();
    assert_eq!(
        names(&users),
        ["user:1:name (string)", "user:2:name (string)"]
    );
    meta.set_filter(&db.id, "queue:*");
    assert_eq!(
        names(&meta.children(&db.id).unwrap()),
        ["Filter: queue:*", "queue:mail (list)"]
    );
    meta.set_filter(&db.id, " ");
    assert_eq!(meta.children(&db.id).unwrap().len(), 6);

    // "Delete Key": o DEL para conferir; depois de rodar, a chave some.
    let del = meta.script(&tags.id, ScriptAction::Drop).unwrap();
    assert_eq!(del, "DEL tags");
    assert_eq!(errors(&run(session.as_ref(), &del)), Vec::<String>::new());
    assert!(
        meta.script(&tags.id, ScriptAction::SelectTop(10))
            .unwrap_err()
            .0
            .contains("no longer exists")
    );
    assert_eq!(meta.children(&db.id).unwrap().len(), 5);
    run(session.as_ref(), "FLUSHDB");
}

/// Mais chaves do que uma leitura traz: a lista corta e avisa.
#[test]
fn explorer_cuts_a_huge_database_and_says_so() {
    use pgstudio_core::metadata::NodeKind;

    let Some((s, pw)) = settings() else { return };
    let connected = connector().connect(&s, &pw).expect("connect");
    let session = connected.queries.open("db13");
    let fill = "FLUSHDB\nEVAL \"for i=1,25000 do redis.call('SET','big:'..i,i) end return 1\" 0";
    assert_eq!(errors(&run(session.as_ref(), fill)), Vec::<String>::new());
    let db = NodeId("db/13".into());
    let top = connected.metadata.children(&db).unwrap();
    assert_eq!(top.len(), 2, "{top:?}");
    assert!(
        top[0].name.starts_with("big:* (2") && top[0].name.ends_with("+)"),
        "{}",
        top[0].name
    );
    assert_eq!(top[1].kind, NodeKind::Note);
    assert!(top[1].name.contains("20000"), "{}", top[1].name);
    // O filtro estreita e a lista volta a caber.
    connected.metadata.set_filter(&db, "big:2499?");
    let filtered = connected.metadata.children(&db).unwrap();
    assert_eq!(filtered.len(), 2, "{filtered:?}");
    assert_eq!(filtered[1].name, "big:* (10)");
    run(session.as_ref(), "FLUSHDB");
}
