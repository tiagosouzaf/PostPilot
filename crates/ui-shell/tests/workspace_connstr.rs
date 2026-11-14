//! Testes do marco R.4: connection string colada no registro de um servidor
//! do workspace e a importação dos Registered Servers desta máquina (com as
//! senhas lembradas) para o workspace. Em nenhum caso a senha aparece em
//! texto puro num arquivo do repositório.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectionSettings, SslMode};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::panels::Panel;
use pgstudio_ui_shell::registered::{RegisteredServer, RegisteredServers, ServerGroup};
use pgstudio_ui_shell::repo::vault::{Kdf, Vault};
use pgstudio_ui_shell::repo::{SERVERS, Workspace};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore};

const DB_PASSWORD: &str = "p@ss:w0rd-do-banco";
const MASTER: &str = "minha senha mestra";
const FAST: Kdf = Kdf {
    m_cost: 8 * 1024,
    t_cost: 1,
    p_cost: 1,
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-r4-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app(system: Arc<MemoryStore>, local: RegisteredServers) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_secret_store(system)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_registered_servers(local);
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    // O "Connect to Server" do início, se apareceu.
    if let Some(cancel) = harness.query_by_label("Cancel") {
        cancel.click();
        harness.run();
    }
    harness
}

/// Abre um workspace novo, com as senhas do workspace ligadas (ou não).
fn open_workspace(harness: &mut Harness<'static, ShellApp>, base: &Path, vault: bool) -> PathBuf {
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    assert!(harness.state_mut().open_workspace(&root));
    harness.run();
    if !harness.state().is_panel_open(Panel::WorkspaceExplorer) {
        let ctx = harness.ctx.clone();
        harness
            .state_mut()
            .execute(&ctx, CommandId::WorkspaceExplorer);
        harness.run();
    }
    if vault {
        let p = harness
            .state_mut()
            .workspace_panel_mut()
            .passwords_mut()
            .unwrap();
        p.set_kdf(FAST);
        p.create(MASTER, MASTER, false).unwrap();
        p.wait(None);
        assert!(p.is_unlocked());
    }
    harness.run();
    root
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Nenhum arquivo do workspace (nem o `.git`) tem a senha em texto puro,
/// nem codificada de um jeito óbvio.
fn assert_no_plain_password(root: &Path) {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(DB_PASSWORD);
    let escaped = DB_PASSWORD.replace('@', "%40").replace(':', "%3A");
    for file in walk(root) {
        let bytes = std::fs::read(&file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for needle in [DB_PASSWORD, &b64, &escaped, MASTER] {
            assert!(!text.contains(needle), "{needle} in {}", file.display());
        }
    }
}

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn pasted_connection_string_fills_the_dialog_and_the_password_goes_to_the_vault() {
    let base = temp_dir("paste");
    let system = Arc::new(MemoryStore::default());
    let mut h = app(
        system.clone(),
        RegisteredServers::in_memory(Default::default()),
    );
    let root = open_workspace(&mut h, &base, true);

    // O mesmo que botão direito na raiz > New Server Registration.
    h.state_mut().new_workspace_server();
    h.run();
    h.get_by_label("New Server Registration");
    {
        let d = h
            .state_mut()
            .workspace_panel_mut()
            .servers_mut()
            .unwrap()
            .server_dialog_mut()
            .expect("New Server Registration");
        d.connection_string = "postgres://nope@".into();
        assert!(d.apply_connection_string().is_ok());
        d.connection_string = "postgresql://h:abc".into();
        assert!(d.apply_connection_string().is_err());
        assert!(d.connection_error().is_some());
        d.connection_string =
            "postgresql://app:p%40ss%3Aw0rd-do-banco@db.vendas.com:6432/vendas?sslmode=require"
                .into();
        d.apply_connection_string().unwrap();
        assert_eq!(d.server, "db.vendas.com,6432");
        assert_eq!(d.user, "app");
        assert_eq!(d.database, "vendas");
        assert_eq!(d.ssl, SslMode::Require);
        assert_eq!(d.password, DB_PASSWORD);
        assert!(d.remember);
        assert!(d.connection_string.is_empty());
        assert!(d.connection_error().is_none());
        d.name = "Vendas".into();
    }
    h.run();
    h.get_by_label("Save").click();
    h.run();

    // O servidor está no servers.json, sem a senha; a senha está no cofre.
    let servers = RegisteredServers::open(root.join(SERVERS));
    let s = &servers.root().servers[0];
    assert_eq!(s.name, "Vendas");
    assert_eq!(s.settings.host, "db.vendas.com");
    assert_eq!(s.settings.port, 6432);
    wait_for("password in the vault", || {
        !Vault::at(&root).names().is_empty()
    });
    assert_eq!(Vault::at(&root).names(), ["app@db.vendas.com:6432"]);
    let mut v = Vault::at(&root);
    v.unlock(MASTER).unwrap();
    assert_eq!(
        v.get("app@db.vendas.com:6432").unwrap().as_deref(),
        Some(DB_PASSWORD)
    );
    // Nem no cofre do sistema (ela é do workspace), nem em texto puro.
    assert!(system.is_empty());
    assert_no_plain_password(&root);
    let _ = std::fs::remove_dir_all(base);
}

fn local_servers() -> RegisteredServers {
    let mut r = RegisteredServers::in_memory(ServerGroup::default());
    r.add_server(
        &[],
        RegisteredServer {
            name: "Produção".into(),
            description: String::new(),
            settings: ConnectionSettings {
                host: "sales-db".into(),
                user: "alice".into(),
                ..Default::default()
            },
        },
    )
    .unwrap();
    let group = r
        .add_group(
            &[],
            ServerGroup {
                name: "Homologação".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(group, [0]);
    r.add_server(
        &[0],
        RegisteredServer {
            name: "QA".into(),
            description: String::new(),
            settings: ConnectionSettings {
                host: "qa-db".into(),
                user: "bob".into(),
                ..Default::default()
            },
        },
    )
    .unwrap();
    r
}

#[test]
fn import_registered_servers_brings_their_passwords_into_the_vault() {
    let base = temp_dir("import");
    let system = Arc::new(MemoryStore::default());
    system.set("alice@sales-db:5432", DB_PASSWORD).unwrap();
    let mut h = app(system.clone(), local_servers());
    let root = open_workspace(&mut h, &base, true);

    h.get_by_label_contains("Import from this computer").click();
    h.run();
    // A cópia das senhas termina sozinha e avisa na status bar.
    let start = Instant::now();
    while h.state().workspace_panel().is_copying() {
        assert!(start.elapsed() < Duration::from_secs(5), "copy finished");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run();
    let status = h.state().status().to_string();
    assert!(status.contains("Copied 1 saved password"), "{status}");

    let servers = RegisteredServers::open(root.join(SERVERS));
    let names: Vec<_> = servers
        .all_servers()
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, ["QA", "Produção"]);
    assert_eq!(servers.root().groups[0].name, "Homologação");
    assert_eq!(Vault::at(&root).names(), ["alice@sales-db:5432"]);
    // O cofre do sistema continua com a dele (é uma cópia).
    assert_eq!(
        system.get("alice@sales-db:5432").unwrap().as_deref(),
        Some(DB_PASSWORD)
    );
    assert_no_plain_password(&root);

    // Importar de novo não duplica nada.
    let again = h.state_mut().import_registered_into_workspace().unwrap();
    assert!(again.contains("Imported 2 servers"), "{again}");
    h.state_mut().workspace_panel_mut().wait_copy();
    let servers = RegisteredServers::open(root.join(SERVERS));
    assert_eq!(servers.all_servers().len(), 2);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn import_without_workspace_passwords_brings_only_the_servers() {
    let base = temp_dir("import-locked");
    let system = Arc::new(MemoryStore::default());
    system.set("alice@sales-db:5432", DB_PASSWORD).unwrap();
    let mut h = app(system.clone(), local_servers());
    let root = open_workspace(&mut h, &base, false);
    let msg = h.state_mut().import_registered_into_workspace().unwrap();
    assert!(msg.contains("unlock the workspace passwords"), "{msg}");
    assert!(!h.state().workspace_panel().is_copying());
    assert_eq!(
        RegisteredServers::open(root.join(SERVERS))
            .all_servers()
            .len(),
        2
    );
    assert!(!Vault::at(&root).exists());
    assert_no_plain_password(&root);

    // Sem servidores locais, nada a importar.
    let mut empty = app(
        Arc::new(MemoryStore::default()),
        RegisteredServers::in_memory(Default::default()),
    );
    open_workspace(&mut empty, &base.join("other"), false);
    assert!(
        empty
            .state_mut()
            .import_registered_into_workspace()
            .is_err()
    );
    let _ = std::fs::remove_dir_all(base);
}
