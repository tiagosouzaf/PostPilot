//! Testes contra um Elasticsearch de verdade. Sem
//! `ELASTICSEARCH_TEST_HOST` eles só avisam. Para rodar (veja
//! `docs/elasticsearch.md`):
//!
//! ```text
//! docker run -d -p 19200:9200 -e discovery.type=single-node \
//!     -e xpack.security.enabled=false elasticsearch:8.15.0
//! docker run -d -p 19201:9200 -e discovery.type=single-node \
//!     -e ELASTIC_PASSWORD='PostPilot!2026' elasticsearch:8.15.0
//! ELASTICSEARCH_TEST_HOST=127.0.0.1 ELASTICSEARCH_TEST_PORT=19200 \
//!     ELASTICSEARCH_TEST_TLS_PORT=19201 ELASTICSEARCH_TEST_PASSWORD='PostPilot!2026' \
//!     cargo test -p pgstudio-elasticsearch
//! ```
//!
//! A porta sem TLS é de um servidor sem segurança; a outra, de um com
//! HTTPS e o usuário `elastic`. Cada teste usa índices só dele, com o
//! prefixo `pptest-`.

use std::sync::mpsc;
use std::time::Duration;

use pgstudio_core::catalog::RelationKind;
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, REDIS_LIST_DATABASES, SslMode,
};
use pgstudio_core::metadata::{NodeId, NodeKind, ObjectNode, ScriptAction};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_elasticsearch::EsConnector;

fn settings() -> Option<ConnectionSettings> {
    let Ok(host) = std::env::var("ELASTICSEARCH_TEST_HOST") else {
        eprintln!("ELASTICSEARCH_TEST_HOST not set; skipping");
        return None;
    };
    let mut s = ConnectionSettings::for_engine(Engine::Elasticsearch);
    s.host = host;
    s.port = std::env::var("ELASTICSEARCH_TEST_PORT")
        .unwrap_or_else(|_| "9200".into())
        .parse()
        .unwrap();
    Some(s)
}

/// O servidor com HTTPS e senha, se houver.
fn secure() -> Option<(ConnectionSettings, String)> {
    let mut s = settings()?;
    let Ok(port) = std::env::var("ELASTICSEARCH_TEST_TLS_PORT") else {
        eprintln!("ELASTICSEARCH_TEST_TLS_PORT not set; skipping");
        return None;
    };
    s.port = port.parse().unwrap();
    s.user = "elastic".into();
    s.ssl = SslMode::Require;
    Some((
        s,
        std::env::var("ELASTICSEARCH_TEST_PASSWORD").unwrap_or_default(),
    ))
}

fn connect() -> Option<Connected> {
    Some(
        EsConnector::new()
            .connect(&settings()?, "")
            .expect("connect"),
    )
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
        let e = rx.recv_timeout(Duration::from_secs(60)).expect("event");
        let end = matches!(e, QueryEvent::Finished { .. });
        events.push(e);
        if end {
            return events;
        }
    }
}

