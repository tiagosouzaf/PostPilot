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
    // Cada servidor recebeu a pergunta do tamanho da tabela e a leitura
    // dele, no banco pedido e só com as colunas usadas. O servidor de
    // Produção não pede confirmação: a consulta só lê.
    let mut log = log.lock().unwrap().clone();
    log.sort();
    assert_eq!(log.len(), 4, "{log:?}");
    assert_eq!(log[0].0, "ms-db");
    assert_eq!(log[0].1, "erp2");
    assert!(log[0].2.contains("sys.partitions"), "{}", log[0].2);
    assert_eq!(log[1].0, "ms-db");
    assert_eq!(log[1].2, "SELECT [total], [cliente_id] FROM dbo.pedidos");
    assert_eq!(log[2].0, "pg-db");
    assert_eq!(log[2].1, "erp");
    assert!(log[2].2.contains("EXPLAIN (FORMAT JSON)"), "{}", log[2].2);
    for entry in &log[2..] {
        assert!(entry.2.contains("READ ONLY"), "{}", entry.2);
        assert!(entry.2.contains("SELECT nome, id FROM public.clientes"));
    }
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
        (
            "TiibControl@MainDoc",
            server(
                Engine::PostgreSql,
                "PGSTUDIO",
                "54329",
                "postgres",
                "tiibcontrol",
            ),
        ),
        (
            "Tiib@SaaS",
            server(
                Engine::PostgreSql,
                "PGSTUDIO",
                "54329",
                "postgres",
                "tiibsaas",
            ),
        ),
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
    assert_eq!(data, vec![vec!["100000", "5000050000", "4"]]);
}

/// Conecta o app no banco `tiibcontrol` (como a janela do usuário) e roda
/// `sql` numa janela entre servidores.
fn real_from_tiibcontrol(sql: &str) -> Option<(Answer, Duration)> {
    let mut harness = start(real_app()?);
    let mut s = ConnectionSettings::for_engine(Engine::PostgreSql);
    s.host = var("PGSTUDIO_TEST_HOST", "127.0.0.1");
    s.port = var("PGSTUDIO_TEST_PORT", "54329").parse().unwrap();
    s.user = var("PGSTUDIO_TEST_USER", "postgres");
    s.database = "tiibcontrol".into();
    s.ssl = SslMode::Disable;
    harness
        .state_mut()
        .connect_to(s, var("PGSTUDIO_TEST_PASSWORD", ""));
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(30), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
    let id = cross_query(&mut harness, sql);
    let start = Instant::now();
    run(&mut harness, id, 120);
    let took = start.elapsed();
    let exec = execution(&harness, id);
    let (columns, data) = match exec.results.first() {
        Some(set) => (set.columns(), rows(exec, 0)),
        None => (Vec::new(), Vec::new()),
    };
    Some(((exec.outcome(), columns, data, messages(exec)), took))
}

