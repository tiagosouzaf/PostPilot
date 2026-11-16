//! Captura de tela do Query History, desenhada sem janela. Só com
//! `--features screenshots`; o PNG vai para `PGSTUDIO_SHOTS` (ou a pasta
//! temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, Engine};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::execution::Outcome;
use pgstudio_ui_shell::history::Entry;

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn entry(
    ago: u64,
    server: &str,
    engine: Engine,
    database: &str,
    sql: &str,
    ms: u64,
    outcome: Outcome,
    rows: u64,
    error: Option<&str>,
) -> Entry {
    let mut e = Entry::finished(
        &format!("app@{server}"),
        &format!("{server} (app)"),
        engine,
        database,
        "SQLQuery1.sql",
        sql,
        Duration::from_millis(ms),
        outcome,
        rows,
        error,
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    e.when = now - ago;
    e
}

#[test]
fn screenshot_of_the_query_history() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector));
    app.connect_to(
        ConnectionSettings {
            host: "producao.cliente-a.com.br".into(),
            user: "app".into(),
            database: "erp".into(),
            ..Default::default()
        },
        "x".into(),
    );
    let pg = Engine::PostgreSql;
    let ms = Engine::SqlServer;
    let a = "producao.cliente-a.com.br:5432";
    let b = "homologacao.cliente-b.com.br:5432";
    let c = "sql.cliente-c.com.br:1433";
    let store = &mut app.history_mut().store;
    for e in [
        entry(
            93_600,
            c,
            ms,
            "Financeiro",
            "SELECT TOP 100 * FROM dbo.Titulos WHERE Vencimento < GETDATE() ORDER BY Vencimento",
            412,
            Outcome::Succeeded,
            100,
            None,
        ),
        entry(
            90_000,
            c,
            ms,
            "Financeiro",
            "ALTER LOGIN integracao WITH PASSWORD = 'Tr0ca-2026!'",
            31,
            Outcome::Succeeded,
            0,
            None,
        ),
        entry(
            86_700,
            a,
            pg,
            "erp",
            "select version()",
            12,
            Outcome::Succeeded,
            1,
            None,
        ),
        entry(
            86_400,
            b,
            pg,
            "erp",
            "select version()",
            15,
            Outcome::Succeeded,
            1,
            None,
        ),
        entry(
            7_300,
            b,
            pg,
            "erp",
            "update pedidos set status = 'faturado' where id = 1042",
            48,
            Outcome::Succeeded,
            1,
            None,
        ),
        entry(
            7_200,
            a,
            pg,
            "erp",
            "select * from pedido_itens where pedido = 1042",
            64,
            Outcome::Failed,
            0,
            Some("relation \"pedido_itens\" does not exist"),
        ),
        entry(
            3_600,
            a,
            pg,
            "erp",
            "vacuum analyze pedidos",
            184_230,
            Outcome::Canceled,
            0,
            None,
        ),
        entry(
            600,
            a,
            pg,
            "erp",
            "-- clientes que mais compraram nos últimos 30 dias\nselect c.nome, count(*) as pedidos, sum(p.total) as total\nfrom clientes c\njoin pedidos p on p.cliente_id = c.id\nwhere p.criado_em >= now() - interval '30 days'\ngroup by c.nome\norder by total desc\nlimit 20;",
            2_340,
            Outcome::Succeeded,
            20,
            None,
        ),
        entry(
            120,
            a,
            pg,
            "erp",
            "select count(*) from pedidos where status = 'aberto'",
            87,
            Outcome::Succeeded,
            1,
            None,
        ),
    ] {
        store.add(e);
    }
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    let start = Instant::now();
    while h.state().connection().is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::NewQuery);
    h.state_mut().execute(&ctx, CommandId::QueryHistory);
    h.run_steps(5);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.get_by_label("-- clientes que mais compraram nos últimos 30 dias select c.nome, count(*) as pedidos, sum(p.total) as total from clientes c join pedidos p on p.cliente_id = c.id where p.criado_em >= now() - interval '30 days' group by c.nome order by total desc limit 20;")
        .click();
    h.run_steps(5);
    h.render()
        .expect("render")
        .save(out.join("historico-de-consultas.png"))
        .expect("save png");

    h.state_mut().history_mut().filter = "pedidos cliente-a".into();
    h.run_steps(5);
    h.render()
        .expect("render")
        .save(out.join("historico-pesquisa.png"))
        .expect("save png");
}
