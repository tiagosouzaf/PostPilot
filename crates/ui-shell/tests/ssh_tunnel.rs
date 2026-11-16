//! Túnel SSH na interface, com um conector falso: os campos do "Connect to
//! Server", o segredo do túnel no cofre (do sistema e do workspace, cifrado
//! pela senha mestra) e a reconexão ao reabrir o app. O túnel de verdade é
//! testado em `crates/ssh/tests` e nos testes `tunnel` dos drivers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{
    ConnectError, Connected, ConnectionSettings, Connector, SshAuth, SshSettings,
};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::registered::RegisteredServer;
use pgstudio_ui_shell::repo::vault::{Kdf, Vault};
use pgstudio_ui_shell::repo::{SERVERS, Workspace};
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};
use pgstudio_ui_shell::session::SessionStore;

const DB_PASSWORD: &str = "s3nh4-do-banco";
const SSH_SECRET: &str = "frase-do-bastion";
const MASTER: &str = "minha senha mestra";
const FAST: Kdf = Kdf {
    m_cost: 8 * 1024,
    t_cost: 1,
    p_cost: 1,
};

/// O segredo digitado e a ligação com o cofre são do processo: um teste
/// por vez.
fn alone() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Conecta se a senha do banco confere e, havendo túnel, se o túnel teria
/// o segredo certo (o digitado ou o do cofre), como o túnel de verdade.
struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if password != DB_PASSWORD {
            return Err(ConnectError::Authentication("Login failed.".into()));
        }
        if let Some(ssh) = &s.ssh
            && pgstudio_ssh::secret(ssh).as_deref() != Some(SSH_SECRET)
        {
            return Err(ConnectError::Authentication(
                "The SSH server refused the password.".into(),
            ));
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
    let dir = std::env::temp_dir().join(format!("postpilot-ssh-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bastion(host: &str) -> SshSettings {
    SshSettings {
        host: host.into(),
        port: 22,
        user: "deploy".into(),
        auth: SshAuth::KeyFile,
        key_file: "~/.ssh/id_ed25519".into(),
    }
}

/// O banco de um cliente, num endereço interno, atrás do bastion dele.
fn client_db(bastion_host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: "10.0.0.5".into(),
        user: "app".into(),
        ssh: Some(bastion(bastion_host)),
        ..Default::default()
    }
}

fn app(store: Arc<MemoryStore>) -> ShellApp {
    ShellApp::default()
        .with_secret_store(store)
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
}

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    Harness::builder()
        .with_size([1280.0, 900.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
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

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Preenche o "Connect to Server" aberto com `settings` e os segredos.
fn fill(harness: &mut Harness<'static, ShellApp>, settings: &ConnectionSettings, secret: &str) {
    let ssh = settings.ssh.as_ref().expect("tunnel");
    let d = harness.state_mut().connect_dialog_mut().expect("dialog");
    d.server = settings.server_name();
    d.user = settings.user.clone();
    d.password = DB_PASSWORD.into();
    d.remember = true;
    d.ssh.enabled = true;
    d.ssh.server = ssh.server_name();
    d.ssh.user = ssh.user.clone();
    d.ssh.auth = ssh.auth;
    d.ssh.key_file = ssh.key_file.clone();
    d.ssh.secret = secret.into();
    harness.run();
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn no_secret_in(dir: &Path) {
    for file in walk(dir) {
        let bytes = std::fs::read(&file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for secret in [SSH_SECRET, DB_PASSWORD, MASTER] {
            assert!(!text.contains(secret), "{}", file.display());
        }
    }
}

#[test]
fn dialog_connects_through_the_tunnel_and_remembers_its_secret() {
    let _alone = alone();
    let settings = client_db("bastion.acme.example");
    let ssh = settings.ssh.clone().unwrap();
    pgstudio_ssh::set_secret(&ssh, "");
    let store = Arc::new(MemoryStore::default());
    let mut h = harness(app(store.clone()));
    h.run();

    // Desligado, só a caixa aparece; ligado, os campos do túnel.
    h.get_by_label("Connect through an SSH tunnel");
    assert_eq!(h.query_all_by_label("SSH server:").count(), 0);
    h.get_by_label("Connect through an SSH tunnel").click();
    h.run();
    for label in [
        "SSH server:",
        "SSH user:",
        "SSH authentication:",
        "Key file:",
        "Passphrase:",
    ] {
        h.get_by_label(label);
    }

    // Faltando o servidor SSH, o diálogo explica em vez de conectar.
    h.state_mut().connect_dialog_mut().unwrap().password = DB_PASSWORD.into();
    h.get_by_label("Connect").click();
    h.run();
    assert_eq!(
        h.state().connect_dialog().unwrap().error(),
        Some("Enter the SSH server of the tunnel.")
    );

    // A frase errada volta como erro do túnel, com o diálogo aberto.
    fill(&mut h, &settings, "errada");
    h.get_by_label("Connect").click();
    run_until(&mut h, "refused", |app| {
        app.connect_dialog().is_some_and(|d| d.error().is_some())
    });
    assert!(h.state().connection().is_none());

    fill(&mut h, &settings, SSH_SECRET);
    h.get_by_label("Connect").click();
    run_until(&mut h, "connected", |app| app.connection().is_some());
    assert_eq!(h.state().connection().unwrap().settings, settings);
    assert_eq!(h.state().recent_connections().list()[0], settings);

    // As duas senhas ficaram lembradas, cada uma na sua chave. O bastion
    // faz parte da chave do banco: outro cliente pode ter o mesmo
    // endereço interno.
    let db_key = password_key(&settings);
    assert_eq!(
        db_key,
        "app@10.0.0.5:5432 via deploy@bastion.acme.example:22"
    );
    wait_for("secrets saved", || store.len() == 2);
    assert_eq!(store.get(&db_key).unwrap().as_deref(), Some(DB_PASSWORD));
    assert_eq!(
        store
            .get("ssh:deploy@bastion.acme.example:22")
            .unwrap()
            .as_deref(),
        Some(SSH_SECRET)
    );
    assert_ne!(db_key, password_key(&client_db("bastion.outro.example")));
}

#[test]
fn session_reopens_the_tunnelled_server_without_asking() {
    let _alone = alone();
    let dir = temp_dir("session");
    let settings = client_db("bastion.sessao.example");
    let ssh = settings.ssh.clone().unwrap();
    pgstudio_ssh::set_secret(&ssh, "");
    let store = Arc::new(MemoryStore::default());
    let start = |store: Arc<MemoryStore>| {
        harness(app(store).with_session(SessionStore::open(dir.join("session.json"))))
    };
    {
        let mut h = start(store.clone());
        h.run();
        fill(&mut h, &settings, SSH_SECRET);
        h.get_by_label("Connect").click();
        run_until(&mut h, "connected", |app| app.connection().is_some());
        wait_for("secrets saved", || store.len() == 2);
        h.state_mut().save_session();
    }
    let text = std::fs::read_to_string(dir.join("session.json")).unwrap();
    assert!(text.contains("bastion.sessao.example"), "{text}");
    assert!(text.contains("~/.ssh/id_ed25519"), "{text}");
    no_secret_in(&dir);

    // Outra execução: nada digitado, o segredo do túnel vem do cofre.
    pgstudio_ssh::set_secret(&ssh, "");
    let mut h = start(store);
    run_until(&mut h, "reconnected through the tunnel", |app| {
        app.connection().is_some()
    });
    assert_eq!(h.state().connection().unwrap().settings, settings);
    assert!(h.state().connect_dialog().is_none());
}

#[test]
fn workspace_carries_the_tunnel_and_its_secret_encrypted() {
    let _alone = alone();
    let base = temp_dir("workspace");
    let settings = client_db("bastion.cliente.example");
    let ssh = settings.ssh.clone().unwrap();
    pgstudio_ssh::set_secret(&ssh, "");
    let root = base.join("ws");
    Workspace::create(&root, "ws").unwrap();
    Workspace::open(&root)
        .unwrap()
        .servers()
        .add_server(
            &[],
            RegisteredServer {
                name: "Cliente".into(),
                description: String::new(),
                environment: None,
                settings: settings.clone(),
            },
        )
        .unwrap();
    // O túnel vai no arquivo do workspace: servidor, usuário e o caminho
    // da chave, sem segredo.
    let servers = std::fs::read_to_string(root.join(SERVERS)).unwrap();
    for part in ["bastion.cliente.example", "deploy", "~/.ssh/id_ed25519"] {
        assert!(servers.contains(part), "{servers}");
    }

    let open = |h: &mut Harness<'static, ShellApp>| {
        h.run();
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state_mut().open_workspace(&root));
        h.state_mut()
            .workspace_panel_mut()
            .passwords_mut()
            .unwrap()
            .set_kdf(FAST);
        h.run();
    };

    // Máquina A: cria a senha mestra e conecta lembrando as senhas.
    let system_a = Arc::new(MemoryStore::default());
    let mut a = harness(app(system_a.clone()));
    open(&mut a);
    a.state_mut()
        .workspace_panel_mut()
        .passwords_mut()
        .unwrap()
        .create(MASTER, MASTER, true)
        .unwrap();
    run_until(&mut a, "vault created", |app| {
        app.workspace_panel().passwords().unwrap().is_unlocked()
    });
    let ctx = a.ctx.clone();
    a.state_mut()
        .execute(&ctx, CommandId::ConnectObjectExplorer);
    a.run();
    fill(&mut a, &settings, SSH_SECRET);
    a.get_by_label("Connect").click();
    run_until(&mut a, "connected", |app| app.connection().is_some());
    let ssh_key = ssh.secret_key();
    let db_key = password_key(&settings);
    wait_for("secrets in the workspace vault", || {
        Vault::at(&root).names().len() == 2
    });
    let mut names = Vault::at(&root).names();
    names.sort();
    assert_eq!(names, [db_key.clone(), ssh_key.clone()]);
    // Nada foi para o cofre do sistema nem em texto puro para o workspace.
    assert_eq!(system_a.get(&ssh_key).unwrap(), None);
    assert_eq!(system_a.get(&db_key).unwrap(), None);
    no_secret_in(&root);
    drop(a);

    // Máquina B (o clone): com a senha mestra, conecta sem digitar nada.
    pgstudio_ssh::set_secret(&ssh, "");
    let system_b = Arc::new(MemoryStore::default());
    let mut b = harness(app(system_b.clone()));
    open(&mut b);
    b.state_mut()
        .workspace_panel_mut()
        .passwords_mut()
        .unwrap()
        .unlock(MASTER, false);
    run_until(&mut b, "unlocked", |app| {
        app.workspace_panel().passwords().unwrap().is_unlocked()
    });
    b.state_mut().connect_registered(&settings, false);
    run_until(&mut b, "connected with the vault secrets", |app| {
        app.connection().is_some()
    });
    assert_eq!(b.state().connection().unwrap().settings, settings);
    assert_eq!(system_b.get(&ssh_key).unwrap(), None);
}

/// O `pg_dump` do Backup pelo túnel de verdade: a porta local do túnel e o
/// `PGHOSTADDR`. Só com `SSH_TEST_HOST` (veja `scripts/ssh-tunnel-test.sh`)
/// e o `pg_dump` instalado.
#[test]
fn pg_dump_goes_through_the_open_tunnel() {
    let Ok(host) = std::env::var("SSH_TEST_HOST") else {
        eprintln!("SSH_TEST_HOST not set; skipping");
        return;
    };
    let pg_dump = "pg_dump";
    if std::process::Command::new(pg_dump)
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("pg_dump not found; skipping");
        return;
    }
    let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let target = var("SSH_TEST_PG", "pgdb:5432");
    let (db_host, db_port) = target.rsplit_once(':').expect("host:port");
    let settings = ConnectionSettings {
        host: db_host.into(),
        port: db_port.parse().unwrap(),
        ssl: pgstudio_core::connection::SslMode::Disable,
        ssh: Some(SshSettings {
            host,
            port: var("SSH_TEST_PORT", "22").parse().unwrap(),
            user: var("SSH_TEST_USER", "tunnel"),
            auth: SshAuth::KeyFile,
            key_file: format!("{}/id_plain", var("SSH_TEST_KEYS", ".")),
        }),
        ..Default::default()
    };
    let password = var("SSH_TEST_PG_PASSWORD", "");
    let rt = pgstudio_pg_client::DbRuntime::new().unwrap();
    // A conexão do Object Explorer mantém o túnel aberto.
    let session = rt
        .block_on(pgstudio_pg_client::Session::connect(&settings, &password))
        .expect("connect through the tunnel");
    rt.block_on(
        session.simple_query(
            "CREATE TABLE IF NOT EXISTS tunel_backup (id int PRIMARY KEY, nome text)",
        ),
    )
    .unwrap();
    let mut args = pgstudio_ui_shell::pgtools::connection_args(&settings);
    assert!(args.contains(&format!("--host={db_host}")), "{args:?}");
    assert!(!args.contains(&format!("--port={db_port}")), "{args:?}");
    args.extend(["--schema-only".to_string(), "--dbname=postgres".to_string()]);
    let out = std::process::Command::new(pg_dump)
        .args(&args)
        .envs(pgstudio_ui_shell::pgtools::connection_env(
            &settings, &password,
        ))
        .output()
        .expect("run pg_dump");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("CREATE TABLE public.tunel_backup"),
        "{}\n{}",
        text,
        String::from_utf8_lossy(&out.stderr)
    );
    rt.block_on(session.simple_query("DROP TABLE tunel_backup"))
        .unwrap();
}
