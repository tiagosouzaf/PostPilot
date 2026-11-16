//! O plano de execução, a atividade do servidor e a regra do ambiente nas
//! ferramentas do Claude, em servidores de verdade (com
//! `PGSTUDIO_TEST_HOST` para o PostgreSQL e `MSSQL_TEST_HOST` para o SQL
//! Server; sem eles, avisa e passa).

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_automation::{Client, Origin, Reply};
use pgstudio_core::connection::{ConnectionSettings, Connector, Engine, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::registered::{
    AiAccess, PRODUCTION, RegisteredServer, RegisteredServers, ServerGroup,
};
use serde_json::{Value, json};

fn pg_settings(database: &str) -> Option<(ConnectionSettings, String)> {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return None;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    Some((
        ConnectionSettings {
            engine: Default::default(),
            host,
            port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
            database: database.into(),
            user: var("PGSTUDIO_TEST_USER", "postgres"),
            ssl: SslMode::Disable,
            ssh: None,
        },
        var("PGSTUDIO_TEST_PASSWORD", ""),
    ))
}

fn mssql_settings(database: &str) -> Option<(ConnectionSettings, String)> {
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

fn connector(engine: Engine) -> Arc<dyn Connector> {
    let runtime = pgstudio_pg_client::DbRuntime::new().unwrap();
    match engine {
        Engine::PostgreSql => Arc::new(pgstudio_pg_metadata::PgConnector::new(runtime)),
        Engine::SqlServer => Arc::new(pgstudio_mssql::MssqlConnector::new(runtime)),
        Engine::Redis => unreachable!("este teste não roda no Redis"),
    }
}

/// Começa a rodar `sql` e devolve o canal dos eventos.
fn start(session: &Arc<dyn QuerySession>, sql: &str) -> std::sync::mpsc::Receiver<QueryEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    rx
}

/// Espera o fim de uma execução; devolve o erro, se houve.
fn finish(rx: &std::sync::mpsc::Receiver<QueryEvent>) -> Option<String> {
    let mut error = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => error = Some(e.message),
            QueryEvent::Finished { .. } => return error,
            _ => {}
        }
    }
}

fn run(session: &Arc<dyn QuerySession>, sql: &str) {
    if let Some(e) = finish(&start(session, sql)) {
        panic!("{sql}: {e}");
    }
}