/// As grades de uma execução: colunas e linhas de cada uma.
fn grids(events: &[QueryEvent]) -> Vec<(Vec<String>, Vec<Vec<String>>)> {
    let mut out: Vec<(Vec<String>, Vec<Vec<String>>)> = Vec::new();
    for e in events {
        match e {
            QueryEvent::Columns(c) => out.push((c.clone(), Vec::new())),
            QueryEvent::Row(r) => out.last_mut().unwrap().1.push(
                r.iter()
                    .map(|c| c.clone().unwrap_or_else(|| "NULL".into()))
                    .collect(),
            ),
            _ => {}
        }
    }
    out
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

/// Roda `text` e exige que não dê erro.
fn ok(session: &dyn QuerySession, text: &str) -> Vec<QueryEvent> {
    let events = run(session, text);
    assert_eq!(errors(&events), Vec::<String>::new(), "{text}");
    events
}

#[test]
fn connects_and_reports_the_version() {
    let Some(connected) = connect() else { return };
    assert!(
        connected.server_version.split('.').count() >= 3,
        "{}",
        connected.server_version
    );
    assert_eq!(connected.settings.engine, Engine::Elasticsearch);
    assert_eq!(connected.settings.database, "cluster");
    let session = connected.queries.open("");
    assert_eq!(session.database(), "cluster");
    let dbs = run(session.as_ref(), REDIS_LIST_DATABASES);
    assert_eq!(grids(&dbs)[0].1, [["cluster"]]);
    let root = connected.metadata.roots().unwrap().remove(0);
    assert!(root.name.contains("Elasticsearch"), "{}", root.name);
    assert!(!connected.metadata.relational());
}

#[test]
fn https_with_a_password_and_its_errors() {
    let Some((s, password)) = secure() else {
        return;
    };
    let connected = EsConnector::new().connect(&s, &password).expect("connect");
    let session = connected.queries.open("");
    let events = ok(session.as_ref(), "GET /_security/_authenticate");
    assert!(
        grids(&events)[0]
            .1
            .contains(&vec!["username".to_string(), "elastic".to_string()])
    );

    // Senha errada.
    match EsConnector::new().connect(&s, "errada") {
        Err(ConnectError::Authentication(_)) => {}
        other => panic!("{:?}", other.err()),
    }
    // Sem credencial.
    let mut anonymous = s.clone();
    anonymous.user.clear();
    assert!(matches!(
        EsConnector::new().connect(&anonymous, ""),
        Err(ConnectError::Authentication(_))
    ));
    // HTTP numa porta HTTPS: o erro sugere a criptografia.
    let mut plain = s.clone();
    plain.ssl = SslMode::Disable;
    match EsConnector::new().connect(&plain, &password) {
        Err(ConnectError::Network(m)) => assert!(m.contains("HTTPS"), "{m}"),
        other => panic!("{:?}", other.err()),
    }
    // O certificado autoassinado não passa na conferência.
    let mut verify = s.clone();
    verify.ssl = SslMode::VerifyFull;
    assert!(matches!(
        EsConnector::new().connect(&verify, &password),
        Err(ConnectError::Tls(_))
    ));
}

#[test]
fn closed_port_is_a_network_error() {
    let Some(mut s) = settings() else { return };
    s.port = 1;
    assert!(matches!(
        EsConnector::new().connect(&s, ""),
        Err(ConnectError::Network(_))
    ));
}

#[test]
fn requests_show_documents_in_a_grid() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("");
    run(session.as_ref(), "DELETE /pptest-consulta");
    let seed = r#"
# O índice com o mapeamento
PUT /pptest-consulta
{
  "mappings": { "properties": {
    "cliente": { "properties": { "cidade": { "type": "keyword" } } },
    "total": { "type": "double" } } }
}

POST /pptest-consulta/_bulk?refresh=true
{ "index": { "_id": "1" } }
{ "numero": 1, "cliente": { "nome": "Ana", "cidade": "Recife" }, "total": 10.5 }
{ "index": { "_id": "2" } }
{
  "numero": 2,
  "cliente": { "nome": "Bia", "cidade": "Recife" },
  "total": 20,
  "cupom": "X"
}
{ "index": { "_id": "3" } }
{ "numero": 3, "cliente": { "nome": "Caio", "cidade": "Natal" }, "total": 5 }
"#;
    let events = ok(session.as_ref(), seed);
    let g = grids(&events);
    // PUT: campo e valor; _bulk: uma linha por operação.
    assert!(g[0].1.contains(&vec!["acknowledged".into(), "true".into()]));
    assert_eq!(g[1].1.len(), 3);
    assert_eq!(
        g[1].1[0][..5],
        ["index", "pptest-consulta", "1", "201", "created"]
    );

    // Uma busca com ordenação e agregação.
    let search = r#"GET /pptest-consulta/_search
{
  "query": { "range": { "total": { "gte": 10 } } },
  "sort": [{ "numero": "desc" }],
  "aggs": { "por_cidade": { "terms": { "field": "cliente.cidade" },
            "aggs": { "soma": { "sum": { "field": "total" } } } } }
}"#;
    let events = ok(session.as_ref(), search);
    let g = grids(&events);
    assert_eq!(
        g[0].0,
        [
            "_index", "_id", "_score", "numero", "cliente", "total", "cupom", "sort"
        ]
    );
    assert_eq!(
        g[0].1,
        [
            [
                "pptest-consulta",
                "2",
                "NULL",
                "2",
                r#"{"nome":"Bia","cidade":"Recife"}"#,
                "20",
                "X",
                "[2]"
            ],
            [
                "pptest-consulta",
                "1",
                "NULL",
                "1",
                r#"{"nome":"Ana","cidade":"Recife"}"#,
                "10.5",
                "NULL",
                "[1]"
            ],
        ]
    );
    assert_eq!(g[1].0, ["por_cidade", "doc_count", "soma"]);
    assert_eq!(g[1].1, [["Recife", "2", "30.5"]]);
    assert!(
        notices(&events)[0].starts_with("2 "),
        "{:?}",
        notices(&events)
    );

    // Um documento, uma contagem, o _cat como grade e o SQL.
    let events = ok(
        session.as_ref(),
        "GET /pptest-consulta/_doc/3\nGET /pptest-consulta/_count\nGET /_cat/indices/pptest-consulta?h=index,docs.count\nPOST /_sql?format=json\n{ \"query\": \"SELECT numero, cupom FROM \\\"pptest-consulta\\\" ORDER BY numero\" }",
    );
    let g = grids(&events);
    assert!(g[0].1.contains(&vec!["numero".into(), "3".into()]));
    assert!(g[0].1.contains(&vec!["found".into(), "true".into()]));
    assert_eq!(g[1].1[0], ["count", "3"]);
    assert_eq!(g[2].0, ["index", "docs.count"]);
    assert_eq!(g[2].1, [["pptest-consulta", "3"]]);
    assert_eq!(g[3].0, ["numero", "cupom"]);
    assert_eq!(g[3].1, [["1", "NULL"], ["2", "X"], ["3", "NULL"]]);

    // Um erro para a execução e aponta a requisição.
    let text =
        "GET /pptest-consulta/_count\nGET /pptest-nao-existe/_search\nDELETE /pptest-consulta";
    let events = run(session.as_ref(), text);
    let error = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Error(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        error.message.contains("index_not_found_exception"),
        "{}",
        error.message
    );
    assert_eq!(error.code, "404");
    assert_eq!(
        error.position,
        Some(text.find("GET /pptest-nao").unwrap() + 1)
    );
    // A terceira não rodou: o índice continua lá.
    assert_eq!(
        grids(&ok(session.as_ref(), "GET /pptest-consulta/_count"))[0].1[0][1],
        "3"
    );
    // JSON inválido: o servidor explica.
    let events = run(
        session.as_ref(),
        "POST /pptest-consulta/_search\n{ \"query\": }",
    );
    assert_eq!(errors(&events).len(), 1);
    // Texto que não é requisição.
    let events = run(session.as_ref(), "SELECT 1");
    assert!(
        errors(&events)[0].contains("GET /_cat/indices"),
        "{:?}",
        errors(&events)
    );

    ok(session.as_ref(), "DELETE /pptest-consulta");
}

