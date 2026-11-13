//! Casca da interface do PGStudio: janela principal, barra de menus, toolbar,
//! status bar, tema e atalhos de teclado.
//!
//! Só esta camada conhece o egui. Menus, toolbar e atalhos viram
//! [`CommandId`] e passam por [`ShellApp::execute`]. A área central tem o
//! docking com o Object Explorer, as tool windows e as janelas de consulta.

pub mod documents;
pub mod keys;
pub mod layout;
pub mod menus;
pub mod panels;
pub mod theme;

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use pgstudio_core::keymap::{ChordMatcher, Keymap, Match};
use pgstudio_core::{APP_NAME, CommandId, Theme, VERSION};

use documents::{Documents, FileDialogs, NoDialogs};
use layout::{LayoutStore, SavedLayout, WindowGeometry};
use menus::{Entry, MENUS, TOOLBAR, Tool};
use panels::Panel;
use pgstudio_docking::Workspace;
use pgstudio_object_explorer::ObjectExplorer;
use pgstudio_object_explorer::fake::FakeProvider;
use std::sync::Arc;

const READY: &str = "Ready";

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
        if self.explorer.is_loading() {
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
        )
    }

    /// Executa um comando, venha ele de menu, toolbar ou atalho.
    pub fn execute(&mut self, ctx: &egui::Context, cmd: CommandId) {
        self.last_command = Some(cmd);
        self.status = READY.to_string();
        match cmd {
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
            CommandId::ConnectObjectExplorer => {
                self.workspace.open(Panel::ObjectExplorer);
                if !self.explorer.is_connected() {
                    self.connect_demo();
                }
            }
            CommandId::DisconnectObjectExplorer => self.explorer.disconnect(),
            _ => {
                self.status = format!("\"{}\" is not available yet.", cmd.label());
            }
        }
        let note = if Self::is_available(cmd) {
            String::new()
        } else {
            " (not available yet)".to_string()
        };
        self.output.push(format!("> {}{note}", cmd.label()));
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
        let mut commands = if self.confirm.is_some() {
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
                    let selected = matches!(
                        (cmd, self.theme),
                        (CommandId::ThemeLight, Theme::Light) | (CommandId::ThemeDark, Theme::Dark)
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
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.status).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{APP_NAME} {VERSION}")).color(color));
                ui.separator();
                ui.label(RichText::new("Not connected").color(color));
            });
        });
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        // Espera o layout assentar e a árvore carregar antes de capturar.
        shot.frames += 1;
        let busy = self.explorer.is_loading() || !self.pending_expand.is_empty();
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
