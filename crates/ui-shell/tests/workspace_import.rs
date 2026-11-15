//! Levar as conexões junto no workspace: quem já usava o PostPilot com uma
//! conexão local (digitada no "Connect to Server", sem registrar) cria um
//! workspace, aceita trazer as conexões, escolhe a senha mestra e envia. Na
//! outra máquina, o clone traz o servidor e a senha dele.
//!
//! Antes, o workspace novo nascia com o `servers.json` vazio, nada avisava,
//! e o clone abria sem nenhuma conexão.

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
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers};
use pgstudio_ui_shell::repo::passwords::DialogKind;
use pgstudio_ui_shell::repo::vault::{Kdf, Vault};
use pgstudio_ui_shell::repo::{SERVERS, Workspace};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore};

const DB_PASSWORD: &str = "S3nha-do-rds-que-nao-vaza";
const MASTER: &str = "a senha mestra do tiago";
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
    let dir = std::env::temp_dir().join(format!("postpilot-import-{name}-{}", std::process::id()));
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

fn machine(
    system: Arc<MemoryStore>,
    recent: Vec<ConnectionSettings>,
    local: RegisteredServers,
) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_secret_store(system)
        .with_recent_connections(RecentConnections::in_memory(recent))
        .with_registered_servers(local)
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

fn rds() -> ConnectionSettings {
    ConnectionSettings {
        host: "prod.abc123.us-east-1.rds.amazonaws.com".into(),
        user: "tiago".into(),
        database: "vendas".into(),
        ..Default::default()
    }
}

