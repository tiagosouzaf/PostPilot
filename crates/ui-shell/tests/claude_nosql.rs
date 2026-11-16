//! As ferramentas do Claude num Redis, num MongoDB e num Elasticsearch de
//! verdade (com `REDIS_TEST_HOST`, `MONGODB_TEST_HOST` e
//! `ELASTICSEARCH_TEST_HOST`; sem eles, cada teste avisa e passa): o
//! catálogo na linguagem de cada motor, leitura livre e escrita só depois
//! da confirmação.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_automation::{Client, Origin, Reply};
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_ui_shell::ShellApp;
use serde_json::{Value, json};

fn var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

/// As configurações do servidor de teste do motor, ou `None` sem ele.
fn settings(engine: Engine, prefix: &str, port: &str) -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var(format!("{prefix}_TEST_HOST")) else {
        eprintln!("{prefix}_TEST_HOST not set; skipping");
        return None;
    };
    let mut s = ConnectionSettings::for_engine(engine);
    s.host = host;
    s.port = var(&format!("{prefix}_TEST_PORT"), port).parse().unwrap();
    s.user = var(&format!("{prefix}_TEST_USER"), "");
    Some((s, var(&format!("{prefix}_TEST_PASSWORD"), "")))
}

fn harness(
    connector: Arc<dyn Connector>,
    settings: ConnectionSettings,
    password: String,
) -> (Harness<'static, ShellApp>, Client) {
    let mut app = ShellApp::default().with_connector(connector);
    app.connect_to(settings, password);
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(15), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
    let ctx = harness.ctx.clone();
    let client = harness.state_mut().automation(&ctx);
    (harness, client)
}

/// Chama a ferramenta e roda frames até a resposta. `confirm` responde o
/// diálogo de confirmação, que tem de aparecer; sem ele, não pode.
fn call(
    h: &mut Harness<'static, ShellApp>,
    c: &Client,
    tool: &str,
    args: Value,
    confirm: Option<bool>,
) -> Reply {
    let rx = c.send(tool, args, Origin::Mcp);
    let start = Instant::now();
    let mut answered = false;
    loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            h.run_steps(2);
            assert_eq!(answered, confirm.is_some(), "{tool}: confirmation");
            return r;
        }
        if !answered && h.state().claude_confirmation().is_some() {
            let Some(run) = confirm else {
                panic!("{tool} is waiting for a confirmation");
            };
            h.run_steps(2);
            let ctx = h.ctx.clone();
            h.state_mut().answer_claude_confirmation(&ctx, run);
            answered = true;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{tool} never answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn json_of(r: &Reply) -> Value {
    assert!(!r.is_error, "{}", r.text);
    serde_json::from_str(&r.text).unwrap()
}

/// A primeira coluna de cada linha do resultado `name`.
fn first_column(v: &Value, name: &str) -> Vec<String> {
    let set = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no result {name}: {v}"));
    set["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].as_str().unwrap_or("NULL").to_string())
        .collect()
}

fn run(h: &mut Harness<'static, ShellApp>, c: &Client, text: &str, confirm: Option<bool>) -> Reply {
    call(h, c, "run_query", json!({ "sql": text }), confirm)
}

#[test]
fn redis_catalog_reads_and_confirmed_writes() {
    let Some((mut s, password)) = settings(Engine::Redis, "REDIS", "6379") else {
        return;
    };
    s.database = "6".into();
    let connector = Arc::new(pgstudio_redis::RedisConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ));
    let (mut h, c) = harness(connector, s, password);

    let v = json_of(&call(&mut h, &c, "list_servers", Value::Null, None));
    assert_eq!(v["servers"][0]["engine"], "redis");

    // Escrever pede confirmação; recusada, nada muda.
    let r = run(
        &mut h,
        &c,
        "FLUSHDB\nSET loja:1 Ana\nSET loja:2 Bia\nRPUSH fila a b",
        Some(true),
    );
    assert!(!r.is_error, "{}", r.text);
    let r = run(&mut h, &c, "DEL loja:1", Some(false));
    assert!(r.is_error, "{}", r.text);

    // Ler roda direto.
    let v = json_of(&run(&mut h, &c, "GET loja:1", None));
    assert_eq!(v["results"][0]["rows"][0][0], "Ana");

    let v = json_of(&call(&mut h, &c, "list_databases", json!({}), None));
    assert!(v["results"][0]["row_count"].as_u64().unwrap() >= 1, "{v}");

    let v = json_of(&call(
        &mut h,
        &c,
        "list_objects",
        json!({ "schema": "loja:*" }),
        None,
    ));
    assert_eq!(first_column(&v, "key_count"), ["3"]);
    let text = v.to_string();
    assert!(text.contains("loja:1") && text.contains("loja:2"), "{v}");
    assert!(!text.contains("fila"), "{v}");

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "fila" }),
        None,
    ));
    assert_eq!(v["messages"], json!(["list"]));
    assert_eq!(first_column(&v, "ttl_seconds"), ["-1"]);

    // O que não existe para o Redis diz o que usar no lugar.
    let r = call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "GET loja:1" }),
        None,
    );
    assert!(r.is_error && r.text.contains("SLOWLOG"), "{}", r.text);
    let r = call(&mut h, &c, "get_activity", json!({}), None);
    assert!(r.is_error && r.text.contains("CLIENT LIST"), "{}", r.text);
    let r = call(
        &mut h,
        &c,
        "describe_function",
        json!({ "function": "f" }),
        None,
    );
    assert!(
        r.is_error && r.text.contains("Redis has no functions"),
        "{}",
        r.text
    );
}

