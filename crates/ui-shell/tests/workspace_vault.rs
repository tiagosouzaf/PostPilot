//! Testes do marco R.3: senhas do workspace criptografadas com a senha
//! mestra, indo e voltando entre duas "máquinas" (dois cofres do sistema)
//! pela mesma pasta, como depois de um clone.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServer;
use pgstudio_ui_shell::repo::passwords::{DialogKind, remembered_key};
use pgstudio_ui_shell::repo::vault::{self, Kdf, Vault};
use pgstudio_ui_shell::repo::{SERVERS, Workspace};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore};

const DB_PASSWORD: &str = "s3nh4-do-banco";
const MASTER: &str = "minha senha mestra";
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
    let dir = std::env::temp_dir().join(format!("postpilot-r3-{name}-{}", std::process::id()));
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

/// Workspace com o servidor "Produção".
fn workspace(base: &Path) -> PathBuf {
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    let mut servers = Workspace::open(&root).unwrap().servers();
    servers
        .add_server(
            &[],
            RegisteredServer {
                name: "Produção".into(),
                description: String::new(),
                settings: prod(),
            },
        )
        .unwrap();
    assert!(root.join(SERVERS).exists());
    root
}

/// Uma "máquina": o app com o cofre do sistema dela.
fn machine(system: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_secret_store(system)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector));
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    harness.run();
    harness.get_by_label("Cancel").click();
    harness.run();
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

fn open(harness: &mut Harness<'static, ShellApp>, root: &Path) {
    assert!(harness.state_mut().open_workspace(root));
    harness
        .state_mut()
        .workspace_panel_mut()
        .passwords_mut()
        .unwrap()
        .set_kdf(FAST);
    harness.run();
}

fn vault_names(root: &Path) -> Vec<String> {
    Vault::at(root).names()
}

#[test]
fn password_goes_encrypted_to_the_repository_and_back() {
    let base = temp_dir("roundtrip");
    let root = workspace(&base);

    // Máquina A: liga as senhas no workspace e conecta lembrando a senha.
    let system_a = Arc::new(MemoryStore::default());
    let mut a = machine(system_a.clone());
    open(&mut a, &root);
    assert_eq!(
        a.state().workspace_panel().passwords().unwrap().dialog(),
        None
    );
    {
        let p = a.state_mut().workspace_panel_mut().passwords_mut().unwrap();
        assert!(p.create("curta", "curta", true).is_err());
        p.create(MASTER, MASTER, true).unwrap();
    }
    run_until(&mut a, "vault created", |app| {
        app.workspace_panel().passwords().unwrap().is_unlocked()
    });
    let ctx = a.ctx.clone();
    a.state_mut()
        .execute(&ctx, CommandId::ConnectObjectExplorer);
    a.run();
    {
        let d = a.state_mut().connect_dialog_mut().unwrap();
        d.server = "sales-db".into();
        d.user = "alice".into();
        d.password = DB_PASSWORD.into();
        d.remember = true;
    }
    a.run();
    a.get_by_label("Connect").click();
    run_until(&mut a, "connected", |app| app.connection().is_some());
    let start = Instant::now();
    while vault_names(&root).is_empty() {
        assert!(start.elapsed() < Duration::from_secs(5), "password saved");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(vault_names(&root), ["alice@sales-db:5432"]);
    // A senha do banco não ficou no cofre do sistema, nem em texto puro
    // em nenhum arquivo do workspace.
    assert_eq!(system_a.get("alice@sales-db:5432").unwrap(), None);
    for entry in walk(&root) {
        let bytes = std::fs::read(&entry).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(DB_PASSWORD), "{}", entry.display());
        assert!(!text.contains(MASTER), "{}", entry.display());
    }
    // A senha mestra ficou lembrada só nesta máquina.
    let id = Vault::at(&root).id().unwrap();
    assert_eq!(
        system_a.get(&remembered_key(&id)).unwrap().as_deref(),
        Some(MASTER)
    );
    drop(a);

    // Máquina B (o clone): pede a senha mestra uma vez.
    let system_b = Arc::new(MemoryStore::default());
    let mut b = machine(system_b.clone());
    open(&mut b, &root);
    assert_eq!(
        b.state().workspace_panel().passwords().unwrap().dialog(),
        Some(DialogKind::Unlock)
    );
    b.get_by_label("Unlock Workspace Passwords");
    b.state_mut()
        .workspace_panel_mut()
        .passwords_mut()
        .unwrap()
        .unlock("errada errada", true);
    run_until(&mut b, "wrong password checked", |app| {
        !app.workspace_panel().passwords().unwrap().is_busy()
    });
    assert!(
        !b.state()
            .workspace_panel()
            .passwords()
            .unwrap()
            .is_unlocked()
    );
    assert!(b.query_all_by_label_contains("not correct").count() >= 1);
    b.state_mut()
        .workspace_panel_mut()
        .passwords_mut()
        .unwrap()
        .unlock(MASTER, true);
    run_until(&mut b, "unlocked", |app| {
        app.workspace_panel().passwords().unwrap().is_unlocked()
    });
    assert_eq!(
        b.state().workspace_panel().passwords().unwrap().dialog(),
        None
    );
    // Conectar o servidor do workspace não pede a senha do banco.
    b.state_mut().connect_registered(&prod(), false);
    run_until(&mut b, "connected with the vault password", |app| {
        app.connection().is_some()
    });
    assert_eq!(system_b.get("alice@sales-db:5432").unwrap(), None);
    drop(b);

    // B reinicia: destranca sozinho com a senha mestra lembrada.
    let mut b = machine(system_b);
    open(&mut b, &root);
    run_until(&mut b, "auto unlocked", |app| {
        app.workspace_panel().passwords().unwrap().is_unlocked()
    });
    assert_eq!(
        b.state().workspace_panel().passwords().unwrap().dialog(),
        None
    );
}

