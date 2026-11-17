//! Capturas do "Test connection" nos diálogos de conexão, desenhadas sem
//! janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector, Engine};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        std::thread::sleep(Duration::from_millis(37));
        if s.host.starts_with("lento") {
            std::thread::sleep(Duration::from_secs(600));
        }
        if password != "secret" {
            return Err(ConnectError::Authentication(format!(
                "password authentication failed for user \"{}\"",
                s.user
            )));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: match s.engine {
                Engine::SqlServer => "16.0.4135".into(),
                _ => "16.4".into(),
            },
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn out() -> PathBuf {
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    out
}

fn start(size: [f32; 2], theme: pgstudio_core::Theme) -> Harness<'static, ShellApp> {
    let settings = ConnectionSettings {
        host: "vendas-prod.c9akciq32.us-east-1.rds.amazonaws.com".into(),
        user: "app".into(),
        database: "vendas".into(),
        ..Default::default()
    };
    let app = ShellApp::default()
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(vec![settings]))
        .with_connector(Arc::new(FakeConnector));
    let mut h = Harness::builder()
        .with_size(size)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, theme);
    h.run_steps(5);
    h
}

fn test_now(h: &mut Harness<'static, ShellApp>, wait: bool) {
    h.get_by_label("Testar conexão").click();
    let start = Instant::now();
    loop {
        h.step();
        let running = h
            .state()
            .connect_dialog()
            .is_some_and(|d| d.test.is_running());
        if !running || !wait || start.elapsed() > Duration::from_secs(25) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(4);
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    h.render()
        .expect("render")
        .save(out().join(name))
        .expect("save png");
}

#[test]
fn test_connection_in_the_dialogs() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let dark = pgstudio_core::Theme::Dark;

    let mut h = start([1100.0, 760.0], dark);
    h.state_mut().connect_dialog_mut().unwrap().password = "secret".into();
    h.run_steps(2);
    test_now(&mut h, true);
    save(&mut h, "1-deu-certo.png");

    h.state_mut().connect_dialog_mut().unwrap().password = "errada".into();
    h.run_steps(2);
    test_now(&mut h, true);
    save(&mut h, "2-senha-errada.png");

    let d = h.state_mut().connect_dialog_mut().unwrap();
    d.server = "lento.interno,5432".into();
    h.run_steps(2);
    test_now(&mut h, false);
    save(&mut h, "3-testando-pode-cancelar.png");

    h.get_by_label("Cancelar teste").click();
    h.run_steps(2);
    // O limite de verdade (15 s), para a mensagem sair como no aplicativo.
    test_now(&mut h, true);
    save(&mut h, "4-sem-resposta.png");

    // Tema claro e janela pequena: o resultado quebra em linhas.
    let mut h = start([520.0, 620.0], pgstudio_core::Theme::Light);
    h.state_mut().connect_dialog_mut().unwrap().password = "secret".into();
    h.run_steps(2);
    test_now(&mut h, true);
    save(&mut h, "5-janela-pequena-claro.png");

    // O registro de servidor (Registered Servers e workspace).
    let mut h = start([1100.0, 860.0], dark);
    h.get_by_label("Cancelar").click();
    h.run_steps(2);
    h.state_mut().registered_mut().new_server(&[], None);
    h.run_steps(2);
    let d = h.state_mut().registered_mut().server_dialog_mut().unwrap();
    d.connection_string =
        "postgresql://app:secret@financeiro-db.empresa.com.br:5432/financeiro".into();
    d.apply_connection_string().expect("connection string");
    d.name = "Financeiro (produção)".into();
    h.run_steps(2);
    h.get_by_label("Testar conexão").click();
    let start_at = Instant::now();
    loop {
        h.step();
        let running = h
            .state()
            .registered()
            .server_dialog()
            .is_some_and(|d| d.test.is_running());
        if !running || start_at.elapsed() > Duration::from_secs(5) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(4);
    save(&mut h, "6-registro-de-servidor.png");
}
