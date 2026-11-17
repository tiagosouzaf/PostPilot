//! A faixa de status da janela de consulta diz em que pasta o servidor
//! está registrado ("MainDoc / TiibControl@MainDoc (tiibcontrol)"), em vez
//! de só o endereço, que não diz de qual cliente é.
//!
//! Em pt-BR, num arquivo só dele: o idioma vale para o processo todo.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers, ServerGroup};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};

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

fn settings(host: &str, user: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: user.into(),
        ..Default::default()
    }
}

fn folder(name: &str, environment: Option<&str>) -> ServerGroup {
    ServerGroup {
        name: name.into(),
        environment: environment.map(str::to_string),
        ..Default::default()
    }
}

fn registration(name: &str, settings: ConnectionSettings) -> RegisteredServer {
    RegisteredServer {
        name: name.into(),
        description: String::new(),
        environment: None,
        settings,
    }
}

/// MainDoc > Produção > TiibControl@MainDoc, MainDoc > Homologação >
/// Clientes > Hml@MainDoc e, fora de pasta, "solto".
fn tree() -> RegisteredServers {
    let mut r = RegisteredServers::default();
    let client = r.add_group(&[], folder("MainDoc", None)).unwrap();
    let production = r
        .add_group(&client, folder("Produção", Some("Production")))
        .unwrap();
    r.add_server(
        &production,
        registration(
            "TiibControl@MainDoc",
            settings("192.168.30.10", "tiibcontrol"),
        ),
    )
    .unwrap();
    let staging = r
        .add_group(&client, folder("Homologação", Some("Staging")))
        .unwrap();
    let inner = r.add_group(&staging, folder("Clientes", None)).unwrap();
    r.add_server(
        &inner,
        registration("Hml@MainDoc", settings("192.168.30.20", "tiibcontrol")),
    )
    .unwrap();
    r.add_server(&[], registration("solto", settings("10.0.0.5", "app")))
        .unwrap();
    r
}

fn harness(size: [f32; 2]) -> Harness<'static, ShellApp> {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    for s in tree().all_servers() {
        store.set(&password_key(&s.settings), "secret").unwrap();
    }
    let app = ShellApp::default()
        .with_secret_store(store)
        .with_registered_servers(tree())
        .with_connector(Arc::new(FakeConnector));
    Harness::builder()
        .with_size(size)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

/// Conecta o servidor registrado e abre uma janela de consulta nele.
fn open_query(h: &mut Harness<'static, ShellApp>, s: &ConnectionSettings) {
    h.state_mut().connect_registered(s, true);
    let start = Instant::now();
    loop {
        h.step();
        let there = h
            .state()
            .connection()
            .is_some_and(|c| c.settings.host == s.host)
            && h.state().active_document().is_some();
        if there {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "did not connect to {}",
            s.host
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(4);
}

#[test]
fn the_title_has_the_folders_without_the_one_named_like_the_environment() {
    let h = harness([1280.0, 800.0]);
    let app = h.state();
    // "Produção" é a pasta e também a etiqueta do ambiente: não repete.
    assert_eq!(
        app.registered_title(&settings("192.168.30.10", "tiibcontrol"))
            .as_deref(),
        Some("MainDoc / TiibControl@MainDoc (tiibcontrol)")
    );
    // As outras pastas do caminho aparecem todas.
    assert_eq!(
        app.registered_title(&settings("192.168.30.20", "tiibcontrol"))
            .as_deref(),
        Some("MainDoc / Clientes / Hml@MainDoc (tiibcontrol)")
    );
    // Fora de pasta: só o nome do registro.
    assert_eq!(
        app.registered_title(&settings("10.0.0.5", "app"))
            .as_deref(),
        Some("solto (app)")
    );
    // Não registrado: nada (a faixa segue com o endereço).
    assert_eq!(app.registered_title(&settings("outro", "app")), None);
}

#[test]
fn the_query_strip_and_the_status_bar_show_the_folder_of_the_server() {
    let mut h = harness([1280.0, 800.0]);
    open_query(&mut h, &settings("192.168.30.10", "tiibcontrol"));
    let title = "MainDoc / TiibControl@MainDoc (tiibcontrol)";
    // Na faixa da janela de consulta e na barra de status de baixo.
    assert_eq!(h.query_all_by_label(title).count(), 2);
    assert_eq!(
        h.query_all_by_label("192.168.30.10 (tiibcontrol)").count(),
        0,
        "the address moved to the tooltip"
    );
}

/// `--features screenshots`; o PNG vai para `PGSTUDIO_SHOTS` (ou a pasta
/// temporária).
#[cfg(feature = "screenshots")]
#[test]
fn screenshot_of_the_folder_in_the_strip() {
    for (file, size, host) in [
        ("pasta-na-faixa.png", [1100.0, 520.0], "192.168.30.10"),
        (
            "pasta-na-faixa-subpasta.png",
            [1100.0, 520.0],
            "192.168.30.20",
        ),
        (
            "pasta-na-faixa-estreita.png",
            [560.0, 420.0],
            "192.168.30.10",
        ),
    ] {
        let mut h = harness(size);
        pgstudio_ui_shell::install_fonts(&h.ctx);
        pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
        open_query(&mut h, &settings(host, "tiibcontrol"));
        let out = std::env::var_os("PGSTUDIO_SHOTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&out).unwrap();
        h.render()
            .expect("render")
            .save(out.join(file))
            .expect("save png");
    }
}