/// Cria o banco de teste do PostgreSQL do zero.
fn pg_setup(db: &str) -> Option<(ConnectionSettings, String)> {
    let (admin, password) = pg_settings("postgres")?;
    let c = connector(Engine::PostgreSql)
        .connect(&admin, &password)
        .unwrap();
    let s = c.queries.open("postgres");
    run(&s, &format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
    run(&s, &format!("CREATE DATABASE {db}"));
    let t = c.queries.open(db);
    run(
        &t,
        "CREATE TABLE cliente (id int PRIMARY KEY, nome text NOT NULL);
         CREATE TABLE pedido (id int PRIMARY KEY, cliente_id int REFERENCES cliente,
             total numeric(10,2) DEFAULT 0);
         INSERT INTO cliente SELECT g, 'cliente ' || g FROM generate_series(1, 5) g;
         INSERT INTO pedido SELECT g, 1 + g % 5, g * 10 FROM generate_series(1, 3000) g;
         ANALYZE;",
    );
    pg_settings(db)
}

/// Cria o banco de teste do SQL Server do zero.
fn mssql_setup(db: &str) -> Option<(ConnectionSettings, String)> {
    let (admin, password) = mssql_settings("master")?;
    let c = connector(Engine::SqlServer)
        .connect(&admin, &password)
        .unwrap();
    let s = c.queries.open("master");
    run(
        &s,
        &format!(
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
             CREATE TABLE dbo.pedido (id int CONSTRAINT pk_pedido PRIMARY KEY, \
                 cliente_id int, total decimal(10,2) DEFAULT 0);\n\
             INSERT INTO dbo.cliente SELECT n, CONCAT(N'cliente ', n) FROM (SELECT TOP (5) \
                 ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n FROM sys.all_columns) x;\n\
             INSERT INTO dbo.pedido SELECT n, 1 + n % 5, n * 10 FROM (SELECT TOP (3000) \
                 ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n FROM sys.all_columns) x;\n\
             GO\n"
        ),
    );
    mssql_settings(db)
}

/// Os Registered Servers com `settings` numa pasta do ambiente
/// `environment`.
fn registered(settings: &ConnectionSettings, environment: &str) -> RegisteredServers {
    let mut r = RegisteredServers::default();
    let g = r
        .add_group(
            &[],
            ServerGroup {
                name: "Clientes".into(),
                environment: Some(environment.into()),
                ..Default::default()
            },
        )
        .unwrap();
    r.add_server(
        &g,
        RegisteredServer {
            name: "loja".into(),
            description: String::new(),
            environment: None,
            settings: settings.clone(),
        },
    )
    .unwrap();
    r
}

fn harness_with(
    settings: ConnectionSettings,
    password: String,
    servers: Option<RegisteredServers>,
) -> (Harness<'static, ShellApp>, Client) {
    let mut app = ShellApp::default().with_connector(connector(settings.engine));
    if let Some(servers) = servers {
        app = app.with_registered_servers(servers);
    }
    app.connect_to(settings, password);
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(20), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(3);
    let ctx = harness.ctx.clone();
    let client = harness.state_mut().automation(&ctx);
    (harness, client)
}

/// Chama a ferramenta e roda frames até a resposta; uma confirmação
/// inesperada é erro.
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
            start.elapsed() < Duration::from_secs(60),
            "{tool} never answered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// O que o diálogo de confirmação mostrou.
struct Asked {
    sql: String,
    reason: String,
    environment: Option<String>,
}

/// Chama uma ferramenta que pede confirmação, responde e devolve a
/// resposta e o que o diálogo mostrou.
fn call_confirmed(
    h: &mut Harness<'static, ShellApp>,
    c: &Client,
    tool: &str,
    args: Value,
    run: bool,
) -> (Reply, Asked) {
    let rx = c.send(tool, args, Origin::Mcp);
    let start = Instant::now();
    while h.state().claude_confirmation().is_none() {
        h.step();
        if let Ok(r) = rx.try_recv() {
            panic!("{tool} answered without asking: {}", r.text);
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no confirmation");
    }
    // O diálogo aparece desenhado antes da resposta.
    h.run_steps(2);
    let (sql, reason) = h.state().claude_confirmation().unwrap();
    let asked = Asked {
        sql: sql.to_string(),
        reason,
        environment: h
            .state()
            .claude_confirmation_environment()
            .map(str::to_string),
    };
    let ctx = h.ctx.clone();
    h.state_mut().answer_claude_confirmation(&ctx, run);
    loop {
        h.step();
        if let Ok(r) = rx.try_recv() {
            h.run_steps(2);
            return (r, asked);
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
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
fn postgres_plans() {
    const DB: &str = "pgstudio_claude_plan";
    let Some((s, password)) = pg_setup(DB) else {
        return;
    };
    let (mut h, c) = harness_with(s, password, None);

    // Estimado: nada roda, um plano por comando.
    let v = json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from pedido where total > 100; select count(*) from cliente" }),
    ));
    assert_eq!(v["engine"], "postgresql");
    assert_eq!(v["kind"], "estimated");
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 2, "{v}");
    assert_eq!(
        plans[0]["statement"],
        "select * from pedido where total > 100"
    );
    let plan = plans[0]["plan"].as_str().unwrap();
    assert!(plan.contains("Seq Scan on pedido"), "{plan}");
    assert!(plan.contains("Filter:"), "{plan}");
    assert!(!plan.contains("actual"), "{plan}");
    assert!(plans[1]["plan"].as_str().unwrap().contains("Aggregate"));

    // O estimado de um comando que altera não pede nada nem altera.
    let v = json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "delete from pedido" }),
    ));
    assert!(
        v["plans"][0]["plan"]
            .as_str()
            .unwrap()
            .contains("Delete on pedido")
    );
    assert_eq!(count(&mut h, &c, "select count(*) from pedido"), "3000");

    // Real de uma leitura: roda na hora, com os números medidos.
    let v = json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from pedido where total > 100", "analyze": true }),
    ));
    assert_eq!(v["kind"], "actual");
    assert_eq!(v["rolled_back"], true);
    let plan = v["plans"][0]["plan"].as_str().unwrap();
    assert!(plan.contains("actual time="), "{plan}");
    assert!(plan.contains("Execution Time"), "{plan}");
    assert!(plan.contains("Buffers:"), "{plan}");

    // Real de um comando que altera: pede confirmação, como rodar.
    let delete = json!({ "sql": "delete from pedido where id > 2000", "analyze": true });
    let (r, asked) = call_confirmed(&mut h, &c, "explain_query", delete.clone(), false);
    assert!(r.is_error && r.text.contains("declined"), "{}", r.text);
    assert_eq!(asked.sql, "delete from pedido where id > 2000");
    assert!(asked.reason.contains("DELETE"), "{}", asked.reason);
    assert!(asked.reason.contains("rolls it back"), "{}", asked.reason);
    assert_eq!(asked.environment, None);
    // Confirmado, executa, mede e desfaz.
    let (r, _) = call_confirmed(&mut h, &c, "explain_query", delete, true);
    let v = json_of(&r);
    let plan = v["plans"][0]["plan"].as_str().unwrap();
    assert!(plan.contains("Delete on pedido"), "{plan}");
    assert!(plan.contains("rows=1000"), "{plan}");
    assert_eq!(count(&mut h, &c, "select count(*) from pedido"), "3000");

    // Uma função que grava, dentro de um SELECT, também pede.
    let (r, _) = call_confirmed(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select set_config('a.b', 'c', false)", "analyze": true }),
        false,
    );
    assert!(r.is_error);

    // Erro do servidor volta como erro; sem comando, também.
    let r = call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from nao_existe" }),
    );
    assert!(r.is_error && r.text.contains("nao_existe"), "{}", r.text);
    let r = call(&mut h, &c, "explain_query", json!({ "sql": "-- nada" }));
    assert!(r.is_error, "{}", r.text);
}

