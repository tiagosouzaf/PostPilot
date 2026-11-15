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