#[test]
fn explorer_lists_indices_fields_and_aliases() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("");
    run(session.as_ref(), "DELETE /pptest-arvore-a,pptest-arvore-b");
    let seed = r#"PUT /pptest-arvore-a
{
  "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
  "mappings": { "properties": {
    "nome": { "type": "text", "fields": { "keyword": { "type": "keyword" } } },
    "cliente": { "properties": { "cidade": { "type": "keyword" } } } } },
  "aliases": { "pptest-arvore-atual": {} }
}
PUT /pptest-arvore-b
POST /pptest-arvore-a/_doc?refresh=true
{ "nome": "Ana" }
"#;
    ok(session.as_ref(), seed);

    let meta = &connected.metadata;
    let server = meta.roots().unwrap().remove(0);
    let groups = meta.children(&server.id).unwrap();
    assert_eq!(
        groups.iter().map(|g| g.id.0.as_str()).collect::<Vec<_>>(),
        ["indices", "hidden", "aliases", "streams"]
    );
    assert!(groups.iter().all(|g| g.kind == NodeKind::KeyGroup));
    let indices_id = NodeId("indices".into());
    let mine = |nodes: Vec<ObjectNode>| -> Vec<ObjectNode> {
        nodes
            .into_iter()
            .filter(|n| n.name.starts_with("pptest-arvore"))
            .collect()
    };
    let indices = mine(meta.children(&indices_id).unwrap());
    assert_eq!(names(&indices), ["pptest-arvore-a", "pptest-arvore-b"]);
    assert_eq!(indices[0].kind, NodeKind::Table);
    assert_eq!(meta.database_of(&indices[0].id), Some("cluster".into()));

    // O filtro vale para os índices.
    assert_eq!(meta.filter(&indices_id), Some(String::new()));
    assert_eq!(meta.filter(&indices[0].id), None);
    meta.set_filter(&indices_id, "arvore-b");
    assert_eq!(
        names(&meta.children(&indices_id).unwrap()),
        ["pptest-arvore-b"]
    );
    meta.set_filter(&indices_id, "pptest-arvore-*");
    assert_eq!(meta.children(&indices_id).unwrap().len(), 2);
    meta.set_filter(&indices_id, "");

    // Os campos do mapeamento.
    let fields = meta.children(&indices[0].id).unwrap();
    assert_eq!(
        names(&fields),
        [
            "cliente (object)",
            "cliente.cidade (keyword)",
            "nome (text)",
            "nome.keyword (keyword)"
        ]
    );
    assert_eq!(fields[0].kind, NodeKind::Column);

    // Aliases.
    let aliases = mine(meta.children(&NodeId("aliases".into())).unwrap());
    assert_eq!(names(&aliases), ["pptest-arvore-atual"]);
    assert_eq!(aliases[0].kind, NodeKind::View);

    // Os comandos do menu rodam como estão.
    let open = meta
        .script(&indices[0].id, ScriptAction::SelectTop(1000))
        .unwrap();
    assert!(open.starts_with("GET /pptest-arvore-a/_search\n"), "{open}");
    assert_eq!(grids(&ok(session.as_ref(), &open))[0].1.len(), 1);
    let open = meta
        .script(&aliases[0].id, ScriptAction::SelectTop(10))
        .unwrap();
    assert_eq!(grids(&ok(session.as_ref(), &open))[0].1.len(), 1);
    let create = meta.script(&indices[0].id, ScriptAction::Create).unwrap();
    assert!(create.starts_with("PUT /pptest-arvore-a\n{"), "{create}");
    assert!(create.contains("\"number_of_shards\": \"1\""), "{create}");
    assert!(create.contains("\"cidade\""), "{create}");
    assert!(create.contains("\"pptest-arvore-atual\""), "{create}");
    assert!(
        !create.contains("uuid") && !create.contains("creation_date"),
        "{create}"
    );
    // O script recria o índice.
    ok(session.as_ref(), "DELETE /pptest-arvore-a");
    ok(session.as_ref(), &create);
    assert_eq!(meta.children(&indices[0].id).unwrap().len(), 4);
    let alias = meta.script(&aliases[0].id, ScriptAction::Create).unwrap();
    assert!(alias.starts_with("POST /_aliases\n"), "{alias}");
    assert!(alias.contains("\"index\": \"pptest-arvore-a\""), "{alias}");
    assert_eq!(
        meta.script(&indices[1].id, ScriptAction::Drop).unwrap(),
        "DELETE /pptest-arvore-b"
    );

    // Propriedades.
    let props = meta.properties(&indices[1].id).unwrap();
    let prop = |name: &str| {
        props
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.clone())
            .unwrap_or_else(|| panic!("{name} missing in {props:?}"))
    };
    assert_eq!(prop("Documents"), "0");
    assert_eq!(prop("Status"), "open");
    assert!(
        prop("Storage size").ends_with(" B"),
        "{}",
        prop("Storage size")
    );
    let props = meta.properties(&aliases[0].id).unwrap();
    assert_eq!(props[0].value, "pptest-arvore-a");

    // O catálogo do IntelliSense.
    let catalog = meta.catalog("cluster").unwrap();
    assert_eq!(catalog.engine, Engine::Elasticsearch);
    let relations = &catalog.schemas[0].relations;
    let a = relations
        .iter()
        .find(|r| r.name == "pptest-arvore-a")
        .unwrap();
    assert_eq!(a.kind, RelationKind::Table);
    assert_eq!(
        a.columns
            .iter()
            .map(|c| (c.name.as_str(), c.ty.as_str()))
            .collect::<Vec<_>>(),
        [
            ("cliente", "object"),
            ("cliente.cidade", "keyword"),
            ("nome", "text"),
            ("nome.keyword", "keyword")
        ]
    );
    let alias = relations
        .iter()
        .find(|r| r.name == "pptest-arvore-atual")
        .unwrap();
    assert_eq!(alias.kind, RelationKind::View);
    assert!(relations.iter().all(|r| !r.name.starts_with('.')));

    ok(session.as_ref(), "DELETE /pptest-arvore-a,pptest-arvore-b");
}

#[test]
fn a_slow_request_can_be_canceled() {
    let Some(connected) = connect() else { return };
    let session = connected.queries.open("");
    let (tx, rx) = mpsc::channel();
    // Espera o cluster ficar num estado que não vem: até o timeout.
    session.execute(
        "GET /_cluster/health?wait_for_nodes=99&timeout=30s".into(),
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    );
    std::thread::sleep(Duration::from_millis(300));
    session.cancel();
    let event = rx.recv_timeout(Duration::from_secs(5)).expect("finished");
    assert!(
        matches!(event, QueryEvent::Finished { canceled: true }),
        "{event:?}"
    );
    // A sessão continua valendo.
    ok(session.as_ref(), "GET /");
}
