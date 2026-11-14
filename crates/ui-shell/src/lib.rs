//! Casca da interface do PGStudio: janela principal, barra de menus, toolbar,
//! status bar, tema e atalhos de teclado.
//!
//! Só esta camada conhece o egui. Menus, toolbar e atalhos viram
//! [`CommandId`] e passam por [`ShellApp::execute`]. A área central tem o
//! docking com o Object Explorer, as tool windows e as janelas de consulta.

pub mod activity;
pub mod backup;
pub mod bench;
pub mod claude;
pub mod connect;
pub mod designer;
pub mod diagrams;
pub mod documents;
pub mod edit_rows;
pub mod execution;
pub mod intellisense;
pub mod keys;
pub mod layout;
pub mod menus;
pub mod object_info;
pub mod panels;
pub mod pgtools;
pub mod properties;
pub mod query_designer;
pub mod registered;
pub mod restore;
pub mod results;
pub mod schema_compare;
pub mod secrets;
pub mod security;
pub mod servers;
pub mod theme;

/// Ações do menu de contexto do Object Explorer.
pub use pgstudio_object_explorer::Event as ExplorerEvent;

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use pgstudio_core::keymap::{ChordMatcher, Keymap, Match};
use pgstudio_core::{APP_NAME, CommandId, Theme, VERSION};

use connect::{ConnectDialog, DialogAction, RecentConnections};
use documents::{Documents, FileDialogs, NoDialogs};
use layout::{LayoutStore, SavedLayout, WindowGeometry};
use menus::{Entry, MENUS, TOOLBAR, Tool};
use panels::Panel;
use pgstudio_core::connection::{Connected, ConnectionSettings, Connector};
use pgstudio_docking::Workspace;
use pgstudio_object_explorer::ObjectExplorer;
use pgstudio_object_explorer::fake::FakeProvider;
use std::sync::Arc;

const READY: &str = "Ready";

/// Linha que o modo de medição escreve quando o primeiro frame foi exibido.
pub const BENCH_FIRST_FRAME: &str = "pgstudio-bench: first-frame";

/// O aplicativo: estado da casca e desenho de cada frame.
pub struct ShellApp {
    theme: Theme,
    about_open: bool,
    /// Renderizador em uso, para o "About" (por exemplo "wgpu, Vulkan, …").
    renderer: Option<String>,
    keymap: Keymap,
    chords: ChordMatcher,
    status: String,
    last_command: Option<CommandId>,
    workspace: Workspace<Panel>,
    output: Vec<String>,
    explorer: ObjectExplorer,
    screenshot: Option<Screenshot>,
    /// Nós a expandir assim que carregarem (demonstração e screenshots).
    pending_expand: std::collections::VecDeque<pgstudio_core::metadata::NodeId>,
    documents: Documents,
    designers: designer::Designers,
    /// Database Diagrams abertos.
    diagrams: diagrams::Diagrams,
    /// Novos diagramas já com todas as tabelas (screenshots).
    diagram_all_tables: bool,
    /// Activity Monitor aberto.
    activity: Option<activity::ActivityMonitor>,
    /// "Save Results As" em andamento: a mensagem chega no fim.
    exports: Vec<std::sync::mpsc::Receiver<String>>,
    /// Janelas Edit Top N Rows, pelo número.
    edit_rows: std::collections::BTreeMap<u32, edit_rows::EditRows>,
    next_edit_rows: u32,
    /// Janela de propriedades de segurança aberta.
    security: Option<properties::SecurityDialog>,
    /// "Back Up Database" aberta.
    backup: Option<backup::BackupDialog>,
    restore: Option<restore::RestoreDialog>,
    compare: Option<schema_compare::SchemaCompare>,
    /// Origem e destino que o Schema Compare já compara ao abrir
    /// (screenshots).
    compare_target: Option<(String, String)>,
    /// Query Designer aberto (Ctrl+Shift+Q).
    query_designer: Option<query_designer::QueryDesigner>,
    /// Abrir o Query Designer na próxima janela de "New Query" (screenshots).
    design_query_on_open: bool,
    /// Arquivo que o "Restore Database" já abre listando (screenshots).
    restore_file: Option<String>,
    /// Texto e comando para a próxima janela de "New Query" (screenshots).
    query_on_open: Option<(String, CommandId)>,
    /// Janela de consulta ativa (a última que teve foco).
    active_doc: Option<u32>,
    focus_editor: Option<u32>,
    dialogs: Box<dyn FileDialogs>,
    confirm: Option<Confirm>,
    /// Pergunta do plano real de comandos que alteram dados.
    actual_confirm: Option<PendingActual>,
    /// O usuário já respondeu à pergunta de salvar; a janela pode fechar.
    allow_close: bool,
    /// Onde o layout é salvo ao sair.
    layout_store: Option<LayoutStore>,
    /// Última geometria conhecida da janela (fora de maximizado).
    window: Option<WindowGeometry>,
    /// Modo de medição: frames desenhados até agora.
    bench_frames: Option<u32>,
    /// Quem abre conexões de verdade. Sem ele, "Connect" usa o servidor de
    /// demonstração.
    connector: Option<Arc<dyn Connector>>,
    connect_dialog: Option<ConnectDialog>,
    /// Servidor conectado no Object Explorer.
    connection: Option<Connected>,
    /// Senha da conexão, só na memória, para pg_dump e pg_restore.
    password: String,
    recent: RecentConnections,
    /// Executar a janela ativa assim que conectar (demonstração e
    /// screenshots).
    run_on_connect: bool,
    /// Catálogos para o IntelliSense, por banco.
    catalogs: intellisense::Catalogs,
    /// Abrir o IntelliSense no fim da janela ativa quando o catálogo chegar
    /// (screenshots).
    list_members_on_load: bool,
    /// Scripts do menu de contexto sendo gerados.
    scripts: Vec<PendingScript>,
    /// Ação do menu de contexto a fazer no nó selecionado depois das
    /// expansões (screenshots).
    pending_events: Vec<pgstudio_object_explorer::Event>,
    /// Cenário de medição em andamento (`cargo xtask bench`).
    bench_run: Option<bench::Run>,
    /// Cofre das senhas lembradas (Remember password). Sem ele, a senha é
    /// sempre pedida.
    secrets: Option<Arc<dyn secrets::SecretStore>>,
    /// Janela Registered Servers.
    registered: registered::RegisteredPanel,
    /// Outros servidores conectados no Object Explorer.
    servers: servers::Servers,
    /// Servidor de cada janela de consulta (pela chave do servidor).
    doc_servers: std::collections::HashMap<u32, String>,
    last_active_doc: Option<u32>,
    /// Abrir uma janela de consulta assim que este servidor conectar
    /// (Registered Servers > Connect > New Query).
    new_query_on_connect: Option<String>,
    selected_action: Option<fn(pgstudio_core::metadata::NodeId) -> pgstudio_object_explorer::Event>,
    /// Ponte com o Claude (MCP e painel): pedidos e confirmações.
    claude: claude::Bridge,
}

/// Um script do Object Explorer sendo gerado fora da thread da interface.
struct PendingScript {
    rx: std::sync::mpsc::Receiver<Result<String, pgstudio_core::metadata::MetadataError>>,
    database: Option<String>,
    /// Executar ao abrir (Select Top, Edit Top).
    run: bool,
    /// Edit Top numa view ou view materializada: avisa que só tabelas
    /// editam na grade e que as linhas ficam só leitura.
    read_only: bool,
}

/// Pergunta "salvar alterações?" antes de fechar documentos ou sair.
#[derive(Clone, Debug)]
struct Confirm {
    /// Documentos com alterações, listados na pergunta.
    save: Vec<u32>,
    /// Documentos a fechar depois da resposta.
    close: Vec<u32>,
    /// Sair do aplicativo depois de fechar.
    exit: bool,
}

/// O que o F5 (ou o Ctrl+L) faz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    Results,
    EstimatedPlan,
    ActualPlan {
        roll_back: bool,
    },
    /// Alt+F1: o SQL é do PGStudio, não do documento.
    ObjectInfo,
}

/// O que o usuário pediu na janela de consulta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    /// F5.
    Run,
    /// Ctrl+L.
    Plan,
    /// Alt+F1.
    Info,
}

/// Plano real de um lote que altera dados, esperando a resposta.
struct PendingActual {
    doc: u32,
    sql: String,
    offset: usize,
}

/// Resposta à pergunta de salvar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

struct Screenshot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

impl Default for ShellApp {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            about_open: false,
            renderer: None,
            keymap: Keymap::ssms(),
            chords: ChordMatcher::default(),
            status: READY.to_string(),
            last_command: None,
            workspace: panels::default_layout(),
            output: Vec::new(),
            explorer: ObjectExplorer::default(),
            screenshot: None,
            pending_expand: Default::default(),
            documents: Documents::default(),
            designers: Default::default(),
            diagrams: Default::default(),
            diagram_all_tables: false,
            password: String::new(),
            activity: None,
            exports: Vec::new(),
            edit_rows: std::collections::BTreeMap::new(),
            next_edit_rows: 0,
            security: None,
            backup: None,
            restore: None,
            compare: None,
            compare_target: None,
            query_designer: None,
            design_query_on_open: false,
            restore_file: None,
            query_on_open: None,
            active_doc: None,
            focus_editor: None,
            dialogs: Box::new(NoDialogs),
            confirm: None,
            actual_confirm: None,
            allow_close: false,
            layout_store: None,
            window: None,
            bench_frames: None,
            connector: None,
            connect_dialog: None,
            connection: None,
            recent: RecentConnections::default(),
            run_on_connect: false,
            catalogs: Default::default(),
            list_members_on_load: false,
            scripts: Vec::new(),
            selected_action: None,
            pending_events: Vec::new(),
            bench_run: None,
            secrets: None,
            registered: Default::default(),
            servers: Default::default(),
            doc_servers: Default::default(),
            last_active_doc: None,
            new_query_on_connect: None,
            claude: Default::default(),
        }
    }
}

impl ShellApp {
    /// Cria o aplicativo, instala a fonte de ícones e aplica o tema padrão.
    pub fn new(ctx: &egui::Context) -> Self {
        install_fonts(ctx);
        let mut app = Self::default();
        theme::apply(ctx, app.theme);
        app.connect_demo();
        app
    }

    /// Conecta o Object Explorer ao servidor de demonstração (provedor falso),
    /// até a Fase 2 trazer conexões reais.
    pub fn connect_demo(&mut self) {
        self.explorer.connect(Arc::new(FakeProvider::default()));
    }

    /// Conecta de verdade com `connector`: tira o servidor de demonstração
    /// e abre o "Connect to Server", como o SSMS faz ao abrir.
    pub fn with_connector(mut self, connector: Arc<dyn Connector>) -> Self {
        self.connector = Some(connector);
        self.explorer.disconnect();
        self.open_connect_dialog();
        self
    }

    /// Usa esta lista de conexões recentes (o aplicativo lê de
    /// `connections.json`).
    pub fn with_recent_connections(mut self, recent: RecentConnections) -> Self {
        self.recent = recent;
        if let Some(dialog) = &mut self.connect_dialog {
            if let Some(last) = self.recent.list().first() {
                dialog.fill(&last.clone());
            }
        }
        self
    }

    /// Conexões recentes.
    pub fn recent_connections(&self) -> &RecentConnections {
        &self.recent
    }

    /// Abre o "Connect to Server" preenchido com a última conexão. Sem
    /// conector, conecta o servidor de demonstração.
    pub fn open_connect_dialog(&mut self) {
        if self.connector.is_none() {
            if !self.explorer.is_connected() {
                self.connect_demo();
            }
            return;
        }
        if self.connect_dialog.is_none() {
            let settings = self.recent.list().first().cloned().unwrap_or_default();
            self.connect_dialog =
                Some(ConnectDialog::new(&settings).with_store(self.secrets.clone()));
        }
    }

    /// Diálogo "Connect to Server", se aberto.
    pub fn connect_dialog(&self) -> Option<&ConnectDialog> {
        self.connect_dialog.as_ref()
    }

    /// Diálogo "Connect to Server", mutável.
    pub fn connect_dialog_mut(&mut self) -> Option<&mut ConnectDialog> {
        self.connect_dialog.as_mut()
    }

