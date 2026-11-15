//! Capturas de tela de "levar as conexões junto no workspace", desenhadas
//! sem janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::repo::vault::Kdf;
use pgstudio_ui_shell::secrets::MemoryStore;

const FAST: Kdf = Kdf {
    m_cost: 8 * 1024,
    t_cost: 1,
    p_cost: 1,
};

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "16.4".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

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

fn show_workspace(h: &mut Harness<'static, ShellApp>) {
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::WorkspaceExplorer);
    h.run();
}

#[test]
fn bring_connections_into_the_workspace() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let root = std::env::temp_dir()
        .join(format!("postpilot-ws-shots-{}", std::process::id()))
        .join("sql");
    let _ = std::fs::remove_dir_all(&root);

    let app = ShellApp::default()
        .with_secret_store(Arc::new(MemoryStore::default()))
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(RegisteredServers::in_memory(Default::default()))
        .with_connector(Arc::new(FakeConnector));
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run();
    h.state_mut().connect_to(
        ConnectionSettings {
            host: "prod.abc123.us-east-1.rds.amazonaws.com".into(),
            user: "tiago".into(),
            database: "vendas".into(),
            ..Default::default()
        },
        "senha".into(),
    );
    let start = Instant::now();
    while h.state().connection().is_none() {
        assert!(start.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run();

    // New Workspace: a pergunta.
    assert!(h.state_mut().create_workspace(&root));
    show_workspace(&mut h);
    save(&mut h, "1-trazer-conexoes.png");

    // "Agora não": o painel continua oferecendo.
    h.state_mut()
        .workspace_panel_mut()
        .answer_import_offer(false);
    h.run();
    save(&mut h, "2-workspace-sem-conexoes.png");

    // Importar: pede a senha mestra.
    h.state_mut().import_registered_into_workspace().unwrap();
    h.run();
    save(&mut h, "3-senha-mestra.png");
    {
        let p = h.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.set_kdf(FAST);
        p.create("uma senha mestra", "uma senha mestra", false)
            .unwrap();
        p.wait(None);
    }
    let start = Instant::now();
    while h.state().workspace_panel().has_pending_copy() || h.state().workspace_panel().is_copying()
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    show_workspace(&mut h);
    save(&mut h, "4-conexao-e-senha-no-workspace.png");

    // Conectado a outro servidor, que não está no workspace.
    h.state_mut().connect_to(
        ConnectionSettings {
            host: "localhost".into(),
            user: "postgres".into(),
            ..Default::default()
        },
        "senha".into(),
    );
    let start = Instant::now();
    while h
        .state()
        .connection()
        .is_none_or(|c| c.settings.host != "localhost")
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    show_workspace(&mut h);
    save(&mut h, "5-adicionar-conexao-ativa.png");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
