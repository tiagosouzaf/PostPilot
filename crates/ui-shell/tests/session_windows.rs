//! Fechar e abrir o app: o que a sessão guarda além das janelas de consulta
//! (veja `session.rs`): Table Designer, Database Diagram e Edit Top Rows com
//! o servidor de cada um e as edições não salvas, o último resultado das
//! consultas (sem rodar de novo) e as pastas recolhidas.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::metadata::NodeId;
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::Event;
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::connect::RecentConnections;
use pgstudio_ui_shell::execution::{MessageKind, Outcome, Tab};
use pgstudio_ui_shell::registered::{RegisteredServers, ServerGroup};
use pgstudio_ui_shell::secrets::MemoryStore;
use pgstudio_ui_shell::session::{SessionStore, WindowKind};
use pgstudio_ui_shell::{ShellApp, repo};

const PASSWORD: &str = "s3cr3t-do-banco";
const TABLE: &str = "srv/Databases/app_01/Schemas/public/Tables/3";
const TABLES: &str = "srv/Databases/app_01/Schemas/public/Tables";
const DIAGRAMS: &str = "srv/Databases/app_01/Database Diagrams";

/// O SQL que chegou ao "servidor", nas duas execuções do app.
type Log = Arc<Mutex<Vec<String>>>;

struct FakeQueries(Log);

impl QueryService for FakeQueries {
    fn open(&self, database: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession(self.0.clone(), database.to_string()))
    }
}

/// `rows N` devolve N linhas; `update` altera 3 linhas; o resto, uma linha.
struct FakeSession(Log, String);

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        self.0.lock().unwrap().push(sql.clone());
        if let Some(n) = sql
            .strip_prefix("rows ")
            .and_then(|n| n.parse::<u64>().ok())
        {
            sink(QueryEvent::Columns(vec!["n".into(), "label".into()]));
            for i in 0..n {
                sink(QueryEvent::Row(vec![Some(i.to_string()), None]));
            }
            sink(QueryEvent::CommandComplete { rows: n });
        } else if sql.starts_with("update") {
            sink(QueryEvent::CommandComplete { rows: 3 });
        } else {
            sink(QueryEvent::Columns(vec!["sql".into()]));
            sink(QueryEvent::Row(vec![Some(sql)]));
            sink(QueryEvent::CommandComplete { rows: 1 });
        }
        sink(QueryEvent::Finished { canceled: false });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.1.clone()
    }
}

struct FakeConnector(Log);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries(self.0.clone())),
        })
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "postpilot-session-windows-{name}-{}",
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

fn app(dir: &Path, store: Arc<MemoryStore>, log: &Log) -> ShellApp {
    ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector(log.clone())))
        .with_secret_store(store)
        .with_session(SessionStore::open(dir.join("session.json")))
}

fn harness(app: ShellApp) -> Harness<'static, ShellApp> {
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

fn start(dir: &Path, store: Arc<MemoryStore>, log: &Log) -> Harness<'static, ShellApp> {
    harness(app(dir, store, log))
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

fn connect(harness: &mut Harness<'static, ShellApp>, host: &str) {
    harness.state_mut().open_connect_dialog();
    let settings = server(host);
    {
        let dialog = harness.state_mut().connect_dialog_mut().expect("dialog");
        dialog.fill(&settings);
        dialog.password = PASSWORD.into();
        dialog.remember = true;
    }
    harness.step();
    harness.state_mut().connect_to(settings, PASSWORD.into());
    wait(harness, "connect", |app| {
        app.connect_dialog().is_none() && app.connection().is_some_and(|c| c.settings.host == host)
    });
}

fn event(harness: &mut Harness<'static, ShellApp>, event: Event) {
    let ctx = harness.ctx.clone();
    harness.state_mut().explorer_event(&ctx, event);
    harness.step();
}

fn all_loaded(app: &ShellApp) -> bool {
    let designers = app.designers();
    let diagrams = app.diagrams();
    designers
        .ids()
        .iter()
        .all(|id| !designers.get(*id).unwrap().is_loading())
        && diagrams
            .ids()
            .iter()
            .all(|id| !diagrams.get(*id).unwrap().is_loading())
}

