//! Consulta entre servidores: a janela, o join com servidores de mentira,
//! a recusa de escrita, a sessão e, com os servidores de verdade
//! (`FEDERATED_TEST=1`, com os dados de `scripts/federated-seed.sh`), o
//! join entre PostgreSQL, SQL Server, MongoDB, Elasticsearch e Redis.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, Engine, EngineConnector, SslMode,
};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::execution::{Execution, Outcome};
use pgstudio_ui_shell::registered::{PRODUCTION, RegisteredServer, RegisteredServers};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};
use pgstudio_ui_shell::session::SessionStore;

/// O que cada servidor recebeu: (host, banco, texto).
type Log = Arc<Mutex<Vec<(String, String, String)>>>;

struct FakeConnector(Log);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if password != "secret" {
            return Err(ConnectError::Authentication("Login failed.".into()));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries(self.0.clone(), s.host.clone())),
        })
    }
}

struct FakeQueries(Log, String);

impl QueryService for FakeQueries {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession(
            self.0.clone(),
            self.1.clone(),
            database.to_string(),
        ))
    }
}

struct FakeSession(Log, String, String);

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let host = self.1.clone();
        self.0
            .lock()
            .unwrap()
            .push((host.clone(), self.2.clone(), sql.clone()));
        std::thread::spawn(move || {
            let grid: Option<(&[&str], Vec<Vec<&str>>)> = if sql.contains("clientes") {
                Some((
                    &["id", "nome"],
                    vec![vec!["1", "Ana"], vec!["2", "Bruno"], vec!["3", "Carla"]],
                ))
            } else if sql.contains("pedidos") {
                Some((
                    &["cliente_id", "total"],
                    vec![vec!["1", "10.50"], vec!["1", "4.50"], vec!["2", "100.00"]],
                ))
            } else {
                None
            };
            match grid {
                Some((columns, rows)) => {
                    sink(QueryEvent::Columns(
                        columns.iter().map(|c| c.to_string()).collect(),
                    ));
                    let n = rows.len() as u64;
                    for row in rows {
                        sink(QueryEvent::Row(
                            row.into_iter().map(|c| Some(c.to_string())).collect(),
                        ));
                    }
                    sink(QueryEvent::CommandComplete { rows: n });
                }
                None => sink(QueryEvent::Error(ServerMessage::error(
                    "relation does not exist",
                ))),
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.2.clone()
    }
}

fn settings(host: &str, engine: Engine) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: "alice".into(),
        database: "erp".into(),
        ..ConnectionSettings::for_engine(engine)
    }
}

/// "Vendas PG" (Produção) e "financeiro" (SQL Server), com senha guardada;
/// "semsenha" sem.
fn app(log: &Log) -> ShellApp {
    let store = Arc::new(MemoryStore::default());
    let mut r = RegisteredServers::default();
    for (name, host, engine, password) in [
        ("Vendas PG", "pg-db", Engine::PostgreSql, true),
        ("financeiro", "ms-db", Engine::SqlServer, true),
        ("semsenha", "x-db", Engine::PostgreSql, false),
    ] {
        let s = settings(host, engine);
        if password {
            store.set(&password_key(&s), "secret").unwrap();
        }
        r.add_server(
            &[],
            RegisteredServer {
                name: name.into(),
                description: String::new(),
                environment: (name == "Vendas PG").then(|| PRODUCTION.into()),
                settings: s,
            },
        )
        .unwrap();
    }
    ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector(log.clone())))
        .with_secret_store(store)
        .with_registered_servers(r)
}

fn start(app: ShellApp) -> Harness<'static, ShellApp> {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    if harness.state().connect_dialog().is_some() {
        harness.get_by_label("Cancel").click();
        harness.run();
    }
    harness
}

/// Abre uma janela entre servidores com `sql`.
fn cross_query(harness: &mut Harness<'static, ShellApp>, sql: &str) -> u32 {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    harness.state_mut().execute(&ctx, CommandId::CrossServer);
    harness.run();
    let id = harness.state().active_document().unwrap();
    assert!(harness.state().is_federated(id));
    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .text = sql.into();
    id
}

