//! A mesma consulta em vários servidores: a escolha no diálogo, o resultado
//! comum com a coluna do servidor, os erros por servidor, a confirmação em
//! Produção e a escolha guardada na sessão.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink, ServerMessage};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::execution::{Execution, Outcome, Tab};
use pgstudio_ui_shell::multi::TargetId;
use pgstudio_ui_shell::registered::{
    NodePath, PRODUCTION, RegisteredServer, RegisteredServers, ServerGroup,
};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};
use pgstudio_ui_shell::session::SessionStore;

/// O que cada servidor recebeu: (host, banco, SQL).
type Log = Arc<Mutex<Vec<(String, String, String)>>>;

struct FakeConnector(Log);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if s.host == "delta-db" {
            return Err(ConnectError::Network("Could not reach delta-db.".into()));
        }
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

/// `bad` dá erro só no beta-db; `update` altera 2 linhas; o resto devolve
/// uma linha com a versão do servidor.
struct FakeSession(Log, String, String);

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        let (host, db) = (self.1.clone(), self.2.clone());
        self.0.lock().unwrap().push((host.clone(), db, sql.clone()));
        std::thread::spawn(move || {
            if sql.contains("bad") && host == "beta-db" {
                sink(QueryEvent::Error(ServerMessage::error(
                    "relation \"bad\" does not exist",
                )));
            } else if sql.starts_with("update") {
                sink(QueryEvent::CommandComplete { rows: 2 });
            } else {
                sink(QueryEvent::Columns(vec!["version".into()]));
                sink(QueryEvent::Row(vec![Some(format!("v-{host}"))]));
                sink(QueryEvent::CommandComplete { rows: 1 });
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.2.clone()
    }
}

fn settings(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: "alice".into(),
        database: format!("{}_erp", host.trim_end_matches("-db")),
        ..Default::default()
    }
}

/// Pasta "Clientes" com Acme (Produção), Beta, Gamma (sem senha guardada)
/// e Delta (fora do ar); "Interno" fica na raiz.
fn tree() -> RegisteredServers {
    let mut r = RegisteredServers::default();
    let g = r
        .add_group(
            &[],
            ServerGroup {
                name: "Clientes".into(),
                ..Default::default()
            },
        )
        .unwrap();
    for (name, host) in [
        ("Acme", "acme-db"),
        ("Beta", "beta-db"),
        ("Gamma", "gamma-db"),
        ("Delta", "delta-db"),
    ] {
        r.add_server(
            &g,
            RegisteredServer {
                name: name.into(),
                description: String::new(),
                environment: (name == "Acme").then(|| PRODUCTION.into()),
                settings: settings(host),
            },
        )
        .unwrap();
    }
    r.add_server(
        &[],
        RegisteredServer {
            name: "Interno".into(),
            description: String::new(),
            environment: None,
            settings: settings("interno-db"),
        },
    )
    .unwrap();
    assert!(r.server(&NodePath::server(&[0], 0)).is_some());
    r
}

fn store() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::default());
    for host in ["acme-db", "beta-db", "delta-db", "interno-db"] {
        store.set(&password_key(&settings(host)), "secret").unwrap();
    }
    store
}

fn app(log: &Log) -> ShellApp {
    ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector(log.clone())))
        .with_secret_store(store())
        .with_registered_servers(tree())
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

fn new_query(harness: &mut Harness<'static, ShellApp>, sql: &str) -> u32 {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    harness.run();
    let id = harness.state().active_document().unwrap();
    set_text(harness, id, sql);
    id
}

fn set_text(harness: &mut Harness<'static, ShellApp>, id: u32, sql: &str) {
    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .text = sql.into();
}

fn ids(hosts: &[&str]) -> Vec<TargetId> {
    hosts.iter().map(|h| TargetId::of(&settings(h))).collect()
}

fn run(harness: &mut Harness<'static, ShellApp>, id: u32) {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    wait(harness, id);
}