fn server_names(root: &Path) -> Vec<String> {
    RegisteredServers::open(root.join(SERVERS))
        .all_servers()
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

#[test]
fn new_workspace_takes_the_local_connection_and_the_clone_brings_it() {
    let base = temp_dir("clone");
    let remote = base.join("sql.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare"]);
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // Máquina A (Linux): conectada pelo "Connect to Server", sem registrar
    // o servidor e sem "Remember password".
    let system_a = Arc::new(MemoryStore::default());
    let mut a = machine(
        system_a.clone(),
        Vec::new(),
        RegisteredServers::in_memory(Default::default()),
    );
    a.state_mut().connect_to(rds(), DB_PASSWORD.into());
    run_until(&mut a, "connected", |app| app.connection().is_some());
    assert!(system_a.is_empty());

    // Um repositório que já tinha consultas fora de `queries/`.
    let root_a = base.join("a").join("sql");
    std::fs::create_dir_all(root_a.join("Querys")).unwrap();
    std::fs::write(root_a.join("Querys").join("clientes.sql"), "select 1;\n").unwrap();

    // New Workspace: oferece trazer a conexão.
    assert!(a.state_mut().create_workspace(&root_a));
    a.run();
    assert_eq!(a.state().workspace_panel().import_offer(), Some(1));
    a.get_by_label("Bring your connections?");
    assert!(server_names(&root_a).is_empty());
    a.get_by_label("Import connections").click();
    a.run();
    a.run();
    assert_eq!(
        server_names(&root_a),
        ["prod.abc123.us-east-1.rds.amazonaws.com"]
    );

    // Sem cofre ainda: a senha mestra é pedida e a senha em uso vai para o
    // cofre assim que ele existe.
    {
        let panel = a.state_mut().workspace_panel_mut();
        assert!(panel.has_pending_copy());
        let p = panel.passwords_mut().unwrap();
        assert_eq!(p.dialog(), Some(DialogKind::Create));
        p.set_kdf(FAST);
        p.create(MASTER, MASTER, false).unwrap();
        p.wait(None);
        assert!(p.is_unlocked());
    }
    run_until(&mut a, "password copied to the vault", |app| {
        !app.workspace_panel().has_pending_copy() && !app.workspace_panel().is_copying()
    });
    let key = "tiago@prod.abc123.us-east-1.rds.amazonaws.com:5432";
    assert_eq!(Vault::at(&root_a).names(), [key]);
    let status = a.state().status().to_string();
    assert!(status.contains("Copied 1 saved password"), "{status}");
    assert!(status.contains("Commit and Push"), "{status}");
    // O aviso some: a conexão ativa já está no workspace.
    assert_eq!(a.state().importable_connections(), 0);
    assert!(a.state().workspace_panel().unsaved_current.is_none());

    // A pasta antiga de consultas aparece na árvore.
    let tree = a.state().workspace_panel().queries().unwrap().clone();
    assert!(tree.folders.iter().any(|f| f.name == "Querys"), "{tree:?}");

    // Commit and Push.
    {
        let sc = a
            .state_mut()
            .workspace_panel_mut()
            .source_control_mut()
            .unwrap();
        sc.wait();
        sc.init();
        sc.wait();
        git(&root_a, &["config", "user.name", "a"]);
        git(&root_a, &["config", "user.email", "a@example.com"]);
        sc.set_remote(remote.to_str().unwrap());
        sc.wait();
        sc.commit("Workspace sql", true);
        sc.wait();
        assert_eq!(sc.take_message().as_deref(), Some("Pushed."));
    }
    drop(a);
    let history = git(&remote, &["log", "--all", "-p", "--format=%H %s"]);
    assert!(history.contains("rds.amazonaws.com"));
    assert!(!history.contains(DB_PASSWORD));
    assert!(!history.contains(MASTER));

    // Máquina B (Windows): o git dela converte as quebras de linha.
    let system_b = Arc::new(MemoryStore::default());
    let mut b = machine(
        system_b.clone(),
        Vec::new(),
        RegisteredServers::in_memory(Default::default()),
    );
    let root_b = base.join("b").join("sql");
    std::fs::create_dir_all(root_b.parent().unwrap()).unwrap();
    let out = Command::new("git")
        .args(["-c", "core.autocrlf=true", "clone"])
        .arg(&remote)
        .arg(&root_b)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = std::fs::read_to_string(root_b.join(SERVERS)).unwrap();
    assert!(text.contains("\r\n"), "clone with CRLF");
    assert!(b.state_mut().open_workspace(&root_b));
    b.run();
    assert_eq!(
        server_names(&root_b),
        ["prod.abc123.us-east-1.rds.amazonaws.com"]
    );
    // Abrir um workspace que já tem as conexões não pergunta nada.
    assert_eq!(b.state().workspace_panel().import_offer(), None);
    b.get_by_label("Unlock Workspace Passwords");
    {
        let p = b.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.unlock(MASTER, false);
        p.wait(None);
        assert!(p.is_unlocked());
    }
    b.run();
    assert_eq!(Vault::at(&root_b).names(), [key]);

    // Conecta pelo servidor do workspace, sem digitar a senha do banco.
    let settings = RegisteredServers::open(root_b.join(SERVERS)).all_servers()[0]
        .settings
        .clone();
    b.state_mut().connect_registered(&settings, false);
    run_until(&mut b, "connected through the vault", |app| {
        app.connection().is_some()
    });
    assert!(system_b.is_empty());
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn not_now_keeps_the_workspace_empty_but_the_panel_keeps_offering() {
    let base = temp_dir("not-now");
    let system = Arc::new(MemoryStore::default());
    // Uma conexão recente com a senha lembrada e um servidor registrado
    // para o mesmo host: contam uma vez só.
    let mut local = RegisteredServers::in_memory(Default::default());
    local
        .add_server(
            &[],
            RegisteredServer {
                name: "Produção".into(),
                settings: rds(),
                ..Default::default()
            },
        )
        .unwrap();
    let other = ConnectionSettings {
        host: "localhost".into(),
        user: "postgres".into(),
        ..Default::default()
    };
    system.set("postgres@localhost:5432", DB_PASSWORD).unwrap();
    let mut h = machine(system.clone(), vec![rds(), other], local);
    let root = base.join("ws");
    assert!(h.state_mut().create_workspace(&root));
    h.run();
    assert_eq!(h.state().workspace_panel().import_offer(), Some(2));
    h.get_by_label("Not now").click();
    h.run();
    assert!(server_names(&root).is_empty());
    assert!(!h.state().workspace_panel().has_pending_copy());

    // O painel continua oferecendo, com o botão grande.
    assert_eq!(h.state().workspace_panel().importable, 2);
    h.get_by_label_contains("Import the 2 connections from this computer")
        .click();
    h.run();
    h.run();
    assert_eq!(server_names(&root), ["Produção", "localhost"]);
    {
        let p = h.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        assert_eq!(p.dialog(), Some(DialogKind::Create));
        p.set_kdf(FAST);
        p.create(MASTER, MASTER, false).unwrap();
        p.wait(None);
    }
    run_until(&mut h, "passwords copied", |app| {
        !app.workspace_panel().has_pending_copy() && !app.workspace_panel().is_copying()
    });
    assert_eq!(Vault::at(&root).names(), ["postgres@localhost:5432"]);
    assert_eq!(h.state().importable_connections(), 0);

    // Importar de novo não duplica.
    h.state_mut().import_registered_into_workspace().unwrap();
    h.state_mut().workspace_panel_mut().wait_copy();
    assert_eq!(server_names(&root), ["Produção", "localhost"]);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn connected_server_outside_the_workspace_can_be_added_from_the_panel() {
    let base = temp_dir("add-current");
    let system = Arc::new(MemoryStore::default());
    let mut h = machine(
        system,
        Vec::new(),
        RegisteredServers::in_memory(Default::default()),
    );
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    assert!(h.state_mut().open_workspace(&root));
    {
        let p = h.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.set_kdf(FAST);
        p.create(MASTER, MASTER, false).unwrap();
        p.wait(None);
    }
    // Já há um servidor no workspace: o botão grande não aparece.
    let other = ConnectionSettings {
        host: "localhost".into(),
        user: "postgres".into(),
        ..Default::default()
    };
    h.state_mut()
        .workspace_panel_mut()
        .servers_mut()
        .unwrap()
        .servers_mut()
        .add_server(
            &[],
            RegisteredServer {
                name: "Local".into(),
                settings: other,
                ..Default::default()
            },
        )
        .unwrap();
    h.state_mut().connect_to(rds(), DB_PASSWORD.into());
    run_until(&mut h, "connected", |app| app.connection().is_some());
    // Conectar traz o Object Explorer para a frente.
    let ctx = h.ctx.clone();
    h.state_mut()
        .execute(&ctx, pgstudio_core::CommandId::WorkspaceExplorer);
    h.run();
    h.get_by_label_contains("which is not in this workspace");
    h.get_by_label("Add to workspace").click();
    h.run();
    h.run();
    run_until(&mut h, "password copied", |app| {
        !app.workspace_panel().is_copying()
    });
    assert_eq!(
        server_names(&root),
        ["Local", "prod.abc123.us-east-1.rds.amazonaws.com"]
    );
    assert_eq!(
        Vault::at(&root).names(),
        ["tiago@prod.abc123.us-east-1.rds.amazonaws.com:5432"]
    );
    assert!(h.query_by_label("Add to workspace").is_none());
    let text = std::fs::read_to_string(root.join(SERVERS)).unwrap();
    assert!(!text.contains(DB_PASSWORD));
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn register_and_properties_on_the_connected_server_use_the_workspace() {
    let base = temp_dir("register");
    let system = Arc::new(MemoryStore::default());
    let mut h = machine(
        system.clone(),
        Vec::new(),
        RegisteredServers::in_memory(Default::default()),
    );
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    assert!(h.state_mut().open_workspace(&root));
    {
        let p = h.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        p.set_kdf(FAST);
        p.create(MASTER, MASTER, false).unwrap();
        p.wait(None);
    }
    // Uma pasta de produção no workspace.
    h.state_mut()
        .workspace_panel_mut()
        .servers_mut()
        .unwrap()
        .servers_mut()
        .add_group(
            &[],
            pgstudio_ui_shell::registered::ServerGroup {
                name: "Clientes".into(),
                ..Default::default()
            },
        )
        .unwrap();
    h.state_mut().connect_to(rds(), DB_PASSWORD.into());
    run_until(&mut h, "connected", |app| app.connection().is_some());
    h.run();

    // Botão direito no servidor: Properties já não fica apagado e abre o
    // registro no workspace, com a pasta para escolher.
    h.get_by_label_contains(pgstudio_object_explorer::fake::SERVER_NAME)
        .click_secondary();
    h.run();
    h.get_by_label("Properties").click();
    h.run();
    assert!(
        h.state()
            .is_panel_open(pgstudio_ui_shell::panels::Panel::WorkspaceExplorer)
    );
    h.get_by_label("Folder:");
    {
        let d = h
            .state_mut()
            .workspace_panel_mut()
            .servers_mut()
            .unwrap()
            .server_dialog_mut()
            .expect("New Server Registration in the workspace");
        assert_eq!(d.server, "prod.abc123.us-east-1.rds.amazonaws.com");
        assert_eq!(d.password, DB_PASSWORD);
        assert!(d.remember);
        d.name = "Produção".into();
        d.environment = Some("Production".into());
    }
    h.run();
    h.get_by_label("Save").click();
    h.run();
    let key = "tiago@prod.abc123.us-east-1.rds.amazonaws.com:5432";
    let start = Instant::now();
    while Vault::at(&root).names().is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "password in vault"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(Vault::at(&root).names(), [key]);
    assert!(system.is_empty());
    let saved = RegisteredServers::open(root.join(SERVERS));
    let server = saved.all_servers()[0].clone();
    assert_eq!(server.name, "Produção");
    assert_eq!(server.environment.as_deref(), Some("Production"));
    // Nada foi para os Registered Servers desta máquina.
    assert!(h.state().registered().servers().all_servers().is_empty());

    // O Object Explorer passa a mostrar o nome do registro com a etiqueta
    // do ambiente na frente, e o menu edita o registro.
    h.run();
    let label = h.state().explorer().server_label().cloned().unwrap();
    assert_eq!(label.name, "Produção");
    assert_eq!(label.environment.unwrap().0, "Production");
    let ctx = h.ctx.clone();
    h.state_mut()
        .execute(&ctx, pgstudio_core::CommandId::ObjectExplorer);
    h.run();
    assert_eq!(h.query_all_by_label("Production").count(), 2);
    // (A status bar também mostra o nome, seguido do usuário.)
    h.get_by(|n| n.label().is_some_and(|l| l.ends_with(" Produção")))
        .click_secondary();
    h.run();
    assert!(h.query_by_label("Register...").is_none());

    // De novo: agora edita o registro que já existe, em vez de duplicar.
    h.get_by_label("Edit Registration...").click();
    h.run();
    {
        let d = h
            .state_mut()
            .workspace_panel_mut()
            .servers_mut()
            .unwrap()
            .server_dialog_mut()
            .expect("edit the registration");
        assert_eq!(d.name, "Produção");
        d.environment = Some("Staging".into());
    }
    h.run();
    h.get_by_label("Save").click();
    h.run();
    let saved = RegisteredServers::open(root.join(SERVERS));
    assert_eq!(saved.all_servers().len(), 1);
    assert_eq!(
        saved.all_servers()[0].environment.as_deref(),
        Some("Staging")
    );
    let _ = std::fs::remove_dir_all(base);
}
