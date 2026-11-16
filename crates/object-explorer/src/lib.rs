//! Object Explorer: árvore de objetos do servidor, carregada sob demanda.
//!
//! Os filhos de um nó só são pedidos ao [`MetadataProvider`] quando o nó é
//! expandido, numa thread separada, para a interface nunca esperar o banco.
//! Só as linhas visíveis são desenhadas, então milhares de nós não pesam.

pub mod fake;

use pgstudio_i18n::{tr, tr_noop, trf};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, Property, ScriptAction,
};

/// Estado de carga dos filhos de um nó (ou das raízes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load {
    NotLoaded,
    Loading,
    Loaded(Vec<NodeId>),
    Failed(String),
}

struct Entry {
    node: ObjectNode,
    expanded: bool,
    children: Load,
}

/// Quem pediu a carga: as raízes ou um nó.
type Target = Option<NodeId>;
type Reply = (Target, Result<Vec<ObjectNode>, MetadataError>);
type PropertiesReply = (NodeId, Result<Vec<Property>, MetadataError>);

/// Propriedades de um nó para a janela Properties, lidas quando ele é
/// mostrado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Properties {
    Loading,
    Loaded(Vec<Property>),
    Failed(String),
}

/// Algo que o usuário pediu pelo Object Explorer e que a casca trata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// "New Query" no menu de contexto (no banco do nó).
    NewQuery(NodeId),
    /// "Select Top 1000 Rows".
    SelectTop(NodeId),
    /// "Edit Top 200 Rows": grade editável numa tabela, só leitura numa
    /// view.
    EditTop(NodeId),
    /// "Import CSV..." numa tabela: carregar um arquivo CSV nela.
    ImportCsv(NodeId),
    /// "Script as" > CREATE ou DROP numa janela de consulta nova.
    Script(NodeId, ScriptAction),
    /// "New Table..." na pasta Tables: abre o Table Designer vazio.
    NewTable(NodeId),
    /// "Design" numa tabela: abre o Table Designer dela.
    Design(NodeId),
    /// "New Database Diagram" na pasta Database Diagrams: escolhe as
    /// tabelas e abre o diagrama delas.
    NewDiagram(NodeId),
    /// "Register..." no servidor: registra a conexão dele no Registered
    /// Servers.
    Register(NodeId),
    /// "Move To" no servidor: a pasta escolhida, pelo índice em
    /// [`ObjectExplorer::set_move_targets`].
    MoveToFolder(usize),
    /// "Activity Monitor" no servidor.
    ActivityMonitor(NodeId),
    /// "Properties" de um objeto (roles e objetos com permissões abrem a
    /// janela de segurança; os outros, a janela Properties) ou "New
    /// Role..." na pasta Roles.
    Properties(NodeId),
    /// Tasks > Back Up... num banco.
    BackUp(NodeId),
    /// Tasks > Restore... num banco, ou "Restore Database..." na pasta
    /// Databases (banco novo).
    Restore(NodeId),
    /// Tasks > Compare Schema... num banco (ele é a origem).
    CompareSchema(NodeId),
    /// "Ask Claude" numa tabela ou view: abre o painel do Claude com a
    /// pergunta sobre ela.
    AskClaude(NodeId),
}

/// Linhas do "Select Top N Rows", como no SSMS.
pub const SELECT_TOP_ROWS: u32 = 1000;
/// Linhas do "Edit Top N Rows".
pub const EDIT_TOP_ROWS: u32 = 200;

/// Altura de cada linha da árvore.
const ROW_HEIGHT: f32 = 20.0;
/// Recuo por nível.
const INDENT: f32 = 16.0;

/// A árvore e as cargas em andamento.
/// Como o nó do servidor aparece quando a conexão está registrada.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerLabel {
    /// Nome do registro ("tiib"); o nome do servidor vai para a dica.
    pub name: String,
    /// Ambiente, já traduzido, e a cor dele: a etiqueta na frente do nome.
    pub environment: Option<(String, [u8; 3])>,
}

