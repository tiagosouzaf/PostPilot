//! Capturas de tela do tamanho da interface (espaçamentos, letra e zoom),
//! desenhadas sem janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária), com o sufixo de
//! `PGSTUDIO_SHOT_SUFFIX`. `PGSTUDIO_SHOT_STEPS=N` aperta Ctrl++ N vezes
//! antes (cada um, 10%); `PGSTUDIO_SHOT_SIZE=LxA` muda o tamanho da janela.
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::metadata::NodeId;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::databases::LIST_SQL;

/// Tamanho da janela: `PGSTUDIO_SHOT_SIZE=1920x1080`, ou 1280 x 800.
fn window() -> [f32; 2] {
    std::env::var("PGSTUDIO_SHOT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1280.0, 800.0])
}

const DATABASES: &[&str] = &[
    "almoxarifado",
    "auditoria",
    "catalogo",
    "cobranca",
    "comercial",
    "compras",
    "contabil",
    "estoque",
    "faturamento",
    "financeiro",
    "fiscal",
    "frota",
    "logistica",
    "loja",
    "marketing",
    "postgres",
    "producao",
    "rh",
    "suporte",
    "vendas",
];

struct Fake;

impl QueryService for Fake {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(Session(database.to_string()))
    }
}

struct Session(String);

impl QuerySession for Session {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        std::thread::spawn(move || {
            if sql == LIST_SQL {
                sink(QueryEvent::Columns(vec!["datname".into()]));
                for db in DATABASES {
                    sink(QueryEvent::Row(vec![Some((*db).to_string())]));
                }
                sink(QueryEvent::CommandComplete {
                    rows: DATABASES.len() as u64,
                });
            } else if sql.contains("clientes") {
                let columns = ["id", "nome", "cidade", "pedidos", "ultimo_pedido"];
                sink(QueryEvent::Columns(
                    columns.iter().map(|c| (*c).to_string()).collect(),
                ));
                let names = [
                    ("Padaria Pão Quente", "Curitiba"),
                    ("Mercado Bom Preço", "São Paulo"),
                    ("Auto Peças Silva", "Belo Horizonte"),
                    ("Farmácia Vida", "Porto Alegre"),
                    ("Livraria Saber", "Recife"),
                    ("Oficina do Zé", "Salvador"),
                    ("Pet Shop Amigo", "Fortaleza"),
                    ("Restaurante Sabor", "Florianópolis"),
                ];
                let rows = 40;
                for i in 0..rows {
                    let (name, city) = names[i % names.len()];
                    sink(QueryEvent::Row(vec![
                        Some((1000 + i).to_string()),
                        Some(name.to_string()),
                        Some(city.to_string()),
                        Some((97 - i * 2).to_string()),
                        Some(format!("2026-11-{:02} 14:{:02}:07", 1 + i % 15, 10 + i)),
                    ]));
                }
                sink(QueryEvent::CommandComplete { rows: rows as u64 });
            } else {
                sink(QueryEvent::Columns(vec!["c".into()]));
                sink(QueryEvent::Row(vec![Some("f".into())]));
                sink(QueryEvent::CommandComplete { rows: 1 });
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.0.clone()
    }
}

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(Fake),
        })
    }
}

fn until(h: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    while !done(h.state()) {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(4);
}

/// O app conectado, com a árvore aberta e uma consulta executada, no
/// tamanho pedido por `PGSTUDIO_SHOT_STEPS`.
fn start() -> Harness<'static, ShellApp> {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector));
    app.connect_to(
        ConnectionSettings {
            host: "producao.cliente-a.com.br".into(),
            user: "tiago".into(),
            database: "loja".into(),
            ..Default::default()
        },
        "x".into(),
    );
    let mut h = Harness::builder()
        .with_size(window())
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    until(&mut h, "never connected", |app| app.connection().is_some());

    let steps: usize = std::env::var("PGSTUDIO_SHOT_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for _ in 0..steps {
        h.key_press_modifiers(Modifiers::CTRL, Key::Plus);
        h.run_steps(2);
    }
    h.run_steps(3);

    for path in ["srv", "srv/Databases", "srv/Databases/postgres"] {
        let id = NodeId(path.into());
        until(&mut h, "node never loaded", |app| {
            app.explorer().node(&id).is_some()
        });
        h.state_mut().explorer_mut().expand(&id);
    }
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::NewQuery);
    h.run_steps(3);
    let id = h.state().active_document().unwrap();
    h.key_press_modifiers(Modifiers::CTRL, Key::A);
    h.event(Event::Text(
        "select c.id, c.nome, c.cidade, count(*) as pedidos, max(p.criado_em) as ultimo_pedido\nfrom clientes c\njoin pedidos p on p.cliente_id = c.id\ngroup by c.id, c.nome, c.cidade\norder by pedidos desc;".into(),
    ));
    h.run_steps(3);
    h.key_press(Key::F5);
    until(&mut h, "query never finished", |app| {
        app.documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running())
    });
    h
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    h.run_steps(4);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    let suffix = std::env::var("PGSTUDIO_SHOT_SUFFIX").unwrap_or_default();
    let image = h.render().expect("render");
    image
        .save(out.join(format!("{name}{suffix}.png")))
        .expect("save png");
}

#[test]
fn main_window_with_a_menu_open() {
    let mut h = start();
    h.get_by_label("Exibir").click();
    save(&mut h, "menu");
}

#[test]
fn database_list_open() {
    let mut h = start();
    h.key_press_modifiers(Modifiers::CTRL, Key::U);
    let start = Instant::now();
    while h.query_by_label("vendas").is_none() && h.query_by_label("almoxarifado").is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(5), "no list");
        std::thread::sleep(Duration::from_millis(2));
    }
    save(&mut h, "bancos");
}

#[test]
fn options_general_page() {
    let mut h = start();
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::Options);
    h.run_steps(2);
    h.state_mut().options_mut().expect("options").page = pgstudio_ui_shell::options::Page::General;
    save(&mut h, "opcoes");
}
