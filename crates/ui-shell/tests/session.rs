//! Fechar e abrir o app: a sessão volta (servidores, árvore, janelas de
//! consulta com o texto não salvo), sem senha no disco.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::metadata::NodeId;
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::secrets::{MemoryStore, SecretStore, password_key};
use pgstudio_ui_shell::session::SessionStore;
use pgstudio_ui_shell::{Answer, ShellApp};

const PASSWORD: &str = "s3cr3t-do-banco";

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, password: &str) -> Result<Connected, ConnectError> {
        if password != PASSWORD {
            return Err(ConnectError::Authentication("Login failed.".into()));
        }
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(pgstudio_core::query::Offline("offline")),
        })
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "postpilot-session-it-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn server(host: &str) -> ConnectionSettings {
    ConnectionSettings {
        host: host.into(),
        ..ConnectionSettings::default()
    }
}

/// Uma execução do app com a sessão em `dir` e este cofre.
fn start(dir: &Path, store: Arc<MemoryStore>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector))
        .with_secret_store(store)
        .with_session(SessionStore::open(dir.join("session.json")));
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

/// Roda frames até `done`.
fn wait(harness: &mut Harness<'static, ShellApp>, what: &str, done: impl Fn(&ShellApp) -> bool) {
    let start = Instant::now();
    loop {
        harness.step();
        if done(harness.state()) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.step();
}

/// Conecta pelo diálogo aberto, com a senha.
fn connect(harness: &mut Harness<'static, ShellApp>, host: &str, remember: bool) {
    harness.state_mut().open_connect_dialog();
    let settings = server(host);
    {
        let dialog = harness.state_mut().connect_dialog_mut().expect("dialog");
        dialog.fill(&settings);
        dialog.password = PASSWORD.into();
        dialog.remember = remember;
    }
    harness.step();
    harness.state_mut().connect_to(settings, PASSWORD.into());
    wait(harness, "connect", |app| {
        app.connect_dialog().is_none() && app.connection().is_some_and(|c| c.settings.host == host)
    });
}

fn expand(harness: &mut Harness<'static, ShellApp>, path: &str) {
    let id = NodeId(path.into());
    wait(harness, "node loaded", |app| {
        app.explorer().node(&id).is_some()
    });
    harness.state_mut().explorer_mut().expand(&id);
    wait(harness, "children loaded", |app| {
        !app.explorer().is_loading()
    });
}

fn all_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(all_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn closing_and_opening_brings_everything_back() {
    let dir = temp_dir("everything");
    let store = Arc::new(MemoryStore::default());
    let saved_file = dir.join("relatorio.sql");
    std::fs::write(&saved_file, "select * from pedidos").unwrap();
    {
        let mut app = start(&dir, store.clone());
        connect(&mut app, "producao", true);
        connect(&mut app, "homologacao", true);
        expand(&mut app, "srv");
        expand(&mut app, "srv/Databases");
        // Uma consulta nunca salva, um arquivo aberto e alterado e um
        // arquivo aberto sem alteração.
        let ctx = app.ctx.clone();
        app.state_mut().execute(&ctx, CommandId::NewQuery);
        let id = app.state().active_document().unwrap();
        {
            let doc = app.state_mut().documents_mut().get_mut(id).unwrap();
            doc.text = "select 'ainda não salvei'".into();
            doc.database = Some("vendas".into());
        }
        assert!(app.state_mut().open_file(&saved_file));
        let edited = app.state().active_document().unwrap();
        app.state_mut()
            .documents_mut()
            .get_mut(edited)
            .unwrap()
            .text = "select * from pedidos where id = 7".into();
        let untouched = dir.join("intacto.sql");
        std::fs::write(&untouched, "select 42").unwrap();
        assert!(app.state_mut().open_file(&untouched));
        app.state_mut().activate_document(id);
        app.step();
        // Nada de fechar direito: o app simplesmente some (uma queda).
        app.state_mut().save_session();
    }
    // Nenhuma senha foi para o disco.
    for file in all_files(&dir) {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(!text.contains(PASSWORD), "password in {}", file.display());
    }

    let mut app = start(&dir, store);
    // O "Connect to Server" vazio não aparece: os servidores voltam sozinhos.
    wait(&mut app, "servers back", |app| {
        app.connected_servers().len() == 2 && app.connect_dialog().is_none()
    });
    app.step();
    assert_eq!(
        app.state().connected_servers(),
        vec![
            ("producao".to_string(), false),
            ("homologacao".to_string(), true)
        ]
    );
    // A árvore do servidor ativo abre como estava.
    wait(&mut app, "tree back", |app| {
        app.explorer().expanded() == vec![NodeId("srv".into()), NodeId("srv/Databases".into())]
    });
    // As três janelas, com o texto, o `*` e o banco.
    let state = app.state();
    let docs: Vec<_> = state
        .documents()
        .ids()
        .into_iter()
        .map(|id| state.documents().get(id).unwrap())
        .collect();
    assert_eq!(docs.len(), 3);
    assert_eq!(docs[0].title(), "SQLQuery1.sql*");
    assert_eq!(docs[0].text, "select 'ainda não salvei'");
    assert_eq!(docs[0].database.as_deref(), Some("vendas"));
    assert_eq!(docs[1].title(), "relatorio.sql*");
    assert_eq!(docs[1].text, "select * from pedidos where id = 7");
    assert_eq!(docs[2].title(), "intacto.sql");
    assert_eq!(docs[2].text, "select 42");
    // A janela que estava na frente continua na frente.
    assert_eq!(state.active_document(), Some(state.documents().ids()[0]));
    // E a próxima consulta nova não repete o nome.
    let ctx = app.ctx.clone();
    app.state_mut().execute(&ctx, CommandId::NewQuery);
    let new = app.state().active_document().unwrap();
    assert_eq!(
        app.state().documents().get(new).unwrap().name(),
        "SQLQuery2.sql"
    );
}

#[test]
fn without_a_remembered_password_the_dialog_asks_only_for_it() {
    let dir = temp_dir("ask");
    let store = Arc::new(MemoryStore::default());
    {
        let mut app = start(&dir, store.clone());
        connect(&mut app, "producao", false);
        app.state_mut().save_session();
    }
    assert!(
        store
            .get(&password_key(&server("producao")))
            .unwrap()
            .is_none()
    );

    let mut app = start(&dir, store);
    wait(&mut app, "dialog asking", |app| {
        app.connect_dialog().is_some_and(|d| !d.is_looking_up())
    });
    let dialog = app.state().connect_dialog().unwrap();
    assert_eq!(dialog.settings().unwrap().host, "producao");
    assert!(dialog.password.is_empty());
    assert!(app.state().connection().is_none());
    // Fechar o app com a pergunta aberta não esquece o servidor.
    app.state_mut().save_session();
    assert_eq!(app.state().session_state().servers.len(), 1);
    // Com a senha, conecta.
    app.state_mut().connect_dialog_mut().unwrap().password = PASSWORD.into();
    app.state_mut()
        .connect_to(server("producao"), PASSWORD.into());
    wait(&mut app, "connected", |app| app.connection().is_some());
    assert_eq!(app.state().session_state().servers.len(), 1);
}

#[test]
fn disconnected_servers_and_closed_windows_do_not_come_back() {
    let dir = temp_dir("closed");
    let store = Arc::new(MemoryStore::default());
    {
        let mut app = start(&dir, store.clone());
        connect(&mut app, "producao", true);
        let ctx = app.ctx.clone();
        app.state_mut().execute(&ctx, CommandId::NewQuery);
        let id = app.state().active_document().unwrap();
        app.state_mut().documents_mut().get_mut(id).unwrap().text = "select 1".into();
        app.step();
        app.state_mut().save_session();
        assert!(dir.join("session").join(format!("query-{id}.sql")).exists());
        // Fecha a janela sem salvar e desconecta.
        app.state_mut().execute(&ctx, CommandId::CloseDocument);
        app.step();
        app.state_mut().answer(&ctx, Answer::No);
        app.state_mut()
            .execute(&ctx, CommandId::DisconnectObjectExplorer);
        app.step();
        app.state_mut().save_session();
        assert!(all_files(&dir.join("session")).is_empty());
    }
    let mut app = start(&dir, store);
    app.step();
    assert!(app.state().documents().ids().is_empty());
    assert!(app.state().connected_servers().is_empty());
    // Sem servidor para reconectar, o "Connect to Server" abre como sempre.
    assert!(app.state().connect_dialog().is_some());
}

#[test]
fn exit_does_not_ask_to_save_when_the_session_keeps_the_text() {
    let dir = temp_dir("exit");
    let mut app = start(&dir, Arc::new(MemoryStore::default()));
    let ctx = app.ctx.clone();
    app.state_mut().connect_dialog_mut();
    app.state_mut().execute(&ctx, CommandId::NewQuery);
    let id = app.state().active_document().unwrap();
    app.state_mut().documents_mut().get_mut(id).unwrap().text = "select 1".into();
    app.step();
    app.state_mut().execute(&ctx, CommandId::Exit);
    assert!(app.state().confirm_pending().is_none());
    // Sem sessão, a pergunta continua.
    let mut plain = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), ShellApp::default());
    let ctx = plain.ctx.clone();
    plain.state_mut().execute(&ctx, CommandId::NewQuery);
    let id = plain.state().active_document().unwrap();
    plain.state_mut().documents_mut().get_mut(id).unwrap().text = "select 1".into();
    plain.step();
    plain.state_mut().execute(&ctx, CommandId::Exit);
    assert!(plain.state().confirm_pending().is_some());
}
