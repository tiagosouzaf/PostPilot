//! As ferramentas do Claude num SQL Server de verdade (com
//! `MSSQL_TEST_HOST`; sem ele, avisa e passa): catálogo pelas views
//! `sys.*`, leitura livre numa transação que é sempre desfeita, e escrita
//! só depois da confirmação.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_automation::safety::read_only_batch_for;
use pgstudio_automation::{Client, Origin, Reply};
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::ShellApp;
use serde_json::{Value, json};

fn settings(database: &str) -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("MSSQL_TEST_HOST") else {
        eprintln!("MSSQL_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let mut s = ConnectionSettings::for_engine(Engine::SqlServer);
    s.host = host;
    s.port = var("MSSQL_TEST_PORT", "1433").parse().unwrap();
    s.user = var("MSSQL_TEST_USER", "sa");
    s.database = database.into();
    s.ssl = SslMode::Require;
    Some((s, var("MSSQL_TEST_PASSWORD", "")))
}

fn connector() -> Arc<dyn Connector> {
    Arc::new(pgstudio_mssql::MssqlConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ))
}

/// Roda um script (com `GO`) até o fim; devolve o erro, se houve.
fn run(session: &Arc<dyn QuerySession>, sql: &str) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    let mut error = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => error = Some(e.message),
            QueryEvent::Finished { .. } => return error,
            _ => {}
        }
    }
}

/// Cria o banco de teste do zero.
fn setup(db: &str) -> Option<(ConnectionSettings, String)> {
    let (admin, password) = settings("master")?;
    let c = connector().connect(&admin, &password).unwrap();
    let s = c.queries.open("master");
    let script = format!(
        "IF DB_ID('{db}') IS NOT NULL BEGIN \
             ALTER DATABASE [{db}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; \
             DROP DATABASE [{db}]; END\n\
         GO\n\
         CREATE DATABASE [{db}]\n\
         GO\n\
         USE [{db}]\n\
         GO\n\
         CREATE TABLE dbo.cliente (id int CONSTRAINT pk_cliente PRIMARY KEY, \
             nome nvarchar(60) NOT NULL);\n\
         EXEC sys.sp_addextendedproperty 'MS_Description', N'Clientes da loja', \
             'SCHEMA', 'dbo', 'TABLE', 'cliente';\n\
         CREATE TABLE dbo.pedido (id int CONSTRAINT pk_pedido PRIMARY KEY, \
             cliente_id int CONSTRAINT fk_pedido_cliente REFERENCES dbo.cliente (id), \
             total decimal(10,2) DEFAULT 0 CONSTRAINT ck_total CHECK (total >= 0));\n\
         CREATE INDEX ix_pedido_cliente ON dbo.pedido (cliente_id);\n\
         INSERT INTO dbo.cliente SELECT value, CONCAT(N'cliente ', value) \
             FROM GENERATE_SERIES(1, 5);\n\
         INSERT INTO dbo.pedido SELECT value, 1 + value % 5, value * 10 \
             FROM GENERATE_SERIES(1, 300);\n\
         GO\n\
         CREATE PROCEDURE dbo.grava AS INSERT INTO dbo.cliente VALUES (99, N'x')\n\
         GO\n"
    );
    if let Some(e) = run(&s, &script) {
        panic!("{e}");
    }
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
    const DB: &str = "pp_claude_read";
    let Some((s, password)) = setup(DB) else {
        return;
    };
    let (mut h, c) = harness(s, password);

    let v = json_of(&call(&mut h, &c, "list_servers", Value::Null));
    assert_eq!(v["servers"][0]["engine"], "sqlserver");
    assert_eq!(v["servers"][0]["default_database"], DB);

    let v = json_of(&call(&mut h, &c, "list_databases", json!({})));
    let dbs: Vec<&str> = v["results"][0]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].as_str().unwrap())
        .collect();
    assert!(dbs.contains(&DB) && dbs.contains(&"master"), "{dbs:?}");

    let v = json_of(&call(
        &mut h,
        &c,
        "list_objects",
        json!({ "schema": "dbo" }),
    ));
    let names: Vec<String> = v["results"][0]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r[2].as_str().unwrap(), r[1].as_str().unwrap()))
        .collect();
    assert_eq!(
        names,
        ["procedure grava", "table cliente", "table pedido"],
        "{v}"
    );

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "dbo.cliente" }),
    ));
    assert_eq!(v["results"][0]["name"], "table");
    assert_eq!(v["results"][0]["rows"][0][0], "dbo.cliente");
    assert_eq!(v["results"][0]["rows"][0][2], "5");
    assert_eq!(v["results"][0]["rows"][0][4], "Clientes da loja");
    let referenced = &v["results"][4];
    assert_eq!(referenced["name"], "referenced_by");
    assert_eq!(referenced["rows"][0][0], "dbo.pedido");

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_table",
        json!({ "table": "[pedido]" }),
    ));
    let columns: Vec<String> = v["results"][1]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r[0].as_str().unwrap(), r[1].as_str().unwrap()))
        .collect();
    assert_eq!(columns, ["id int", "cliente_id int", "total decimal(10,2)"]);
    let text = v.to_string();
    assert!(text.contains("PRIMARY KEY (id)"), "{text}");
    assert!(
        text.contains("FOREIGN KEY (cliente_id) REFERENCES dbo.cliente (id)"),
        "{text}"
    );
    assert!(text.contains("CHECK ([total]>=(0))"), "{text}");
    assert!(text.contains("NONCLUSTERED (cliente_id)"), "{text}");

    let v = json_of(&call(
        &mut h,
        &c,
        "describe_function",
        json!({ "function": "dbo.grava" }),
    ));
    assert_eq!(v["results"][0]["rows"][0][1], "procedure");
    assert!(
        v["results"][0]["rows"][0][2]
            .as_str()
            .unwrap()
            .contains("INSERT INTO dbo.cliente")
    );

    // Leitura em T-SQL: roda sem perguntar e respeita o limite de linhas.
    let v = json_of(&call(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "select id, total from dbo.pedido order by id", "max_rows": 5 }),
    ));
    let r = &v["results"][0];
    assert_eq!(r["columns"], json!(["id", "total"]));
    assert_eq!(r["rows"].as_array().unwrap().len(), 5);
    assert_eq!(r["row_count"], 300);
    assert_eq!(r["truncated"], true);
    assert_eq!(
        count(
            &mut h,
            &c,
            "select top (1) nome from dbo.cliente order by id"
        ),
        "cliente 1"
    );

    // Erro de SQL volta como erro, e a próxima chamada funciona.
    let r = call(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "select * from dbo.nao_existe" }),
    );
    assert!(r.is_error && r.text.contains("nao_existe"), "{}", r.text);
    assert_eq!(count(&mut h, &c, "select count(*) from dbo.cliente"), "5");

    let output = h.state().output().join("\n");
    assert!(
        output.contains("Claude Code: run_query (read only)"),
        "{output}"
    );
}