/// Roda `sql` numa janela de consulta nova e espera terminar.
fn run(harness: &mut Harness<'static, ShellApp>, sql: &str) -> u32 {
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::NewQuery);
    let id = harness.state().active_document().unwrap();
    harness
        .state_mut()
        .documents_mut()
        .get_mut(id)
        .unwrap()
        .text = sql.into();
    harness.step();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    wait(harness, "query finished", |app| {
        app.documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running())
    });
    id
}

#[test]
fn designers_diagrams_and_edit_rows_come_back_with_their_unsaved_edits() {
    let dir = temp_dir("windows");
    let store = Arc::new(MemoryStore::default());
    let log = Log::default();
    let (script, diagram_script, position, edited);
    {
        let mut app = start(&dir, store.clone(), &log);
        connect(&mut app, "producao");
        // No primeiro servidor, um designer sem alterações.
        event(&mut app, Event::Design(NodeId(TABLE.into())));
        connect(&mut app, "homologacao");
        // No segundo: uma tabela com uma coluna renomeada, uma tabela nova
        // com uma coluna a mais, um diagrama com uma tabela nova e uma
        // janela Edit Top Rows.
        event(&mut app, Event::Design(NodeId(TABLE.into())));
        event(&mut app, Event::NewTable(NodeId(TABLES.into())));
        event(&mut app, Event::NewDiagram(NodeId(DIAGRAMS.into())));
        event(&mut app, Event::EditTop(NodeId(TABLE.into())));
        wait(&mut app, "windows loaded", all_loaded);
        let ids = app.state().designers().ids();
        assert_eq!(ids.len(), 3);
        {
            let d = app.state_mut().designers_mut().get_mut(ids[1]).unwrap();
            let table = d.table_id().unwrap();
            let column = d.model().table(table).unwrap().columns[1].id;
            d.rename_column(column, "nome_novo");
            script = d.change_script();
            assert!(!script.is_empty());
        }
        {
            let d = app.state_mut().designers_mut().get_mut(ids[2]).unwrap();
            d.add_column("preco").unwrap();
        }
        let diagram = app.state().diagrams().ids()[0];
        {
            let d = app.state_mut().diagrams_mut().get_mut(diagram).unwrap();
            let all: Vec<u64> = d
                .candidates()
                .into_iter()
                .take(2)
                .map(|(id, _)| id)
                .collect();
            d.add_tables(&all);
            d.new_table().expect("new table");
            d.diagram_mut().nodes[0].pos = eframe::egui::pos2(321.0, 123.0);
            position = d.diagram().nodes[0].pos;
            diagram_script = d.change_script();
            assert!(!diagram_script.is_empty());
        }
        assert_eq!(app.state().edit_rows_ids().len(), 1);
        let id = app.state().edit_rows_ids()[0];
        edited = app.state().edit_rows(id).unwrap().target();
        assert_eq!(edited.0, "app_01");
        app.step();
        // Sair não pergunta pelas alterações: elas voltam.
        let ctx = app.ctx.clone();
        app.state_mut().execute(&ctx, CommandId::Exit);
        let designers = app.state().designers();
        assert!(
            designers
                .ids()
                .iter()
                .all(|id| !designers.get(*id).unwrap().is_confirming_close())
        );
        // O app some sem fechar direito.
        app.state_mut().save_session();
        let state = app.state().session_state();
        assert_eq!(state.windows.len(), 5);
        assert_eq!(
            state
                .windows
                .iter()
                .filter(|w| w.server.contains("homologacao"))
                .count(),
            4
        );
    }

    let mut app = start(&dir, store, &log);
    wait(&mut app, "servers back", |app| {
        app.connected_servers().len() == 2 && app.connect_dialog().is_none()
    });
    wait(&mut app, "windows back", |app| {
        app.designers().ids().len() == 3
            && app.diagrams().ids().len() == 1
            && app.edit_rows_ids().len() == 1
            && all_loaded(app)
    });
    let state = app.state();
    let designers: Vec<_> = state
        .designers()
        .ids()
        .into_iter()
        .map(|id| state.designers().get(id).unwrap())
        .collect();
    // O designer sem alterações volta limpo; os outros, com o `*` e o mesmo
    // script de alteração.
    let clean = designers.iter().filter(|d| !d.is_dirty()).count();
    assert_eq!(clean, 1);
    let renamed = designers
        .iter()
        .find(|d| d.change_script() == script)
        .expect("the renamed column came back");
    assert!(renamed.title().ends_with('*'), "{}", renamed.title());
    let new = designers
        .iter()
        .find(|d| d.qualified_name().contains("Table_"))
        .expect("the new table came back");
    assert!(new.is_dirty());
    let table = new.model().table(new.table_id().unwrap()).unwrap();
    assert!(table.columns.iter().any(|c| c.name == "preco"));
    // O diagrama: as tabelas no lugar e a tabela nova ainda por salvar.
    let diagram = state.diagrams().get(state.diagrams().ids()[0]).unwrap();
    assert_eq!(diagram.diagram().nodes.len(), 3);
    assert_eq!(diagram.diagram().nodes[0].pos, position);
    assert_eq!(diagram.change_script(), diagram_script);
    assert!(diagram.is_dirty());
    // O Edit Top Rows, da mesma tabela.
    let edit = state.edit_rows(state.edit_rows_ids()[0]).unwrap();
    assert_eq!(edit.target(), edited);
    // E cada janela continua ligada ao servidor dela.
    let session = state.session_state();
    assert_eq!(session.windows.len(), 5);
    let production: Vec<_> = session
        .windows
        .iter()
        .filter(|w| w.server.contains("producao"))
        .collect();
    assert_eq!(production.len(), 1);
    assert!(matches!(production[0].kind, WindowKind::TableDesigner(_)));
}

