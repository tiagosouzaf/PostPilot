//! As ferramentas do Claude (marco 7.1) num PostgreSQL de verdade (com
//! `PGSTUDIO_TEST_HOST`; sem ele, avisa e passa): catálogo, leitura livre
//! numa transação READ ONLY, escrita só depois da confirmação, e o editor
//! e os resultados das janelas de consulta.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use pgstudio_automation::{Client, Origin, Reply};
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::ShellApp;
use serde_json::{Value, json};

fn settings(database: &str) -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    Some((
        ConnectionSettings {
            host,
            port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
            database: database.into(),
            user: var("PGSTUDIO_TEST_USER", "postgres"),
            ssl: SslMode::Disable,
        },
        var("PGSTUDIO_TEST_PASSWORD", ""),
    ))
}

fn connector() -> Arc<dyn Connector> {
    Arc::new(pgstudio_pg_metadata::PgConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

fn run(session: &Arc<dyn QuerySession>, sql: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

/// Cria o banco de teste do zero.
fn setup(db: &str) -> Option<(ConnectionSettings, String)> {
    let (admin, password) = settings("postgres")?;
    let c = connector().connect(&admin, &password).unwrap();
    let s = c.queries.open("postgres");
    run(&s, &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
    run(&s, &format!("CREATE DATABASE {db}"));
    let t = c.queries.open(db);
    run(
        &t,
        "CREATE TABLE cliente (id int PRIMARY KEY, nome text NOT NULL);
         COMMENT ON TABLE cliente IS 'Clientes da loja';
         CREATE TABLE pedido (id int PRIMARY KEY, cliente_id int REFERENCES cliente,
             total numeric(10,2) DEFAULT 0 CHECK (total >= 0), dados jsonb);
         CREATE INDEX pedido_cliente_idx ON pedido (cliente_id);
         INSERT INTO cliente SELECT g, 'cliente ' || g FROM generate_series(1, 5) g;
         INSERT INTO pedido SELECT g, 1 + g % 5, g * 10, jsonb_build_object('n', g)
             FROM generate_series(1, 300) g;
         CREATE FUNCTION grava() RETURNS int LANGUAGE plpgsql AS
             $$ BEGIN INSERT INTO cliente VALUES (99, 'x'); RETURN 1; END $$;",
    );
    settings(db)
}

fn harness(settings: ConnectionSettings, password: String) -> (Harness<'static, ShellApp>, Client) {
    let mut app = ShellApp::default().with_connector(connector());
    app.connect_to(settings, password);
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(10), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
    let ctx = harness.ctx.clone();
    let client = harness.state_mut().automation(&ctx);
    (harness, client)
}

/// Chama a ferramenta e roda frames até a resposta.
fn call(h: &mut Harness<'static, ShellApp>, c: &Client, tool: &str, args: Value) -> Reply {
    let rx = c.send(tool, args, Origin::Mcp);
    let start = Instant::now();
    loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            h.run_steps(2);
            return r;
        }
        if h.state().claude_confirmation().is_some() {
            panic!("{tool} is waiting for a confirmation");
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{tool} never answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Chama uma ferramenta que pede confirmação, responde e devolve a resposta.
fn call_confirmed(
    h: &mut Harness<'static, ShellApp>,
    c: &Client,
    tool: &str,
    args: Value,
    run: bool,
) -> Reply {
    let rx = c.send(tool, args, Origin::Mcp);
    let start = Instant::now();
    while h.state().claude_confirmation().is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(10), "no confirmation");
    }
    // O diálogo aparece desenhado antes da resposta.
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().answer_claude_confirmation(&ctx, run);
    loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            h.run_steps(2);
            return r;
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

fn count(h: &mut Harness<'static, ShellApp>, c: &Client, sql: &str) -> String {
    let v = json_of(&call(h, c, "run_query", json!({ "sql": sql })));
    v["results"][0]["rows"][0][0].as_str().unwrap().to_string()
}

#[test]
fn catalog_and_read_only_queries() {
    const DB: &str = "pgstudio_claude_read";
    let Some((s, password)) = setup(DB) else {
        return;
    };
    let (mut h, c) = harness(s, password);

    let v = json_of(&call(&mut h, &c, "list_servers", Value::Null));
    assert_eq!(v["servers"][0]["active"], true);
    assert_eq!(v["servers"][0]["default_database"], DB);
    let key = v["servers"][0]["server"].as_str().unwrap().to_string();

    let v = json_of(&call(
        &mut h,
        &c,
        "list_databases",
        json!({ "server": key }),
    ));
    let dbs: Vec<&str> = v["results"][0]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].as_str().unwrap())
        .collect();
    assert!(dbs.contains(&DB), "{dbs:?}");

    let v = json_of(&call(
        &mut h,
        &c,
        "list_objects",
        json!({ "schema": "public" }),
    ));
    let names: Vec<String> = v["results"][0]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r[2].as_str().unwrap(), r[1].as_str().unwrap()))
        .collect();
    assert!(names.contains(&"table pedido".to_string()), "{names:?}");
    assert!(names.contains(&"function grava()".to_string()), "{names:?}");

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "cliente" }),
    ));
    assert_eq!(v["results"][0]["name"], "table");
    assert_eq!(v["results"][0]["rows"][0][4], "Clientes da loja");
    let referenced = &v["results"][4];
    assert_eq!(referenced["name"], "referenced_by");
    assert_eq!(referenced["rows"][0][0], "pedido");

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "pedido" }),
    ));
    let columns: Vec<&str> = v["results"][1]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].as_str().unwrap())
        .collect();
    assert_eq!(columns, ["id", "cliente_id", "total", "dados"]);
    let text = v.to_string();
    assert!(
        text.contains("FOREIGN KEY (cliente_id) REFERENCES cliente(id)"),
        "{text}"
    );
    assert!(text.contains("pedido_cliente_idx"), "{text}");

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_function",
        json!({ "function": "grava" }),
    ));
    assert!(
        v["results"][0]["rows"][0][1]
            .as_str()
            .unwrap()
            .contains("INSERT INTO cliente")
    );

    // Leitura: roda sem perguntar e respeita o limite de linhas.
    let v = json_of(&call(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "select id, dados->>'n' as n from pedido order by id", "max_rows": 5 }),
    ));
    let r = &v["results"][0];
    assert_eq!(r["columns"], json!(["id", "n"]));
    assert_eq!(r["rows"].as_array().unwrap().len(), 5);
    assert_eq!(r["row_count"], 300);
    assert_eq!(r["truncated"], true);

    // O classificador não conhece grava(); a transação READ ONLY barra.
    let r = call(&mut h, &c, "run_query", json!({ "sql": "select grava()" }));
    assert!(r.is_error);
    assert!(r.text.contains("read-only transaction"), "{}", r.text);
    assert_eq!(count(&mut h, &c, "select count(*) from cliente"), "5");

    // Erro de SQL volta como erro, e a próxima chamada funciona.
    let r = call(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "select * from nao_existe" }),
    );
    assert!(r.is_error && r.text.contains("42P01"), "{}", r.text);

    let r = call(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "select 1", "server": "outro" }),
    );
    assert!(r.is_error && r.text.contains("not connected"), "{}", r.text);

    let output = h.state().output().join("\n");
    assert!(
        output.contains("Claude Code: run_query (read only)"),
        "{output}"
    );
}

