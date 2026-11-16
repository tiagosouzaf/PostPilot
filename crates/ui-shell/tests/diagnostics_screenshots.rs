//! Capturas de tela das consultas de diagnóstico, num PostgreSQL de verdade
//! (`PGSTUDIO_TEST_HOST`) com três sessões presas uma na outra. Só com
//! `--features screenshots`; os PNGs vão para `PGSTUDIO_SHOTS` (ou a pasta
//! temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_core::query::{QueryEvent, QuerySession};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::secrets::MemoryStore;
use pgstudio_ui_shell::templates::builtin;

fn out(name: &str) -> PathBuf {
    let dir = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    h.run_steps(3);
    let image = h.render().expect("render");
    image.save(out(name)).expect("save png");
}

fn start(session: &Arc<dyn QuerySession>, sql: &str) -> std::sync::mpsc::Receiver<QueryEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    session.execute(sql.into(), Box::new(move |e| drop(tx.send(e))));
    rx
}

fn run(session: &Arc<dyn QuerySession>, sql: &str) {
    let rx = start(session, sql);
    loop {
        match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
            QueryEvent::Error(e) => panic!("{sql}: {}", e.message),
            QueryEvent::Finished { .. } => return,
            _ => {}
        }
    }
}

fn run_ready(h: &mut Harness<'static, ShellApp>, name: &str) {
    let index = builtin::ALL
        .iter()
        .position(|t| t.name == name)
        .expect("template");
    let before = h.state().active_document();
    let ctx = h.ctx.clone();
    h.state_mut().run_ready_template(&ctx, index);
    let start = Instant::now();
    loop {
        h.step();
        let app = h.state();
        let done = app
            .active_document()
            .filter(|id| Some(*id) != before)
            .and_then(|id| app.documents().get(id))
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running());
        if done {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

#[test]
fn diagnostic_queries() {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let admin = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
        ssh: None,
    };
    let password = var("PGSTUDIO_TEST_PASSWORD", "");
    let connector: Arc<dyn Connector> = Arc::new(pgstudio_pg_metadata::PgConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ));
    let c = connector.connect(&admin, &password).unwrap();
    let s = c.queries.open("postgres");
    run(&s, "DROP DATABASE IF EXISTS vendas WITH (FORCE)");
    run(&s, "CREATE DATABASE vendas");
    let settings = ConnectionSettings {
        database: "vendas".into(),
        ..admin
    };
    let c = connector.connect(&settings, &password).unwrap();
    let setup = c.queries.open("vendas");
    run(
        &setup,
        "create extension if not exists pg_stat_statements;
         create table pedido (id int primary key, cliente_id int, status text, total numeric(10,2));
         create table estoque (produto_id int primary key, quantidade int);
         insert into pedido select g, g % 500, 'aberto', g * 3.5 from generate_series(1, 200000) g;
         insert into estoque select g, 100 from generate_series(1, 1000) g;
         analyze;",
    );
    // Um pouco de histórico para as consultas mais caras.
    for _ in 0..3 {
        run(
            &setup,
            "select cliente_id, sum(total) from pedido group by cliente_id order by 2 desc limit 10",
        );
        run(
            &setup,
            "select count(*) from pedido where status = 'aberto'",
        );
        run(&setup, "select * from estoque where produto_id = 42");
    }
    // Três sessões presas: o fechamento do mês segura o estoque, a baixa
    // de um pedido espera por ele e um relatório espera pela baixa.
    let a = c.queries.open("vendas");
    run(
        &a,
        "begin; update estoque set quantidade = quantidade - 1 where produto_id = 42 /* fechamento do mês */",
    );
    let b = c.queries.open("vendas");
    let _b = start(
        &b,
        "begin; lock table pedido in share row exclusive mode; \
         update estoque set quantidade = quantidade - 5 where produto_id = 42 /* baixa do pedido 1881 */",
    );
    std::thread::sleep(Duration::from_millis(700));
    let d = c.queries.open("vendas");
    let _d = start(
        &d,
        "update pedido set status = 'faturado' where id = 1881 /* faturamento */",
    );
    std::thread::sleep(Duration::from_millis(700));

    let mut app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(RegisteredServers::in_memory(Default::default()))
        .with_connector(connector.clone());
    app.connect_to(settings, password);
    let mut h = Harness::builder()
        .with_size([1440.0, 860.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    let begin = Instant::now();
    while h.state().connection().is_none() {
        h.step();
        assert!(begin.elapsed() < Duration::from_secs(20), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);

    // 1. O painel na pasta de diagnóstico e "Quem Bloqueia Quem" rodando.
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::DiagnosticQueries);
    h.run_steps(3);
    h.get_by_label_contains("Quem Bloqueia Quem").click();
    h.run_steps(2);
    run_ready(&mut h, "Who Blocks Whom");
    save(&mut h, "1-quem-bloqueia-quem.png");

    // 2. O diagnóstico completo: um resultado por parte.
    run_ready(&mut h, "Full Diagnosis");
    save(&mut h, "2-diagnostico-completo.png");

    // 3. As consultas mais caras do histórico.
    run_ready(&mut h, "Most Expensive Queries (History)");
    save(&mut h, "3-consultas-mais-caras.png");

    // 4. O menu da toolbar: um clique roda.
    let caption = format!(
        "{} Diagnóstico {}",
        egui_phosphor::regular::STETHOSCOPE,
        egui_phosphor::regular::CARET_DOWN
    );
    h.get_by_label(&caption).click();
    h.run_steps(3);
    save(&mut h, "4-menu-da-toolbar.png");

    a.cancel();
    b.cancel();
    d.cancel();
}
