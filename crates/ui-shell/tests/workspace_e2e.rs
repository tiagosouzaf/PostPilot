//! Teste de ponta a ponta da Fase R: a máquina A registra um servidor no
//! workspace colando a connection string, guarda a senha no cofre e envia
//! tudo para o remoto; a máquina B clona pelo PostPilot, destranca com a
//! senha mestra e conecta sem digitar a senha do banco. Em nenhum commit do
//! histórico a senha aparece em texto puro.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServers;
use pgstudio_ui_shell::repo::vault::{Kdf, Vault};
use pgstudio_ui_shell::repo::{QUERIES, SERVERS};
use pgstudio_ui_shell::secrets::MemoryStore;

const DB_PASSWORD: &str = "Sup3r-s3nh4-do-banco";
const MASTER: &str = "a senha mestra do time";
const FAST: Kdf = Kdf {
    m_cost: 8 * 1024,
    t_cost: 1,
    p_cost: 1,
};

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if password != DB_PASSWORD {
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

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-r5-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn identity(dir: &Path, name: &str) {
    git(dir, &["config", "user.name", name]);
    git(
        dir,
        &["config", "user.email", &format!("{name}@example.com")],
    );
}

fn machine(system: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_secret_store(system)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(RegisteredServers::in_memory(Default::default()))
        .with_connector(Arc::new(FakeConnector));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    if let Some(cancel) = harness.query_by_label("Cancel") {
        cancel.click();
        harness.run();
    }
    harness
}

fn run_until(harness: &mut Harness<'static, ShellApp>, what: &str, ok: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    while !ok(harness.state()) {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run();
}

#[test]
fn register_push_clone_and_connect_without_typing_the_password() {
    let base = temp_dir("e2e");
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare"]);
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // Máquina A: workspace novo, senhas do workspace ligadas.
    let system_a = Arc::new(MemoryStore::default());
    let mut a = machine(system_a.clone());
    let root_a = base.join("a").join("vendas");
    assert!(a.state_mut().create_workspace(&root_a));
    {
        let p = a.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.set_kdf(FAST);
        p.create(MASTER, MASTER, false).unwrap();
        p.wait(None);
        assert!(p.is_unlocked());
    }
    a.run();

    // Registra o servidor colando a connection string.
    a.state_mut().new_workspace_server();
    a.run();
    {
        let d = a
            .state_mut()
            .workspace_panel_mut()
            .servers_mut()
            .unwrap()
            .server_dialog_mut()
            .unwrap();
        d.connection_string =
            format!("postgresql://alice:{DB_PASSWORD}@sales-db:5432/vendas?sslmode=prefer");
        d.apply_connection_string().unwrap();
        d.name = "Produção".into();
    }
    a.run();
    a.get_by_label("Save").click();
    a.run();
    let start = Instant::now();
    while Vault::at(&root_a).names().is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "password in vault"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::write(
        root_a.join(QUERIES).join("total.sql"),
        "-- postpilot: server=Produção; database=vendas\nselect count(*) from pedidos;\n",
    )
    .unwrap();

    // Git: init, remoto e Commit and Push.
    {
        let sc = a
            .state_mut()
            .workspace_panel_mut()
            .source_control_mut()
            .unwrap();
        sc.wait();
        sc.init();
        sc.wait();
        identity(&root_a, "a");
        sc.set_remote(remote.to_str().unwrap());
        sc.wait();
        sc.commit("Workspace de vendas", true);
        sc.wait();
        assert_eq!(sc.take_message().as_deref(), Some("Pushed."));
        assert!(sc.status().unwrap().is_clean());
    }
    let tracked = git(&root_a, &["ls-files"]);
    for f in [SERVERS, "secrets.json", "queries/total.sql"] {
        assert!(
            tracked.lines().any(|l| l == f),
            "{f} not committed:\n{tracked}"
        );
    }
    assert!(system_a.is_empty());
    drop(a);

    // Nenhum commit do remoto tem a senha (nem a mestra) em texto puro.
    let history = git(&remote, &["log", "--all", "-p", "--format=%H %s"]);
    assert!(history.contains("secrets.json"));
    assert!(!history.contains(DB_PASSWORD));
    assert!(!history.contains(MASTER));

    // Máquina B: clona pelo PostPilot.
    let system_b = Arc::new(MemoryStore::default());
    let mut b = machine(system_b.clone());
    let parent = base.join("b");
    std::fs::create_dir_all(&parent).unwrap();
    b.state_mut()
        .clone_workspace(remote.to_str().unwrap(), &parent);
    let result = b.state_mut().workspace_panel_mut().wait_clone().unwrap();
    b.state_mut().finish_clone(result);
    b.run();
    let root_b = b.state().current_workspace().unwrap().root().to_path_buf();
    assert_eq!(root_b, parent.join("remote"));

    // Pede a senha mestra uma vez.
    b.get_by_label("Unlock Workspace Passwords");
    {
        let p = b.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.unlock(MASTER, false);
        p.wait(None);
        assert!(p.is_unlocked());
    }
    b.run();

    // Abrir a consulta conecta no servidor dela, com a senha do cofre.
    b.state_mut()
        .open_file(&root_b.join(QUERIES).join("total.sql"));
    run_until(&mut b, "connected through the vault", |app| {
        app.connection().is_some()
    });
    let c = b.state().connection().unwrap();
    assert_eq!(
        (c.settings.host.as_str(), c.settings.user.as_str()),
        ("sales-db", "alice")
    );
    assert!(system_b.is_empty());
    let _ = std::fs::remove_dir_all(base);
}
