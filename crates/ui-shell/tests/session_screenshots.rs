//! Captura de tela do app reaberto com a sessão anterior, desenhada sem
//! janela. Só com `--features screenshots`; o PNG vai para `PGSTUDIO_SHOTS`
//! (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::metadata::NodeId;
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::secrets::MemoryStore;
use pgstudio_ui_shell::session::SessionStore;

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

fn start(dir: &Path, store: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_secret_store(store)
        .with_session(SessionStore::open(dir.join("session.json")));
    let h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h
}

fn wait(h: &mut Harness<'static, ShellApp>, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    loop {
        h.step();
        if done(h.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(3);
}

fn connect(h: &mut Harness<'static, ShellApp>, host: &str, user: &str) {
    let settings = ConnectionSettings {
        host: host.into(),
        user: user.into(),
        ..ConnectionSettings::default()
    };
    h.state_mut().open_connect_dialog();
    {
        let dialog = h.state_mut().connect_dialog_mut().expect("dialog");
        dialog.fill(&settings);
        dialog.password = "x".into();
        dialog.remember = true;
    }
    h.step();
    h.state_mut().connect_to(settings, "x".into());
    wait(h, |app| {
        app.connect_dialog().is_none() && app.connection().is_some_and(|c| c.settings.host == host)
    });
}

#[test]
fn reopened_with_the_last_session() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let dir = std::env::temp_dir().join(format!("postpilot-session-shot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(MemoryStore::default());
    {
        let mut h = start(&dir, store.clone());
        connect(&mut h, "producao.cliente-a.com.br", "tiib");
        connect(&mut h, "homologacao.cliente-b.com.br", "app");
        for path in ["srv", "srv/Databases", "srv/Databases/postgres"] {
            let id = NodeId(path.into());
            wait(&mut h, |app| app.explorer().node(&id).is_some());
            h.state_mut().explorer_mut().expand(&id);
        }
        let ctx = h.ctx.clone();
        for text in [
            "-- consulta que ainda não foi salva em arquivo\nselect c.nome, count(*) as pedidos\nfrom clientes c\njoin pedidos p on p.cliente_id = c.id\nwhere p.criado_em >= now() - interval '30 days'\ngroup by c.nome\norder by pedidos desc;\n",
            "update pedidos set status = 'faturado' where id = 1042;\n",
        ] {
            h.state_mut().execute(&ctx, CommandId::NewQuery);
            let id = h.state().active_document().unwrap();
            let doc = h.state_mut().documents_mut().get_mut(id).unwrap();
            doc.text = text.into();
            doc.database = Some("postgres".into());
        }
        let first = h.state().documents().ids()[0];
        h.state_mut().activate_document(first);
        h.run_steps(3);
        h.state_mut().save_session();
    }
    // O app abre de novo.
    let mut h = start(&dir, store);
    wait(&mut h, |app| {
        app.connected_servers().len() == 2
            && app.connect_dialog().is_none()
            && app.explorer().expanded().len() == 3
            && !app.explorer().is_loading()
    });
    h.run_steps(5);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    let image = h.render().expect("render");
    image
        .save(out.join("sessao-reaberta.png"))
        .expect("save png");
}

/// Um servidor que devolve alguns clientes para qualquer consulta.
struct SampleQueries;

impl pgstudio_core::query::QueryService for SampleQueries {
    fn open(&self, database: &str) -> Arc<dyn pgstudio_core::query::QuerySession> {
        Arc::new(SampleSession(database.to_string()))
    }
}

struct SampleSession(String);

impl pgstudio_core::query::QuerySession for SampleSession {
    fn execute(&self, _sql: String, mut sink: pgstudio_core::query::QuerySink) {
        use pgstudio_core::query::QueryEvent;
        sink(QueryEvent::Columns(vec![
            "nome".into(),
            "cidade".into(),
            "pedidos".into(),
        ]));
        let rows = [
            ("Padaria Estrela", "Curitiba", "184"),
            ("Mercado Bom Preço", "Londrina", "97"),
            ("Auto Peças Silva", "Maringá", "63"),
            ("Farmácia Central", "Cascavel", "41"),
            ("Livraria Aurora", "Ponta Grossa", "12"),
        ];
        for (a, b, c) in rows {
            sink(QueryEvent::Row(vec![
                Some(a.into()),
                Some(b.into()),
                Some(c.into()),
            ]));
        }
        sink(QueryEvent::CommandComplete { rows: 5 });
        sink(QueryEvent::Finished { canceled: false });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.0.clone()
    }
}

struct SampleConnector;

impl Connector for SampleConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(SampleQueries),
        })
    }
}

fn start_sample(dir: &Path, store: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(SampleConnector))
        .with_secret_store(store)
        .with_session(SessionStore::open(dir.join("session.json")));
    let h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h
}

/// O resultado da consulta e o Table Designer com alterações, de volta.
#[test]
fn reopened_with_results_and_designer() {
    results_and_designer(false, "sessao-resultado-de-volta.png");
    results_and_designer(true, "sessao-designer-de-volta.png");
}

/// Fecha com a consulta ou o designer na frente e fotografa o app reaberto.
fn results_and_designer(designer_front: bool, file: &str) {
    use pgstudio_object_explorer::Event;
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let dir = std::env::temp_dir().join(format!(
        "postpilot-session-shot-windows-{designer_front}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(MemoryStore::default());
    {
        let mut h = start_sample(&dir, store.clone());
        connect(&mut h, "producao.cliente-a.com.br", "tiib");
        let ctx = h.ctx.clone();
        h.state_mut().execute(&ctx, CommandId::NewQuery);
        let id = h.state().active_document().unwrap();
        {
            let doc = h.state_mut().documents_mut().get_mut(id).unwrap();
            doc.text = "select c.nome, c.cidade, count(*) as pedidos\nfrom clientes c\njoin pedidos p on p.cliente_id = c.id\ngroup by c.nome, c.cidade\norder by pedidos desc;\n".into();
            doc.database = Some("app_01".into());
        }
        h.step();
        h.state_mut().execute(&ctx, CommandId::Execute);
        wait(&mut h, |app| {
            app.documents()
                .get(id)
                .and_then(|d| d.execution.as_ref())
                .is_some_and(|e| !e.is_running())
        });
        let table = NodeId("srv/Databases/app_01/Schemas/public/Tables/3".into());
        h.state_mut().explorer_event(&ctx, Event::Design(table));
        wait(&mut h, |app| {
            let d = app.designers();
            d.ids()
                .first()
                .is_some_and(|id| !d.get(*id).unwrap().is_loading())
        });
        let designer = h.state().designers().ids()[0];
        {
            let d = h.state_mut().designers_mut().get_mut(designer).unwrap();
            let table = d.table_id().unwrap();
            let column = d.model().table(table).unwrap().columns[1].id;
            d.rename_column(column, "nome_fantasia");
            d.add_column("telefone").unwrap();
        }
        if !designer_front {
            h.state_mut().activate_document(id);
        }
        h.run_steps(3);
        h.state_mut().save_session();
    }
    let mut h = start_sample(&dir, store);
    wait(&mut h, |app| {
        let d = app.designers();
        app.connected_servers().len() == 1
            && app.connect_dialog().is_none()
            && d.ids()
                .first()
                .is_some_and(|id| !d.get(*id).unwrap().is_loading())
    });
    h.run_steps(5);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.render()
        .expect("render")
        .save(out.join(file))
        .expect("save png");
}