#[test]
fn a_closed_designer_does_not_come_back() {
    let dir = temp_dir("closed");
    let store = Arc::new(MemoryStore::default());
    let log = Log::default();
    {
        let mut app = start(&dir, store.clone(), &log);
        connect(&mut app, "producao");
        event(&mut app, Event::Design(NodeId(TABLE.into())));
        wait(&mut app, "designer loaded", all_loaded);
        app.state_mut().save_session();
        assert_eq!(app.state().session_state().windows.len(), 1);
        let id = app.state().designers().ids()[0];
        app.state_mut().designers_mut().remove(id);
        app.run_steps(3);
        app.state_mut().save_session();
        assert!(app.state().session_state().windows.is_empty());
    }
    let mut app = start(&dir, store, &log);
    wait(&mut app, "server back", |app| {
        app.connected_servers().len() == 1 && app.connect_dialog().is_none()
    });
    app.run_steps(5);
    assert!(app.state().designers().ids().is_empty());
}

#[test]
fn the_last_results_come_back_without_running_anything() {
    let dir = temp_dir("results");
    let store = Arc::new(MemoryStore::default());
    let log = Log::default();
    let (small, big, change, never);
    {
        let mut app = start(&dir, store.clone(), &log);
        connect(&mut app, "producao");
        small = run(&mut app, "rows 3");
        big = run(&mut app, "rows 6000");
        change = run(&mut app, "update pedidos set pago = true");
        // Uma janela que nunca executou.
        let ctx = app.ctx.clone();
        app.state_mut().execute(&ctx, CommandId::NewQuery);
        never = app.state().active_document().unwrap();
        app.step();
        app.state_mut().save_session();
        assert!(
            dir.join("session")
                .join(format!("result-{small}.json"))
                .exists()
        );
        assert!(
            !dir.join("session")
                .join(format!("result-{never}.json"))
                .exists()
        );
    }
    let ran = log.lock().unwrap().len();
    assert_eq!(ran, 3);

    let mut app = start(&dir, store, &log);
    wait(&mut app, "server back", |app| {
        app.connected_servers().len() == 1 && app.connect_dialog().is_none()
    });
    app.run_steps(5);
    {
        let docs = app.state().documents();
        let ids = docs.ids();
        assert_eq!(ids.len(), 4);
        let exec = docs
            .get(ids[0])
            .unwrap()
            .execution
            .as_ref()
            .expect("result");
        assert_eq!(exec.outcome(), Outcome::Succeeded);
        assert!(!exec.is_running());
        assert_eq!(exec.results.len(), 1);
        assert_eq!(exec.results[0].columns(), ["n", "label"]);
        assert_eq!(exec.results[0].len(), 3);
        assert_eq!(exec.results[0].row(2), [Some("2".to_string()), None]);
        assert!(exec.messages.iter().any(|m| m.text == "(3 rows affected)"));
        let restored = exec.restored().expect("marked as from the last session");
        assert_eq!(restored.cut, None);
        assert!(restored.notice().contains("previous session"));
        // O resultado grande volta cortado, dizendo quantas linhas tinha.
        let exec = docs.get(ids[1]).unwrap().execution.as_ref().unwrap();
        assert_eq!(exec.results[0].len(), 5000);
        assert_eq!(exec.restored().unwrap().cut, Some((5000, 6000)));
        // O comando que altera dados volta só com as mensagens.
        let exec = docs.get(ids[2]).unwrap().execution.as_ref().unwrap();
        assert!(exec.results.is_empty());
        assert_eq!(exec.tab, Tab::Messages);
        assert!(
            exec.messages
                .iter()
                .any(|m| m.kind == MessageKind::Info && m.text == "(3 rows affected)")
        );
        assert!(docs.get(ids[3]).unwrap().execution.is_none());
    }
    // Nada foi executado para isso, nem o UPDATE.
    assert_eq!(log.lock().unwrap().len(), ran);
    // Gravar de novo mantém o corte; F5 traz um resultado de agora.
    app.state_mut().save_session();
    let ids = app.state().documents().ids();
    let ctx = app.ctx.clone();
    app.state_mut().activate_document(ids[0]);
    app.step();
    app.state_mut().execute(&ctx, CommandId::Execute);
    wait(&mut app, "ran again", |app| {
        app.documents()
            .get(ids[0])
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running() && e.restored().is_none())
    });
    assert_eq!(log.lock().unwrap().len(), ran + 1);
    let _ = (big, change);
    drop(app);
    let mut app = start(&dir, Arc::new(MemoryStore::default()), &log);
    app.run_steps(3);
    let docs = app.state().documents();
    let exec = docs.get(docs.ids()[1]).unwrap().execution.as_ref().unwrap();
    assert_eq!(exec.restored().unwrap().cut, Some((5000, 6000)));
}