fn run(harness: &mut Harness<'static, ShellApp>, id: u32, seconds: u64) {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    let start = Instant::now();
    loop {
        harness.step();
        let doc = harness.state().documents().get(id).unwrap();
        if doc.execution.as_ref().is_some_and(|e| !e.is_running()) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(seconds),
            "query never finished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
}

fn execution<'a>(harness: &'a Harness<'static, ShellApp>, id: u32) -> &'a Execution {
    let doc = harness.state().documents().get(id).unwrap();
    doc.execution.as_ref().unwrap()
}

fn rows(exec: &Execution, i: usize) -> Vec<Vec<String>> {
    let set = &exec.results[i];
    (0..set.len())
        .map(|r| {
            set.row(r)
                .into_iter()
                .map(|c| c.unwrap_or_else(|| "NULL".into()))
                .collect()
        })
        .collect()
}

fn messages(exec: &Execution) -> String {
    format!("{:?}", exec.messages)
}

#[test]
fn the_command_turns_a_window_into_a_cross_server_query_with_an_example() {
    let log = Log::default();
    let mut harness = start(app(&log));
    let ctx = harness.ctx.clone();
    // Sem janela aberta, o comando abre uma.
    harness.state_mut().execute(&ctx, CommandId::CrossServer);
    harness.run();
    let id = harness.state().active_document().unwrap();
    assert!(harness.state().is_federated(id));
    let text = harness.state().documents().get(id).unwrap().text.clone();
    assert!(
        text.contains("\"Vendas PG\", financeiro, semsenha"),
        "{text}"
    );
    assert!(text.contains("FROM \"Vendas PG\"."), "{text}");
    // De novo, a janela volta a ser comum (o texto fica).
    harness.state_mut().execute(&ctx, CommandId::CrossServer);
    harness.run();
    assert!(!harness.state().is_federated(id));
}

#[test]
fn joins_two_saved_servers_without_connecting_in_the_object_explorer() {
    let log = Log::default();
    let mut harness = start(app(&log));
    let id = cross_query(
        &mut harness,
        "select c.nome, sum(p.total) as total\n\
         from vendas_pg.public.clientes c\n\
         join financeiro.erp2.dbo.pedidos p on p.cliente_id = c.id\n\
         group by c.nome order by total desc",
    );
    run(&mut harness, id, 10);
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Succeeded, "{}", messages(exec));
    assert_eq!(exec.results[0].columns(), ["nome", "total"]);
    assert_eq!(rows(exec, 0), vec![vec!["Bruno", "100"], vec!["Ana", "15"]]);
    // Cada servidor recebeu a leitura dele, no banco pedido. O servidor de
    // Produção não pede confirmação: a consulta só lê.
    let mut log = log.lock().unwrap().clone();
    log.sort();
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!(log[0].0, "ms-db");
    assert_eq!(log[0].1, "erp2");
    assert_eq!(log[0].2, "SELECT * FROM dbo.pedidos");
    assert_eq!(log[1].0, "pg-db");
    assert_eq!(log[1].1, "erp");
    assert!(log[1].2.contains("READ ONLY"), "{}", log[1].2);
    assert!(log[1].2.contains("SELECT * FROM public.clientes"));
}

#[test]
fn a_query_that_writes_never_reaches_a_server() {
    let log = Log::default();
    let mut harness = start(app(&log));
    for sql in [
        "select * from financeiro($$ delete from dbo.pedidos $$) x",
        "select * from vendas_pg($$ update clientes set nome = 'x' returning * $$) x",
        "delete from vendas_pg.public.clientes",
    ] {
        let id = cross_query(&mut harness, sql);
        run(&mut harness, id, 10);
        let exec = execution(&harness, id);
        assert_eq!(exec.outcome(), Outcome::Failed, "{sql}");
        assert!(messages(exec).contains("only reads"), "{}", messages(exec));
    }
    let log = log.lock().unwrap();
    assert!(
        log.iter()
            .all(|(_, _, sql)| !sql.to_lowercase().contains("delete")
                && !sql.to_lowercase().contains("update")),
        "{log:?}"
    );
}

#[test]
fn a_server_without_a_saved_password_fails_with_its_name() {
    let log = Log::default();
    let mut harness = start(app(&log));
    let id = cross_query(
        &mut harness,
        "select * from semsenha.public.clientes a join vendas_pg.public.clientes b on a.id = b.id",
    );
    run(&mut harness, id, 10);
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Failed);
    let text = messages(exec);
    assert!(text.contains("semsenha"), "{text}");
    assert!(text.contains("No saved password"), "{text}");
}