/// A consulta que travou o app: duas tabelas de milhões de linhas em dois
/// servidores, a última linha por id. Lê as últimas linhas de uma e só as
/// chaves delas da outra.
#[test]
fn real_latest_row_of_two_huge_tables_reads_almost_nothing() {
    let Some(((outcome, columns, data, messages), took)) = real_from_tiibcontrol(
        "SELECT d.caixa_id,l.id\n\
         FROM documento d\n\
         inner join \"Tiib@SaaS\".lotes l on l.chave_de_processamento=d.chave_processamento_tiib\n\
         ORDER BY d.id DESC\n\
         LIMIT 1",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(columns, ["caixa_id", "id"]);
    assert_eq!(data, vec![vec!["0", "500000"]]);
    assert!(took < Duration::from_secs(10), "took {took:?}\n{messages}");
    assert!(
        messages.contains("WHERE chave_processamento_tiib IS NOT NULL ORDER BY id DESC LIMIT 1000"),
        "{messages}"
    );
    assert!(
        messages.contains("[documento] TiibControl@MainDoc → SELECT caixa_id"),
        "{messages}"
    );
    assert!(messages.contains("keys of documento)"), "{messages}");
}

/// Quando as últimas linhas não têm par, a consulta lê mais e, no fim, vai
/// pelo lado que o filtro deixou pequeno.
#[test]
fn real_latest_row_far_from_the_end_still_answers_right() {
    let Some(((outcome, _, data, messages), _)) = real_from_tiibcontrol(
        "select d.id, l.id from documento d \
         join \"Tiib@SaaS\".lotes l on l.chave_de_processamento = d.chave_processamento_tiib \
         where l.id < 100 order by d.id desc limit 2",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(
        data,
        vec![vec!["2500098", "98"], vec!["2500097", "97"]],
        "{messages}"
    );
    assert!(messages.contains("id < 100"), "{messages}");
    assert!(messages.contains("reading more"), "{messages}");
}

/// Sem nada que diminua as tabelas, a consulta para antes de baixar e diz
/// quantas linhas viriam.
#[test]
fn real_two_huge_tables_without_a_filter_stop_before_downloading() {
    let Some(((outcome, _, data, messages), took)) = real_from_tiibcontrol(
        "select count(*) from documento d \
         join \"Tiib@SaaS\".lotes l on l.chave_de_processamento = d.chave_processamento_tiib",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Failed, "{messages}");
    assert!(data.is_empty());
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(
        messages.contains("[documento] Stopped before downloading: about 3,000,0"),
        "{messages}"
    );
    assert!(
        messages
            .contains("Stopped before downloading: about 2,000,000 rows would come from Tiib@SaaS"),
        "{messages}"
    );
    assert!(messages.contains("-- baixar tudo"), "{messages}");
    assert!(!messages.contains("rows so far"), "{messages}");
}

/// Filtro de data e de texto no servidor; a outra tabela vem pelas chaves.
#[test]
fn real_filters_and_keys_go_to_the_servers() {
    let Some(((outcome, _, data, messages), took)) = real_from_tiibcontrol(
        "select count(*), min(d.id), max(l.id) from documento d \
         join \"Tiib@SaaS\".lotes l on l.chave_de_processamento = d.chave_processamento_tiib \
         where d.criado >= '2020-02-04' and d.criado < '2020-02-04 00:10:00' and d.status = 'ERRO'",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(data, vec![vec!["40", "2937600", "438190"]], "{messages}");
    assert!(took < Duration::from_secs(10), "took {took:?}");
    assert!(
        messages.contains(
            "WHERE criado >= '2020-02-04' AND criado <= '2020-02-04 00:10:00' AND status = 'ERRO'"
        ),
        "{messages}"
    );
    assert!(messages.contains("[documento] 61 rows"), "{messages}");
}

/// O comentário `-- baixar tudo` libera a tabela inteira, até o limite.
#[test]
fn real_download_all_is_an_explicit_choice() {
    let Some(((outcome, _, data, messages), _)) =
        real_from_tiibcontrol("select count(*) from \"Tiib@SaaS\".lotes -- baixar tudo")
    else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(data, vec![vec!["1000000"]]);
    assert!(messages.contains("rows so far"), "{messages}");
    assert!(messages.contains("WARNING: only the first"), "{messages}");
}

/// No SQL Server: as últimas linhas com TOP, a outra tabela pelas chaves, e
/// a parada pelo tamanho da tabela.
#[test]
fn real_sql_server_reads_the_top_rows_and_stops_on_a_big_table() {
    let Some((outcome, _, data, messages)) = real(
        "select p.id, c.nome from financeiro.erp.dbo.pedidos p \
         join pg_vendas.clientes c on c.id = p.cliente_id order by p.id desc limit 2",
    ) else {
        return;
    };
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(
        data,
        vec![vec!["3", "Bruno Lima"], vec!["2", "Ana Souza"]],
        "{messages}"
    );
    assert!(messages.contains(
        "SELECT TOP (1000) [id], [cliente_id] FROM dbo.pedidos WHERE [cliente_id] IS NOT NULL"
    ));
    assert!(messages.contains("keys of financeiro.erp.dbo.pedidos)"));

    let (outcome, _, data, messages) = real(
        "select c.nome, p.total from pg_vendas.clientes c \
         join financeiro.erp.dbo.pedidos p on p.cliente_id = c.id \
         where c.cidade = 'São Paulo' and p.total > 100",
    )
    .unwrap();
    assert_eq!(outcome, Outcome::Succeeded, "{messages}");
    assert_eq!(data, vec![vec!["Ana Souza", "150.5"]], "{messages}");
    assert!(
        messages.contains("WHERE cidade = 'São Paulo'"),
        "{messages}"
    );
    assert!(messages.contains("WHERE [total] > 100"), "{messages}");

    let (outcome, _, _, messages) =
        real("select sum(m.valor) from financeiro.erp.dbo.movimentos m").unwrap();
    assert_eq!(outcome, Outcome::Failed, "{messages}");
    assert!(
        messages.contains("Stopped before downloading: about 300,000 rows"),
        "{messages}"
    );
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
