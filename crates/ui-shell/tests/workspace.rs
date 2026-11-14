//! Testes do marco R.1: workspace em pasta, Workspace Explorer e consultas
//! ligadas a um servidor e banco do workspace, com um conector falso.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::panels::Panel;
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers};
use pgstudio_ui_shell::repo::{MANIFEST, QUERIES, QueryLink, SERVERS, Workspace};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore};

/// Aceita a senha "secret".
struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if password != "secret" {
            return Err(ConnectError::Authentication("Login failed.".into()));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "16.4".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

/// Respostas prontas para os diálogos: a pasta e o arquivo a salvar.
#[derive(Clone, Default)]
struct Dialogs(Arc<Mutex<(Option<PathBuf>, Option<PathBuf>)>>);

impl Dialogs {
    fn folder(&self, p: &Path) {
        self.0.lock().unwrap().0 = Some(p.to_path_buf());
    }
    fn save(&self, p: &Path) {
        self.0.lock().unwrap().1 = Some(p.to_path_buf());
    }
}

impl FileDialogs for Dialogs {
    fn save_path(&mut self, _: &str, _: Option<&Path>) -> Option<PathBuf> {
        self.0.lock().unwrap().1.clone()
    }
    fn open_path(&mut self) -> Option<PathBuf> {
        None
    }
    fn folder_path(&mut self, _: &str) -> Option<PathBuf> {
        self.0.lock().unwrap().0.clone()
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pgstudio-r1-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn prod() -> ConnectionSettings {
    ConnectionSettings {
        host: "sales-db".into(),
        user: "alice".into(),
        ..Default::default()
    }
}

fn harness(dialogs: Dialogs, memory: Option<&Path>) -> Harness<'static, ShellApp> {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    store.set("alice@sales-db:5432", "secret").unwrap();
    let mut app = ShellApp::default()
        .with_secret_store(store)
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_file_dialogs(dialogs);
    if let Some(m) = memory {
        app = app.with_workspace_memory(m);
    }
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    harness.get_by_label("Cancel").click();
    harness.run();
    harness
}

/// Roda frames até `ok`.
fn run_until(harness: &mut Harness<'static, ShellApp>, what: &str, ok: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    while !ok(harness.state()) {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
}

/// Põe o servidor "Produção" no workspace aberto.
fn add_prod(harness: &mut Harness<'static, ShellApp>) {
    let panel = harness
        .state_mut()
        .workspace_panel_mut()
        .servers_mut()
        .unwrap();
    panel
        .servers_mut()
        .add_server(
            &[],
            RegisteredServer {
                name: "Produção".into(),
                description: String::new(),
                settings: prod(),
            },
        )
        .unwrap();
}

#[test]
fn new_workspace_creates_the_folder_and_opens_the_explorer() {
    let root = temp_dir("new").join("vendas");
    let dialogs = Dialogs::default();
    dialogs.folder(&root);
    let mut harness = harness(dialogs, None);
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewWorkspace);
    harness.run();
    assert!(root.join(MANIFEST).exists());
    assert!(root.join(SERVERS).exists());
    assert!(root.join(QUERIES).is_dir());
    assert!(root.join(".gitignore").exists());
    assert_eq!(
        harness.state().current_workspace().unwrap().name(),
        "vendas"
    );
    assert!(harness.state().is_panel_open(Panel::WorkspaceExplorer));
    assert!(harness.query_all_by_label_contains("vendas").count() >= 1);
    assert!(harness.query_all_by_label_contains("Queries").count() >= 1);

    // Abrir uma pasta qualquer como workspace explica o que falta.
    let other = temp_dir("not-a-workspace");
    harness.state_mut().close_workspace();
    assert!(!harness.state_mut().open_workspace(&other));
    assert!(
        harness
            .state()
            .status()
            .contains("not a PGStudio workspace")
    );
}

#[test]
fn saved_query_is_linked_and_reconnects_after_a_restart() {
    let base = temp_dir("link");
    let root = base.join("ws");
    let memory = base.join("workspace.txt");
    Workspace::create(&root, "ws").unwrap();
    let dialogs = Dialogs::default();
    let mut harness = harness(dialogs.clone(), Some(&memory));
    assert!(harness.state_mut().open_workspace(&root));
    add_prod(&mut harness);

    // Conecta pelo servidor do workspace e abre uma consulta no banco "vendas".
    let mut s = prod();
    s.database = "vendas".into();
    harness.state_mut().connect_registered(&s, true);
    run_until(&mut harness, "connected", |a| {
        a.connection().is_some() && !a.documents().ids().is_empty()
    });
    let id = harness.state().active_document().unwrap();
    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .text = "select 1;\n".into();
    let file = root.join(QUERIES).join("total.sql");
    dialogs.save(&file);
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::SaveAs);
    harness.run();
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        text,
        "-- pgstudio: server=Produção; database=vendas\nselect 1;\n"
    );
    assert!(
        harness
            .state()
            .status()
            .contains("Linked to Produção / vendas")
    );
    // O Workspace Explorer já mostra o arquivo.
    harness
        .state_mut()
        .execute(&ctx, CommandId::WorkspaceExplorer);
    harness.run();
    harness.get_by_label_contains("total.sql");
    // Salvar de novo não duplica a linha.
    harness.state_mut().execute(&ctx, CommandId::Save);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    drop(harness);

    // Reinicia: o workspace volta sozinho e abrir a consulta conecta no
    // servidor dela, no banco dela.
    let mut harness = self::harness(Dialogs::default(), Some(&memory));
    assert_eq!(harness.state().current_workspace().unwrap().root(), root);
    assert!(harness.state().connection().is_none());
    assert!(harness.state_mut().open_file(&file));
    run_until(&mut harness, "reconnected", |a| a.connection().is_some());
    assert_eq!(
        harness.state().connection().unwrap().settings.host,
        "sales-db"
    );
    let id = harness.state().active_document().unwrap();
    assert_eq!(
        harness
            .state()
            .documents()
            .get(id)
            .unwrap()
            .database
            .as_deref(),
        Some("vendas")
    );
}