#[test]
fn mongodb_catalog_reads_and_confirmed_writes() {
    let Some((mut s, password)) = settings(Engine::MongoDb, "MONGODB", "27017") else {
        return;
    };
    s.database = "pptest_claude".into();
    let connector = Arc::new(pgstudio_mongodb::MongoConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ));
    let (mut h, c) = harness(connector, s, password);

    let v = json_of(&call(&mut h, &c, "list_servers", Value::Null, None));
    assert_eq!(v["servers"][0]["engine"], "mongodb");

    let r = run(
        &mut h,
        &c,
        "db.dropDatabase()\n\
         db.pedidos.insertMany([{ _id: 1, cliente: 'Ana', total: 10 }, { _id: 2, cliente: 'Bia', total: 20 }])\n\
         db.pedidos.createIndex({ cliente: 1 })",
        Some(true),
    );
    assert!(!r.is_error, "{}", r.text);
    let r = run(&mut h, &c, "db.pedidos.deleteMany({})", Some(false));
    assert!(r.is_error, "{}", r.text);

    let v = json_of(&run(
        &mut h,
        &c,
        "db.pedidos.find({ total: { $gt: 15 } })",
        None,
    ));
    assert_eq!(
        v["results"][0]["columns"],
        json!(["_id", "cliente", "total"])
    );
    assert_eq!(v["results"][0]["rows"], json!([["2", "Bia", "20"]]));

    let v = json_of(&call(&mut h, &c, "list_databases", json!({}), None));
    assert!(v.to_string().contains("pptest_claude"), "{v}");

    let v = json_of(&call(&mut h, &c, "list_objects", json!({}), None));
    assert_eq!(first_column(&v, "collections"), ["pedidos"]);

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "pedidos" }),
        None,
    ));
    assert_eq!(first_column(&v, "sample_documents"), ["1", "2"]);
    assert_eq!(first_column(&v, "document_count"), ["2"]);
    assert!(v.to_string().contains("cliente_1"), "{v}");

    let r = call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "db.pedidos.find()" }),
        None,
    );
    assert!(r.is_error && r.text.contains(".explain()"), "{}", r.text);
    let r = call(&mut h, &c, "get_activity", json!({}), None);
    assert!(r.is_error && r.text.contains("currentOp"), "{}", r.text);
}

#[test]
fn elasticsearch_catalog_reads_and_confirmed_writes() {
    let Some((mut s, _)) = settings(Engine::Elasticsearch, "ELASTICSEARCH", "9200") else {
        return;
    };
    s.ssl = SslMode::Disable;
    s.user = String::new();
    let connector = Arc::new(pgstudio_elasticsearch::EsConnector::new());
    let (mut h, c) = harness(connector, s, String::new());

    let v = json_of(&call(&mut h, &c, "list_servers", Value::Null, None));
    assert_eq!(v["servers"][0]["engine"], "elasticsearch");

    // Apagar e gravar pedem confirmação.
    let _ = run(&mut h, &c, "DELETE /pptest-claude", Some(true));
    let r = run(
        &mut h,
        &c,
        "PUT /pptest-claude/_doc/1?refresh=true\n{ \"cliente\": \"Ana\", \"total\": 10 }\n\
         PUT /pptest-claude/_doc/2?refresh=true\n{ \"cliente\": \"Bia\", \"total\": 20 }",
        Some(true),
    );
    assert!(!r.is_error, "{}", r.text);
    let r = run(&mut h, &c, "DELETE /pptest-claude", Some(false));
    assert!(r.is_error, "{}", r.text);

    let v = json_of(&run(
        &mut h,
        &c,
        "GET /pptest-claude/_search\n{ \"query\": { \"match\": { \"cliente\": \"bia\" } } }",
        None,
    ));
    assert!(
        v.to_string().contains("Bia") && !v.to_string().contains("Ana"),
        "{v}"
    );

    let v = json_of(&call(&mut h, &c, "list_databases", json!({}), None));
    assert!(v.to_string().contains("cluster_name"), "{v}");

    let v = json_of(&call(
        &mut h,
        &c,
        "list_objects",
        json!({ "schema": "pptest-cl*" }),
        None,
    ));
    assert_eq!(first_column(&v, "indices"), ["pptest-claude"]);

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "pptest-claude" }),
        None,
    ));
    assert!(
        v.to_string().contains("cliente") && v.to_string().contains("total"),
        "{v}"
    );
    assert!(
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "document_count"),
        "{v}"
    );

    // Um nome que tentaria outra requisição é recusado sem rodar nada.
    let r = call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "x/_mapping\nDELETE /pptest-claude" }),
        None,
    );
    assert!(
        r.is_error && r.text.contains("not a valid index name"),
        "{}",
        r.text
    );
    let r = call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "GET /_search" }),
        None,
    );
    assert!(r.is_error && r.text.contains("profile"), "{}", r.text);

    let _ = run(&mut h, &c, "DELETE /pptest-claude", Some(true));
}