fn wait(harness: &mut Harness<'static, ShellApp>, id: u32) {
    let start = Instant::now();
    loop {
        harness.step();
        let doc = harness.state().documents().get(id).unwrap();
        if doc.execution.as_ref().is_some_and(|e| !e.is_running()) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
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

fn cell(v: &str) -> Option<String> {
    Some(v.to_string())
}

#[test]
fn the_dialog_picks_servers_and_whole_folders() {
    let log = Log::default();
    let mut harness = start(app(&log));
    let id = new_query(&mut harness, "select version()");
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::MultiServer);
    harness.run();
    harness.get_by_label("Run on Multiple Servers");
    harness.get_by_label("0 servers selected");

    // A caixa da pasta marca os quatro clientes; a do Interno soma mais um.
    harness
        .get_by_label("Registered Servers / Clientes")
        .click();
    harness.run();
    harness.get_by_label("4 servers selected");
    harness.get_by_label("Interno").click();
    harness.run();
    harness.get_by_label("5 servers selected");
    harness.get_by_label("Delta").click();
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();

    assert_eq!(
        harness.state().multi_selection(id),
        ids(&["interno-db", "acme-db", "beta-db", "gamma-db"])
    );
    assert_eq!(
        harness.state().status(),
        "This query window runs on 4 servers."
    );
    // A faixa da janela diz onde ela roda, com o ambiente que pede cuidado.
    harness.get_by_label("Runs on multiple servers.");
    harness.get_by_label("4 servers");

    // Sem nenhum marcado, a janela volta para a conexão dela.
    harness.state_mut().execute(&ctx, CommandId::MultiServer);
    harness.run();
    harness.get_by_label("Clear").click();
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();
    assert!(harness.state().multi_selection(id).is_empty());
}

#[test]
fn one_result_with_the_server_column_and_errors_per_server() {
    let log = Log::default();
    let mut harness = start(app(&log));
    let id = new_query(&mut harness, "select version()");
    harness
        .state_mut()
        .set_multi_selection(id, ids(&["acme-db", "beta-db", "gamma-db", "delta-db"]));
    run(&mut harness, id);

    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Succeeded);
    assert_eq!(exec.tab, Tab::Results);
    assert_eq!(exec.results.len(), 2);
    // O resultado comum, com o servidor na frente.
    assert_eq!(exec.results[0].columns(), ["Server", "version"]);
    assert_eq!(exec.results[0].row(0), [cell("Acme"), cell("v-acme-db")]);
    assert_eq!(exec.results[0].row(1), [cell("Beta"), cell("v-beta-db")]);
    assert_eq!(exec.results[0].len(), 2);
    // O resumo: quem respondeu e por que os outros não.
    let summary = &exec.results[1];
    assert_eq!(summary.len(), 4);
    assert_eq!(summary.row(0)[..2], [cell("Acme"), cell("OK")]);
    assert_eq!(summary.row(2)[..2], [cell("Gamma"), cell("Failed")]);
    assert!(
        summary.row(2)[4]
            .as_deref()
            .unwrap()
            .starts_with("No saved password."),
    );
    assert_eq!(
        summary.row(3)[4].as_deref(),
        Some("Could not reach delta-db.")
    );
    let texts: Vec<&str> = exec.messages.iter().map(|m| m.text.as_str()).collect();
    assert!(
        texts.contains(&"[Delta] ERROR: Could not reach delta-db."),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"2 of 4 servers answered; 2 failed."),
        "{texts:?}"
    );

    // Cada servidor recebeu o lote no banco do registro dele.
    let mut got = log.lock().unwrap().clone();
    got.sort();
    assert_eq!(
        got,
        [
            (
                "acme-db".into(),
                "acme_erp".into(),
                "select version()".into()
            ),
            (
                "beta-db".into(),
                "beta_erp".into(),
                "select version()".into()
            ),
        ]
    );

    // Um erro de SQL num servidor só não derruba os outros.
    set_text(&mut harness, id, "select * from bad");
    harness
        .state_mut()
        .set_multi_selection(id, ids(&["acme-db", "beta-db"]));
    run(&mut harness, id);
    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Succeeded);
    assert_eq!(exec.results[0].len(), 1);
    assert_eq!(exec.results[0].row(0)[0], cell("Acme"));
    assert_eq!(
        exec.results[1].row(1)[4].as_deref(),
        Some("relation \"bad\" does not exist")
    );
}

