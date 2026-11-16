//! Testes contra um MongoDB de verdade. Sem `MONGODB_TEST_HOST` eles só
//! avisam. Para rodar (veja `docs/mongodb.md`):
//!
//! ```text
//! docker run -d -p 27117:27017 -e MONGO_INITDB_ROOT_USERNAME=root \
//!     -e MONGO_INITDB_ROOT_PASSWORD='PostPilot!2026' mongo:7
//! MONGODB_TEST_HOST=127.0.0.1 MONGODB_TEST_PORT=27117 MONGODB_TEST_USER=root \
//!     MONGODB_TEST_PASSWORD='PostPilot!2026' cargo test -p pgstudio-mongodb
//! ```
//!
//! Cada teste usa um banco só dele, com o prefixo `pptest_`.

use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::catalog::RelationKind;
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, REDIS_LIST_DATABASES,
};
use pgstudio_core::metadata::{NodeKind, ObjectNode, ScriptAction};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_mongodb::MongoConnector;
use pgstudio_pg_client::DbRuntime;

fn var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn settings() -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("MONGODB_TEST_HOST") else {
        eprintln!("MONGODB_TEST_HOST not set; skipping");
        return None;
    };
    let mut s = ConnectionSettings::for_engine(Engine::MongoDb);
    s.host = host;
    s.port = var("MONGODB_TEST_PORT", "27017").parse().unwrap();
    s.user = var("MONGODB_TEST_USER", "");
    Some((s, var("MONGODB_TEST_PASSWORD", "")))
}

fn connector() -> MongoConnector {
    MongoConnector::new(DbRuntime::new().unwrap())
}