pub struct ObjectExplorer {
    /// O registro do servidor desta árvore, se ele está registrado.
    server_label: Option<ServerLabel>,
    /// Pastas do "Move To" do nó do servidor.
    move_targets: Vec<String>,
    provider: Option<Arc<dyn MetadataProvider>>,
    roots: Load,
    entries: HashMap<NodeId, Entry>,
    selected: Option<NodeId>,
    /// Nós a expandir assim que chegarem (a árvore de uma sessão anterior).
    restore: HashSet<NodeId>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    properties: HashMap<NodeId, Properties>,
    props_tx: Sender<PropertiesReply>,
    props_rx: Receiver<PropertiesReply>,
    /// Contexto do egui, para pedir um novo frame quando uma carga termina.
    ctx: Option<egui::Context>,
}

impl Default for ObjectExplorer {
    fn default() -> Self {
        let (tx, rx) = channel();
        let (props_tx, props_rx) = channel();
        ObjectExplorer {
            server_label: None,
            move_targets: Vec::new(),
            provider: None,
            roots: Load::NotLoaded,
            entries: HashMap::new(),
            selected: None,
            restore: HashSet::new(),
            tx,
            rx,
            properties: HashMap::new(),
            props_tx,
            props_rx,
            ctx: None,
        }
    }
}

impl ObjectExplorer {
    /// Conecta a um provedor e começa a carregar as raízes.
    /// Provedor conectado (o IntelliSense lê o catálogo por ele).
    pub fn provider(&self) -> Option<Arc<dyn MetadataProvider>> {
        self.provider.clone()
    }

    pub fn connect(&mut self, provider: Arc<dyn MetadataProvider>) {
        self.disconnect();
        self.provider = Some(provider);
        self.request(None);
    }

    /// Desconecta e limpa a árvore. Respostas atrasadas são descartadas.
    pub fn disconnect(&mut self) {
        let (tx, rx) = channel();
        self.tx = tx;
        self.rx = rx;
        let (props_tx, props_rx) = channel();
        self.props_tx = props_tx;
        self.props_rx = props_rx;
        self.properties.clear();
        self.provider = None;
        self.roots = Load::NotLoaded;
        self.entries.clear();
        self.selected = None;
        self.restore.clear();
    }