#[test]
fn production_still_asks_before_changing_data() {
    let log = Log::default();
    let mut harness = start(app(&log));
    let id = new_query(&mut harness, "select version()");
    harness
        .state_mut()
        .set_multi_selection(id, ids(&["acme-db", "beta-db"]));
    let ctx = harness.ctx.clone();

    // Leitura roda sem perguntar.
    run(&mut harness, id);
    assert!(!harness.state().is_confirming_environment());
    log.lock().unwrap().clear();

    // UPDATE pergunta, dizendo qual servidor é de Produção; Cancel não roda
    // em nenhum.
    set_text(&mut harness, id, "update pedidos set pago = true");
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert!(harness.state().is_confirming_environment());
    harness.get_by_label("Confirm Changes");
    harness.get_by_label("• Acme");
    assert!(harness.query_by_label("• Beta").is_none());
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(harness.state().status(), "The query was not executed.");

    // Execute roda em todos.
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    harness.get_by_label("Execute").click();
    wait(&mut harness, id);
    assert_eq!(log.lock().unwrap().len(), 2);
    let exec = execution(&harness, id);
    // Sem resultado, o resumo conta as linhas alteradas em cada servidor.
    assert_eq!(exec.results.len(), 1);
    assert_eq!(
        exec.results[0].row(0)[..3],
        [cell("Acme"), cell("OK"), cell("2")]
    );

    // Sem servidor de Produção na lista, não pergunta.
    harness
        .state_mut()
        .set_multi_selection(id, ids(&["beta-db", "interno-db"]));
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert!(!harness.state().is_confirming_environment());
    wait(&mut harness, id);
}

#[test]
fn the_chosen_servers_come_back_with_the_session() {
    let dir = std::env::temp_dir().join(format!("postpilot-multi-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::default();
    let chosen = ids(&["acme-db", "beta-db"]);
    {
        let app = app(&log).with_session(SessionStore::open(dir.join("session.json")));
        let mut harness = start(app);
        let id = new_query(&mut harness, "select version()");
        harness.state_mut().set_multi_selection(id, chosen.clone());
        harness.state_mut().save_session();
    }
    let text = std::fs::read_to_string(dir.join("session.json")).unwrap();
    assert!(!text.contains("secret"));

    let app = app(&log).with_session(SessionStore::open(dir.join("session.json")));
    let mut harness = start(app);
    let id = harness.state().active_document().unwrap();
    assert_eq!(harness.state().multi_selection(id), chosen);
    run(&mut harness, id);
    assert_eq!(execution(&harness, id).results[0].len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- PostgreSQL de verdade ---

/// O mesmo servidor de teste registrado quatro vezes: dois nomes de host,
/// outro banco e um banco que não existe.
#[test]
fn real_servers_answer_together() {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let a = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: pgstudio_core::connection::SslMode::Disable,
        ssh: None,
    };
    let other = if a.host == "127.0.0.1" {
        "localhost"
    } else {
        "127.0.0.1"
    };
    let servers = [
        ("Um", a.clone()),
        (
            "Dois",
            ConnectionSettings {
                host: other.into(),
                ..a.clone()
            },
        ),
        (
            "Modelo",
            ConnectionSettings {
                database: "template1".into(),
                ..a.clone()
            },
        ),
        (
            "Sumiu",
            ConnectionSettings {
                database: "nao_existe".into(),
                ..a.clone()
            },
        ),
    ];
    let store = Arc::new(MemoryStore::default());
    let mut r = RegisteredServers::default();
    for (name, s) in &servers {
        store
            .set(&password_key(s), &var("PGSTUDIO_TEST_PASSWORD", ""))
            .unwrap();
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
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(pgstudio_pg_metadata::PgConnector::new(runtime)))
        .with_secret_store(store)
        .with_registered_servers(r);
    let mut harness = start(app);
    let id = new_query(
        &mut harness,
        "select current_database() as db, 1 + 1 as soma",
    );
    harness
        .state_mut()
        .set_multi_selection(id, servers.iter().map(|(_, s)| TargetId::of(s)).collect());
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    let begin = Instant::now();
    while execution(&harness, id).is_running() {
        harness.step();
        assert!(begin.elapsed() < Duration::from_secs(30), "never finished");
        std::thread::sleep(Duration::from_millis(5));
    }

    let exec = execution(&harness, id);
    assert_eq!(exec.outcome(), Outcome::Succeeded, "{:?}", exec.messages);
    assert_eq!(exec.results[0].columns(), ["Server", "db", "soma"]);
    assert_eq!(
        (0..3).map(|i| exec.results[0].row(i)).collect::<Vec<_>>(),
        [
            [cell("Um"), cell("postgres"), cell("2")],
            [cell("Dois"), cell("postgres"), cell("2")],
            [cell("Modelo"), cell("template1"), cell("2")],
        ]
    );
    let summary = exec.results[1].row(3);
    assert_eq!(summary[..2], [cell("Sumiu"), cell("Failed")]);
    assert!(summary[4].as_deref().unwrap().contains("nao_existe"));
}