fn connect() -> Option<Connected> {
    let (s, pw) = settings()?;
    Some(connector().connect(&s, &pw).expect("connect"))
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

fn columns(events: &[QueryEvent]) -> Vec<Vec<String>> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Columns(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
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

fn notices(events: &[QueryEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Notice(m) => Some(m.message.clone()),
            _ => None,
        })
        .collect()
}

fn names(nodes: &[ObjectNode]) -> Vec<&str> {
    nodes.iter().map(|n| n.name.as_str()).collect()
}

#[test]
fn connects_and_reports_the_version() {
    let Some(connected) = connect() else { return };
    assert_eq!(connected.settings.engine, Engine::MongoDb);
    let major: u32 = connected
        .server_version
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .expect("version");
    assert!(major >= 4, "{}", connected.server_version);
    let roots = connected.metadata.roots().unwrap();
    assert!(roots[0].name.contains("MongoDB"), "{}", roots[0].name);
    assert!(!connected.metadata.relational());
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
    // Sem login num servidor que pede: também não entra.
    let mut anonymous = s.clone();
    anonymous.user.clear();
    match connector().connect(&anonymous, "") {
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
    let start = std::time::Instant::now();
    match connector().connect(&s, &pw) {
        Err(ConnectError::Network(m)) => assert!(!m.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn queries_show_documents_in_a_grid() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("pptest_consultas");
    assert_eq!(session.engine(), Engine::MongoDb);
    let seed = r#"
        db.dropDatabase()
        db.pedidos.insertMany([
            { _id: 1, cliente: "Ana", total: 120.5, itens: [{ sku: "A", qtd: 2 }], pago: true },
            { _id: 2, cliente: "Bia", total: 80, endereco: { cidade: "Recife" } },
            { _id: 3, cliente: "Caio", total: 300, quando: ISODate("2024-05-01T10:30:00Z") },
        ])
    "#;
    let events = run(session.as_ref(), seed);
    assert_eq!(errors(&events), Vec::<String>::new());
    assert!(
        notices(&events).iter().any(|n| n.starts_with("3 document")),
        "{:?}",
        notices(&events)
    );

    // Colunas: a união dos campos, na ordem em que aparecem.
    let events = run(
        session.as_ref(),
        "db.pedidos.find({ total: { $gte: 100 } }).sort({ total: -1 })",
    );
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(
        columns(&events),
        [["_id", "cliente", "total", "quando", "itens", "pago"]]
    );
    assert_eq!(
        rows(&events),
        [
            ["3", "Caio", "300", "2024-05-01T10:30:00Z", "NULL", "NULL"],
            [
                "1",
                "Ana",
                "120.5",
                "NULL",
                r#"[{"sku":"A","qtd":2}]"#,
                "true"
            ],
        ]
    );
    assert!(events.contains(&QueryEvent::CommandComplete { rows: 2 }));

    // Vários comandos: cada um a sua grade.
    let events = run(
        session.as_ref(),
        "db.pedidos.countDocuments({})\n\
         db.pedidos.distinct('cliente')\n\
         db.pedidos.aggregate([{ $group: { _id: null, soma: { $sum: '$total' } } }])\n\
         db.pedidos.findOne({ _id: 2 }, { endereco: 1 })\n\
         db.pedidos.find({ cliente: /^b/i }).count()",
    );
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(
        columns(&events),
        [
            vec!["count"],
            vec!["value"],
            vec!["_id", "soma"],
            vec!["_id", "endereco"],
            vec!["count"],
        ]
    );
    assert_eq!(
        rows(&events),
        [
            vec!["3"],
            vec!["Ana"],
            vec!["Bia"],
            vec!["Caio"],
            vec!["NULL", "500.5"],
            vec!["2", r#"{"cidade":"Recife"}"#],
            vec!["1"],
        ]
    );

    // Escritas: a contagem vai para Messages.
    let events = run(
        session.as_ref(),
        "db.pedidos.updateMany({ total: { $lt: 200 } }, { $set: { pequeno: true } })\n\
         db.pedidos.updateOne({ _id: 9 }, { $set: { cliente: 'Dani' } }, { upsert: true })\n\
         db.pedidos.deleteOne({ _id: 9 })\n\
         db.pedidos.createIndex({ cliente: 1 }, { unique: true })\n\
         db.pedidos.getIndexes()",
    );
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(
        notices(&events),
        [
            "2 document(s) matched, 2 modified.",
            "1 document inserted by the upsert (_id 9).",
            "1 document(s) deleted.",
            "Command completed.",
        ]
    );
    let indexes = rows(&events);
    assert_eq!(indexes.len(), 2);
    assert!(indexes[1].contains(&"cliente_1".to_string()), "{indexes:?}");

    // Um erro do servidor para a execução e diz onde.
    let events = run(
        session.as_ref(),
        "db.pedidos.find({})\ndb.pedidos.insertOne({ _id: 10, cliente: 'Ana' })\ndb.pedidos.find({})",
    );
    assert_eq!(columns(&events).len(), 1);
    let error = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Error(m) => Some(m.clone()),
            _ => None,
        })
        .expect("duplicate key error");
    assert!(error.message.contains("duplicate key"), "{error:?}");
    assert_eq!(error.position, Some(21));

    // Erro de leitura: nada roda.
    let events = run(session.as_ref(), "db.pedidos.find({ a: })");
    assert_eq!(errors(&events).len(), 1);
    assert!(columns(&events).is_empty());

    assert_eq!(
        errors(&run(session.as_ref(), "db.dropDatabase()")),
        Vec::<String>::new()
    );
}

#[test]
fn use_and_show_follow_the_window_database() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("pptest_use_a");
    let events = run(
        session.as_ref(),
        "db.dropDatabase()\ndb.a.insertOne({ x: 1 })\nuse pptest_use_b\ndb.dropDatabase()\ndb.b.insertOne({ y: 1 })\nshow collections",
    );
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(session.database(), "pptest_use_b");
    assert_eq!(rows(&events), [["b", "collection"]]);
    assert!(
        notices(&events).contains(&"Switched to database pptest_use_b.".to_string()),
        "{:?}",
        notices(&events)
    );

    let events = run(session.as_ref(), "show dbs");
    let dbs: Vec<String> = rows(&events).into_iter().map(|r| r[0].clone()).collect();
    assert!(dbs.contains(&"pptest_use_a".to_string()), "{dbs:?}");
    assert!(dbs.contains(&"pptest_use_b".to_string()), "{dbs:?}");

    // O seletor de bancos da barra.
    let events = run(session.as_ref(), REDIS_LIST_DATABASES);
    let dbs: Vec<String> = rows(&events).into_iter().map(|r| r[0].clone()).collect();
    assert!(dbs.contains(&"admin".to_string()), "{dbs:?}");
    assert!(dbs.windows(2).all(|w| w[0] <= w[1]), "{dbs:?}");

    let events = run(session.as_ref(), "db.runCommand({ ping: 1 })");
    assert_eq!(columns(&events), [["field", "value"]]);
    assert_eq!(rows(&events)[0], ["ok", "1.0"]);

    run(
        session.as_ref(),
        "db.dropDatabase()\nuse pptest_use_a\ndb.dropDatabase()",
    );
}

#[test]
fn late_fields_are_reported() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("pptest_campos");
    // 1.200 documentos; o campo `tarde` só aparece depois dos 1.000 que
    // decidem as colunas.
    let mut text = String::from("db.dropDatabase()\ndb.c.insertMany([");
    for i in 0..1200 {
        if i < 1100 {
            text.push_str(&format!("{{ _id: {i}, n: {i} }},"));
        } else {
            text.push_str(&format!("{{ _id: {i}, n: {i}, tarde: true }},"));
        }
    }
    text.push_str("])\ndb.c.find({}).sort({ _id: 1 })");
    let events = run(session.as_ref(), &text);
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(columns(&events), [["_id", "n"]]);
    assert_eq!(rows(&events).len(), 1200);
    assert!(
        notices(&events).iter().any(|n| n.contains("tarde")),
        "{:?}",
        notices(&events)
    );
    run(session.as_ref(), "db.dropDatabase()");
}

#[test]
fn explorer_lists_databases_collections_and_indexes() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("pptest_arvore");
    let seed = "db.dropDatabase()\n\
        db.clientes.insertMany([{ nome: 'Ana' }, { nome: 'Bia' }])\n\
        db.clientes.createIndex({ nome: 1 }, { unique: true })\n\
        db.getCollection('meus pedidos').insertOne({ total: 1 })\n\
        db.createCollection('ativos', { viewOn: 'clientes', pipeline: [] })";
    assert_eq!(errors(&run(session.as_ref(), seed)), Vec::<String>::new());

    let meta = &connected.metadata;
    let server = meta.roots().unwrap().remove(0);
    let dbs = meta.children(&server.id).unwrap();
    // O banco da conexão (`test`) aparece mesmo sem existir ainda.
    assert!(names(&dbs).contains(&"test"), "{:?}", names(&dbs));
    let db = dbs.iter().find(|d| d.name == "pptest_arvore").unwrap();
    assert_eq!(db.kind, NodeKind::Database);
    assert_eq!(meta.database_of(&db.id), Some("pptest_arvore".into()));

    let collections = meta.children(&db.id).unwrap();
    assert_eq!(names(&collections), ["ativos", "clientes", "meus pedidos"]);
    assert_eq!(
        collections.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [NodeKind::View, NodeKind::Table, NodeKind::Table]
    );
    let clientes = &collections[1];
    assert_eq!(meta.database_of(&clientes.id), Some("pptest_arvore".into()));

    let indexes = meta.children(&clientes.id).unwrap();
    assert_eq!(
        names(&indexes),
        [r#"_id_ {"_id":1}"#, r#"nome_1 {"nome":1}"#]
    );
    assert_eq!(indexes[1].kind, NodeKind::Index);

    // Os comandos do menu rodam como estão.
    let open = meta
        .script(&clientes.id, ScriptAction::SelectTop(1000))
        .unwrap();
    assert_eq!(open, "db.clientes.find({}).limit(1000)");
    assert_eq!(rows(&run(session.as_ref(), &open)).len(), 2);
    let open = meta
        .script(&collections[2].id, ScriptAction::SelectTop(10))
        .unwrap();
    assert_eq!(
        open,
        r#"db.getCollection("meus pedidos").find({}).limit(10)"#
    );
    assert_eq!(rows(&run(session.as_ref(), &open)).len(), 1);
    assert_eq!(
        meta.script(&clientes.id, ScriptAction::Create).unwrap(),
        "db.createCollection(\"clientes\")\ndb.clientes.createIndex({\"nome\":1}, {\"name\":\"nome_1\",\"unique\":true})"
    );
    let drop_index = meta.script(&indexes[1].id, ScriptAction::Drop).unwrap();
    assert_eq!(drop_index, r#"db.clientes.dropIndex("nome_1")"#);
    assert_eq!(
        errors(&run(session.as_ref(), &drop_index)),
        Vec::<String>::new()
    );
    assert_eq!(meta.children(&clientes.id).unwrap().len(), 1);
    assert_eq!(
        meta.script(&clientes.id, ScriptAction::Drop).unwrap(),
        "db.clientes.drop()"
    );

    // Propriedades.
    let props = meta.properties(&clientes.id).unwrap();
    let prop = |name: &str| {
        props
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.clone())
            .unwrap_or_else(|| panic!("{name} missing in {props:?}"))
    };
    assert_eq!(prop("Documents"), "2");
    assert_eq!(prop("Indexes"), "1");
    assert!(prop("Data size").ends_with(" B"), "{}", prop("Data size"));
    let props = meta.properties(&db.id).unwrap();
    assert!(
        props.iter().any(|p| p.name == "Views" && p.value == "1"),
        "{props:?}"
    );
    assert!(meta.properties(&collections[0].id).unwrap().is_empty());

    run(session.as_ref(), "db.dropDatabase()");
}

/// O catálogo do IntelliSense: coleções com os campos de uma amostra.
#[test]
fn catalog_has_collections_and_sampled_fields() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("pptest_catalogo");
    let seed = "db.dropDatabase()\n\
        db.pedidos.insertMany([\
            { numero: 1, cliente: { nome: 'Ana', cidade: 'Recife' }, cupom: null },\
            { numero: 2, total: NumberDecimal('9.90'), itens: [{ sku: 'a' }], cupom: 'X' }])\n\
        db.createCollection('vazia')\n\
        db.createCollection('pagos', { viewOn: 'pedidos', pipeline: [] })";
    assert_eq!(errors(&run(session.as_ref(), seed)), Vec::<String>::new());

    let catalog = connected.metadata.catalog("pptest_catalogo").unwrap();
    assert_eq!(catalog.engine, Engine::MongoDb);
    let relations = &catalog.schemas[0].relations;
    assert_eq!(
        relations
            .iter()
            .map(|r| (r.name.as_str(), r.kind))
            .collect::<Vec<_>>(),
        [
            ("pagos", RelationKind::View),
            ("pedidos", RelationKind::Table),
            ("vazia", RelationKind::Table),
        ]
    );
    let fields: Vec<(&str, &str)> = relations[1]
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.ty.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            ("_id", "objectId"),
            ("cliente", "object"),
            ("cliente.cidade", "string"),
            ("cliente.nome", "string"),
            // O tipo vem do primeiro documento em que o campo tem valor.
            ("cupom", "string"),
            ("itens", "array"),
            ("numero", "int"),
            ("total", "decimal"),
        ]
    );
    assert!(relations[2].columns.is_empty());
    // A view tem os campos do que ela mostra.
    assert_eq!(relations[0].columns.len(), 8);

    run(session.as_ref(), "db.dropDatabase()");
}