#[test]
fn the_window_is_still_cross_server_after_reopening_the_app() {
    let dir = std::env::temp_dir().join(format!("postpilot-fed-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::default();
    let sql = "select count(*) as n from vendas_pg.public.clientes";
    {
        let app = app(&log).with_session(SessionStore::open(dir.join("session.json")));
        let mut harness = start(app);
        cross_query(&mut harness, sql);
        harness.state_mut().save_session();
    }
    let app = app(&log).with_session(SessionStore::open(dir.join("session.json")));
    let mut harness = start(app);
    let id = harness.state().active_document().unwrap();
    assert!(harness.state().is_federated(id));
    assert_eq!(harness.state().documents().get(id).unwrap().text, sql);
    run(&mut harness, id, 10);
    assert_eq!(rows(execution(&harness, id), 0), vec![vec!["3"]]);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Servidores de verdade ---

/// Roda quadros até `done` valer.
fn wait(harness: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    loop {
        harness.step();
        if done(harness.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_cited_server_without_an_open_connection_loads_its_tables_in_the_background() {
    use pgstudio_ui_shell::federated::Load;
    let log = Log::default();
    let mut harness = start(app(&log));
    cross_query(&mut harness, "SELECT * FROM \"Vendas PG\".");
    let tables = |app: &ShellApp, server: &str| {
        app.federated_catalog()
            .schema(server)
            .map_or(0, |s| s.relations.len())
    };
    // Os servidores aparecem na hora; as tabelas, quando a leitura acaba.
    harness.step();
    assert!(
        harness
            .state()
            .federated_catalog()
            .schema("financeiro")
            .is_some()
    );
    wait(&mut harness, "tables never arrived", |app| {
        tables(app, "\"Vendas PG\"") > 0
    });
    assert_eq!(harness.state().federated_load("Vendas PG"), None);
    let rel = harness
        .state()
        .federated_catalog()
        .relation(Some("\"Vendas PG\""), "table_00000")
        .expect("table of the fake catalog");
    assert!(rel.columns.iter().any(|c| c.name == "email"));
    assert!(
        harness.state().status().contains("\"Vendas PG\""),
        "{}",
        harness.state().status()
    );
    // Quem a consulta não cita fica como estava: sem conexão.
    assert_eq!(tables(harness.state(), "financeiro"), 0);
    assert_eq!(harness.state().federated_load("financeiro"), None);

    // Sem senha guardada, a falha fica dita em vez de silêncio.
    let id = harness.state().active_document().unwrap();
    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .text = "SELECT * FROM \"Vendas PG\".table_00000 a JOIN semsenha.".into();
    wait(&mut harness, "failure never arrived", |app| {
        matches!(app.federated_load("semsenha"), Some(Load::Failed(_)))
    });
    assert!(
        harness.state().status().contains("semsenha"),
        "{}",
        harness.state().status()
    );
}

fn var(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

/// PostgreSQL, SQL Server, MongoDB, Elasticsearch e Redis de verdade, com
/// os dados de `scripts/federated-seed.sh`.
fn real_app() -> Option<ShellApp> {
    if std::env::var("FEDERATED_TEST").is_err() {
        eprintln!("FEDERATED_TEST not set; skipping");
        return None;
    }
    let server = |engine: Engine, prefix: &str, port: &str, user: &str, database: &str| {
        let mut s = ConnectionSettings::for_engine(engine);
        s.host = var(&format!("{prefix}_TEST_HOST"), "127.0.0.1");
        s.port = var(&format!("{prefix}_TEST_PORT"), port).parse().unwrap();
        s.user = var(&format!("{prefix}_TEST_USER"), user);
        if !database.is_empty() {
            s.database = database.into();
        }
        s.ssl = SslMode::Disable;
        (s, var(&format!("{prefix}_TEST_PASSWORD"), ""))
    };
    let mut mssql = server(Engine::SqlServer, "MSSQL", "14330", "sa", "erp");
    // O contêiner do SQL Server só aceita conexão cifrada.
    mssql.0.ssl = ConnectionSettings::for_engine(Engine::SqlServer).ssl;
    let servers = [
        (
            "PG Vendas",
            server(Engine::PostgreSql, "PGSTUDIO", "54329", "postgres", "loja"),
        ),
        ("financeiro", mssql),
        (
            "mongo",
            server(Engine::MongoDb, "MONGODB", "27117", "root", "app"),
        ),
        (
            "busca",
            server(Engine::Elasticsearch, "ELASTICSEARCH", "19200", "", ""),
        ),
        ("cache", server(Engine::Redis, "REDIS", "16379", "", "")),
    ];
    let store = Arc::new(MemoryStore::default());
    let mut r = RegisteredServers::default();
    for (name, (s, password)) in &servers {
        store.set(&password_key(s), password).unwrap();
        r.add_server(
            &[],
            RegisteredServer {
                name: (*name).into(),
                description: String::new(),
                environment: None,
                settings: s.clone(),
            },
        )
        .unwrap();
    }
    let runtime = pgstudio_pg_metadata::Runtime::new().unwrap();
    let connector = EngineConnector::default()
        .with(
            Engine::PostgreSql,
            Arc::new(pgstudio_pg_metadata::PgConnector::new(runtime.clone())),
        )
        .with(
            Engine::SqlServer,
            Arc::new(pgstudio_mssql::MssqlConnector::new(runtime.clone())),
        )
        .with(
            Engine::Redis,
            Arc::new(pgstudio_redis::RedisConnector::new(runtime.clone())),
        )
        .with(
            Engine::MongoDb,
            Arc::new(pgstudio_mongodb::MongoConnector::new(runtime)),
        )
        .with(
            Engine::Elasticsearch,
            Arc::new(pgstudio_elasticsearch::EsConnector::new()),
        );
    Some(
        ShellApp::default()
            .with_recent_connections(RecentConnections::in_memory(Vec::new()))
            .with_connector(Arc::new(connector))
            .with_secret_store(store)
            .with_registered_servers(r),
    )
}

/// Como terminou, as colunas, as linhas e as mensagens.
type Answer = (Outcome, Vec<String>, Vec<Vec<String>>, String);

fn real(sql: &str) -> Option<Answer> {
    let mut harness = start(real_app()?);
    let id = cross_query(&mut harness, sql);
    run(&mut harness, id, 120);
    let exec = execution(&harness, id);
    let (columns, data) = match exec.results.first() {
        Some(set) => (set.columns(), rows(exec, 0)),
        None => (Vec::new(), Vec::new()),
    };
    Some((exec.outcome(), columns, data, messages(exec)))
}

#[test]
fn real_postgresql_joins_sql_server() {
    let Some((outcome, columns, data, messages)) = real(
        "select c.nome, c.cpf, count(p.id) as pedidos, sum(p.total) as total\n\
         from pg_vendas.loja.public.clientes c\n\
         left join financeiro.erp.dbo.pedidos p on p.cliente_id = c.id\n\
         group by c.id order by c.id",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(columns, ["nome", "cpf", "pedidos", "total"]);
    assert_eq!(
        data,
        vec![
            vec!["Ana Souza", "01234567890", "2", "200"],
            vec!["Bruno Lima", "98765432100", "1", "1000"],
            vec!["Carla Dias", "11122233344", "0", "NULL"],
            vec!["Diego Alves", "NULL", "0", "NULL"],
        ]
    );
}

#[test]
fn real_filters_run_on_each_server() {
    let Some((outcome, _, data, messages)) = real(
        "select c.nome, p.total\n\
         from pg_vendas($$ select id, nome from clientes where ativo and cidade like 'S%' $$) c\n\
         join financeiro($$ select cliente_id, total from dbo.pedidos where pago = 1 $$) p\n\
           on p.cliente_id = c.id",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(data, vec![vec!["Ana Souza", "150.5"]]);
}

#[test]
fn real_sql_joins_mongodb_elasticsearch_and_redis() {
    let Some((outcome, columns, data, messages)) = real(
        "select c.nome,\n\
                (select count(*) from mongo.app.sessoes s where s.usuario = c.id) as sessoes,\n\
                (select max(a.nota) from busca.avaliacoes a where a.cliente_id = c.id) as melhor_nota,\n\
                json_extract(r.value, '$.pontos') as pontos,\n\
                k.value as carrinho\n\
         from pg_vendas.clientes c\n\
         left join cache.\"usuario:*\" r on r.key = 'usuario:' || c.id\n\
         left join cache.\"carrinho:*\" k on k.key = 'carrinho:' || c.id\n\
         where c.id <= 3 order by c.id",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(
        columns,
        ["nome", "sessoes", "melhor_nota", "pontos", "carrinho"]
    );
    assert_eq!(
        data,
        vec![
            vec!["Ana Souza", "1", "5", "120", "3"],
            vec!["Bruno Lima", "2", "3", "15", "0"],
            vec!["Carla Dias", "0", "NULL", "NULL", "NULL"],
        ]
    );
}

#[test]
fn real_native_queries_of_the_nosql_engines() {
    let Some((outcome, _, data, messages)) = real(
        "select s.dispositivo, json_extract(s.perfil, '$.plano') as plano, a.texto, h.value as nome\n\
         from mongo($$ db.sessoes.find({usuario: 1}) $$) s\n\
         join busca($$ GET /avaliacoes/_search\n{\"query\": {\"term\": {\"nota\": 5}}} $$) a\n\
           on a.cliente_id = s.usuario\n\
         join cache($$ HGETALL usuario:1 $$) h on h.field = 'nome'",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(data, vec![vec!["celular", "pro", "ótimo", "Ana"]]);
}

#[test]
fn real_big_table_comes_whole() {
    let Some((outcome, _, data, messages)) = real(
        "select count(*) as n, sum(g.id) as soma, count(c.id) as com_cliente\n\
         from pg_vendas.grande g left join pg_vendas.clientes c on c.id = g.id",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(data, vec![vec!["200000", "20000100000", "4"]]);
}

/// Captura de tela com os servidores de verdade, em português. Roda só a
/// pedido, porque troca o idioma do processo:
/// `FEDERATED_TEST=1 PGSTUDIO_SHOTS=pasta cargo test -p pgstudio-ui-shell
/// --features screenshots --test federated -- --ignored screenshot`.
#[cfg(feature = "screenshots")]
#[test]
#[ignore]
fn screenshot_of_a_cross_server_query() {
    let Some(app) = real_app() else { return };
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut h = Harness::builder()
        .with_size([1280.0, 860.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run_steps(3);
    if h.state().connect_dialog().is_some() {
        h.get_by_label("Cancelar").click();
        h.run_steps(3);
    }
    let save = |h: &mut Harness<'static, ShellApp>, name: &str| {
        let out = std::env::var_os("PGSTUDIO_SHOTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&out).unwrap();
        h.render().expect("render").save(out.join(name)).unwrap();
    };
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::CrossServer);
    h.run_steps(3);
    save(&mut h, "1-janela-nova.png");

    let id = h.state().active_document().unwrap();
    h.state_mut().documents_mut().get_mut(id).unwrap().text = "\
-- Clientes do PostgreSQL com os pedidos do SQL Server, as sessões do MongoDB,
-- as avaliações do Elasticsearch e os pontos guardados no Redis.
SELECT c.nome,
       c.cidade,
       count(p.id)                          AS pedidos,
       sum(p.total)                         AS total,
       (SELECT count(*) FROM mongo.app.sessoes s WHERE s.usuario = c.id)      AS sessoes,
       (SELECT max(a.nota) FROM busca.avaliacoes a WHERE a.cliente_id = c.id) AS melhor_nota,
       json_extract(r.value, '$.pontos')    AS pontos
FROM \"PG Vendas\".loja.public.clientes c
LEFT JOIN financeiro($$ SELECT id, cliente_id, total FROM dbo.pedidos WHERE pago = 1 $$) p
       ON p.cliente_id = c.id
LEFT JOIN cache.\"usuario:*\" r ON r.key = 'usuario:' || c.id
GROUP BY c.id
ORDER BY total DESC;
"
    .into();
    h.state_mut().execute(&ctx, CommandId::Execute);
    let start = Instant::now();
    loop {
        h.step();
        let doc = h.state().documents().get(id).unwrap();
        if doc.execution.as_ref().is_some_and(|e| !e.is_running()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(5);
    save(&mut h, "2-join-cinco-servidores.png");
    h.get_by_label_contains("Mensagens").click();
    h.run_steps(3);
    save(&mut h, "3-mensagens.png");
}