#[test]
fn servers_outside_the_workspace_keep_using_the_system_keyring() {
    let base = temp_dir("outside");
    let root = workspace(&base);
    let system = Arc::new(MemoryStore::default());
    let mut a = machine(system.clone());
    open(&mut a, &root);
    a.state_mut()
        .workspace_panel_mut()
        .passwords_mut()
        .unwrap()
        .create(MASTER, MASTER, false)
        .unwrap();
    run_until(&mut a, "vault created", |app| {
        app.workspace_panel().passwords().unwrap().is_unlocked()
    });
    // Sem "lembrar", a senha mestra não vai para o cofre do sistema.
    assert!(system.is_empty());
    let ctx = a.ctx.clone();
    a.state_mut()
        .execute(&ctx, CommandId::ConnectObjectExplorer);
    a.run();
    {
        let d = a.state_mut().connect_dialog_mut().unwrap();
        d.server = "outro-db".into();
        d.user = "alice".into();
        d.password = DB_PASSWORD.into();
        d.remember = true;
    }
    a.run();
    a.get_by_label("Connect").click();
    run_until(&mut a, "connected", |app| app.connection().is_some());
    let start = Instant::now();
    while system.get("alice@outro-db:5432").unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5), "saved in keyring");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(vault_names(&root).is_empty());
}

#[test]
fn lock_and_change_master_password() {
    let base = temp_dir("change");
    let root = workspace(&base);
    let mut v = Vault::at(&root);
    v.create(MASTER, FAST).unwrap();
    v.set("alice@sales-db:5432", DB_PASSWORD).unwrap();

    let system = Arc::new(MemoryStore::default());
    let mut a = machine(system.clone());
    open(&mut a, &root);
    let p = a.state_mut().workspace_panel_mut().passwords_mut().unwrap();
    p.unlock(MASTER, false);
    p.wait(None);
    assert!(p.is_unlocked());
    assert_eq!(p.count(), 1);
    assert!(p.change("nova", "nova", false).is_err());
    p.change("nova senha mestra", "nova senha mestra", true)
        .unwrap();
    let store: Arc<dyn SecretStore> = system.clone();
    p.wait(Some(&store));
    assert!(p.is_unlocked());
    p.lock();
    assert!(!p.is_unlocked());
    let mut check = Vault::at(&root);
    assert_eq!(check.unlock(MASTER), Err(vault::VaultError::WrongPassword));
    check.unlock("nova senha mestra").unwrap();
    assert_eq!(
        check.get("alice@sales-db:5432").unwrap().as_deref(),
        Some(DB_PASSWORD)
    );
    let id = check.id().unwrap();
    assert_eq!(
        system.get(&remembered_key(&id)).unwrap().as_deref(),
        Some("nova senha mestra")
    );
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
