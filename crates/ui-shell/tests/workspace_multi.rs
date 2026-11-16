//! Vários workspaces abertos ao mesmo tempo: cada um com as suas conexões,
//! o seu git e o seu cofre; a lista volta ao reabrir o aplicativo.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers};
use pgstudio_ui_shell::repo::source_control::SyncKind;
use pgstudio_ui_shell::repo::vault::Kdf;
use pgstudio_ui_shell::repo::{MANIFEST, QUERIES, QueryLink, Workspace};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore};

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

/// A pasta que o seletor de pastas devolve.
#[derive(Clone, Default)]
struct Dialogs(Arc<Mutex<Option<PathBuf>>>);

impl FileDialogs for Dialogs {
    fn save_path(&mut self, _: &str, _: Option<&Path>) -> Option<PathBuf> {
        None
    }
    fn open_path(&mut self) -> Option<PathBuf> {
        None
    }
    fn folder_path(&mut self, _: &str) -> Option<PathBuf> {
        self.0.lock().unwrap().clone()
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pgstudio-multi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn server(name: &str, host: &str) -> RegisteredServer {
    RegisteredServer {
        name: name.into(),
        description: String::new(),
        environment: None,
        settings: ConnectionSettings {
            host: host.into(),
            user: "alice".into(),
            ..Default::default()
        },
    }
}

fn app(dialogs: Dialogs, memory: Option<&Path>) -> ShellApp {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    store.set("alice@acme-db:5432", "secret").unwrap();
    let mut app = ShellApp::default()
        .with_secret_store(store)
        .with_registered_servers(RegisteredServers::default())
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_file_dialogs(dialogs);
    if let Some(m) = memory {
        app = app.with_workspace_memory(m);
    }
    app
}

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    if let Some(cancel) = harness.query_by_label("Cancel") {
        cancel.click();
    }
    harness.run();
    harness
}

/// Um workspace em `base/name` com um servidor de mesmo nome do host.
fn workspace(base: &Path, name: &str, host: &str) -> PathBuf {
    let root = base.join(name);
    let ws = Workspace::create(&root, name).unwrap();
    let mut servers = ws.servers();
    servers.add_server(&[], server(host, host)).unwrap();
    std::fs::write(
        root.join(QUERIES).join(format!("{name}.sql")),
        "select 1;\n",
    )
    .unwrap();
    root
}

fn names(app: &ShellApp) -> Vec<(String, bool)> {
    app.workspace_panel()
        .list()
        .into_iter()
        .map(|w| (w.name, w.active))
        .collect()
}

#[test]
fn several_workspaces_stay_open_and_the_last_opened_is_active() {
    let base = temp_dir("open");
    let acme = workspace(&base, "acme", "acme-db");
    let time = workspace(&base, "time", "time-db");
    let mut app = app(Dialogs::default(), None);
    assert!(app.open_workspace(&acme));
    assert!(app.open_workspace(&time));
    assert_eq!(
        names(&app),
        [("acme".to_string(), false), ("time".to_string(), true)]
    );
    assert_eq!(app.current_workspace().unwrap().name(), "time");

    // Abrir de novo um que já está aberto só o traz para a frente.
    assert!(app.open_workspace(&acme));
    assert_eq!(app.workspace_panel().count(), 2);
    assert_eq!(app.current_workspace().unwrap().name(), "acme");

    // As conexões dos dois aparecem no "Connect to Server", com o nome do
    // workspace de cada uma; as do ativo primeiro.
    let saved: Vec<(String, Option<String>)> = app
        .saved_connections()
        .into_iter()
        .map(|c| (c.name, c.workspace))
        .collect();
    assert_eq!(
        saved,
        [
            ("acme-db".to_string(), Some("acme".to_string())),
            ("time-db".to_string(), Some("time".to_string())),
        ]
    );
    // E como destinos de "executar em vários servidores".
    let targets: Vec<String> = app.multi_targets().into_iter().map(|t| t.folder).collect();
    assert_eq!(targets, ["acme", "time"]);

    // Fechar o ativo deixa o outro aberto.
    app.close_workspace();
    assert_eq!(names(&app), [("time".to_string(), true)]);
    app.close_workspace();
    assert!(app.current_workspace().is_none());
}

#[test]
fn the_open_workspaces_and_the_active_one_come_back_on_reopen() {
    let base = temp_dir("memory");
    let acme = workspace(&base, "acme", "acme-db");
    let time = workspace(&base, "time", "time-db");
    let outro = workspace(&base, "outro", "outro-db");
    let memory = base.join("config").join("workspace.txt");
    {
        let mut app = app(Dialogs::default(), Some(&memory));
        app.open_workspace(&acme);
        app.open_workspace(&time);
        app.open_workspace(&outro);
        app.activate_workspace(1);
    }
    let text = std::fs::read_to_string(&memory).unwrap();
    assert_eq!(text.lines().count(), 3, "{text}");
    // Uma pasta que sumiu fica de fora, sem atrapalhar as outras.
    std::fs::remove_dir_all(&outro).unwrap();
    let app2 = app(Dialogs::default(), Some(&memory));
    assert_eq!(
        names(&app2),
        [("acme".to_string(), false), ("time".to_string(), true)]
    );

    // O arquivo de antes (só a pasta do único workspace) continua valendo.
    std::fs::write(&memory, acme.display().to_string()).unwrap();
    let app3 = app(Dialogs::default(), Some(&memory));
    assert_eq!(names(&app3), [("acme".to_string(), true)]);
}

#[test]
fn each_workspace_keeps_its_own_passwords() {
    let base = temp_dir("vaults");
    let acme = workspace(&base, "acme", "acme-db");
    let time = workspace(&base, "time", "time-db");
    let mut app = app(Dialogs::default(), None);
    app.open_workspace(&acme);
    app.open_workspace(&time);
    // Senha mestra diferente em cada um.
    for (i, master) in [(0, "senha do acme 1"), (1, "senha do time 2")] {
        let p = app.workspace_panel_mut().passwords_at_mut(i).unwrap();
        p.set_kdf(FAST);
        p.create(master, master, false).unwrap();
        p.wait(None);
        assert!(p.is_unlocked());
    }
    // Um frame para o painel dizer a cada cofre quais servidores são dele.
    let mut h = harness(app);
    let store = h.state().secret_store().unwrap();
    store.set("alice@acme-db:5432", "s-acme").unwrap();
    store.set("alice@time-db:5432", "s-time").unwrap();
    store.set("alice@fora:5432", "s-fora").unwrap();
    assert_eq!(
        store.get("alice@acme-db:5432").unwrap().as_deref(),
        Some("s-acme")
    );
    assert_eq!(
        store.get("alice@time-db:5432").unwrap().as_deref(),
        Some("s-time")
    );
    h.run();

    // Cada senha foi para o secrets.json do seu workspace, cifrada.
    let read = |root: &Path| std::fs::read_to_string(root.join("secrets.json")).unwrap();
    assert!(read(&acme).contains("alice@acme-db:5432"));
    assert!(!read(&acme).contains("alice@time-db:5432"));
    assert!(read(&time).contains("alice@time-db:5432"));
    assert!(!read(&time).contains("alice@acme-db:5432"));
    for root in [&acme, &time] {
        let text = read(root);
        assert!(!text.contains("s-acme") && !text.contains("s-time"));
        assert!(!text.contains("alice@fora"));
    }
}

#[test]
fn a_query_links_to_the_server_of_its_own_workspace() {
    let base = temp_dir("link");
    let acme = workspace(&base, "acme", "acme-db");
    let time = workspace(&base, "time", "time-db");
    let mut app = app(Dialogs::default(), None);
    app.open_workspace(&acme);
    app.open_workspace(&time);
    // A consulta é do "acme", que não é o ativo.
    let file = acme.join(QUERIES).join("acme.sql");
    let link = QueryLink {
        server: "acme-db".into(),
        database: Some("vendas".into()),
    };
    std::fs::write(&file, link.apply("select 1;\n")).unwrap();
    let mut h = harness(app);
    assert!(h.state_mut().open_file(&file));
    let start = Instant::now();
    while h.state().connection().map(|c| c.settings.host.as_str()) != Some("acme-db") {
        h.step();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "connect to acme-db"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.state().current_workspace().unwrap().name(), "time");
}

#[test]
fn use_a_folder_opens_a_workspace_or_starts_one() {
    let base = temp_dir("folder");
    let acme = workspace(&base, "acme", "acme-db");
    let novo = base.join("novo");
    std::fs::create_dir_all(&novo).unwrap();
    let mut app = app(Dialogs::default(), None);
    assert!(app.add_workspace_folder(&acme));
    assert!(app.add_workspace_folder(&novo));
    assert!(novo.join(MANIFEST).exists());
    assert_eq!(
        names(&app),
        [("acme".to_string(), false), ("novo".to_string(), true)]
    );
}

#[test]
fn the_panel_shows_a_card_per_workspace_and_switches_on_click() {
    let base = temp_dir("cards");
    let acme = workspace(&base, "acme", "acme-db");
    let time = workspace(&base, "time", "time-db");
    let mut app = app(Dialogs::default(), None);
    app.open_workspace(&acme);
    app.open_workspace(&time);
    for i in 0..2 {
        app.workspace_panel_mut()
            .source_control_at_mut(i)
            .unwrap()
            .wait();
    }
    // Sem git, o cartão diz que o workspace fica só neste computador.
    let summary = app
        .workspace_panel()
        .source_control_at(0)
        .unwrap()
        .summary();
    assert_eq!(summary.kind, SyncKind::Local);
    let mut h = harness(app);
    h.get_by_label("Workspaces (2)");
    // O servidor do ativo aparece; o do outro, só depois de trocar.
    assert!(h.query_all_by_label_contains("time-db").count() >= 1);
    assert_eq!(h.query_all_by_label_contains("acme-db").count(), 0);
    h.state_mut().activate_workspace(0);
    h.run();
    assert!(h.query_all_by_label_contains("acme-db").count() >= 1);
    assert_eq!(h.state().current_workspace().unwrap().name(), "acme");

    // "Add" abre o diálogo com as duas formas; sem a URL, ele explica.
    h.get_by_label_contains("Add").click();
    h.run();
    h.get_by_label("Add Workspace");
    assert!(!h.state_mut().workspace_panel_mut().confirm_clone());
    h.run();
    h.get_by_label("Enter the repository URL.");
    h.state_mut()
        .workspace_panel_mut()
        .set_clone_url("git@example.com:a/b.git");
    h.state_mut().workspace_panel_mut().set_clone_folder(&base);
    assert!(h.state_mut().workspace_panel_mut().confirm_clone());
    assert_eq!(
        h.state_mut().workspace_panel_mut().take_clone_request(),
        Some(("git@example.com:a/b.git".to_string(), base.clone()))
    );
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git")
        .status
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn the_card_says_what_is_pending_and_synchronize_pulls_then_pushes() {
    let base = temp_dir("sync");
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]);
    let root = workspace(&base, "acme", "acme-db");
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "a"]);
    git(&root, &["config", "user.email", "a@example.com"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "Primeiro"]);
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&root, &["push", "-u", "origin", "main"]);
    // Outra máquina envia um commit.
    let other = base.join("outra");
    git(&base, &["clone", remote.to_str().unwrap(), "outra"]);
    git(&other, &["config", "user.name", "b"]);
    git(&other, &["config", "user.email", "b@example.com"]);
    std::fs::write(other.join(QUERIES).join("da-outra.sql"), "select 2;\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-m", "Da outra"]);
    git(&other, &["push"]);
    // Aqui: um commit a enviar e uma alteração a salvar.
    std::fs::write(root.join(QUERIES).join("daqui.sql"), "select 3;\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "Daqui"]);
    git(&root, &["fetch"]);
    std::fs::write(root.join(QUERIES).join("rascunho.sql"), "select 4;\n").unwrap();

    let mut app = app(Dialogs::default(), None);
    app.open_workspace(&root);
    let sc = app.workspace_panel_mut().source_control_mut().unwrap();
    sc.wait();
    let summary = sc.summary();
    assert_eq!(summary.kind, SyncKind::Pending);
    assert_eq!(summary.text, "1 change · 1 to send · 1 to receive");

    // Sincronizar recebe o commit da outra máquina e envia o daqui; a
    // alteração não salva continua esperando.
    sc.sync();
    sc.wait();
    assert_eq!(sc.take_message().as_deref(), Some("Pushed."));
    assert!(root.join(QUERIES).join("da-outra.sql").exists());
    let s = sc.status().unwrap();
    assert_eq!((s.ahead, s.behind, s.changes.len()), (0, 0, 1));
    assert_eq!(sc.summary().text, "1 change");
    git(&other, &["pull"]);
    assert!(other.join(QUERIES).join("daqui.sql").exists());

    // Sem nada a enviar, só recebe.
    sc.sync();
    sc.wait();
    assert_eq!(sc.take_message().as_deref(), Some("Synchronized."));
}
