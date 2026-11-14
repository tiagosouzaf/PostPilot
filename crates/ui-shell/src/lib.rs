//! Casca da interface do PGStudio: janela principal, barra de menus, toolbar,
//! status bar, tema e atalhos de teclado.
//!
//! Só esta camada conhece o egui. Menus, toolbar e atalhos viram
//! [`CommandId`] e passam por [`ShellApp::execute`]. A área central tem o
//! docking com o Object Explorer, as tool windows e as janelas de consulta.

pub mod connect;
pub mod documents;
pub mod execution;
pub mod intellisense;
pub mod keys;
pub mod layout;
pub mod menus;
pub mod panels;
pub mod results;
pub mod theme;

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
    /// Janela de consulta ativa (a última que teve foco).
    active_doc: Option<u32>,
    focus_editor: Option<u32>,
    dialogs: Box<dyn FileDialogs>,
    confirm: Option<Confirm>,
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
    recent: RecentConnections,
    /// Executar a janela ativa assim que conectar (demonstração e
    /// screenshots).
    run_on_connect: bool,
    /// Catálogos para o IntelliSense, por banco.
    catalogs: intellisense::Catalogs,
    /// Abrir o IntelliSense no fim da janela ativa quando o catálogo chegar
    /// (screenshots).
    list_members_on_load: bool,
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
            active_doc: None,
            focus_editor: None,
            dialogs: Box::new(NoDialogs),
            confirm: None,
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
            self.connect_dialog = Some(ConnectDialog::new(&settings));
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
            Some(connected) => {
                self.connect_dialog = None;
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
            docking: self.workspace.layout(|p| !matches!(p, Panel::Query(_))),
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
                | CancelQuery
                | ResultsToText
                | ResultsToGrid
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
                if dirty.is_empty() {
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
            CommandId::Output => self.workspace.open(Panel::Output),
            CommandId::ResetWindowLayout => {
                self.workspace = panels::default_layout();
                for id in self.documents.ids() {
                    self.workspace.open(Panel::Query(id));
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
                    Some(p) if !matches!(p, Panel::Query(_)) => {
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
            CommandId::Execute => self.execute_query(ctx),
            CommandId::CancelQuery => self.cancel_query(),
            CommandId::ResultsToText | CommandId::ResultsToGrid => {
                let mode = if cmd == CommandId::ResultsToText {
                    execution::ResultsMode::Text
                } else {
                    execution::ResultsMode::Grid
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
            }
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
    /// consulta ativa.
    fn execute_query(&mut self, ctx: &egui::Context) {
        let Some(id) = self.active_doc else {
            self.status = "No query window is open.".to_string();
            return;
        };
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
        let session = doc
            .session
            .get_or_insert_with(|| documents::Session(queries.0.open(&queries.1)))
            .0
            .clone();
        let mut exec = execution::Execution::start(session, sql, offset, ctx);
        exec.mode = doc.results_mode;
        doc.execution = Some(exec);
        self.status = execution::Outcome::Running.label().to_string();
        self.workspace.open(Panel::Query(id));
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
            self.execute_query(ctx);
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
            None => self.default_database(),
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
        self.execution_step(&ctx);
        self.intellisense_step(&ctx);
        let mut commands = if self.confirm.is_some() || self.connect_dialog.is_some() {
            Vec::new()
        } else {
            self.keyboard(&ctx)
        };
        self.track_window(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            let dirty = self.documents.dirty();
            if self.allow_close || dirty.is_empty() {
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
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::palette(self.theme).document_well))
            .show(ui, |ui| {
                let mut viewer = panels::Viewer {
                    output: &self.output,
                    explorer: &mut self.explorer,
                    documents: &mut self.documents,
                    focus_editor: self.focus_editor.take(),
                    commands: Vec::new(),
                    close_requests: Vec::new(),
                    editor_colors: match self.theme {
                        Theme::Light => pgstudio_sql_editor::Colors::LIGHT,
                        Theme::Dark => pgstudio_sql_editor::Colors::DARK,
                    },
                    server,
                    catalogs: &self.catalogs,
                    default_database: default_db,
                };
                self.workspace.show(ui, &mut viewer);
                commands.extend(viewer.commands);
                for id in viewer.close_requests {
                    self.workspace.open(Panel::Query(id));
                    self.active_doc = Some(id);
                    self.request_close(vec![id], false);
                }
            });
        self.sync_documents();
        self.confirm_window(&ctx);
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
                    ui.label("PostgreSQL management studio.");
                    ui.label("Licensed under MIT or Apache-2.0.");
                });
            self.about_open = open;
        }

        for cmd in commands {
            self.execute(&ctx, cmd);
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
                    let selected = matches!(
                        (cmd, self.theme),
                        (CommandId::ThemeLight, Theme::Light) | (CommandId::ThemeDark, Theme::Dark)
                    ) || matches!(
                        (cmd, mode),
                        (CommandId::ResultsToText, Some(execution::ResultsMode::Text))
                            | (CommandId::ResultsToGrid, Some(execution::ResultsMode::Grid))
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
                        let button = egui::Button::new(caption);
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
                                .selected_text("(no database)")
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
            || self.list_members_on_load;
        if !shot.requested && shot.frames >= 5 && !busy {
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.bench_step();
        self.expand_step();
        self.show(ui);
        let ctx = ui.ctx().clone();
        if !self.pending_expand.is_empty() {
            ctx.request_repaint();
        }
        self.screenshot_step(&ctx);
    }
}

/// Fontes padrão do egui mais os ícones Phosphor.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