#[test]
fn postgres_activity_says_who_blocks_whom() {
    const DB: &str = "pgstudio_claude_activity";
    let Some((s, password)) = pg_setup(DB) else {
        return;
    };
    let conn = connector(Engine::PostgreSql)
        .connect(&s, &password)
        .unwrap();
    // Uma sessão segura a linha numa transação aberta; a outra espera.
    let holder = conn.queries.open(DB);
    run(
        &holder,
        "BEGIN; UPDATE cliente SET nome = 'segura' WHERE id = 1",
    );
    let waiter = conn.queries.open(DB);
    let waiting = start(&waiter, "UPDATE cliente SET nome = 'espera' WHERE id = 1");

    let (mut h, c) = harness_with(s, password, None);
    let begin = Instant::now();
    let v = loop {
        let v = json_of(&call(&mut h, &c, "get_activity", json!({})));
        if v["summary"]["blocked"].as_u64() == Some(1) {
            break v;
        }
        assert!(
            begin.elapsed() < Duration::from_secs(20),
            "never blocked: {v}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let chain = v["blocking"].as_array().unwrap();
    assert_eq!(chain.len(), 2, "{v}");
    assert_eq!(chain[0]["role"], "head blocker");
    assert_eq!(chain[0]["state"], "idle in transaction");
    assert!(chain[0]["query"].as_str().unwrap().contains("'segura'"));
    assert_eq!(chain[1]["role"], "blocked");
    assert_eq!(chain[1]["blocked_by"], json!([chain[0]["pid"]]));
    assert!(chain[1]["query"].as_str().unwrap().contains("'espera'"));
    assert!(chain[1]["wait"].as_str().unwrap().starts_with("Lock"));
    assert!(v["summary"]["locks_waiting"].as_u64().unwrap() >= 1);
    let locks = v["locks"].as_array().unwrap();
    assert!(
        locks
            .iter()
            .any(|l| l["granted"] == false && l["pid"] == chain[1]["pid"]),
        "{v}"
    );
    assert!(v["idle_sessions_not_listed"].is_number());
    // Sem pg_stat_statements, diz que não tem em vez de falhar.
    let v = json_of(&call(
        &mut h,
        &c,
        "get_activity",
        json!({ "top_statements": true, "include_idle": true }),
    ));
    assert!(
        v.get("top_statements").is_some() || v.get("top_statements_unavailable").is_some(),
        "{v}"
    );

    run(&conn.queries.open(DB), "SELECT 1");
    holder.execute("ROLLBACK".into(), Box::new(|_| {}));
    assert_eq!(finish(&waiting), None);
}

#[test]
fn production_asks_before_the_ai_reads_and_can_be_blocked() {
    const DB: &str = "pgstudio_claude_env";
    let Some((s, password)) = pg_setup(DB) else {
        return;
    };
    let servers = registered(&s, PRODUCTION);
    // De início, Production pede confirmação de tudo o que roda.
    assert_eq!(
        servers.environment(PRODUCTION).unwrap().ai_access(),
        AiAccess::Confirm
    );
    let (mut h, c) = harness_with(s.clone(), password.clone(), Some(servers.clone()));

    let v = json_of(&call(&mut h, &c, "list_servers", json!({})));
    assert_eq!(v["servers"][0]["environment"], "Production");
    assert_eq!(v["servers"][0]["ai_access"], "confirm");

    // Esquema e plano estimado continuam livres.
    let r = call(&mut h, &c, "describe_table", json!({ "table": "pedido" }));
    assert!(!r.is_error && r.text.contains("cliente_id"), "{}", r.text);
    json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from pedido" }),
    ));

    // Uma leitura pede confirmação, com o ambiente no diálogo, e depois
    // roda no embrulho de leitura.
    let select = json!({ "sql": "select count(*) from pedido" });
    let (r, asked) = call_confirmed(&mut h, &c, "run_query", select.clone(), false);
    assert!(r.is_error && r.text.contains("declined"), "{}", r.text);
    assert_eq!(asked.environment.as_deref(), Some("Production"));
    assert!(asked.reason.contains("only reads"), "{}", asked.reason);
    assert!(asked.reason.contains("Production"), "{}", asked.reason);
    let (r, _) = call_confirmed(&mut h, &c, "run_query", select, true);
    assert_eq!(json_of(&r)["results"][0]["rows"][0][0], "3000");
    // Uma função que grava é barrada pela transação READ ONLY mesmo depois
    // do "Run".
    let (r, asked) = call_confirmed(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "delete from pedido where id = 1" }),
        false,
    );
    assert!(r.is_error);
    assert_eq!(asked.environment.as_deref(), Some("Production"));
    assert!(asked.reason.contains("DELETE"), "{}", asked.reason);

    // Plano real e atividade também pedem.
    let (r, asked) = call_confirmed(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from pedido", "analyze": true }),
        true,
    );
    assert_eq!(json_of(&r)["kind"], "actual");
    assert_eq!(asked.environment.as_deref(), Some("Production"));
    let (r, asked) = call_confirmed(&mut h, &c, "get_activity", json!({}), true);
    assert!(json_of(&r)["summary"]["sessions"].is_number());
    assert_eq!(asked.environment.as_deref(), Some("Production"));
    let (r, _) = call_confirmed(&mut h, &c, "get_activity", json!({}), false);
    assert!(r.is_error);

    // Executar a janela: uma confirmação só (a do Claude, com o ambiente),
    // sem a segunda pergunta do ambiente.
    let v = json_of(&call(
        &mut h,
        &c,
        "open_query_window",
        json!({ "text": "update cliente set nome = 'novo' where id = 2" }),
    ));
    let window = v["window"].clone();
    let (r, asked) = call_confirmed(
        &mut h,
        &c,
        "execute_query_window",
        json!({ "window": window }),
        true,
    );
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(asked.environment.as_deref(), Some("Production"));
    assert!(!h.state().is_confirming_environment());
    let (r, _) = call_confirmed(
        &mut h,
        &c,
        "run_query",
        json!({ "sql": "select nome from cliente where id = 2" }),
        true,
    );
    assert_eq!(json_of(&r)["results"][0]["rows"][0][0], "novo");
    // Um provedor por chave de API passa pela mesma pergunta.
    let rx = c.send("run_query", json!({ "sql": "select 1" }), Origin::Assistant);
    let start = Instant::now();
    while h.state().claude_confirmation().is_none() {
        h.step();
        assert!(rx.try_recv().is_err(), "answered without asking");
        assert!(start.elapsed() < Duration::from_secs(10), "no confirmation");
    }
    assert_eq!(
        h.state().claude_confirmation_environment(),
        Some("Production")
    );
    let ctx = h.ctx.clone();
    h.state_mut().answer_claude_confirmation(&ctx, false);
    drop(h);

    // Bloqueado: nenhuma ferramenta chega ao servidor.
    let mut blocked = servers;
    let mut list = blocked.environments();
    let renamed: Vec<Option<String>> = list.iter().map(|e| Some(e.name.clone())).collect();
    for e in &mut list {
        if e.name == PRODUCTION {
            e.ai = Some(AiAccess::Blocked);
        }
    }
    blocked.set_environments(list, &renamed).unwrap();
    let (mut h, c) = harness_with(s, password, Some(blocked));
    let v = json_of(&call(&mut h, &c, "list_servers", json!({})));
    assert_eq!(v["servers"][0]["ai_access"], "blocked");
    for (tool, args) in [
        ("list_databases", json!({})),
        ("list_objects", json!({})),
        ("describe_table", json!({ "table": "pedido" })),
        ("run_query", json!({ "sql": "select 1" })),
        ("run_query", json!({ "sql": "delete from pedido" })),
        ("explain_query", json!({ "sql": "select 1" })),
        (
            "explain_query",
            json!({ "sql": "select 1", "analyze": true }),
        ),
        ("get_activity", json!({})),
    ] {
        let r = call(&mut h, &c, tool, args);
        assert!(r.is_error, "{tool}: {}", r.text);
        assert!(r.text.contains("blocked AI access"), "{tool}: {}", r.text);
        assert!(r.text.contains("Production"), "{tool}: {}", r.text);
    }
    // Vale igual para o painel e para os provedores por chave de API.
    for origin in [Origin::Panel, Origin::Assistant] {
        let rx = c.send("run_query", json!({ "sql": "select 1" }), origin);
        let start = Instant::now();
        let r = loop {
            h.step();
            if let Ok(r) = rx.try_recv() {
                break r;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
        };
        assert!(
            r.is_error && r.text.contains("blocked AI access"),
            "{}",
            r.text
        );
    }
    // Escrever no editor continua valendo; executar, não.
    let v = json_of(&call(
        &mut h,
        &c,
        "open_query_window",
        json!({ "text": "select 1" }),
    ));
    let r = call(
        &mut h,
        &c,
        "execute_query_window",
        json!({ "window": v["window"] }),
    );
    assert!(
        r.is_error && r.text.contains("blocked AI access"),
        "{}",
        r.text
    );
    let r = call(
        &mut h,
        &c,
        "get_query_results",
        json!({ "window": v["window"] }),
    );
    assert!(
        r.is_error && r.text.contains("blocked AI access"),
        "{}",
        r.text
    );
}

