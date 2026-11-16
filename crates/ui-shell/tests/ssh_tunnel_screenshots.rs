//! Captura do "Connect to Server" com os campos do túnel SSH, desenhada
//! sem janela. Só com `--features screenshots`; o PNG vai para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use egui_kittest::Harness;
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, SshAuth, SshSettings,
};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;

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

#[test]
fn connect_dialog_with_the_tunnel_fields() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let settings = ConnectionSettings {
        host: "10.20.0.15".into(),
        user: "app".into(),
        database: "vendas".into(),
        ssh: Some(SshSettings {
            host: "bastion.cliente-a.com.br".into(),
            port: 22,
            user: "tiago".into(),
            auth: SshAuth::KeyFile,
            key_file: "~/.ssh/id_ed25519".into(),
        }),
        ..Default::default()
    };
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(vec![settings]))
        .with_connector(Arc::new(FakeConnector));
    let mut h = Harness::builder()
        .with_size([1100.0, 760.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run_steps(5);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.render()
        .expect("render")
        .save(out.join("conectar-tunel-ssh.png"))
        .expect("save png");
}