#[test]
fn changes_need_confirmation() {
    const DB: &str = "pgstudio_claude_write";
    let Some((s, password)) = setup(DB) else {
        return;
    };
    let (mut h, c) = harness(s, password);

    let delete = json!({ "sql": "delete from pedido where id > 290" });
    let r = call_confirmed(&mut h, &c, "run_query", delete.clone(), false);
    assert!(r.is_error && r.text.contains("declined"), "{}", r.text);
    assert_eq!(count(&mut h, &c, "select count(*) from pedido"), "300");

    // O diálogo mostra o SQL e o motivo.
    let rx = c.send("run_query", delete.clone(), Origin::Mcp);
    while h.state().claude_confirmation().is_none() {
        h.step();
    }
    let (sql, why) = h.state().claude_confirmation().unwrap();
    assert_eq!(sql, "delete from pedido where id > 290");
    assert!(why.contains("DELETE"), "{why}");
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().answer_claude_confirmation(&ctx, true);
    let start = Instant::now();
    let r = loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            break r;
        }
        assert!(start.elapsed() < Duration::from_secs(30));
    };
    assert_eq!(json_of(&r)["rows_affected"], json!([10]));
    assert_eq!(count(&mut h, &c, "select count(*) from pedido"), "290");

    let r = call_confirmed(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "create table nova (id int)" }),
        true,
    );
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(
        count(
            &mut h,
            &c,
            "select count(*) from pg_tables where tablename = 'nova'"
        ),
        "1"
    );
}