#[test]
fn the_read_wrapper_undoes_whatever_runs_inside_it() {
    const DB: &str = "pp_claude_wrap";
    let Some((s, password)) = setup(DB) else {
        return;
    };
    let c = connector().connect(&s, &password).unwrap();
    // Se o classificador errasse e deixasse uma escrita passar como
    // leitura, a transação do embrulho desfaria.
    let batch = read_only_batch_for(
        Engine::SqlServer,
        "insert into dbo.cliente values (50, N'intruso'); select count(*) from dbo.cliente",
    );
    assert_eq!(run(&c.queries.open(DB), &batch), None);
    let batch = read_only_batch_for(Engine::SqlServer, "exec dbo.grava; select 1 / 0");
    assert!(run(&c.queries.open(DB), &batch).is_some());
    let check = c.queries.open(DB);
    let (tx, rx) = std::sync::mpsc::channel();
    check.execute(
        "select count(*), @@TRANCOUNT from dbo.cliente".into(),
        Box::new(move |e| drop(tx.send(e))),
    );
    let mut row = None;
    while let Ok(e) = rx.recv_timeout(Duration::from_secs(30)) {
        match e {
            QueryEvent::Row(r) => row = Some(r),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(row.unwrap(), [Some("5".to_string()), Some("0".to_string())]);
}

#[test]
fn changes_need_confirmation() {
    const DB: &str = "pp_claude_write";
    let Some((s, password)) = setup(DB) else {
        return;
    };
    let (mut h, c) = harness(s, password);

    let delete = json!({ "sql": "delete from dbo.pedido where id > 290" });
    let r = call_confirmed(&mut h, &c, "run_query", delete.clone(), false);
    assert!(r.is_error && r.text.contains("declined"), "{}", r.text);
    assert_eq!(count(&mut h, &c, "select count(*) from dbo.pedido"), "300");

    let r = call_confirmed(&mut h, &c, "run_query", delete, true);
    assert_eq!(json_of(&r)["rows_affected"], json!([10]));
    assert_eq!(count(&mut h, &c, "select count(*) from dbo.pedido"), "290");

    // Procedure e SELECT INTO também perguntam.
    for sql in ["exec dbo.grava", "select * into dbo.copia from dbo.cliente"] {
        let r = call_confirmed(&mut h, &c, "run_query", json!({ "sql": sql }), true);
        assert!(!r.is_error, "{sql}: {}", r.text);
    }
    assert_eq!(count(&mut h, &c, "select count(*) from dbo.copia"), "6");
}