    /// Conecta direto, sem mostrar o diálogo (linha de comando e testes).
    /// O resultado chega nos próximos frames, como pelo diálogo.
    pub fn connect_to(&mut self, settings: ConnectionSettings, password: String) {
        let Some(connector) = self.connector.clone() else {
            return;
        };
        let dialog = self
            .connect_dialog
            .get_or_insert_with(|| ConnectDialog::new(&settings));
        dialog.start(connector, settings, password);
    }

    /// Servidor conectado no Object Explorer.
    pub fn connection(&self) -> Option<&Connected> {
        self.connection.as_ref()
    }

    /// Recebe o resultado de uma conexão em andamento.
    fn connect_step(&mut self) {
        let Some(dialog) = &mut self.connect_dialog else {
            return;
        };
        let was_connecting = dialog.is_connecting();
        match dialog.poll() {
            Some((connected, password)) => {
                if let Some(dialog) = self.connect_dialog.take() {
                    dialog.save_password(&connected.settings, &password);
                }
                self.make_room_for(&connected.settings);
                self.password = password;
                let name = connected.settings.server_name();
                self.recent.remember(&connected.settings);
                self.explorer.connect(connected.metadata.clone());
                self.workspace.open(Panel::ObjectExplorer);
                self.output.push(format!(
                    "Connected to {name} (PostgreSQL {}) as {}.",
                    connected.server_version, connected.settings.user
                ));
                self.status = format!("Connected to {name}.");
                self.connection = Some(connected);
                self.catalogs.clear();
            }
            None if was_connecting => {
                if let Some(error) = dialog.error() {
                    self.output.push(format!("Connection failed: {error}"));
                }
            }
            None => {}
        }
    }