    /// Os nós expandidos, na ordem da árvore (o pai antes dos filhos), mais
    /// os que ainda esperam para expandir ([`Self::restore_expanded`]). É o
    /// que a sessão salva para reabrir a árvore como estava.
    pub fn expanded(&self) -> Vec<NodeId> {
        fn walk(explorer: &ObjectExplorer, ids: &[NodeId], out: &mut Vec<NodeId>) {
            for id in ids {
                let Some(entry) = explorer.entries.get(id) else {
                    continue;
                };
                if entry.expanded {
                    out.push(id.clone());
                    if let Load::Loaded(kids) = &entry.children {
                        walk(explorer, kids, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        if let Load::Loaded(ids) = &self.roots {
            walk(self, ids, &mut out);
        }
        let mut waiting: Vec<NodeId> = self
            .restore
            .iter()
            .filter(|id| !out.contains(id))
            .cloned()
            .collect();
        waiting.sort_by(|a, b| a.0.cmp(&b.0));
        out.extend(waiting);
        out
    }

    /// Expande estes nós à medida que a árvore carrega (os que já estão
    /// carregados, agora). Um nó que não existe mais é esquecido quando a
    /// árvore termina de carregar sem ele.
    pub fn restore_expanded(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        self.restore.extend(ids);
        self.apply_restore();
    }

    /// Expande os nós de [`Self::restore_expanded`] que já chegaram.
    fn apply_restore(&mut self) {
        if self.restore.is_empty() {
            return;
        }
        let ready: Vec<NodeId> = self
            .restore
            .iter()
            .filter(|id| self.entries.contains_key(*id))
            .cloned()
            .collect();
        for id in ready {
            self.restore.remove(&id);
            self.expand(&id);
        }
        // Nada mais a caminho: o que sobrou não existe mais no servidor.
        if matches!(self.roots, Load::Loaded(_) | Load::Failed(_)) && !self.is_loading() {
            self.restore.clear();
        }
    }

    /// Se há um provedor conectado.
    pub fn is_connected(&self) -> bool {
        self.provider.is_some()
    }

    /// Estado de carga das raízes.
    pub fn roots(&self) -> &Load {
        &self.roots
    }

    /// Estado de carga dos filhos de um nó conhecido.
    pub fn children(&self, id: &NodeId) -> Option<&Load> {
        self.entries.get(id).map(|e| &e.children)
    }

    /// Dados de um nó já carregado.
    pub fn node(&self, id: &NodeId) -> Option<&ObjectNode> {
        self.entries.get(id).map(|e| &e.node)
    }

    /// Nó selecionado.
    pub fn selected(&self) -> Option<&ObjectNode> {
        self.selected.as_ref().and_then(|id| self.node(id))
    }

    /// Seleciona um nó.
    pub fn select(&mut self, id: &NodeId) {
        if self.entries.contains_key(id) {
            self.selected = Some(id.clone());
        }
    }

    /// Propriedades de um objeto, pedidas ao provedor na primeira vez (numa
    /// thread, como os filhos). None para o servidor, pastas e nós
    /// desconhecidos.
    pub fn properties(&mut self, id: &NodeId) -> Option<&Properties> {
        let kind = self.entries.get(id)?.node.kind;
        if !kind.has_properties() {
            return None;
        }
        if !self.properties.contains_key(id) {
            let provider = self.provider.clone()?;
            self.properties.insert(id.clone(), Properties::Loading);
            let tx = self.props_tx.clone();
            let id = id.clone();
            // Sem repaint daqui: quem mostra as propriedades pede um frame
            // adiado enquanto carregam (ver `properties_loading`).
            std::thread::spawn(move || {
                let result = provider.properties(&id);
                let _ = tx.send((id, result));
            });
        }
        self.properties.get(id)
    }

    /// Se alguma carga da árvore ainda não voltou.
    pub fn is_loading(&self) -> bool {
        self.roots == Load::Loading || self.entries.values().any(|e| e.children == Load::Loading)
    }

    /// Se alguma leitura de propriedades ainda não voltou. A thread que lê
    /// não pede frame novo; quem mostra as propriedades pede um adiado.
    pub fn properties_loading(&self) -> bool {
        self.properties.values().any(|p| *p == Properties::Loading)
    }

    /// Expande um nó, pedindo os filhos se ainda não foram lidos.
    pub fn expand(&mut self, id: &NodeId) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        if !entry.node.has_children {
            return;
        }
        entry.expanded = true;
        if matches!(entry.children, Load::NotLoaded | Load::Failed(_)) {
            self.request(Some(id.clone()));
        }
    }

    /// Recolhe um nó (os filhos ficam em memória).
    pub fn collapse(&mut self, id: &NodeId) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.expanded = false;
        }
    }

    /// Relê os filhos de um nó, como "Refresh" no SSMS.
    pub fn refresh(&mut self, id: &NodeId) {
        if let Some(Entry {
            children: Load::Loaded(old),
            ..
        }) = self.entries.get(id)
        {
            let old = old.clone();
            self.forget(&old);
        }
        if let Some(entry) = self.entries.get_mut(id) {
            entry.children = Load::NotLoaded;
            entry.expanded = false;
        }
        // As propriedades são relidas na próxima vez que aparecerem.
        self.properties.clear();
        self.expand(id);
    }

    fn forget(&mut self, ids: &[NodeId]) {
        for id in ids {
            if let Some(Entry {
                children: Load::Loaded(kids),
                ..
            }) = self.entries.remove(id)
            {
                self.forget(&kids);
            }
            if self.selected.as_ref() == Some(id) {
                self.selected = None;
            }
        }
    }

    fn request(&mut self, target: Target) {
        let Some(provider) = self.provider.clone() else {
            return;
        };
        match &target {
            None => self.roots = Load::Loading,
            Some(id) => {
                if let Some(e) = self.entries.get_mut(id) {
                    e.children = Load::Loading;
                }
            }
        }
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = match &target {
                None => provider.roots(),
                Some(id) => provider.children(id),
            };
            // O receptor some ao desconectar; aí a resposta não interessa.
            let _ = tx.send((target, result));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    /// Aplica as respostas que já chegaram. Devolve quantas foram aplicadas.
    pub fn poll(&mut self) -> usize {
        let mut applied = 0;
        while let Ok((id, result)) = self.props_rx.try_recv() {
            applied += 1;
            let props = match result {
                Ok(p) => Properties::Loaded(p),
                Err(e) => Properties::Failed(e.0),
            };
            self.properties.insert(id, props);
        }
        while let Ok((target, result)) = self.rx.try_recv() {
            applied += 1;
            let load = match result {
                Ok(nodes) => {
                    let ids = nodes.iter().map(|n| n.id.clone()).collect();
                    for node in nodes {
                        self.entries.insert(
                            node.id.clone(),
                            Entry {
                                node,
                                expanded: false,
                                children: Load::NotLoaded,
                            },
                        );
                    }
                    Load::Loaded(ids)
                }
                Err(e) => Load::Failed(e.0),
            };
            match target {
                None => self.roots = load,
                Some(id) => {
                    if let Some(e) = self.entries.get_mut(&id) {
                        e.children = load;
                    }
                }
            }
        }
        if applied > 0 {
            self.apply_restore();
        }
        applied
    }

    /// Espera as cargas em andamento (árvore e propriedades) terminarem
    /// (para testes).
    pub fn wait_idle(&mut self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while self.is_loading() || self.properties_loading() {
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.poll();
        }
        true
    }

    /// Linhas visíveis, na ordem da árvore.
    pub fn visible_rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        match &self.roots {
            Load::Loaded(ids) => self.push_rows(ids, 0, &mut rows),
            Load::Loading => rows.push(Row::Loading { depth: 0 }),
            Load::Failed(e) => rows.push(Row::Failed { depth: 0, error: e }),
            Load::NotLoaded => {}
        }
        rows
    }

    fn push_rows<'a>(&'a self, ids: &'a [NodeId], depth: usize, rows: &mut Vec<Row<'a>>) {
        for id in ids {
            let Some(entry) = self.entries.get(id) else {
                continue;
            };
            rows.push(Row::Node {
                depth,
                node: &entry.node,
                expanded: entry.expanded,
            });
            if entry.expanded {
                match &entry.children {
                    Load::Loaded(kids) => self.push_rows(kids, depth + 1, rows),
                    Load::Loading => rows.push(Row::Loading { depth: depth + 1 }),
                    Load::Failed(e) => rows.push(Row::Failed {
                        depth: depth + 1,
                        error: e,
                    }),
                    Load::NotLoaded => {}
                }
            }
        }
    }

    /// Diz como o nó do servidor aparece: com o nome e o ambiente do
    /// registro (`None`: o nome do servidor, como veio da conexão).
    pub fn set_server_label(&mut self, label: Option<ServerLabel>) {
        self.server_label = label;
    }

    /// O registro mostrado no nó do servidor.
    pub fn server_label(&self) -> Option<&ServerLabel> {
        self.server_label.as_ref()
    }

    /// Desenha a árvore e devolve os pedidos para a casca.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<Event> {
        self.begin(ui.ctx());
        let mut events = Vec::new();
        if !self.is_connected() {
            ui.weak(tr("Not connected. Use File > Connect Object Explorer."));
            return events;
        }
        let rows = self.visible_rows().len();
        egui::ScrollArea::both()
            .auto_shrink(false)
            .show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
                events = self.show_range(ui, range, 0)
            });
        events
    }

    /// Antes de desenhar: guarda o contexto e recebe as cargas prontas.
    /// Quem desenha a árvore com [`ObjectExplorer::show_range`] chama isto
    /// uma vez por frame, mesmo com a árvore fora da área visível.
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        self.poll();
    }

    /// As pastas para onde o servidor pode ir ("Move To" no menu do nó do
    /// servidor); o pedido volta como [`Event::MoveToFolder`] com o índice.
    pub fn set_move_targets(&mut self, targets: Vec<String>) {
        self.move_targets = targets;
    }

    /// Desenha só as linhas `range` de [`ObjectExplorer::visible_rows`],
    /// sem área de rolagem e com `indent` níveis a mais de recuo: para a
    /// árvore entrar no meio de outra lista (as pastas de conexões).
    pub fn show_range(
        &mut self,
        ui: &mut egui::Ui,
        range: std::ops::Range<usize>,
        indent: usize,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        // Ações coletadas durante o desenho e aplicadas depois, porque as
        // linhas emprestam a árvore.
        let mut toggle: Option<NodeId> = None;
        let mut select: Option<NodeId> = None;
        let mut refresh: Option<NodeId> = None;
        let rows = self.visible_rows();
        let selected = self.selected.clone();
        let server_label = self.server_label.clone();
        let move_targets = self.move_targets.clone();
        let range = range.start.min(rows.len())..range.end.min(rows.len());
        for row in &rows[range] {
            ui.horizontal(|ui| {
                ui.set_height(ROW_HEIGHT);
                ui.add_space((row.depth() + indent) as f32 * INDENT);
                match *row {
                    Row::Node { node, expanded, .. } => {
                        let caret = if !node.has_children {
                            " "
                        } else if expanded {
                            icon::CARET_DOWN
                        } else {
                            icon::CARET_RIGHT
                        };
                        let arrow = ui.add(
                            egui::Button::new(caret)
                                .frame(false)
                                .min_size(egui::vec2(14.0, 0.0)),
                        );
                        if arrow.clicked() && node.has_children {
                            toggle = Some(node.id.clone());
                        }
                        let is_selected = selected.as_ref() == Some(&node.id);
                        // Pastas ("Tables", "Views"...) saem traduzidas; o
                        // nome em inglês continua no caminho do nó.
                        let name = match node.kind {
                            NodeKind::Folder(_) => tr(&node.name),
                            _ => node.name.as_str(),
                        };
                        // Servidor registrado: a etiqueta do ambiente, o
                        // nome do registro e o servidor na dica.
                        let registered = server_label
                            .as_ref()
                            .filter(|_| node.kind == NodeKind::Server);
                        if let Some((env, [r, g, b])) =
                            registered.and_then(|l| l.environment.as_ref())
                        {
                            let light =
                                0.299 * *r as f32 + 0.587 * *g as f32 + 0.114 * *b as f32 > 150.0;
                            let ink = if light {
                                egui::Color32::from_rgb(0x1E, 0x1E, 0x1E)
                            } else {
                                egui::Color32::WHITE
                            };
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgb(*r, *g, *b))
                                .corner_radius(3.0)
                                .inner_margin(egui::Margin::symmetric(5, 0))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(env).strong().color(ink))
                                });
                        }
                        let name = registered.map_or(name, |l| l.name.as_str());
                        let text = format!("{} {name}", kind_icon(node.kind, expanded));
                        let label = ui.add(
                            egui::Button::selectable(is_selected, text).frame_when_inactive(false),
                        );
                        let label = match registered {
                            Some(_) => label.on_hover_text(node.name.as_str()),
                            None => label,
                        };
                        if label.clicked() {
                            select = Some(node.id.clone());
                        }
                        if label.double_clicked() && node.has_children {
                            toggle = Some(node.id.clone());
                        }
                        label.context_menu(|ui| {
                            select = Some(node.id.clone());
                            if node.kind == NodeKind::Folder(Folder::Tables) {
                                if ui.button(tr("New Table...")).clicked() {
                                    events.push(Event::NewTable(node.id.clone()));
                                    ui.close();
                                }
                                ui.separator();
                            }
                            if node.kind == NodeKind::Folder(Folder::Diagrams) {
                                if ui.button(tr("New Database Diagram")).clicked() {
                                    events.push(Event::NewDiagram(node.id.clone()));
                                    ui.close();
                                }
                                ui.separator();
                            }
                            let new_principal = match node.kind {
                                NodeKind::Folder(Folder::Roles) => Some(tr("New Role...")),
                                NodeKind::Folder(Folder::Logins) => Some(tr("New Login...")),
                                NodeKind::Folder(Folder::ServerRoles) => {
                                    Some(tr("New Server Role..."))
                                }
                                NodeKind::Folder(Folder::Users) => Some(tr("New User...")),
                                NodeKind::Folder(Folder::DatabaseRoles) => {
                                    Some(tr("New Database Role..."))
                                }
                                _ => None,
                            };
                            if let Some(text) = new_principal {
                                if ui.button(text).clicked() {
                                    events.push(Event::Properties(node.id.clone()));
                                    ui.close();
                                }
                                ui.separator();
                            }
                            if node.kind == NodeKind::Database {
                                ui.menu_button(tr("Tasks"), |ui| {
                                    if ui.button(tr("Back Up...")).clicked() {
                                        events.push(Event::BackUp(node.id.clone()));
                                        ui.close();
                                    }
                                    if ui.button(tr("Restore...")).clicked() {
                                        events.push(Event::Restore(node.id.clone()));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button(tr("Compare Schema...")).clicked() {
                                        events.push(Event::CompareSchema(node.id.clone()));
                                        ui.close();
                                    }
                                });
                                ui.separator();
                            }
                            if node.kind == NodeKind::Folder(Folder::Databases) {
                                if ui.button(tr("Restore Database...")).clicked() {
                                    events.push(Event::Restore(node.id.clone()));
                                    ui.close();
                                }
                                ui.separator();
                            }
                            if node.kind == NodeKind::Server {
                                // Já registrado: o mesmo pedido abre o
                                // registro para editar.
                                let register = if registered.is_some() {
                                    tr("Edit Registration...")
                                } else {
                                    tr("Register...")
                                };
                                if ui.button(register).clicked() {
                                    events.push(Event::Register(node.id.clone()));
                                    ui.close();
                                }
                                if !move_targets.is_empty() {
                                    ui.menu_button(tr("Move To"), |ui| {
                                        for (i, folder) in move_targets.iter().enumerate() {
                                            let text = format!("{} {}", icon::FOLDER, folder);
                                            if ui.button(text).clicked() {
                                                events.push(Event::MoveToFolder(i));
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                                ui.separator();
                                if ui.button(tr("Activity Monitor")).clicked() {
                                    events.push(Event::ActivityMonitor(node.id.clone()));
                                    ui.close();
                                }
                                ui.separator();
                            }
                            if node.kind == NodeKind::Table {
                                if ui.button(tr("Design")).clicked() {
                                    events.push(Event::Design(node.id.clone()));
                                    ui.close();
                                }
                                ui.separator();
                            }
                            let queryable = matches!(
                                node.kind,
                                NodeKind::Server
                                    | NodeKind::Database
                                    | NodeKind::Table
                                    | NodeKind::View
                            );
                            if ui
                                .add_enabled(queryable, egui::Button::new(tr("New Query")))
                                .clicked()
                            {
                                events.push(Event::NewQuery(node.id.clone()));
                                ui.close();
                            }
                            if node.kind.has_rows() {
                                ui.separator();
                                let top = trf!("Select Top {n} Rows", n = SELECT_TOP_ROWS);
                                if ui.button(top).clicked() {
                                    events.push(Event::SelectTop(node.id.clone()));
                                    ui.close();
                                }
                                let edit = trf!("Edit Top {n} Rows", n = EDIT_TOP_ROWS);
                                if ui.button(edit).clicked() {
                                    events.push(Event::EditTop(node.id.clone()));
                                    ui.close();
                                }
                                if node.kind == NodeKind::Table
                                    && ui.button(tr("Import CSV...")).clicked()
                                {
                                    events.push(Event::ImportCsv(node.id.clone()));
                                    ui.close();
                                }
                                let ask = format!(
                                    "{} {}",
                                    egui_phosphor::regular::SPARKLE,
                                    tr("Ask Claude")
                                );
                                if ui.button(ask).clicked() {
                                    events.push(Event::AskClaude(node.id.clone()));
                                    ui.close();
                                }
                            }
                            if node.kind.scriptable() {
                                let label = trf!("Script {kind} as", kind = node.kind.label());
                                ui.menu_button(label, |ui| {
                                    for (text, action) in [
                                        (
                                            tr_noop("CREATE To New Query Window"),
                                            ScriptAction::Create,
                                        ),
                                        (tr_noop("DROP To New Query Window"), ScriptAction::Drop),
                                    ] {
                                        if ui.button(tr(text)).clicked() {
                                            events.push(Event::Script(node.id.clone(), action));
                                            ui.close();
                                        }
                                    }
                                });
                            } else {
                                ui.add_enabled_ui(false, |ui| {
                                    let _ = ui.button(tr("Script as"));
                                });
                            }
                            ui.separator();
                            if ui
                                .add_enabled(node.has_children, egui::Button::new(tr("Refresh")))
                                .clicked()
                            {
                                refresh = Some(node.id.clone());
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    // No servidor: o registro dele
                                    // (nome, pasta e ambiente).
                                    node.kind.has_properties() || node.kind == NodeKind::Server,
                                    egui::Button::new(tr("Properties")),
                                )
                                .clicked()
                            {
                                events.push(Event::Properties(node.id.clone()));
                                ui.close();
                            }
                        });
                    }
                    Row::Loading { .. } => {
                        ui.weak(format!("{} {}", icon::SPINNER, tr("Loading...")));
                    }
                    Row::Failed { error, .. } => {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            // i18n-ignore
                            format!("{} {error}", icon::WARNING),
                        );
                    }
                }
            });
        }
        drop(rows);
        if let Some(id) = select {
            // Já pede as propriedades, para a janela Properties não esperar
            // um frame a mais.
            let _ = self.properties(&id);
            self.selected = Some(id);
        }
        if let Some(id) = toggle {
            let expanded = self.entries.get(&id).is_some_and(|e| e.expanded);
            if expanded {
                self.collapse(&id);
            } else {
                self.expand(&id);
            }
        }
        if let Some(id) = refresh {
            self.refresh(&id);
        }
        events
    }
}