#[test]
fn collapsed_folders_stay_collapsed() {
    let dir = temp_dir("folders");
    let log = Log::default();
    let servers = || {
        let mut servers = RegisteredServers::in_memory(ServerGroup::default());
        let clientes = servers
            .add_group(
                &[],
                ServerGroup {
                    name: "Clientes".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        for name in ["Acme", "Globex"] {
            servers
                .add_group(
                    &clientes,
                    ServerGroup {
                        name: name.into(),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        servers
    };
    let root = dir.join("ws");
    repo::Workspace::create(&root, "ws").expect("workspace");
    std::fs::create_dir_all(root.join("queries").join("relatorios")).unwrap();
    let open = |dir: &Path| {
        let mut app =
            app(dir, Arc::new(MemoryStore::default()), &log).with_registered_servers(servers());
        assert!(app.open_workspace(&root));
        app
    };
    let folder = root.join("queries").join("relatorios");
    {
        let mut app = harness(open(&dir));
        app.state_mut()
            .registered_mut()
            .set_collapsed(&[0, 1], true);
        app.state_mut()
            .workspace_panel_mut()
            .collapse_folders(std::slice::from_ref(&folder));
        app.step();
        app.state_mut().save_session();
    }
    // Reabrir: a sessão vem depois dos servidores registrados e do
    // workspace, como no app.
    let app = ShellApp::default()
        .with_recent_connections(RecentConnections::in_memory(Vec::new()))
        .with_connector(Arc::new(FakeConnector(log.clone())))
        .with_registered_servers(servers());
    let mut app = app;
    assert!(app.open_workspace(&root));
    let app = app.with_session(SessionStore::open(dir.join("session.json")));
    assert_eq!(
        app.registered().collapsed_names(),
        [["Clientes".to_string(), "Globex".to_string()]]
    );
    assert_eq!(app.workspace_panel().collapsed_folders(), [folder]);
}
