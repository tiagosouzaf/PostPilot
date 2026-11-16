//! Capturas de tela da regra da IA por ambiente (o diálogo de confirmação
//! com a etiqueta de Produção e o diálogo Ambientes), desenhadas sem
//! janela. Só com `--features screenshots` e `PGSTUDIO_TEST_HOST`; os PNGs
//! vão para `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_automation::Origin;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectionSettings, Connector, SslMode};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::registered::{PRODUCTION, RegisteredServer, RegisteredServers, ServerGroup};
use serde_json::json;

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

#[test]
fn production_confirmation_and_environments() {
    let Ok(host) = std::env::var("PGSTUDIO_TEST_HOST") else {
        eprintln!("PGSTUDIO_TEST_HOST not set; skipping");
        return;
    };
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let settings = ConnectionSettings {
        engine: Default::default(),
        host,
        port: var("PGSTUDIO_TEST_PORT", "5432").parse().unwrap(),
        database: "postgres".into(),
        user: var("PGSTUDIO_TEST_USER", "postgres"),
        ssl: SslMode::Disable,
        ssh: None,
    };
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);

    let mut servers = RegisteredServers::default();
    let group = servers
        .add_group(
            &[],
            ServerGroup {
                name: "Cliente Acme".into(),
                environment: Some(PRODUCTION.into()),
                ..Default::default()
            },
        )
        .unwrap();
    servers
        .add_server(
            &group,
            RegisteredServer {
                name: "acme-producao".into(),
                description: String::new(),
                environment: None,
                settings: settings.clone(),
            },
        )
        .unwrap();
    let connector: Arc<dyn Connector> = Arc::new(pgstudio_pg_metadata::PgConnector::new(
        pgstudio_pg_client::DbRuntime::new().unwrap(),
    ));
    let mut app = ShellApp::default()
        .with_connector(connector)
        .with_registered_servers(servers);
    app.connect_to(settings, var("PGSTUDIO_TEST_PASSWORD", ""));
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    let start = Instant::now();
    while h.state().connection().is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(20), "never connected");
        std::thread::sleep(Duration::from_millis(5));
    }
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::NewQuery);
    h.run_steps(5);

    // Uma leitura em Produção: o diálogo pergunta, com a etiqueta.
    let client = h.state_mut().automation(&ctx);
    let _rx = client.send(
        "run_query",
        json!({ "sql": "select usename, count(*)\n  from pg_stat_activity\n group by usename" }),
        Origin::Panel,
    );
    while h.state().claude_confirmation().is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(30), "no confirmation");
    }
    save(&mut h, "confirmacao-em-producao.png");
    h.state_mut().answer_claude_confirmation(&ctx, false);
    h.run_steps(3);

    // O diálogo Ambientes, com a regra da IA de cada um.
    h.state_mut().execute(&ctx, CommandId::RegisteredServers);
    h.run_steps(3);
    h.state_mut().registered_mut().edit_environments();
    save(&mut h, "ambientes-regra-da-ia.png");
}