#[test]
fn query_on_a_server_outside_the_workspace_is_saved_without_a_link() {
    let base = temp_dir("nolink");
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    let dialogs = Dialogs::default();
    let mut harness = harness(dialogs.clone(), None);
    harness.state_mut().open_workspace(&root);
    harness.state_mut().connect_registered(&prod(), true);
    run_until(&mut harness, "connected", |a| {
        a.connection().is_some() && !a.documents().ids().is_empty()
    });
    let id = harness.state().active_document().unwrap();
    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .text = "select 2;".into();
    let file = root.join(QUERIES).join("solta.sql");
    dialogs.save(&file);
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::SaveAs);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "select 2;");
    assert!(harness.state().status().contains("Register this server"));
    // Fora do workspace, nada muda.
    let outside = base.join("fora.sql");
    dialogs.save(&outside);
    harness.state_mut().execute(&ctx, CommandId::SaveAs);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "select 2;");
    assert_eq!(harness.state().status(), "Item(s) Saved");
}

#[test]
fn new_query_and_new_folder_in_the_explorer() {
    let base = temp_dir("newq");
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    let mut harness = harness(Dialogs::default(), None);
    harness.state_mut().open_workspace(&root);
    add_prod(&mut harness);
    harness.state_mut().connect_registered(&prod(), false);
    run_until(&mut harness, "connected", |a| a.connection().is_some());

    let panel = harness.state_mut().workspace_panel_mut();
    panel.new_folder(&root.join(QUERIES));
    assert!(panel.is_naming_folder());
    assert!(panel.create_folder("../fora").is_err());
    let folder = panel.create_folder("relatorios").unwrap();
    assert!(folder.is_dir());

    harness.state_mut().new_workspace_query(&folder);
    harness.run();
    let file = folder.join("SQLQuery1.sql");
    let link = QueryLink::read(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(link.server, "Produção");
    assert_eq!(link.database.as_deref(), Some("postgres"));
    let id = harness.state().active_document().unwrap();
    assert_eq!(
        harness.state().documents().get(id).unwrap().path(),
        Some(file.as_path())
    );
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .execute(&ctx, CommandId::WorkspaceExplorer);
    harness.run();
    harness.get_by_label_contains("relatorios");
    assert!(harness.query_all_by_label_contains("SQLQuery1.sql").count() >= 1);
    let queries = harness.state().workspace_panel().queries().unwrap();
    assert_eq!(queries.count(), 1);
}

#[test]
fn ctrl_alt_l_opens_the_workspace_explorer_without_a_workspace() {
    let mut harness = harness(Dialogs::default(), None);
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .execute(&ctx, CommandId::WorkspaceExplorer);
    harness.run();
    assert!(harness.state().is_panel_open(Panel::WorkspaceExplorer));
    harness.get_by_label_contains("New Workspace...");
    harness.get_by_label_contains("Open Workspace...");
}