    fn connect_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.connect_dialog else {
            return;
        };
        match dialog.show(ctx, self.recent.list()) {
            DialogAction::None => {}
            DialogAction::Cancel => self.connect_dialog = None,
            DialogAction::Connect(settings, password) => {
                if let Some(connector) = self.connector.clone() {
                    dialog.start(connector, settings, password);
                }
            }
        }
    }

    /// Texto da conexão na status bar.
    pub fn connection_label(&self) -> String {
        match &self.connection {
            Some(c) => format!("{} ({})", c.settings.server_name(), c.settings.user),
            None if self.explorer.is_connected() => "Demo server".to_string(),
            None => "Not connected".to_string(),
        }
    }

    /// Object Explorer, para testes e para a casca.
    pub fn explorer(&self) -> &ObjectExplorer {
        &self.explorer
    }

    /// Object Explorer, mutável.
    pub fn explorer_mut(&mut self) -> &mut ObjectExplorer {
        &mut self.explorer
    }

    /// Usa outro keymap (ex.: o padrão com o arquivo do usuário aplicado).
    pub fn with_keymap(mut self, keymap: Keymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Usa estes diálogos de arquivo (o aplicativo passa os nativos).
    pub fn with_file_dialogs(mut self, dialogs: impl FileDialogs + 'static) -> Self {
        self.dialogs = Box::new(dialogs);
        self
    }

    /// Restaura o layout lido do arquivo e o salva lá ao sair. Se o arquivo
    /// estava ruim, usa o layout padrão e avisa na status bar.
    pub fn with_layout_store(mut self, store: LayoutStore) -> Self {
        if let Some(saved) = &store.saved {
            self.workspace = pgstudio_docking::Workspace::from_layout(saved.docking.clone());
            self.window = saved.window;
        }
        if let Some(error) = &store.error {
            eprintln!("layout: {}: {error}", store.path().display());
            self.status =
                "The saved window layout could not be read; using the default layout.".to_string();
        }
        self.layout_store = Some(store);
        self
    }

    /// Retrato do layout atual, como seria salvo.
    pub fn current_layout(&self) -> SavedLayout {
        SavedLayout {
            window: self.window,
            docking: self.workspace.layout(|p| {
                !matches!(
                    p,
                    Panel::Query(_)
                        | Panel::TableDesigner(_)
                        | Panel::Diagram(_)
                        | Panel::ActivityMonitor
                        | Panel::EditRows(_)
                )
            }),
        }
    }

    /// Salva o layout no arquivo, se houver um. Chamado ao fechar a janela.
    pub fn save_layout(&mut self) {
        let Some(store) = &self.layout_store else {
            return;
        };
        if let Err(e) = store.save(&self.current_layout()) {
            eprintln!("layout: could not save {}: {e}", store.path().display());
        }
    }

    /// Guarda a geometria da janela; maximizada, mantém o tamanho normal.
    fn track_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, maximized, fullscreen) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.maximized, v.fullscreen)
        });
        if fullscreen == Some(true) {
            return;
        }
        if maximized == Some(true) {
            if let Some(w) = &mut self.window {
                w.maximized = true;
            }
            return;
        }
        if let Some(inner) = inner {
            self.window = Some(WindowGeometry {
                position: outer.map(|r| [r.min.x, r.min.y]),
                size: [inner.width(), inner.height()],
                maximized: false,
            });
        }
    }

    /// Janelas de consulta abertas.
    pub fn documents(&self) -> &Documents {
        &self.documents
    }

    /// Janelas de consulta, mutável.
    pub fn documents_mut(&mut self) -> &mut Documents {
        &mut self.documents
    }

    /// Abre um arquivo numa janela de consulta (ou traz a que já o tem).
    /// Em caso de erro, avisa na status bar e devolve `false`.
    pub fn open_file(&mut self, path: &std::path::Path) -> bool {
        if path.extension().is_some_and(|e| e == diagrams::EXTENSION) {
            let Some(provider) = self.explorer.provider() else {
                self.status = "Connect to the server of the diagram first.".to_string();
                return false;
            };
            return match self.diagrams.open_file(provider, path) {
                Ok(id) => {
                    self.workspace.open(Panel::Diagram(id));
                    true
                }
                Err(e) => {
                    self.status = format!("Could not open {}: {e}", path.display());
                    false
                }
            };
        }
        match self.documents.open(path) {
            Ok(id) => {
                self.activate_document(id);
                true
            }
            Err(e) => {
                self.status = format!("Could not open {}: {e}", path.display());
                false
            }
        }
    }

    /// Janela de consulta ativa.
    pub fn active_document(&self) -> Option<u32> {
        self.active_doc
    }

    /// Torna a janela de consulta ativa e traz a aba para a frente.
    pub fn activate_document(&mut self, id: u32) {
        if self.documents.get(id).is_some() {
            self.workspace.open(Panel::Query(id));
            self.active_doc = Some(id);
            self.focus_editor = Some(id);
        }
    }

    /// Se a pergunta "salvar alterações?" está aberta, e sobre quais
    /// documentos.
    pub fn confirm_pending(&self) -> Option<&[u32]> {
        self.confirm.as_ref().map(|c| c.save.as_slice())
    }

    /// Descreve o renderizador gráfico em uso no "About".
    pub fn with_renderer(mut self, renderer: impl Into<String>) -> Self {
        self.renderer = Some(renderer.into());
        self
    }

    /// Mostra uma mensagem inicial na status bar no lugar de "Ready".
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }

    /// Expande estes nós do Object Explorer, um por vez, à medida que
    /// carregam. Usado para screenshots e demonstrações.
    pub fn with_expanded(mut self, paths: impl IntoIterator<Item = String>) -> Self {
        self.pending_expand = paths
            .into_iter()
            .map(pgstudio_core::metadata::NodeId)
            .collect();
        self
    }

    /// Faz uma ação do menu de contexto no nó selecionado assim que as
    /// expansões de [`Self::with_expanded`] terminarem. Usado para
    /// screenshots.
    pub fn with_selected_action(
        mut self,
        make: fn(pgstudio_core::metadata::NodeId) -> pgstudio_object_explorer::Event,
    ) -> Self {
        self.selected_action = Some(make);
        self
    }

    fn expand_step(&mut self) {
        // A árvore também é atualizada quando o painel está escondido
        // (desafixado ou fechado), não só quando é desenhada.
        self.explorer.poll();
        // Espera a conexão (que pode estar a caminho) antes de expandir.
        if self.explorer.is_loading() || !self.explorer.is_connected() {
            return;
        }
        if let Some(id) = self.pending_expand.pop_front() {
            self.explorer.select(&id);
            self.explorer.expand(&id);
        } else if let Some(make) = self.selected_action.take()
            && let Some(node) = self.explorer.selected()
        {
            self.pending_events.push(make(node.id.clone()));
        }
    }

    /// Salva um PNG da janela em `path` assim que ela estiver desenhada e fecha
    /// o aplicativo. Usado pelo CI para mostrar a janela em cada sistema.
    pub fn with_screenshot(mut self, path: PathBuf) -> Self {
        self.screenshot = Some(Screenshot {
            path,
            frames: 0,
            requested: false,
        });
        self
    }

    /// Modo de medição (`cargo xtask bench`): avisa na saída padrão, com a
    /// linha [`BENCH_FIRST_FRAME`], assim que o primeiro frame foi exibido.
    /// Depois o app fica parado, para a memória em repouso ser medida.
    pub fn with_bench(mut self) -> Self {
        self.bench_frames = Some(0);
        self
    }

    /// Roda um cenário de medição (grade ou IntelliSense) e escreve o
    /// resultado na saída padrão. Veja o módulo [`mod@bench`].
    pub fn with_bench_scenario(mut self, scenario: bench::Scenario) -> Self {
        self.bench_run = Some(bench::Run::new(scenario));
        self
    }

    /// Avança o cenário de medição depois de um frame que levou `ui_ms` em
    /// [`Self::show`].
    fn bench_scenario_step(&mut self, ctx: &egui::Context, ui_ms: f64) {
        use bench::{Phase, Scenario};
        let Some(mut run) = self.bench_run.take() else {
            return;
        };
        match run.phase {
            Phase::Start if self.explorer.is_connected() => {
                let doc = match run.scenario {
                    Scenario::Grid => {
                        let id = self.documents.new_script(
                            format!("-- {} synthetic rows\nselect *;\n", bench::GRID_ROWS),
                            None,
                        );
                        if let Some(doc) = self.documents.get_mut(id) {
                            doc.session = Some(documents::Session(Arc::new(
                                bench::SyntheticSession::new(bench::GRID_ROWS),
                            )));
                        }
                        self.activate_document(id);
                        self.execute_query(ctx, Ask::Run);
                        id
                    }
                    Scenario::IntelliSense => {
                        let db = Some(bench::INTELLISENSE_DATABASE.to_string());
                        let id = self.documents.new_script(String::new(), db);
                        self.activate_document(id);
                        id
                    }
                    Scenario::Diagram | Scenario::DiagramLarge => {
                        let start = std::time::Instant::now();
                        let model = bench::synthetic_model(run.scenario.tables());
                        let id = self.diagrams.open_model("bench".into(), model, true);
                        self.workspace.open(Panel::Diagram(id));
                        run.load_ms = start.elapsed().as_secs_f64() * 1000.0;
                        id
                    }
                    Scenario::Designer => {
                        let provider = Arc::new(bench::ModelProvider(bench::designer_model()));
                        let target = pgstudio_core::metadata::DesignTarget {
                            database: "bench".into(),
                            schema: "public".into(),
                            table: Some(bench::DESIGNER_TABLE),
                        };
                        let node = pgstudio_core::metadata::NodeId("bench".into());
                        let id = self.designers.open(provider, target, node, ctx);
                        self.workspace.open(Panel::TableDesigner(id));
                        id
                    }
                    Scenario::Plan => {
                        let id = self.documents.new_script(
                            "select * from pedido join cliente on cliente.id = pedido.cliente_id \
                             where total > 100 order by total desc;\n"
                                .into(),
                            None,
                        );
                        if let Some(doc) = self.documents.get_mut(id) {
                            doc.session =
                                Some(documents::Session(Arc::new(bench::SyntheticPlanSession)));
                        }
                        self.activate_document(id);
                        self.execute_query(ctx, Ask::Plan);
                        id
                    }
                    Scenario::Activity => {
                        let mut monitor = activity::ActivityMonitor::new(
                            Arc::new(bench::SyntheticActivitySession::new()),
                            "bench".into(),
                        );
                        monitor.interval = 1;
                        monitor.refresh(ctx);
                        self.activity = Some(monitor);
                        self.workspace.open(Panel::ActivityMonitor);
                        0
                    }
                    Scenario::Edit => {
                        let session = Arc::new(bench::SyntheticEditSession);
                        self.open_edit_rows(session, 1, ctx)
                    }
                    Scenario::Export => {
                        let id = self.documents.new_script(
                            format!("-- {} synthetic rows\nselect *;\n", bench::EXPORT_ROWS),
                            None,
                        );
                        let session: Arc<dyn pgstudio_core::query::QuerySession> =
                            Arc::new(bench::SyntheticSession::new(bench::EXPORT_ROWS));
                        let path = std::env::temp_dir()
                            .join(format!("pgstudio-bench-{}.csv", std::process::id()));
                        if let Some(doc) = self.documents.get_mut(id) {
                            doc.session = Some(documents::Session(session.clone()));
                            match execution::Execution::start_to_file(
                                session,
                                "select *".into(),
                                0,
                                &path,
                                ctx,
                            ) {
                                Ok(exec) => doc.execution = Some(exec),
                                Err(e) => eprintln!("bench: {e}"),
                            }
                        }
                        self.activate_document(id);
                        id
                    }
                };
                run.phase = Phase::Loading {
                    since: std::time::Instant::now(),
                    doc,
                };
            }
            Phase::Start => {}
            Phase::Loading { since, doc } => {
                let ready = match run.scenario {
                    Scenario::Grid => self
                        .documents
                        .get(doc)
                        .and_then(|d| d.execution.as_ref())
                        .is_some_and(|e| !e.is_running()),
                    Scenario::IntelliSense => {
                        self.catalog_for(doc).is_some() && !self.catalogs.is_loading()
                    }
                    Scenario::Diagram | Scenario::DiagramLarge => true,
                    Scenario::Designer => {
                        self.designers.get(doc).is_some_and(|d| !d.is_loading())
                            && !self.designers.is_loading()
                    }
                    Scenario::Plan => self
                        .documents
                        .get(doc)
                        .and_then(|d| d.execution.as_ref())
                        .is_some_and(|e| !e.is_running() && !e.plans.is_empty()),
                    Scenario::Activity => self
                        .activity
                        .as_ref()
                        .is_some_and(|a| a.snapshot().is_some()),
                    Scenario::Edit => self.edit_rows.get(&doc).is_some_and(|e| !e.is_loading()),
                    // Mede enquanto o arquivo é escrito.
                    Scenario::Export => true,
                };
                if ready {
                    if !matches!(run.scenario, Scenario::Diagram | Scenario::DiagramLarge) {
                        run.load_ms = since.elapsed().as_secs_f64() * 1000.0;
                    }
                    run.phase = Phase::Running { doc, step: 0 };
                }
            }
            Phase::Running { doc, step } => match run.scenario {
                Scenario::Grid => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let set = self
                        .documents
                        .get_mut(doc)
                        .and_then(|d| d.execution.as_mut())
                        .and_then(|e| e.results.first_mut());
                    let rows = set.as_ref().map_or(0, |s| s.len());
                    let start = *run.scroll_start.get_or_insert_with(std::time::Instant::now);
                    let done = start.elapsed().as_secs_f64() / bench::GRID_SCROLL_SECONDS;
                    match set {
                        Some(set) if step == 0 || done < 1.0 => {
                            let last = rows.saturating_sub(40) as f64;
                            set.grid.top = last * done.min(1.0);
                            run.phase = Phase::Running {
                                doc,
                                step: step + 1,
                            };
                        }
                        _ => {
                            let line = run.result_line(&[("rows", rows as f64)]);
                            bench_print(&line);
                            run.phase = Phase::Done;
                        }
                    }
                }
                Scenario::Designer => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let Some(d) = self.designers.get_mut(doc) else {
                        run.phase = Phase::Done;
                        self.bench_run = Some(run);
                        return;
                    };
                    if step < bench::DESIGNER_EDITS {
                        let start = std::time::Instant::now();
                        d.add_column(&format!("extra_{step:03}"));
                        run.edit_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        run.phase = Phase::Running {
                            doc,
                            step: step + 1,
                        };
                    } else {
                        let start = std::time::Instant::now();
                        let script = d.change_script();
                        let script_ms = start.elapsed().as_secs_f64() * 1000.0;
                        let tables = d.model().tables.len();
                        let mut edits = run.edit_ms.clone();
                        let edit_p95 = bench::percentile(&mut edits, 95.0);
                        let line = run.result_line(&[
                            ("edit_p95", (edit_p95 * 100.0).round() / 100.0),
                            ("tables", tables as f64),
                            ("edits", step as f64),
                            ("statements", script.len() as f64),
                            ("script_ms", (script_ms * 100.0).round() / 100.0),
                        ]);
                        bench_print(&line);
                        run.phase = Phase::Done;
                    }
                }
                Scenario::Plan => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let start = *run.scroll_start.get_or_insert_with(std::time::Instant::now);
                    let done = start.elapsed().as_secs_f64() / bench::PLAN_SECONDS;
                    let plan = self
                        .documents
                        .get_mut(doc)
                        .and_then(|d| d.execution.as_mut())
                        .and_then(|e| e.plans.first_mut());
                    match plan {
                        Some((_, view)) if step == 0 || done < 1.0 => {
                            run.drawn_max = run.drawn_max.max(view.drawn());
                            let (zoom, offset) = bench::plan_camera(view.size, done);
                            view.zoom = zoom;
                            view.scroll_to = Some(offset);
                            run.phase = Phase::Running {
                                doc,
                                step: step + 1,
                            };
                        }
                        plan => {
                            let nodes = plan.map_or(0, |(p, _)| p.nodes.len());
                            let line = run.result_line(&[
                                ("nodes", nodes as f64),
                                ("drawn_max", run.drawn_max as f64),
                            ]);
                            bench_print(&line);
                            run.phase = Phase::Done;
                        }
                    }
                }
                Scenario::Activity => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let start = *run.scroll_start.get_or_insert_with(std::time::Instant::now);
                    let done = start.elapsed().as_secs_f64() / bench::ACTIVITY_SECONDS;
                    match &self.activity {
                        Some(a) if done < 1.0 => {
                            run.drawn_max = run.drawn_max.max(a.drawn_rows());
                            // Do pedido à leitura pronta, vista frame a frame.
                            match (a.is_refreshing(), run.refresh_start) {
                                (true, None) => run.refresh_start = Some(std::time::Instant::now()),
                                (false, Some(t)) => {
                                    run.edit_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                                    run.refresh_start = None;
                                }
                                _ => {}
                            }
                            run.phase = Phase::Running {
                                doc,
                                step: step + 1,
                            };
                        }
                        a => {
                            let sessions = a
                                .as_ref()
                                .and_then(|a| a.snapshot())
                                .map_or(0, |s| s.processes.len());
                            let mut refresh = run.edit_ms.clone();
                            let line = run.result_line(&[
                                ("sessions", sessions as f64),
                                ("refreshes", refresh.len() as f64),
                                (
                                    "refresh_p95",
                                    (bench::percentile(&mut refresh, 95.0) * 100.0).round() / 100.0,
                                ),
                                ("drawn_max", run.drawn_max as f64),
                            ]);
                            bench_print(&line);
                            run.phase = Phase::Done;
                        }
                    }
                }
                Scenario::Edit => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let start = *run.scroll_start.get_or_insert_with(std::time::Instant::now);
                    let done = start.elapsed().as_secs_f64() / bench::EDIT_SECONDS;
                    match self.edit_rows.get_mut(&doc) {
                        Some(e) if done < 1.0 => {
                            if !e.is_saving() {
                                // A gravação anterior voltou: conta a ida e
                                // volta, vista frame a frame.
                                if let Some(t) = run.refresh_start.take() {
                                    run.edit_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                                }
                                let edits = run.edit_ms.len();
                                let rows = e.row_count().max(1);
                                let row = edits % rows;
                                let col = 1 + edits % (bench::EDIT_COLUMNS - 1);
                                e.select(row, col, ctx);
                                e.set_cell(row, col, Some(format!("edit {edits}")));
                                e.select((row + 1) % rows, col, ctx);
                                if e.is_saving() {
                                    run.refresh_start = Some(std::time::Instant::now());
                                }
                            }
                            run.phase = Phase::Running {
                                doc,
                                step: step + 1,
                            };
                        }
                        e => {
                            let rows = e.map_or(0, |e| e.row_count());
                            let mut saves = run.edit_ms.clone();
                            let line = run.result_line(&[
                                ("rows", rows as f64),
                                ("saves", saves.len() as f64),
                                (
                                    "save_p95",
                                    (bench::percentile(&mut saves, 95.0) * 100.0).round() / 100.0,
                                ),
                            ]);
                            bench_print(&line);
                            run.phase = Phase::Done;
                        }
                    }
                }
                Scenario::Export => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let exec = self.documents.get(doc).and_then(|d| d.execution.as_ref());
                    match exec {
                        Some(e) if e.is_running() => {
                            run.phase = Phase::Running {
                                doc,
                                step: step + 1,
                            };
                        }
                        e => {
                            let ok = e.is_some_and(|e| {
                                e.outcome() == execution::Outcome::Succeeded && e.results.is_empty()
                            });
                            let (bytes, rows) = e
                                .and_then(|e| e.file())
                                .map(|p| {
                                    // Conta as linhas lendo aos pedaços, sem
                                    // trazer o arquivo para a memória medida.
                                    use std::io::BufRead;
                                    let (mut bytes, mut lines) = (0, 0);
                                    if let Ok(f) = std::fs::File::open(p) {
                                        let mut r = std::io::BufReader::new(f);
                                        loop {
                                            let buf = match r.fill_buf() {
                                                Ok(b) if !b.is_empty() => b,
                                                _ => break,
                                            };
                                            lines += buf.iter().filter(|b| **b == b'\n').count();
                                            let n = buf.len();
                                            bytes += n;
                                            r.consume(n);
                                        }
                                    }
                                    let _ = std::fs::remove_file(p);
                                    (bytes, lines.saturating_sub(1))
                                })
                                .unwrap_or((0, 0));
                            let export_ms = e.map_or(0.0, |e| e.elapsed().as_secs_f64() * 1000.0);
                            let line = run.result_line(&[
                                ("rows", if ok { rows as f64 } else { 0.0 }),
                                ("export_ms", export_ms.round()),
                                ("mb", (bytes as f64 / 1e5).round() / 10.0),
                            ]);
                            bench_print(&line);
                            run.phase = Phase::Done;
                        }
                    }
                }
                Scenario::Diagram | Scenario::DiagramLarge => {
                    if step > 0 {
                        run.record(ui_ms);
                    }
                    let canvas = self
                        .workspace
                        .group_rect(&Panel::Diagram(doc))
                        .map_or(egui::vec2(1000.0, 600.0), |r| r.size());
                    let start = *run.scroll_start.get_or_insert_with(std::time::Instant::now);
                    let done = start.elapsed().as_secs_f64() / bench::DIAGRAM_SECONDS;
                    match self.diagrams.get_mut(doc) {
                        Some(view) if step == 0 || done < 1.0 => {
                            let d = view.diagram_mut();
                            run.drawn_max = run.drawn_max.max(d.drawn().0);
                            d.camera = bench::diagram_camera(d.bounds(), canvas, done);
                            run.phase = Phase::Running {
                                doc,
                                step: step + 1,
                            };
                        }
                        view => {
                            let (tables, edges) = view.map_or((0, 0), |v| {
                                (v.diagram().nodes.len(), v.diagram().edges.len())
                            });
                            let line = run.result_line(&[
                                ("tables", tables as f64),
                                ("relationships", edges as f64),
                                ("drawn_max", run.drawn_max as f64),
                            ]);
                            bench_print(&line);
                            run.phase = Phase::Done;
                        }
                    }
                }
                Scenario::IntelliSense => {
                    // Espera o editor ganhar o foco antes da primeira tecla.
                    if run.warmup < 10 {
                        run.warmup += 1;
                    } else {
                        if step > 0 {
                            run.record(ui_ms);
                            if self
                                .documents
                                .get(doc)
                                .is_some_and(|d| d.editor.is_completing())
                            {
                                run.popup_keys += 1;
                            }
                        }
                        match bench::INTELLISENSE_SCRIPT.chars().nth(step) {
                            Some(c) => {
                                run.next_key = Some(c);
                                run.phase = Phase::Running {
                                    doc,
                                    step: step + 1,
                                };
                            }
                            None => {
                                let tables = self.catalog_for(doc).map_or(0, |c| {
                                    c.schemas.iter().map(|s| s.relations.len()).sum::<usize>()
                                });
                                let line = run.result_line(&[
                                    ("keys", step as f64),
                                    ("popup_keys", run.popup_keys as f64),
                                    ("relations", tables as f64),
                                ]);
                                bench_print(&line);
                                run.phase = Phase::Done;
                            }
                        }
                    }
                }
            },
            Phase::Done => {}
        }
        if !matches!(run.phase, Phase::Done) {
            ctx.request_repaint();
        }
        self.bench_run = Some(run);
    }

    fn bench_step(&mut self) {
        let Some(frames) = &mut self.bench_frames else {
            return;
        };
        *frames += 1;
        // O frame N é exibido antes de o N+1 começar.
        if *frames == 2 {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{BENCH_FIRST_FRAME}");
            let _ = out.flush();
        }
    }

    /// Tema em uso.
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// Troca o tema e aplica no contexto.
    pub fn set_theme(&mut self, ctx: &egui::Context, theme: Theme) {
        self.theme = theme;
        theme::apply(ctx, theme);
    }

    /// Se a janela "About" está aberta.
    pub fn about_open(&self) -> bool {
        self.about_open
    }

    /// Texto atual da status bar.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Último comando executado, por qualquer caminho.
    pub fn last_command(&self) -> Option<CommandId> {
        self.last_command
    }

    /// Se o painel está aberto (encaixado ou flutuando).
    pub fn is_panel_open(&self, panel: Panel) -> bool {
        self.workspace.is_open(&panel)
    }

    /// Se dois painéis estão como abas do mesmo grupo.
    pub fn panels_grouped(&self, a: Panel, b: Panel) -> bool {
        self.workspace.same_group(&a, &b)
    }

    /// Retângulo do grupo de abas onde o painel está (barra de abas e corpo).
    pub fn panel_rect(&self, panel: Panel) -> Option<egui::Rect> {
        self.workspace.group_rect(&panel)
    }

    /// Desafixa um painel (alfinete), como "Auto Hide" no menu da aba.
    pub fn auto_hide_panel(&mut self, panel: Panel) -> bool {
        self.workspace.auto_hide(&panel)
    }

    /// Fixa de novo um painel desafixado.
    pub fn dock_panel(&mut self, panel: Panel) {
        self.workspace.dock(&panel);
    }

    /// Se o painel está desafixado, numa faixa da borda.
    pub fn is_panel_hidden(&self, panel: Panel) -> bool {
        self.workspace.is_hidden(&panel)
    }

    /// Painel desafixado aberto por cima dos documentos.
    pub fn flyout_panel(&self) -> Option<Panel> {
        self.workspace.flyout().copied()
    }

    /// Retângulo do botão do painel na faixa de auto-hide.
    pub fn strip_button_rect(&self, panel: Panel) -> Option<egui::Rect> {
        self.workspace.strip_button_rect(&panel)
    }

    /// Pede para fechar um painel como o X da aba faz: um Table Designer
    /// com alterações pergunta antes.
    pub fn close_panel_request(&mut self, panel: Panel) {
        if let Panel::TableDesigner(id) = panel
            && let Some(d) = self.designers.get_mut(id)
            && !d.request_close()
        {
            self.workspace.open(panel);
            return;
        }
        if let Panel::Diagram(id) = panel
            && let Some(d) = self.diagrams.get_mut(id)
            && !d.request_close()
        {
            self.workspace.open(panel);
            return;
        }
        self.workspace.close(&panel);
    }

    /// Abre o painel (se preciso) e dá o foco a ele, como clicar na aba.
    pub fn focus_panel(&mut self, panel: Panel) {
        self.workspace.open(panel);
    }

    /// Fecha um painel, como o botão de fechar da aba.
    pub fn close_panel(&mut self, panel: Panel) {
        self.workspace.close(&panel);
    }

    /// Linhas do painel Output.
    pub fn output(&self) -> &[String] {
        &self.output
    }

    /// Se o comando já tem implementação. Os demais aparecem desabilitados
    /// nos menus e, pelo atalho, só avisam na status bar.
    pub fn is_available(cmd: CommandId) -> bool {
        use CommandId::*;
        matches!(
            cmd,
            Exit | ThemeLight
                | ThemeDark
                | FullScreen
                | About
                | ObjectExplorer
                | PropertiesWindow
                | Output
                | ResetWindowLayout
                | ConnectObjectExplorer
                | DisconnectObjectExplorer
                | RegisteredServers
                | NewQuery
                | OpenFile
                | CloseDocument
                | CloseAllDocuments
                | Save
                | SaveAs
                | SaveAll
                | AutoHide
                | AutoHideAll
                | CommentSelection
                | UncommentSelection
                | QuickFind
                | GoTo
                | ListMembers
                | CompleteWord
                | RefreshLocalCache
                | Execute
                | EstimatedPlan
                | ActualPlan
                | CancelQuery
                | ResultsToText
                | ResultsToGrid
                | ResultsToFile
                | DesignQuery
                | ActivityMonitor
                | SchemaCompare
                | ObjectInfo
        )
    }

    /// Executa um comando, venha ele de menu, toolbar ou atalho.
    pub fn execute(&mut self, ctx: &egui::Context, cmd: CommandId) {
        self.last_command = Some(cmd);
        self.status = READY.to_string();
        let mut available = Self::is_available(cmd);
        match cmd {
            // F5 no Object Explorer atualiza o nó selecionado, como no SSMS.
            CommandId::Execute
                if matches!(self.workspace.focused(), Some(Panel::ObjectExplorer)) =>
            {
                available = true;
                match self.explorer.selected().map(|n| n.id.clone()) {
                    Some(id) => self.explorer.refresh(&id),
                    None => self.status = "Select a node to refresh.".to_string(),
                }
            }
            CommandId::Exit => {
                let dirty = self.documents.dirty();
                if self.ask_dirty_designer() {
                    self.status = "Save or discard the table changes first.".to_string();
                } else if dirty.is_empty() {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.confirm = Some(Confirm {
                        save: dirty,
                        close: self.documents.ids(),
                        exit: true,
                    });
                }
            }
            CommandId::ThemeLight => self.set_theme(ctx, Theme::Light),
            CommandId::ThemeDark => self.set_theme(ctx, Theme::Dark),
            CommandId::FullScreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            CommandId::About => self.about_open = true,
            CommandId::ObjectExplorer => self.workspace.open(Panel::ObjectExplorer),
            CommandId::PropertiesWindow => self.workspace.open(Panel::Properties),
            CommandId::ActivityMonitor => self.open_activity_monitor(ctx),
            CommandId::SchemaCompare => self.open_compare(ctx, None),
            CommandId::Output => self.workspace.open(Panel::Output),
            CommandId::ResetWindowLayout => {
                self.workspace = panels::default_layout();
                for id in self.documents.ids() {
                    self.workspace.open(Panel::Query(id));
                }
                for id in self.designers.ids() {
                    self.workspace.open(Panel::TableDesigner(id));
                }
                for id in self.diagrams.ids() {
                    self.workspace.open(Panel::Diagram(id));
                }
                for id in self.edit_rows.keys() {
                    self.workspace.open(Panel::EditRows(*id));
                }
                if let Some(id) = self.active_doc {
                    self.workspace.open(Panel::Query(id));
                }
            }
            CommandId::AutoHide => {
                let target = match self.workspace.flyout() {
                    Some(p) => Some(*p),
                    None => self.workspace.focused().copied(),
                };
                match target {
                    Some(p) if self.workspace.is_hidden(&p) => self.workspace.dock(&p),
                    Some(p)
                        if !matches!(
                            p,
                            Panel::Query(_)
                                | Panel::TableDesigner(_)
                                | Panel::Diagram(_)
                                | Panel::ActivityMonitor
                                | Panel::EditRows(_)
                        ) =>
                    {
                        self.workspace.auto_hide(&p);
                    }
                    _ => self.status = "Select a tool window to auto hide.".to_string(),
                }
            }
            CommandId::AutoHideAll => self.workspace.auto_hide_all(),
            CommandId::NewQuery => {
                let id = self.documents.new_query();
                self.activate_document(id);
            }
            CommandId::OpenFile => {
                if let Some(path) = self.dialogs.open_path() {
                    self.open_file(&path);
                }
            }
            CommandId::Save | CommandId::SaveAs | CommandId::Undo | CommandId::Redo
                if self.focused_diagram().is_some() =>
            {
                available = true;
                let id = self.focused_diagram().expect("checado no guard");
                let Some(d) = self.diagrams.get_mut(id) else {
                    return;
                };
                match cmd {
                    CommandId::Save | CommandId::SaveAs => {
                        let path = match d.path() {
                            Some(p) if cmd == CommandId::Save => Some(p.to_path_buf()),
                            current => {
                                let current = current.map(|p| p.to_path_buf());
                                self.dialogs.save_path(&d.file_name(), current.as_deref())
                            }
                        };
                        d.save(path);
                    }
                    CommandId::Undo => d.undo(),
                    _ => d.redo(),
                }
            }
            CommandId::Save | CommandId::Undo | CommandId::Redo
                if self.focused_designer().is_some() =>
            {
                available = true;
                let id = self.focused_designer().expect("checado no guard");
                if let Some(d) = self.designers.get_mut(id) {
                    match cmd {
                        CommandId::Save => d.save(ctx),
                        CommandId::Undo => d.undo(),
                        _ => d.redo(),
                    }
                }
            }
            CommandId::Save | CommandId::SaveAs => match self.active_doc {
                Some(id) => {
                    self.save(id, cmd == CommandId::SaveAs);
                }
                None => self.status = "No query window is open.".to_string(),
            },
            CommandId::SaveAll => {
                for id in self.documents.dirty() {
                    if !self.save(id, false) {
                        break;
                    }
                }
            }
            CommandId::CloseDocument => match self.active_doc {
                Some(id) => self.request_close(vec![id], false),
                None => self.status = "No query window is open.".to_string(),
            },
            CommandId::CloseAllDocuments => self.request_close(self.documents.ids(), false),
            CommandId::CommentSelection
            | CommandId::UncommentSelection
            | CommandId::QuickFind
            | CommandId::ListMembers
            | CommandId::CompleteWord
            | CommandId::GoTo => match self.active_doc {
                Some(id) => self.editor_command(ctx, id, cmd),
                None => self.status = "No query window is open.".to_string(),
            },
            CommandId::DesignQuery => self.open_query_designer(ctx),
            CommandId::Execute => self.execute_query(ctx, Ask::Run),
            CommandId::EstimatedPlan => self.execute_query(ctx, Ask::Plan),
            CommandId::ObjectInfo => self.execute_query(ctx, Ask::Info),
            CommandId::ActualPlan => {
                match self.active_doc.and_then(|id| self.documents.get_mut(id)) {
                    Some(doc) => {
                        doc.actual_plan = !doc.actual_plan;
                        self.status = if doc.actual_plan {
                            "The actual execution plan will be included (the next run uses EXPLAIN ANALYZE).".to_string()
                        } else {
                            "The actual execution plan will not be included.".to_string()
                        };
                    }
                    None => self.status = "No query window is open.".to_string(),
                }
            }
            CommandId::CancelQuery => self.cancel_query(),
            CommandId::ResultsToText | CommandId::ResultsToGrid | CommandId::ResultsToFile => {
                let mode = match cmd {
                    CommandId::ResultsToText => execution::ResultsMode::Text,
                    CommandId::ResultsToFile => execution::ResultsMode::File,
                    _ => execution::ResultsMode::Grid,
                };
                match self.active_doc.and_then(|id| self.documents.get_mut(id)) {
                    Some(doc) => doc.results_mode = mode,
                    None => self.status = "No query window is open.".to_string(),
                }
            }
            CommandId::ConnectObjectExplorer => {
                self.workspace.open(Panel::ObjectExplorer);
                self.open_connect_dialog();
            }
            CommandId::DisconnectObjectExplorer => {
                if let Some(c) = self.connection.take() {
                    self.output
                        .push(format!("Disconnected from {}.", c.settings.server_name()));
                }
                self.explorer.disconnect();
                self.catalogs.clear();
                self.password.clear();
                self.promote_next_server();
            }
            CommandId::RegisteredServers => self.workspace.open(Panel::RegisteredServers),
            CommandId::RefreshLocalCache => {
                let db = self.active_doc.and_then(|id| self.document_database(id));
                match (
                    self.explorer.provider(),
                    db.or_else(|| self.default_database()),
                ) {
                    (Some(provider), Some(db)) => {
                        self.catalogs.refresh(&db, &provider, ctx);
                        self.status = "Refreshing local cache...".to_string();
                    }
                    _ => self.status = "Connect to a server to use IntelliSense.".to_string(),
                }
            }
            _ => {
                self.status = format!("\"{}\" is not available yet.", cmd.label());
            }
        }
        let note = if available {
            String::new()
        } else {
            " (not available yet)".to_string()
        };
        self.output.push(format!("> {}{note}", cmd.label()));
    }

    /// F5: roda a seleção (ou o documento inteiro) na conexão da janela de
    /// consulta ativa. Com [`Ask::Plan`] (Ctrl+L), pede só o plano estimado
    /// de cada comando, sem rodar nada; com [`Ask::Info`] (Alt+F1), mostra
    /// as informações do objeto selecionado ou sob o cursor.
    fn execute_query(&mut self, ctx: &egui::Context, ask: Ask) {
        let Some(id) = self.active_doc else {
            self.status = "No query window is open.".to_string();
            return;
        };
        self.activate_server_of(id);
        let queries = match &self.connection {
            Some(c) => (c.queries.clone(), c.settings.database.clone()),
            None if self.connector.is_none() && self.explorer.is_connected() => (
                Arc::new(pgstudio_core::query::Offline(
                    "The demo server cannot run queries. Connect to a PostgreSQL server.",
                )) as Arc<dyn pgstudio_core::query::QueryService>,
                "postgres".to_string(),
            ),
            None => {
                self.status = "Connect to a server to run queries.".to_string();
                self.open_connect_dialog();
                return;
            }
        };
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        if doc.execution.as_ref().is_some_and(|e| e.is_running()) {
            self.status = "A query is already executing in this window.".to_string();
            return;
        }
        let editor = panels::editor_id(id);
        let (sql, offset) = match pgstudio_sql_editor::selection(ctx, editor) {
            Some((a, b)) if a != b => {
                let (a, b) = (a.min(b), a.max(b));
                (doc.text.chars().skip(a).take(b - a).collect(), a)
            }
            _ => (doc.text.clone(), 0),
        };
        let (sql, offset) = if ask == Ask::Info {
            let name = match pgstudio_sql_editor::selection(ctx, editor) {
                Some((a, b)) if a != b => Some(sql.trim().to_string()),
                Some((_, cursor)) => object_info::name_at(&doc.text, cursor),
                // Editor ainda não desenhado: o cursor está no começo.
                None => object_info::name_at(&doc.text, 0),
            };
            match name {
                Some(n) if !n.is_empty() => (object_info::info_sql(&n), 0),
                _ => {
                    self.status =
                        "Select an object name, or place the cursor on one, for Object Information."
                            .to_string();
                    return;
                }
            }
        } else {
            (sql, offset)
        };
        doc.session.get_or_insert_with(|| {
            let db = doc.database.clone().unwrap_or(queries.1);
            documents::Session(queries.0.open(&db))
        });
        let kind = if ask == Ask::Info {
            Run::ObjectInfo
        } else if ask == Ask::Plan {
            Run::EstimatedPlan
        } else if doc.actual_plan {
            if execution::changes_data(&sql) {
                // O EXPLAIN ANALYZE executa o comando: pergunta antes.
                self.actual_confirm = Some(PendingActual {
                    doc: id,
                    sql,
                    offset,
                });
                return;
            }
            Run::ActualPlan { roll_back: false }
        } else {
            Run::Results
        };
        self.launch(ctx, id, sql, offset, kind);
    }

    /// Começa a execução na sessão (já aberta) da janela `id`.
    fn launch(&mut self, ctx: &egui::Context, id: u32, sql: String, offset: usize, kind: Run) {
        // Results to File: pergunta o arquivo antes de rodar.
        let to_file = matches!(kind, Run::Results)
            && self
                .documents
                .get(id)
                .is_some_and(|d| d.results_mode == execution::ResultsMode::File);
        let file = if to_file {
            match self.dialogs.save_results_path("Results.csv") {
                Some(path) => Some(path),
                None => {
                    self.status = "Query canceled: no file chosen for the results.".to_string();
                    return;
                }
            }
        } else {
            None
        };
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        let Some(session) = doc.session.as_ref().map(|s| s.0.clone()) else {
            return;
        };
        let exec = match kind {
            Run::Results => match &file {
                Some(path) => {
                    match execution::Execution::start_to_file(session, sql, offset, path, ctx) {
                        Ok(exec) => Some(exec),
                        Err(e) => {
                            let text = format!("Could not create {}: {e}", path.display());
                            self.output.push(text.clone());
                            self.status = text;
                            return;
                        }
                    }
                }
                None => Some(execution::Execution::start(session, sql, offset, ctx)),
            },
            Run::EstimatedPlan => execution::Execution::start_plan(session, sql, offset, ctx),
            Run::ActualPlan { roll_back } => {
                execution::Execution::start_actual_plan(session, sql, offset, roll_back, ctx)
            }
            Run::ObjectInfo => Some(execution::Execution::start_generated(session, sql, ctx)),
        };
        let Some(mut exec) = exec else {
            self.status = "There is no statement to show a plan for.".to_string();
            return;
        };
        exec.mode = doc.results_mode;
        doc.execution = Some(exec);
        self.status = execution::Outcome::Running.label().to_string();
        self.workspace.open(Panel::Query(id));
    }

    /// "Save Results As...": pergunta o arquivo e exporta o resultado `i`
    /// da janela `doc` numa thread, no formato da extensão.
    fn save_results_as(&mut self, ctx: &egui::Context, doc: u32, i: usize) {
        let Some(set) = self
            .documents
            .get(doc)
            .and_then(|d| d.execution.as_ref())
            .and_then(|e| e.results.get(i))
        else {
            return;
        };
        let store = set.shared_store();
        let rows = set.len();
        let Some(path) = self.dialogs.save_results_path("Results.csv") else {
            return;
        };
        let format = pgstudio_result_grid::export::Format::from_path(&path);
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        self.status = format!("Saving results to {}...", path.display());
        std::thread::spawn(move || {
            let result = std::fs::File::create(&path).and_then(|f| {
                pgstudio_result_grid::export::write_rows(
                    &store,
                    0..rows,
                    format,
                    pgstudio_result_grid::grid::INSERT_TABLE,
                    std::io::BufWriter::new(f),
                )
            });
            let text = match result {
                Ok(n) => format!(
                    "Saved {n} row{} to {}.",
                    if n == 1 { "" } else { "s" },
                    path.display()
                ),
                Err(e) => format!("Could not save the results to {}: {e}", path.display()),
            };
            let _ = tx.send(text);
            ctx.request_repaint();
        });
        self.exports.push(rx);
    }

    /// Recebe o fim dos "Save Results As" em andamento.
    fn export_step(&mut self) {
        let mut i = 0;
        while i < self.exports.len() {
            match self.exports[i].try_recv() {
                Ok(text) => {
                    self.status = text.clone();
                    self.output.push(text);
                    self.exports.remove(i);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.exports.remove(i);
                }
            }
        }
    }

    /// Exportações em andamento (testes).
    pub fn is_exporting(&self) -> bool {
        !self.exports.is_empty()
    }

    /// Pergunta antes do plano real de comandos que alteram dados, com a
    /// opção (padrão) de desfazer tudo no fim.
    fn actual_confirm_window(&mut self, ctx: &egui::Context) {
        if self.actual_confirm.is_none() {
            return;
        }
        let mut answer: Option<Option<bool>> = None;
        let modal = egui::Modal::new(egui::Id::new("actual_plan_dml")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Include Actual Execution Plan");
            ui.label(
                "The actual plan is collected with EXPLAIN ANALYZE, which runs the statements. \
                 This batch changes data (INSERT, UPDATE, DELETE or MERGE).",
            );
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    answer = Some(None);
                }
                if ui.button("Run and Keep Changes").clicked() {
                    answer = Some(Some(false));
                }
                if ui.button("Run and Roll Back").clicked() {
                    answer = Some(Some(true));
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(None);
        }
        let Some(answer) = answer else { return };
        let Some(pending) = self.actual_confirm.take() else {
            return;
        };
        match answer {
            Some(roll_back) => self.launch(
                ctx,
                pending.doc,
                pending.sql,
                pending.offset,
                Run::ActualPlan { roll_back },
            ),
            None => self.status = "The query was not executed.".to_string(),
        }
    }

    /// Alt+Break: cancela a execução da janela ativa.
    fn cancel_query(&mut self) {
        let running = self
            .active_doc
            .and_then(|id| self.documents.get(id))
            .and_then(|d| d.execution.as_ref())
            .filter(|e| e.is_running());
        match running {
            Some(e) => {
                e.cancel();
                self.status = "Canceling query...".to_string();
            }
            None => self.status = "No query is executing.".to_string(),
        }
    }

    /// Executa a janela de consulta ativa assim que houver conexão, como um
    /// F5 (para demonstrações e screenshots).
    pub fn with_run_on_connect(mut self) -> Self {
        self.run_on_connect = true;
        self
    }

    /// Se alguma janela de consulta está executando.
    pub fn is_executing(&self) -> bool {
        self.documents.ids().into_iter().any(|id| {
            self.documents
                .get(id)
                .and_then(|d| d.execution.as_ref())
                .is_some_and(|e| e.is_running())
        })
    }

    /// Recebe os eventos das execuções em andamento.
    fn execution_step(&mut self, ctx: &egui::Context) {
        if self.run_on_connect && self.connection.is_some() && self.active_doc.is_some() {
            self.run_on_connect = false;
            self.execute_query(ctx, Ask::Run);
        }
        let active = self.active_doc;
        for id in self.documents.ids() {
            let Some(doc) = self.documents.get_mut(id) else {
                continue;
            };
            let name = doc.name().to_string();
            let Some(exec) = &mut doc.execution else {
                continue;
            };
            let finished = exec.poll();
            if exec.is_running() {
                // Atualiza o relógio da faixa de status.
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
            if let Some((pos, message)) = exec.take_error_position() {
                pgstudio_sql_editor::set_selection(ctx, panels::editor_id(id), (pos, pos));
                doc.editor.mark_error(pos, message);
                doc.editor.focus_editor();
            }
            if finished {
                let outcome = exec.outcome();
                self.output.push(format!(
                    "{name}: {} ({})",
                    outcome.label(),
                    execution::format_elapsed(exec.elapsed(), true)
                ));
                if active == Some(id) {
                    self.status = outcome.label().to_string();
                }
            }
        }
    }

    /// Banco da conexão do Object Explorer (o servidor de demonstração usa
    /// `postgres`).
    fn default_database(&self) -> Option<String> {
        match &self.connection {
            Some(c) => Some(c.settings.database.clone()),
            None if self.explorer.is_connected() => Some("postgres".to_string()),
            None => None,
        }
    }

    /// Banco da janela de consulta: o da conexão dela, ou o padrão.
    fn document_database(&self, id: u32) -> Option<String> {
        let doc = self.documents.get(id)?;
        match &doc.session {
            Some(s) => Some(s.0.database()),
            None => doc.database.clone().or_else(|| self.default_database()),
        }
    }

    /// Ações do menu de contexto do Object Explorer.
    fn explorer_event(&mut self, ctx: &egui::Context, event: pgstudio_object_explorer::Event) {
        use pgstudio_core::metadata::ScriptAction;
        use pgstudio_object_explorer::{EDIT_TOP_ROWS, Event, SELECT_TOP_ROWS};
        let Some(provider) = self.explorer.provider() else {
            return;
        };
        let (node, action, run, read_only) = match event {
            Event::NewQuery(node) => {
                let db = provider.database_of(&node);
                let pending = self.query_on_open.take();
                let text = pending.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
                let len = text.chars().count();
                let id = self.documents.new_script(text, db);
                self.activate_document(id);
                // O Query Designer lê o texto selecionado (screenshots).
                if matches!(pending, Some((_, CommandId::DesignQuery))) {
                    pgstudio_sql_editor::set_selection(ctx, panels::editor_id(id), (0, len));
                }
                if std::mem::take(&mut self.design_query_on_open) {
                    self.open_query_designer(ctx);
                }
                match pending {
                    // Liga o plano real e roda (F5), como Ctrl+M e F5.
                    Some((_, CommandId::ActualPlan)) => {
                        if let Some(doc) = self.documents.get_mut(id) {
                            doc.actual_plan = true;
                        }
                        self.execute(ctx, CommandId::Execute);
                    }
                    Some((_, cmd)) => self.execute(ctx, cmd),
                    None => {}
                }
                return;
            }
            Event::NewTable(node) | Event::Design(node) => {
                match provider.design_target(&node) {
                    Some(target) => {
                        let id = self.designers.open(provider, target, node, ctx);
                        self.workspace.open(Panel::TableDesigner(id));
                    }
                    None => self.status = "This object cannot be designed.".to_string(),
                }
                return;
            }
            Event::ActivityMonitor(_) => {
                self.open_activity_monitor(ctx);
                return;
            }
            Event::Register(_) => {
                self.register_active_server();
                return;
            }
            Event::BackUp(node) => {
                match provider.database_of(&node) {
                    Some(db) => self.open_backup(ctx, db),
                    None => self.status = "Select a database to back up.".to_string(),
                }
                return;
            }
            Event::Restore(node) => {
                let db = provider.database_of(&node);
                self.open_restore(ctx, db);
                return;
            }
            Event::CompareSchema(node) => {
                let db = provider.database_of(&node);
                self.open_compare(ctx, db);
                return;
            }
            Event::Properties(node) => {
                match provider.securable(&node) {
                    Some(target) => self.open_security(ctx, target),
                    // Sem permissões: as propriedades ficam na janela F4.
                    None => self.workspace.open(Panel::Properties),
                }
                return;
            }
            Event::NewDiagram(node) => {
                match provider.database_of(&node) {
                    Some(db) => {
                        let all = self.diagram_all_tables;
                        let id = self.diagrams.open(provider, db, Some(node), all, ctx);
                        self.workspace.open(Panel::Diagram(id));
                    }
                    None => self.status = "Select a database to diagram.".to_string(),
                }
                return;
            }
            Event::SelectTop(node) => (node, ScriptAction::SelectTop(SELECT_TOP_ROWS), true, false),
            Event::EditTop(node) => {
                // Tabela com conexão de verdade: a grade editável. View (ou
                // o servidor de demonstração): o SELECT, só leitura.
                let table = provider
                    .design_target(&node)
                    .and_then(|t| Some((t.database, u32::try_from(t.table?).ok()?)));
                if let (Some((db, oid)), Some(c)) = (table, &self.connection) {
                    let session = c.queries.open(&db);
                    self.open_edit_rows(session, oid, ctx);
                    return;
                }
                (node, ScriptAction::SelectTop(EDIT_TOP_ROWS), true, true)
            }
            Event::Script(node, action) => (node, action, false, false),
        };
        let database = provider.database_of(&node);
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(provider.script(&node, action));
            repaint.request_repaint();
        });
        self.status = "Generating script...".to_string();
        self.scripts.push(PendingScript {
            rx,
            database,
            run,
            read_only,
        });
    }

    /// Abre as janelas dos scripts que ficaram prontos.
    fn script_step(&mut self, ctx: &egui::Context) {
        let mut i = 0;
        while i < self.scripts.len() {
            let result = match self.scripts[i].rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(
                    pgstudio_core::metadata::MetadataError("Script generation failed.".into()),
                ),
            };
            let pending = self.scripts.remove(i);
            match result {
                Ok(sql) => {
                    let id = self.documents.new_script(sql, pending.database);
                    self.activate_document(id);
                    self.status = "Ready".to_string();
                    if pending.run {
                        self.execute_query(ctx, Ask::Run);
                    }
                    if pending.read_only {
                        self.output.push(
                            "Edit Top Rows: only tables can be edited in the grid; \
                             the rows are read-only."
                                .to_string(),
                        );
                    }
                }
                Err(e) => {
                    self.output.push(format!("Script failed: {e}"));
                    self.status = e.to_string();
                }
            }
        }
    }

    /// Lê os catálogos que as janelas de consulta precisam e recebe os que
    /// chegaram.
    fn intellisense_step(&mut self, ctx: &egui::Context) {
        if let Some(provider) = self.explorer.provider() {
            for id in self.documents.ids() {
                if let Some(db) = self.document_database(id) {
                    self.catalogs.ensure(&db, &provider, ctx);
                }
            }
        }
        if self.list_members_on_load
            && let Some(id) = self.active_doc
            && self.catalog_for(id).is_some()
            && let Some(doc) = self.documents.get_mut(id)
        {
            self.list_members_on_load = false;
            let end = doc.text.trim_end().chars().count();
            pgstudio_sql_editor::set_selection(ctx, panels::editor_id(id), (end, end));
            doc.editor.list_members();
        }
        for loaded in self.catalogs.poll() {
            match loaded.result {
                Ok(n) => {
                    self.output.push(format!(
                        "IntelliSense: {n} objects loaded from {}.",
                        loaded.database
                    ));
                    if self.status == "Refreshing local cache..." {
                        self.status = "Local cache refreshed.".to_string();
                    }
                }
                Err(e) => self.output.push(format!(
                    "IntelliSense could not read the catalog of {}: {e}",
                    loaded.database
                )),
            }
        }
    }

    /// Catálogo do banco da janela, se já foi lido.
    pub fn catalog_for(&self, id: u32) -> Option<&pgstudio_core::catalog::Catalog> {
        self.catalogs.get(&self.document_database(id)?)
    }

    /// Abre o IntelliSense no fim da janela ativa assim que o catálogo dela
    /// chegar (para screenshots).
    pub fn with_list_members_on_load(mut self) -> Self {
        self.list_members_on_load = true;
        self
    }

    /// Se algum catálogo do IntelliSense está sendo lido.
    pub fn is_loading_catalog(&self) -> bool {
        self.catalogs.is_loading()
    }

    /// Comandos do editor de SQL no documento `id`.
    fn editor_command(&mut self, ctx: &egui::Context, id: u32, cmd: CommandId) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        let editor = panels::editor_id(id);
        match cmd {
            CommandId::CommentSelection | CommandId::UncommentSelection => {
                pgstudio_sql_editor::comment_selection(
                    ctx,
                    editor,
                    &mut doc.text,
                    cmd == CommandId::UncommentSelection,
                );
                doc.editor.focus_editor();
            }
            CommandId::QuickFind => {
                let selected = pgstudio_sql_editor::selected_text(ctx, editor, &doc.text);
                doc.editor.open_find(selected);
            }
            CommandId::GoTo => doc.editor.open_goto(),
            CommandId::ListMembers | CommandId::CompleteWord => doc.editor.list_members(),
            _ => {}
        }
        self.workspace.open(Panel::Query(id));
    }

    /// Salva o documento; `ask` pede o arquivo mesmo se já tiver um.
    /// Devolve `false` se o usuário cancelou ou a gravação falhou.
    fn save(&mut self, id: u32, ask: bool) -> bool {
        let Some(doc) = self.documents.get(id) else {
            return false;
        };
        let path = match doc.path() {
            Some(p) if !ask => p.to_path_buf(),
            current => match self.dialogs.save_path(doc.name(), current) {
                Some(p) => p,
                None => return false,
            },
        };
        let Some(doc) = self.documents.get_mut(id) else {
            return false;
        };
        match doc.save_to(&path) {
            Ok(()) => {
                self.status = "Item(s) Saved".to_string();
                true
            }
            Err(e) => {
                self.status = format!("Could not save {}: {e}", path.display());
                false
            }
        }
    }

    /// Fecha os documentos, perguntando antes se algum tem alterações.
    fn request_close(&mut self, close: Vec<u32>, exit: bool) {
        let save: Vec<u32> = close
            .iter()
            .copied()
            .filter(|id| self.documents.get(*id).is_some_and(|d| d.is_dirty()))
            .collect();
        if save.is_empty() {
            self.close_documents(&close);
        } else {
            self.confirm = Some(Confirm { save, close, exit });
        }
    }

    fn close_documents(&mut self, ids: &[u32]) {
        for id in ids {
            self.workspace.close(&Panel::Query(*id));
            self.documents.remove(*id);
        }
    }

    /// Responde à pergunta "salvar alterações?".
    pub fn answer(&mut self, ctx: &egui::Context, answer: Answer) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        match answer {
            Answer::Cancel => return,
            Answer::Yes => {
                for id in &confirm.save {
                    if !self.save(*id, false) {
                        return;
                    }
                }
            }
            Answer::No => {}
        }
        self.close_documents(&confirm.close);
        if confirm.exit {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let names: Vec<String> = confirm
            .save
            .iter()
            .filter_map(|id| self.documents.get(*id))
            .map(|d| {
                d.path()
                    .map_or(d.name().to_string(), |p| p.display().to_string())
            })
            .collect();
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("save_changes")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(APP_NAME);
            ui.label("Save changes to the following items?");
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for name in &names {
                    ui.label(name);
                }
            });
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    answer = Some(Answer::Cancel);
                }
                if ui.button("No").clicked() {
                    answer = Some(Answer::No);
                }
                if ui.button("Yes").clicked() {
                    answer = Some(Answer::Yes);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(Answer::Cancel);
        }
        if let Some(answer) = answer {
            self.answer(ctx, answer);
        }
    }

    /// Desenha a casca inteira num `Ui` que ocupa a janela.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.connect_step();
        self.expand_step();
        self.execution_step(&ctx);
        self.claude_step(&ctx);
        self.intellisense_step(&ctx);
        self.script_step(&ctx);
        let mut commands = if self.confirm.is_some()
            || self.connect_dialog.is_some()
            || self.actual_confirm.is_some()
            || self.claude.is_confirming()
            || self.activity.as_ref().is_some_and(|a| a.is_confirming())
            || self.security.is_some()
            || self.backup.is_some()
            || self.restore.is_some()
            || self.compare.is_some()
        {
            Vec::new()
        } else {
            self.keyboard(&ctx)
        };
        self.track_window(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            let dirty = self.documents.dirty();
            if !self.allow_close && self.ask_dirty_designer() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.allow_close || dirty.is_empty() {
                self.save_layout();
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.confirm.is_none() {
                    self.confirm = Some(Confirm {
                        save: dirty,
                        close: self.documents.ids(),
                        exit: true,
                    });
                }
            }
        }

        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                for menu in MENUS {
                    ui.menu_button(menu.title, |ui| {
                        self.menu_entries(ui, menu.entries, &mut commands);
                    });
                }
            });
        });
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui, &mut commands));
        egui::Panel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::palette(self.theme).accent)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| self.status_bar(ui));
        let server = self.connection_label();
        let default_db = self.default_database();
        let doc_servers = self.document_servers();
        let mut explorer_events = std::mem::take(&mut self.pending_events);
        let mut registered_actions = Vec::new();
        let mut activate = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::palette(self.theme).document_well))
            .show(ui, |ui| {
                let mut viewer = panels::Viewer {
                    output: &self.output,
                    explorer: &mut self.explorer,
                    documents: &mut self.documents,
                    designers: &mut self.designers,
                    diagrams: &mut self.diagrams,
                    activity: &mut self.activity,
                    edit_rows: &mut self.edit_rows,
                    focus_editor: self.focus_editor.take(),
                    commands: Vec::new(),
                    explorer_events: Vec::new(),
                    close_requests: Vec::new(),
                    save_results: Vec::new(),
                    editor_colors: match self.theme {
                        Theme::Light => pgstudio_sql_editor::Colors::LIGHT,
                        Theme::Dark => pgstudio_sql_editor::Colors::DARK,
                    },
                    server,
                    doc_servers,
                    catalogs: &self.catalogs,
                    default_database: default_db,
                    servers: &mut self.servers,
                    registered: &mut self.registered,
                    secrets: self.secrets.as_ref(),
                    activate_server: None,
                    registered_actions: Vec::new(),
                };
                self.workspace.show(ui, &mut viewer);
                activate = viewer.activate_server;
                registered_actions = viewer.registered_actions;
                commands.extend(viewer.commands);
                explorer_events.extend(viewer.explorer_events);
                let save_results = viewer.save_results;
                for id in viewer.close_requests {
                    self.workspace.open(Panel::Query(id));
                    self.active_doc = Some(id);
                    self.request_close(vec![id], false);
                }
                for (doc, i) in save_results {
                    self.save_results_as(ui.ctx(), doc, i);
                }
            });
        if let Some(i) = activate {
            self.activate_parked(i);
        }
        self.sync_documents();
        self.servers_step(&ctx, registered_actions);
        self.export_step();
        self.designer_step();
        self.diagram_step();
        self.activity_step(&ctx);
        self.edit_rows_step(&ctx);
        self.query_designer_step(&ctx);
        if self.diagrams.is_loading() {
            // Um diagrama aberto de arquivo não tem quem acorde a janela
            // quando o modelo chegar.
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        self.confirm_window(&ctx);
        self.actual_confirm_window(&ctx);
        self.claude_confirm_window(&ctx);
        self.security_window(&ctx);
        self.backup_window(&ctx);
        self.restore_window(&ctx);
        self.compare_window(&ctx);
        self.connect_window(&ctx);

        if self.about_open {
            let mut open = true;
            egui::Window::new(format!("About {APP_NAME}"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    ui.heading(APP_NAME);
                    ui.label(format!("Version {VERSION}"));
                    if let Some(renderer) = &self.renderer {
                        ui.label(format!("Renderer: {renderer}"));
                    }
                    ui.label("PostgreSQL management studio.");
                    ui.label("Licensed under MIT or Apache-2.0.");
                });
            self.about_open = open;
        }

        for cmd in commands {
            self.execute(&ctx, cmd);
        }
        for event in explorer_events {
            self.explorer_event(&ctx, event);
        }
    }

    /// Table Designer com o foco, se for o painel focado.
    fn focused_designer(&mut self) -> Option<u32> {
        match self.workspace.focused() {
            Some(Panel::TableDesigner(id)) => Some(*id),
            _ => None,
        }
    }

    /// Se há um Table Designer ou diagrama com alterações, traz ele para a
    /// frente e pergunta. Devolve se perguntou.
    fn ask_dirty_designer(&mut self) -> bool {
        if let Some(id) = self.designers.dirty().first().copied() {
            self.workspace.open(Panel::TableDesigner(id));
            if let Some(d) = self.designers.get_mut(id) {
                d.request_close();
            }
            return true;
        }
        if let Some(id) = self.diagrams.dirty().first().copied() {
            self.workspace.open(Panel::Diagram(id));
            if let Some(d) = self.diagrams.get_mut(id) {
                d.request_close();
            }
            return true;
        }
        false
    }

    /// Diagrama com o foco, se for o painel focado.
    fn focused_diagram(&mut self) -> Option<u32> {
        match self.workspace.focused() {
            Some(Panel::Diagram(id)) => Some(*id),
            _ => None,
        }
    }

    /// Table Designers abertos.
    pub fn designers(&self) -> &designer::Designers {
        &self.designers
    }

    /// Table Designers, mutável.
    pub fn designers_mut(&mut self) -> &mut designer::Designers {
        &mut self.designers
    }

    /// Abre o Query Designer para a janela de consulta ativa.
    fn open_query_designer(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.active_doc else {
            self.status = "Open a query window to design a query.".to_string();
            return;
        };
        let Some(provider) = self.explorer.provider() else {
            self.status = "Connect to a server to design a query.".to_string();
            return;
        };
        let Some(db) = self.document_database(doc) else {
            self.status = "Choose a database for the query window first.".to_string();
            return;
        };
        let selected = self
            .documents
            .get(doc)
            .and_then(|d| pgstudio_sql_editor::selected_text(ctx, panels::editor_id(doc), &d.text));
        let qd = query_designer::QueryDesigner::open(provider, db, doc, ctx)
            .with_sql(selected.unwrap_or_default());
        self.query_designer = Some(if self.diagram_all_tables {
            qd.with_all_tables()
        } else {
            qd
        });
    }

    /// A próxima janela aberta por "New Query" já abre o Query Designer
    /// (screenshots).
    pub fn with_design_query_on_open(mut self) -> Self {
        self.design_query_on_open = true;
        self
    }

    /// O "Schema Compare" já abre comparando `source` com `target`
    /// (screenshots).
    pub fn with_compare(mut self, source: String, target: String) -> Self {
        self.compare_target = Some((source, target));
        self
    }

    /// O "Restore Database" já abre com `file` listado (screenshots).
    pub fn with_restore_file(mut self, file: String) -> Self {
        self.restore_file = Some(file);
        self
    }

    /// A próxima janela aberta por "New Query" já vem com `text` e roda
    /// `cmd` (screenshots).
    pub fn with_query_on_open(mut self, text: String, cmd: CommandId) -> Self {
        self.query_on_open = Some((text, cmd));
        self
    }

    /// Query Designer aberto.
    pub fn query_designer(&self) -> Option<&query_designer::QueryDesigner> {
        self.query_designer.as_ref()
    }

    pub fn query_designer_mut(&mut self) -> Option<&mut query_designer::QueryDesigner> {
        self.query_designer.as_mut()
    }

    /// Desenha o Query Designer e, no OK, põe o SQL no lugar da seleção da
    /// janela de consulta.
    fn query_designer_step(&mut self, ctx: &egui::Context) {
        let Some(qd) = &mut self.query_designer else {
            return;
        };
        qd.show(ctx);
        let Some(result) = qd.take_result() else {
            return;
        };
        let doc = qd.doc;
        let replace = qd.replaces_selection();
        self.query_designer = None;
        let Some(sql) = result else {
            return;
        };
        let editor = panels::editor_id(doc);
        if let Some(d) = self.documents.get_mut(doc) {
            let selection = pgstudio_sql_editor::selection(ctx, editor).unwrap_or((0, 0));
            let inserted = if replace {
                query_designer::insert(&mut d.text, selection, &sql)
            } else {
                query_designer::insert_after(&mut d.text, selection, &sql)
            };
            pgstudio_sql_editor::set_selection(ctx, editor, inserted);
            d.editor.focus_editor();
        }
        self.activate_document(doc);
    }

    /// Database Diagrams abertos.
    pub fn diagrams(&self) -> &diagrams::Diagrams {
        &self.diagrams
    }

    /// Database Diagrams, mutável.
    pub fn diagrams_mut(&mut self) -> &mut diagrams::Diagrams {
        &mut self.diagrams
    }

    /// Novos diagramas abrem com todas as tabelas do banco, sem o "Add
    /// Table" (para screenshots).
    pub fn with_diagram_all_tables(mut self) -> Self {
        self.diagram_all_tables = true;
        self
    }

    /// Abre (ou traz para a frente) o Activity Monitor do servidor
    /// conectado, numa sessão própria no banco da conexão.
    fn open_activity_monitor(&mut self, ctx: &egui::Context) {
        if self.activity.is_none() {
            let Some(c) = &self.connection else {
                self.status = "Connect to a server to open the Activity Monitor.".to_string();
                if !self.explorer.is_connected() {
                    self.open_connect_dialog();
                }
                return;
            };
            let session = c.queries.open(&c.settings.database);
            let mut monitor = activity::ActivityMonitor::new(session, c.settings.server_name());
            monitor.refresh(ctx);
            self.activity = Some(monitor);
            self.output.push(format!(
                "Activity Monitor: monitoring {}.",
                c.settings.server_name()
            ));
        }
        self.workspace.open(Panel::ActivityMonitor);
    }

    /// Abre a janela de propriedades de segurança de `target`, numa
    /// sessão no banco do objeto (ou no da conexão, para roles).
    fn open_security(&mut self, ctx: &egui::Context, target: pgstudio_core::metadata::Securable) {
        let Some(c) = &self.connection else {
            self.status = "Connect to a PostgreSQL server to edit security.".to_string();
            return;
        };
        let db = target
            .database()
            .map(str::to_string)
            .unwrap_or_else(|| c.settings.database.clone());
        let session = c.queries.open(&db);
        self.security = Some(properties::SecurityDialog::open(target, session, ctx));
    }

    /// Abre "Back Up Database" para `database`.
    fn open_backup(&mut self, ctx: &egui::Context, database: String) {
        let Some(c) = &self.connection else {
            self.status = "Connect to a PostgreSQL server to back up a database.".to_string();
            return;
        };
        let session = c.queries.open(&database);
        self.backup = Some(backup::BackupDialog::open(
            database,
            c.settings.clone(),
            self.password.clone(),
            &c.server_version,
            session,
            ctx,
        ));
    }

    /// "Back Up Database", para testes.
    pub fn backup_dialog(&self) -> Option<&backup::BackupDialog> {
        self.backup.as_ref()
    }

    /// "Back Up Database", mutável (testes).
    pub fn backup_dialog_mut(&mut self) -> Option<&mut backup::BackupDialog> {
        self.backup.as_mut()
    }

    fn backup_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.backup else {
            return;
        };
        match dialog.show(ctx) {
            None => {}
            Some(backup::Request::Log(lines)) => self.output.extend(lines),
            Some(backup::Request::Closed) => self.backup = None,
        }
    }

    /// Abre "Restore Database"; `database` é o destino sugerido (None:
    /// banco novo).
    fn open_restore(&mut self, ctx: &egui::Context, database: Option<String>) {
        let Some(c) = &self.connection else {
            self.status = "Connect to a PostgreSQL server to restore a database.".to_string();
            return;
        };
        let session = c.queries.open(&c.settings.database);
        let mut dialog = restore::RestoreDialog::open(
            database,
            c.settings.clone(),
            self.password.clone(),
            &c.server_version,
            session,
            ctx,
        );
        if let Some(file) = self.restore_file.take() {
            dialog.options.file = file;
            dialog.list(ctx);
        }
        self.restore = Some(dialog);
    }

    /// Abre "Schema Compare"; `source` é a origem sugerida.
    fn open_compare(&mut self, ctx: &egui::Context, source: Option<String>) {
        let Some(c) = &self.connection else {
            self.status = "Connect to a PostgreSQL server to compare schemas.".to_string();
            return;
        };
        let mut dialog = schema_compare::SchemaCompare::open(
            source,
            &c.settings.database,
            c.metadata.clone(),
            c.queries.clone(),
            ctx,
        )
        .with_connector(self.connector.clone(), &c.settings);
        if let Some((source, target)) = self.compare_target.take() {
            dialog.source.database = source;
            dialog.target.database = target;
            dialog.compare(ctx);
        }
        self.compare = Some(dialog);
    }

    /// "Schema Compare", para testes.
    pub fn schema_compare(&self) -> Option<&schema_compare::SchemaCompare> {
        self.compare.as_ref()
    }

    /// "Schema Compare", mutável (testes).
    pub fn schema_compare_mut(&mut self) -> Option<&mut schema_compare::SchemaCompare> {
        self.compare.as_mut()
    }

    fn compare_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.compare else {
            return;
        };
        match dialog.show(ctx, &mut *self.dialogs) {
            None => {}
            Some(schema_compare::Request::Log(lines)) => self.output.extend(lines),
            Some(schema_compare::Request::Script(sql, db)) => {
                self.compare = None;
                let id = self.documents.new_script(sql, Some(db));
                self.activate_document(id);
                self.output
                    .push("Schema Compare: the script was opened in a new query window.".into());
            }
            Some(schema_compare::Request::Applied(db, lines)) => {
                self.output.extend(lines);
                self.status = format!("The changes were applied to {db}.");
                self.explorer
                    .refresh(&pgstudio_core::metadata::NodeId(format!(
                        "server/Databases/{db}"
                    )));
            }
            Some(schema_compare::Request::Closed) => self.compare = None,
        }
    }

    /// "Restore Database", para testes.
    pub fn restore_dialog(&self) -> Option<&restore::RestoreDialog> {
        self.restore.as_ref()
    }

    /// "Restore Database", mutável (testes).
    pub fn restore_dialog_mut(&mut self) -> Option<&mut restore::RestoreDialog> {
        self.restore.as_mut()
    }

    fn restore_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.restore else {
            return;
        };
        match dialog.show(ctx) {
            None => {}
            Some(restore::Request::Log(lines)) => self.output.extend(lines),
            Some(restore::Request::Restored(lines)) => {
                self.output.extend(lines);
                self.status = "The restore completed.".to_string();
                self.explorer
                    .refresh(&pgstudio_core::metadata::NodeId("server/Databases".into()));
            }
            Some(restore::Request::Closed) => self.restore = None,
        }
    }

    /// Janela de propriedades de segurança, para testes.
    pub fn security_dialog(&self) -> Option<&properties::SecurityDialog> {
        self.security.as_ref()
    }

    /// Janela de propriedades de segurança, mutável (testes).
    pub fn security_dialog_mut(&mut self) -> Option<&mut properties::SecurityDialog> {
        self.security.as_mut()
    }

    /// Desenha a janela de segurança e atende o que ela pedir.
    fn security_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.security else {
            return;
        };
        let roles = matches!(
            dialog.securable(),
            pgstudio_core::metadata::Securable::Role(_)
                | pgstudio_core::metadata::Securable::NewRole
        );
        match dialog.show(ctx) {
            None => {}
            Some(properties::Request::Closed) => self.security = None,
            Some(properties::Request::Script(sql, db)) => {
                let id = self.documents.new_script(sql, db);
                self.activate_document(id);
                self.output
                    .push("Security: the changes were scripted to a new query window.".into());
            }
            Some(properties::Request::Applied(log)) => {
                self.security = None;
                self.output.extend(log);
                self.status = "The changes were applied.".to_string();
                if roles {
                    self.explorer.refresh(&pgstudio_core::metadata::NodeId(
                        "server/Security/Roles".into(),
                    ));
                }
            }
        }
    }

    /// Activity Monitor aberto, para testes.
    pub fn activity_monitor(&self) -> Option<&activity::ActivityMonitor> {
        self.activity.as_ref()
    }

    /// Activity Monitor aberto, mutável (testes).
    pub fn activity_monitor_mut(&mut self) -> Option<&mut activity::ActivityMonitor> {
        self.activity.as_mut()
    }

    /// Abre uma janela Edit Top N Rows da tabela `oid` em `session`.
    pub fn open_edit_rows(
        &mut self,
        session: Arc<dyn pgstudio_core::query::QuerySession>,
        oid: u32,
        ctx: &egui::Context,
    ) -> u32 {
        let id = self.next_edit_rows;
        self.next_edit_rows += 1;
        let limit = pgstudio_object_explorer::EDIT_TOP_ROWS;
        self.edit_rows
            .insert(id, edit_rows::EditRows::new(session, oid, limit, ctx));
        self.workspace.open(Panel::EditRows(id));
        id
    }

    /// Janela Edit Top N Rows aberta, para testes.
    pub fn edit_rows(&self, id: u32) -> Option<&edit_rows::EditRows> {
        self.edit_rows.get(&id)
    }

    /// Janela Edit Top N Rows aberta, mutável (testes).
    pub fn edit_rows_mut(&mut self, id: u32) -> Option<&mut edit_rows::EditRows> {
        self.edit_rows.get_mut(&id)
    }

    /// Números das janelas Edit Top N Rows abertas.
    pub fn edit_rows_ids(&self) -> Vec<u32> {
        self.edit_rows.keys().copied().collect()
    }

    /// Recebe as leituras e gravações das janelas Edit Rows, leva o log ao
    /// Output e esquece as que fecharam.
    fn edit_rows_step(&mut self, ctx: &egui::Context) {
        let mut closed = Vec::new();
        for (id, e) in &mut self.edit_rows {
            e.step(ctx);
            self.output.extend(e.take_log());
            if e.take_closed() {
                closed.push(*id);
            }
        }
        for id in closed {
            self.workspace.close(&Panel::EditRows(id));
        }
        let open: Vec<u32> = self.edit_rows.keys().copied().collect();
        for id in open {
            if !self.workspace.is_open(&Panel::EditRows(id)) {
                self.edit_rows.remove(&id);
            }
        }
    }

    /// Atualiza o Activity Monitor; fechado o painel, ele some.
    fn activity_step(&mut self, ctx: &egui::Context) {
        if self.activity.is_some() && !self.workspace.is_open(&Panel::ActivityMonitor) {
            self.activity = None;
        }
        if let Some(a) = &mut self.activity {
            let log = a.step(ctx);
            self.output.extend(log);
        }
    }

    /// Recebe os modelos e gravações dos diagramas, leva o log ao Output,
    /// fecha os que pediram e esquece os de abas fechadas.
    fn diagram_step(&mut self) {
        self.diagrams.step();
        self.output.extend(self.diagrams.take_log());
        for node in self.diagrams.take_saved() {
            self.explorer.refresh(&node);
        }
        for id in self.diagrams.take_closed() {
            self.workspace.close(&Panel::Diagram(id));
        }
        for id in self.diagrams.ids() {
            if !self.workspace.is_open(&Panel::Diagram(id)) {
                self.diagrams.remove(id);
            }
        }
    }

    /// Recebe as cargas e gravações dos designers, leva o log ao Output,
    /// fecha os que pediram e esquece os de abas fechadas.
    fn designer_step(&mut self) {
        self.designers.step();
        self.output.extend(self.designers.take_log());
        for node in self.designers.take_saved() {
            self.explorer.refresh(&node);
        }
        for id in self.designers.take_closed() {
            self.workspace.close(&Panel::TableDesigner(id));
        }
        for id in self.designers.ids() {
            if !self.workspace.is_open(&Panel::TableDesigner(id)) {
                self.designers.remove(id);
            }
        }
    }

    /// Esquece documentos cuja aba foi fechada e acompanha a aba ativa.
    fn sync_documents(&mut self) {
        let workspace = &self.workspace;
        self.documents
            .retain(|id| workspace.is_open(&Panel::Query(id)));
        if let Some(Panel::Query(id)) = self.workspace.focused() {
            self.active_doc = Some(*id);
        }
        if self
            .active_doc
            .is_some_and(|id| self.documents.get(id).is_none())
        {
            self.active_doc = self.documents.ids().last().copied();
        }
    }

    /// Lê as teclas do frame e devolve os comandos completados.
    fn keyboard(&mut self, ctx: &egui::Context) -> Vec<CommandId> {
        let chords: Vec<_> = ctx.input(|i| i.events.iter().filter_map(keys::chord).collect());
        // Com um campo de texto em foco, as teclas de edição são dele.
        let typing = ctx.egui_wants_keyboard_input();
        let mut commands = Vec::new();
        for chord in chords {
            if typing && self.chords.pending().is_none() && keys::is_text_editing(chord) {
                continue;
            }
            match self.chords.feed(&self.keymap, chord) {
                Match::Command(cmd) => commands.push(cmd),
                Match::Pending(first) => {
                    self.status =
                        format!("({first}) was pressed. Waiting for second key of chord...");
                }
                Match::Unbound(first, second) => {
                    self.status =
                        format!("The key combination ({first}, {second}) is not a command.");
                }
                Match::None => {}
            }
        }
        commands
    }

    /// Desenha as entradas de um menu e guarda o comando clicado.
    fn menu_entries(&self, ui: &mut egui::Ui, entries: &[Entry], clicked: &mut Vec<CommandId>) {
        for entry in entries {
            match *entry {
                Entry::Item(cmd) => {
                    let mode = self
                        .active_doc
                        .and_then(|id| self.documents.get(id))
                        .map(|d| d.results_mode);
                    let actual = self
                        .active_doc
                        .and_then(|id| self.documents.get(id))
                        .is_some_and(|d| d.actual_plan);
                    let selected = (cmd == CommandId::ActualPlan && actual)
                        || matches!(
                            (cmd, self.theme),
                            (CommandId::ThemeLight, Theme::Light)
                                | (CommandId::ThemeDark, Theme::Dark)
                        )
                        || matches!(
                            (cmd, mode),
                            (CommandId::ResultsToText, Some(execution::ResultsMode::Text))
                                | (CommandId::ResultsToGrid, Some(execution::ResultsMode::Grid))
                                | (CommandId::ResultsToFile, Some(execution::ResultsMode::File))
                        );
                    let mut button = egui::Button::new(cmd.label()).selected(selected);
                    if let Some(s) = self.keymap.shortcut_for(cmd) {
                        button = button.shortcut_text(s.to_string());
                    }
                    if ui.add_enabled(Self::is_available(cmd), button).clicked() {
                        clicked.push(cmd);
                        ui.close();
                    }
                }
                Entry::Submenu { label, entries } => {
                    ui.menu_button(label, |ui| self.menu_entries(ui, entries, clicked));
                }
                Entry::Separator => {
                    ui.separator();
                }
            }
        }
    }

    /// Toolbar padrão do SSMS.
    fn toolbar(&self, ui: &mut egui::Ui, clicked: &mut Vec<CommandId>) {
        let database = self
            .active_doc
            .and_then(|id| self.document_database(id))
            .unwrap_or_else(|| "(no database)".to_string());
        ui.horizontal(|ui| {
            for tool in TOOLBAR {
                match *tool {
                    Tool::Button {
                        icon,
                        text,
                        command,
                    } => {
                        let caption = if text.is_empty() {
                            icon.to_string()
                        } else {
                            format!("{icon} {text}")
                        };
                        let tip = match self.keymap.shortcut_for(command) {
                            Some(s) => format!("{} ({s})", command.label()),
                            None => command.label().to_string(),
                        };
                        let pressed = command == CommandId::ActualPlan
                            && self
                                .active_doc
                                .and_then(|id| self.documents.get(id))
                                .is_some_and(|d| d.actual_plan);
                        let button = egui::Button::new(caption).selected(pressed);
                        let response = ui
                            .add_enabled(Self::is_available(command), button)
                            .on_hover_text(&tip)
                            .on_disabled_hover_text(&tip);
                        if response.clicked() {
                            clicked.push(command);
                        }
                    }
                    Tool::Database => {
                        ui.add_enabled_ui(false, |ui| {
                            egui::ComboBox::from_id_salt("database")
                                .selected_text(&database)
                                .width(160.0)
                                .show_ui(ui, |_| {});
                        });
                    }
                    Tool::Separator => {
                        ui.separator();
                    }
                }
            }
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let color = theme::palette(self.theme).on_accent;
        let connection = self.connection_label();
        let cursor = self
            .active_doc
            .and_then(|id| self.documents.get(id))
            .and_then(|d| d.cursor);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.status).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{APP_NAME} {VERSION}")).color(color));
                ui.separator();
                ui.label(RichText::new(connection).color(color));
                if let Some((line, col)) = cursor {
                    ui.separator();
                    ui.label(RichText::new(format!("Ln {line}   Col {col}")).color(color));
                }
            });
        });
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let executing = self.is_executing() || self.run_on_connect;
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        // Espera o layout assentar e a árvore carregar antes de capturar.
        shot.frames += 1;
        let connecting = self
            .connect_dialog
            .as_ref()
            .is_some_and(ConnectDialog::is_connecting);
        let expanding = !self.pending_expand.is_empty() && self.explorer.is_connected();
        let busy = self.explorer.is_loading()
            || expanding
            || connecting
            || executing
            || self.catalogs.is_loading()
            || self.list_members_on_load
            || self.selected_action.is_some()
            || !self.scripts.is_empty()
            || self.designers.is_loading()
            || self.diagrams.is_loading()
            || self
                .edit_rows
                .values()
                .any(|e| e.is_loading() || e.is_saving())
            || !self.exports.is_empty()
            || self.query_designer.as_ref().is_some_and(|q| q.is_loading())
            || self.compare.as_ref().is_some_and(|c| c.is_busy());
        // Conta os frames parados: o que acabou de aparecer (um diálogo
        // modal entra com fade) precisa de alguns frames para assentar.
        if busy {
            shot.frames = 0;
        }
        if !shot.requested {
            ctx.request_repaint();
        }
        if !shot.requested && shot.frames >= 10 && !busy {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let result = image::RgbaImage::from_raw(w as u32, h as u32, image.as_raw().to_vec())
                .ok_or_else(|| "tamanho de imagem inválido".to_string())
                .and_then(|img| img.save(&shot.path).map_err(|e| e.to_string()));
            match result {
                Ok(()) => println!("screenshot={}", shot.path.display()),
                Err(e) => eprintln!("falha ao salvar o screenshot: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for ShellApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(c) = self.bench_run.as_mut().and_then(|r| r.next_key.take()) {
            raw_input.events.push(egui::Event::Text(c.to_string()));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.bench_step();
        let started = std::time::Instant::now();
        self.show(ui);
        let ui_ms = started.elapsed().as_secs_f64() * 1000.0;
        let ctx = ui.ctx().clone();
        self.bench_scenario_step(&ctx, ui_ms);
        if !self.pending_expand.is_empty() {
            ctx.request_repaint();
        }
        self.screenshot_step(&ctx);
    }
}

/// Escreve uma linha de medição na saída padrão, já descarregada.
fn bench_print(line: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Fontes padrão do egui mais os ícones Phosphor.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
