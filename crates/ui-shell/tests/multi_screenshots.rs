//! Capturas de tela da consulta em vários servidores, desenhadas sem
//! janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária).
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::multi::TargetId;
use pgstudio_ui_shell::registered::{
    PRODUCTION, RegisteredServer, RegisteredServers, STAGING, ServerGroup,
};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};

const CLIENTS: [(&str, &str, &str, &str); 5] = [
    ("Acme", "db.acme.com.br", "4.12.0", "2026-11-10"),
    (
        "Beta Logística",
        "pg.betalog.com.br",
        "4.12.0",
        "2026-11-09",
    ),
    ("Casa Verde", "10.20.0.14", "4.11.3", "2026-10-02"),
    (
        "Delta Saúde",
        "db.deltasaude.com.br",
        "4.12.0",
        "2026-11-12",
    ),
    ("Estrela", "sql.estrela.com.br", "4.10.1", "2026-08-21"),
];

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        if s.host.starts_with("10.") {
            return Err(ConnectError::Network(
                "Tempo esgotado ao conectar em 10.20.0.14:5432.".into(),
            ));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(Fake(s.host.clone())),
        })
    }
}

struct Fake(String);

impl QueryService for Fake {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession(self.0.clone(), database.to_string()))
    }
}

struct FakeSession(String, String);

impl QuerySession for FakeSession {
    fn execute(&self, _: String, mut sink: QuerySink) {
        let host = self.0.clone();
        std::thread::spawn(move || {
            let (_, _, version, date) = CLIENTS.iter().find(|c| c.1 == host).unwrap();
            sink(QueryEvent::Columns(vec![
                "versao".into(),
                "atualizado_em".into(),
            ]));
            sink(QueryEvent::Row(vec![
                Some(version.to_string()),
                Some(date.to_string()),
            ]));
            sink(QueryEvent::CommandComplete { rows: 1 });
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.1.clone()
    }
}

fn settings(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        user: "suporte".into(),
        database: "erp".into(),
        ..Default::default()
    }
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.render().expect("render").save(out.join(name)).unwrap();
}

#[test]
fn screenshots_of_the_multi_server_query() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let store = Arc::new(MemoryStore::default());
    let mut r = RegisteredServers::default();
    let folders = [("Clientes", PRODUCTION), ("Homologação", STAGING)];
    for (i, (folder, environment)) in folders.into_iter().enumerate() {
        let g = r
            .add_group(
                &[],
                ServerGroup {
                    name: folder.into(),
                    environment: Some(environment.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        for (name, host, ..) in CLIENTS.iter().take(if i == 0 { 5 } else { 2 }) {
            let host = if i == 0 {
                host.to_string()
            } else {
                format!("hml.{host}")
            };
            store.set(&password_key(&settings(&host)), "x").unwrap();
            r.add_server(
                &g,
                RegisteredServer {
                    name: (*name).into(),
                    description: String::new(),
                    environment: None,
                    settings: settings(&host),
                },
            )
            .unwrap();
        }
    }
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_secret_store(store)
        .with_registered_servers(r);
    let mut h = Harness::builder()
        .with_size([1280.0, 940.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.run_steps(3);
    if h.state().connect_dialog().is_some() {
        h.get_by_label("Cancelar").click();
        h.run_steps(3);
    }
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::NewQuery);
    h.run_steps(2);
    let id = h.state().active_document().unwrap();
    h.state_mut().documents_mut().get_mut(id).unwrap().text =
        "-- qual versão do sistema cada cliente está rodando?\nselect versao, atualizado_em\nfrom sistema.versao;\n".into();

    h.state_mut().execute(&ctx, CommandId::MultiServer);
    h.run_steps(2);
    let dialog = h.state_mut().multi_dialog_mut().unwrap();
    for (name, ..) in CLIENTS {
        dialog.set(name, true);
    }
    h.run_steps(3);
    save(&mut h, "1-escolher-servidores.png");

    let chosen: Vec<TargetId> = CLIENTS
        .iter()
        .map(|c| TargetId::of(&settings(c.1)))
        .collect();
    h.state_mut().close_multi_dialog();
    h.state_mut().set_multi_selection(id, chosen);
    h.state_mut().execute(&ctx, CommandId::Execute);
    let start = Instant::now();
    loop {
        h.step();
        let doc = h.state().documents().get(id).unwrap();
        if doc.execution.as_ref().is_some_and(|e| !e.is_running()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(5);
    save(&mut h, "2-resultado-unico.png");
}