#[test]
fn sql_server_plans_and_activity() {
    const DB: &str = "postpilot_claude_plan";
    let Some((s, password)) = mssql_setup(DB) else {
        return;
    };
    let (mut h, c) = harness_with(s.clone(), password.clone(), None);

    // Estimado: nada roda.
    let v = json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select c.nome, sum(p.total) from dbo.pedido p join dbo.cliente c on c.id = p.cliente_id where p.total > 100 group by c.nome" }),
    ));
    assert_eq!(v["engine"], "sqlserver");
    assert_eq!(v["kind"], "estimated");
    let plan = v["plans"][0]["plan"].as_str().unwrap();
    assert!(plan.contains("pedido"), "{plan}");
    assert!(plan.contains("est_rows="), "{plan}");
    assert!(plan.contains("Predicate:"), "{plan}");
    assert!(!plan.contains("actual rows"), "{plan}");
    assert!(plan.lines().count() >= 3, "{plan}");
    let v = json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "delete from dbo.pedido" }),
    ));
    assert!(v["plans"][0]["plan"].as_str().unwrap().contains("Delete"));
    assert_eq!(count(&mut h, &c, "select count(*) from dbo.pedido"), "3000");

    // Real de uma leitura: roda na hora e traz os números medidos.
    let v = json_of(&call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from dbo.pedido where total > 100", "analyze": true }),
    ));
    assert_eq!(v["kind"], "actual");
    let plan = v["plans"][0]["plan"].as_str().unwrap();
    assert!(plan.contains("actual rows=2990"), "{plan}");
    assert_eq!(v["result_rows"], json!([2990]));

    // Real de um comando que altera: confirma, executa e desfaz.
    let delete = json!({ "sql": "delete from dbo.pedido where id > 2000", "analyze": true });
    let (r, asked) = call_confirmed(&mut h, &c, "explain_query", delete.clone(), false);
    assert!(r.is_error && r.text.contains("declined"), "{}", r.text);
    assert!(asked.reason.contains("DELETE"), "{}", asked.reason);
    let (r, _) = call_confirmed(&mut h, &c, "explain_query", delete, true);
    let v = json_of(&r);
    let plan = v["plans"][0]["plan"].as_str().unwrap();
    assert!(plan.contains("Delete"), "{plan}");
    assert!(plan.contains("actual rows=1000"), "{plan}");
    assert_eq!(count(&mut h, &c, "select count(*) from dbo.pedido"), "3000");

    let r = call(
        &mut h,
        &c,
        "explain_query",
        json!({ "sql": "select * from dbo.nao_existe" }),
    );
    assert!(r.is_error && r.text.contains("nao_existe"), "{}", r.text);

    // Atividade: uma sessão segura a linha, a outra espera.
    let conn = connector(Engine::SqlServer).connect(&s, &password).unwrap();
    let holder = conn.queries.open(DB);
    run(
        &holder,
        "BEGIN TRANSACTION; UPDATE dbo.cliente SET nome = N'segura' WHERE id = 1",
    );
    let waiter = conn.queries.open(DB);
    let waiting = start(
        &waiter,
        "UPDATE dbo.cliente SET nome = N'espera' WHERE id = 1",
    );
    let begin = Instant::now();
    let v = loop {
        let v = json_of(&call(
            &mut h,
            &c,
            "get_activity",
            json!({ "top_statements": true }),
        ));
        if v["summary"]["blocked"].as_u64() == Some(1) {
            break v;
        }
        assert!(
            begin.elapsed() < Duration::from_secs(20),
            "never blocked: {v}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let chain = v["blocking"].as_array().unwrap();
    assert_eq!(chain.len(), 2, "{v}");
    assert_eq!(chain[0]["role"], "head blocker");
    assert!(
        chain[0]["query"].as_str().unwrap().contains("segura"),
        "{v}"
    );
    assert_eq!(chain[1]["blocked_by"], json!([chain[0]["pid"]]));
    assert!(
        chain[1]["query"].as_str().unwrap().contains("cliente"),
        "{v}"
    );
    assert!(v["top_statements"].is_array(), "{v}");
    assert!(
        v["locks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["granted"] == false),
        "{v}"
    );
    holder.execute("ROLLBACK TRANSACTION".into(), Box::new(|_| {}));
    assert_eq!(finish(&waiting), None);
}