/// Uma linha da árvore.
#[derive(Clone, Copy, Debug)]
pub enum Row<'a> {
    Node {
        depth: usize,
        node: &'a ObjectNode,
        expanded: bool,
    },
    Loading {
        depth: usize,
    },
    Failed {
        depth: usize,
        error: &'a str,
    },
}

impl Row<'_> {
    /// Nível de recuo.
    pub fn depth(&self) -> usize {
        match *self {
            Row::Node { depth, .. } | Row::Loading { depth } | Row::Failed { depth, .. } => depth,
        }
    }
}

/// Ícone de cada tipo de nó.
pub fn kind_icon(kind: NodeKind, expanded: bool) -> &'static str {
    match kind {
        NodeKind::Server => icon::DESKTOP_TOWER,
        NodeKind::Folder(Folder::Databases) => icon::HARD_DRIVES,
        NodeKind::Folder(Folder::Security) => icon::SHIELD,
        NodeKind::Folder(Folder::Diagrams) => icon::TREE_STRUCTURE,
        NodeKind::Folder(_) if expanded => icon::FOLDER_OPEN,
        NodeKind::Folder(_) => icon::FOLDER,
        NodeKind::Database => icon::DATABASE,
        NodeKind::Schema => icon::STACK,
        NodeKind::Table => icon::TABLE,
        NodeKind::View => icon::EYE,
        NodeKind::MaterializedView => icon::CUBE,
        NodeKind::Function => icon::FUNCTION,
        NodeKind::Procedure => icon::GEAR,
        NodeKind::Trigger => icon::LIGHTNING_A,
        NodeKind::Type => icon::SHAPES,
        NodeKind::Extension => icon::PUZZLE_PIECE,
        NodeKind::Column => icon::COLUMNS,
        NodeKind::Index => icon::LIGHTNING,
        NodeKind::Sequence => icon::LIST_NUMBERS,
        NodeKind::Constraint => icon::KEY,
        NodeKind::Role => icon::USER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake::{FakeProvider, STRESS_TABLES};
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    fn connected() -> ObjectExplorer {
        let mut oe = ObjectExplorer::default();
        oe.connect(Arc::new(FakeProvider::new(Duration::ZERO)));
        assert!(oe.wait_idle(WAIT));
        oe
    }

    fn id(s: &str) -> NodeId {
        NodeId(s.into())
    }

    fn expand(oe: &mut ObjectExplorer, path: &str) {
        oe.expand(&id(path));
        assert!(oe.wait_idle(WAIT), "{path}");
    }

    #[test]
    fn children_load_only_when_expanded() {
        let mut oe = connected();
        assert_eq!(oe.visible_rows().len(), 1);
        assert_eq!(oe.children(&id("srv")), Some(&Load::NotLoaded));
        expand(&mut oe, "srv");
        assert!(matches!(oe.children(&id("srv")), Some(Load::Loaded(_))));
        assert_eq!(oe.visible_rows().len(), 2);
        // A pasta Databases ainda não foi lida.
        assert_eq!(oe.children(&id("srv/Databases")), Some(&Load::NotLoaded));
    }

    #[test]
    fn properties_load_once_for_objects_only() {
        let mut oe = connected();
        expand(&mut oe, "srv");
        expand(&mut oe, "srv/Databases");
        // Servidor e pastas não têm propriedades para ler.
        assert_eq!(oe.properties(&id("srv")), None);
        assert_eq!(oe.properties(&id("srv/Databases")), None);
        assert_eq!(oe.properties(&id("nope")), None);

        let db = id("srv/Databases/postgres");
        assert_eq!(oe.properties(&db), Some(&Properties::Loading));
        assert!(oe.wait_idle(WAIT));
        let owner = Property::new("Owner", fake::FAKE_OWNER);
        assert_eq!(
            oe.properties(&db),
            Some(&Properties::Loaded(vec![owner.clone()]))
        );
        // Refresh esquece e lê de novo.
        oe.refresh(&id("srv/Databases"));
        assert!(oe.wait_idle(WAIT));
        assert_eq!(oe.properties(&db), Some(&Properties::Loading));
        assert!(oe.wait_idle(WAIT));
        assert_eq!(oe.properties(&db), Some(&Properties::Loaded(vec![owner])));
    }

    #[test]
    fn expanding_shows_loading_until_the_reply_arrives() {
        let mut oe = ObjectExplorer::default();
        oe.connect(Arc::new(FakeProvider::new(Duration::from_millis(50))));
        assert_eq!(oe.roots(), &Load::Loading);
        assert!(matches!(oe.visible_rows()[0], Row::Loading { .. }));
        assert!(oe.wait_idle(WAIT));
        assert!(matches!(oe.roots(), Load::Loaded(_)));
    }

    #[test]
    fn a_saved_tree_expands_again_as_it_loads() {
        let mut oe = connected();
        expand(&mut oe, "srv");
        expand(&mut oe, "srv/Databases");
        expand(&mut oe, "srv/Databases/postgres");
        let saved = oe.expanded();
        assert_eq!(
            saved,
            vec![id("srv"), id("srv/Databases"), id("srv/Databases/postgres")]
        );
        let rows = oe.visible_rows().len();

        // Outra execução do app: a mesma árvore, mais um nó que sumiu.
        let mut again = ObjectExplorer::default();
        again.connect(Arc::new(FakeProvider::new(Duration::from_millis(2))));
        again.restore_expanded(saved.iter().cloned().chain([id("srv/Databases/gone")]));
        // Enquanto carrega, o que falta expandir continua na lista salva.
        assert!(again.expanded().contains(&id("srv/Databases/postgres")));
        assert!(again.wait_idle(WAIT));
        assert_eq!(again.visible_rows().len(), rows);
        assert_eq!(again.expanded(), saved);
    }

    #[test]
    fn ten_thousand_tables() {
        let mut oe = connected();
        for p in [
            "srv",
            "srv/Databases",
            "srv/Databases/stress",
            "srv/Databases/stress/Schemas",
            "srv/Databases/stress/Schemas/public",
            "srv/Databases/stress/Schemas/public/Tables",
        ] {
            expand(&mut oe, p);
        }
        let tables = oe
            .visible_rows()
            .iter()
            .filter(|r| matches!(r, Row::Node { node, .. } if node.kind == NodeKind::Table))
            .count();
        assert_eq!(tables, STRESS_TABLES);
        // Montar as linhas visíveis precisa ser barato para rodar a cada frame.
        let start = std::time::Instant::now();
        for _ in 0..10 {
            assert!(oe.visible_rows().len() > STRESS_TABLES);
        }
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn collapse_keeps_children_and_refresh_reloads() {
        let mut oe = connected();
        expand(&mut oe, "srv");
        oe.collapse(&id("srv"));
        assert_eq!(oe.visible_rows().len(), 1);
        assert!(matches!(oe.children(&id("srv")), Some(Load::Loaded(_))));
        oe.select(&id("srv/Databases"));
        oe.refresh(&id("srv"));
        assert_eq!(oe.children(&id("srv")), Some(&Load::Loading));
        assert!(oe.selected().is_none());
        assert!(oe.wait_idle(WAIT));
        assert_eq!(oe.visible_rows().len(), 2);
    }

    #[test]
    fn disconnect_drops_late_replies() {
        let mut oe = ObjectExplorer::default();
        oe.connect(Arc::new(FakeProvider::new(Duration::from_millis(30))));
        oe.disconnect();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(oe.poll(), 0);
        assert_eq!(oe.roots(), &Load::NotLoaded);
        assert!(oe.visible_rows().is_empty());
    }

    #[test]
    fn provider_errors_show_up_in_the_tree() {
        let mut oe = connected();
        oe.entries.insert(
            id("bad"),
            Entry {
                node: ObjectNode {
                    id: id("bad"),
                    name: "bad".into(),
                    kind: NodeKind::Schema,
                    has_children: true,
                },
                expanded: false,
                children: Load::NotLoaded,
            },
        );
        if let Load::Loaded(ids) = &mut oe.roots {
            ids.push(id("bad"));
        }
        expand(&mut oe, "bad");
        assert!(matches!(oe.children(&id("bad")), Some(Load::Failed(_))));
        assert!(
            oe.visible_rows()
                .iter()
                .any(|r| matches!(r, Row::Failed { .. }))
        );
    }
}