#[test]
fn query_windows() {
    const DB: &str = "pgstudio_claude_windows";
    let Some((s, password)) = setup(DB) else {
        return;
    };
    let (mut h, c) = harness(s, password);

    let r = call(&mut h, &c, "get_query_window", json!({}));
    assert!(
        r.is_error && r.text.contains("open_query_window"),
        "{}",
        r.text
    );

    let v = json_of(&call(
        &mut h,
        &c,
        "open_query_window",
        json!({ "text": "select count(*) from pedido;" }),
    ));
    let id = v["window"].as_u64().unwrap() as u32;
    assert_eq!(v["database"], DB);
    assert_eq!(h.state().active_document(), Some(id));

    let v = json_of(&call(&mut h, &c, "list_query_windows", Value::Null));
    assert_eq!(v["windows"][0]["window"], id);
    assert_eq!(v["windows"][0]["active"], true);
    assert_eq!(v["windows"][0]["database"], DB);

    // Escrever no editor: o texto novo fica selecionado e o Ctrl+Z desfaz.
    let v = json_of(&call(
        &mut h,
        &c,
        "edit_query_window",
        json!({ "text": "select 2;", "mode": "append" }),
    ));
    assert_eq!(v["selection"], json!({ "start": 28, "end": 38 }));
    let text = || "select count(*) from pedido;\nselect 2;".to_string();
    assert_eq!(h.state().documents().get(id).unwrap().text, text());
    let v = json_of(&call(
        &mut h,
        &c,
        "get_query_window",
        json!({ "window": id }),
    ));
    assert_eq!(v["text"], text());
    assert_eq!(v["selection"]["text"], "\nselect 2;");

    let v = json_of(&call(
        &mut h,
        &c,
        "edit_query_window",
        json!({ "text": "select 3;", "mode": "replace_selection" }),
    ));
    assert_eq!(v["selection"], json!({ "start": 28, "end": 37 }));
    assert_eq!(
        h.state().documents().get(id).unwrap().text,
        "select count(*) from pedido;select 3;"
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().documents().get(id).unwrap().text, text());

    json_of(&call(
        &mut h,
        &c,
        "edit_query_window",
        json!({ "text": "select id from pedido order by id;", "mode": "replace_all" }),
    ));
    // Executar a janela: leitura roda direto e os resultados ficam na grade.
    let v = json_of(&call(
        &mut h,
        &c,
        "execute_query_window",
        json!({ "max_rows": 3 }),
    ));
    assert_eq!(v["outcome"], "Query executed successfully.");
    assert_eq!(v["results"][0]["row_count"], 300);
    assert_eq!(v["results"][0]["rows"], json!([["1"], ["2"], ["3"]]));
    let exec = h
        .state()
        .documents()
        .get(id)
        .unwrap()
        .execution
        .as_ref()
        .unwrap();
    assert_eq!(exec.total_rows(), 300);

    let v = json_of(&call(
        &mut h,
        &c,
        "get_query_results",
        json!({ "max_rows": 1 }),
    ));
    assert_eq!(v["results"][0]["rows"], json!([["1"]]));

    // Alterar dados pela janela: pergunta antes.
    json_of(&call(
        &mut h,
        &c,
        "edit_query_window",
        json!({ "text": "update pedido set total = 0 where id = 1;", "mode": "replace_all" }),
    ));
    let r = call_confirmed(&mut h, &c, "execute_query_window", json!({}), false);
    assert!(r.is_error && r.text.contains("declined"), "{}", r.text);
    let r = call_confirmed(&mut h, &c, "execute_query_window", json!({}), true);
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(
        count(&mut h, &c, "select total from pedido where id = 1"),
        "0.00"
    );
}
