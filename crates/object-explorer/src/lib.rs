//! Object Explorer: árvore de objetos do servidor, carregada sob demanda.
//!
//! Os filhos de um nó só são pedidos ao [`MetadataProvider`] quando o nó é
//! expandido, numa thread separada, para a interface nunca esperar o banco.
//! Só as linhas visíveis são desenhadas, então milhares de nós não pesam.

pub mod fake;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_core::metadata::{
    Folder, MetadataError, MetadataProvider, NodeId, NodeKind, ObjectNode, ScriptAction,
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

/// Algo que o usuário pediu pelo Object Explorer e que a casca trata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// "New Query" no menu de contexto (no banco do nó).
    NewQuery(NodeId),
    /// "Select Top 1000 Rows".
    SelectTop(NodeId),
    /// "Edit Top 200 Rows" (por enquanto abre só para leitura).
    EditTop(NodeId),
    /// "Script as" > CREATE ou DROP numa janela de consulta nova.
    Script(NodeId, ScriptAction),
    /// "New Table..." na pasta Tables: abre o Table Designer vazio.
    NewTable(NodeId),
    /// "Design" numa tabela: abre o Table Designer dela.
    Design(NodeId),
    /// "New Database Diagram" na pasta Database Diagrams: escolhe as
    /// tabelas e abre o diagrama delas.
    NewDiagram(NodeId),
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
pub struct ObjectExplorer {
    provider: Option<Arc<dyn MetadataProvider>>,
    roots: Load,
    entries: HashMap<NodeId, Entry>,
    selected: Option<NodeId>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    /// Contexto do egui, para pedir um novo frame quando uma carga termina.
    ctx: Option<egui::Context>,
}

impl Default for ObjectExplorer {
    fn default() -> Self {
        let (tx, rx) = channel();
        ObjectExplorer {
            provider: None,
            roots: Load::NotLoaded,
            entries: HashMap::new(),
            selected: None,
            tx,
            rx,
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
        self.provider = None;
        self.roots = Load::NotLoaded;
        self.entries.clear();
        self.selected = None;
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

    /// Se algum pedido ainda não voltou.
    pub fn is_loading(&self) -> bool {
        self.roots == Load::Loading || self.entries.values().any(|e| e.children == Load::Loading)
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
        applied
    }

    /// Espera as cargas em andamento terminarem (para testes).
    pub fn wait_idle(&mut self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while self.is_loading() {
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

    /// Desenha a árvore e devolve os pedidos para a casca.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<Event> {
        self.ctx = Some(ui.ctx().clone());
        self.poll();
        let mut events = Vec::new();
        if !self.is_connected() {
            ui.weak("Not connected. Use File > Connect Object Explorer.");
            return events;
        }
        // Ações coletadas durante o desenho e aplicadas depois, porque as
        // linhas emprestam a árvore.
        let mut toggle: Option<NodeId> = None;
        let mut select: Option<NodeId> = None;
        let mut refresh: Option<NodeId> = None;
        let rows = self.visible_rows();
        let selected = self.selected.clone();
        egui::ScrollArea::both().auto_shrink(false).show_rows(
            ui,
            ROW_HEIGHT,
            rows.len(),
            |ui, range| {
                for row in &rows[range] {
                    ui.horizontal(|ui| {
                        ui.set_height(ROW_HEIGHT);
                        ui.add_space(row.depth() as f32 * INDENT);
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
                                let text =
                                    format!("{} {}", kind_icon(node.kind, expanded), node.name);
                                let label = ui.add(
                                    egui::Button::selectable(is_selected, text)
                                        .frame_when_inactive(false),
                                );
                                if label.clicked() {
                                    select = Some(node.id.clone());
                                }
                                if label.double_clicked() && node.has_children {
                                    toggle = Some(node.id.clone());
                                }
                                label.context_menu(|ui| {
                                    select = Some(node.id.clone());
                                    if node.kind == NodeKind::Folder(Folder::Tables) {
                                        if ui.button("New Table...").clicked() {
                                            events.push(Event::NewTable(node.id.clone()));
                                            ui.close();
                                        }
                                        ui.separator();
                                    }
                                    if node.kind == NodeKind::Folder(Folder::Diagrams) {
                                        if ui.button("New Database Diagram").clicked() {
                                            events.push(Event::NewDiagram(node.id.clone()));
                                            ui.close();
                                        }
                                        ui.separator();
                                    }
                                    if node.kind == NodeKind::Table {
                                        if ui.button("Design").clicked() {
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
                                        .add_enabled(queryable, egui::Button::new("New Query"))
                                        .clicked()
                                    {
                                        events.push(Event::NewQuery(node.id.clone()));
                                        ui.close();
                                    }
                                    if node.kind.has_rows() {
                                        ui.separator();
                                        let top = format!("Select Top {SELECT_TOP_ROWS} Rows");
                                        if ui.button(top).clicked() {
                                            events.push(Event::SelectTop(node.id.clone()));
                                            ui.close();
                                        }
                                        let edit = format!("Edit Top {EDIT_TOP_ROWS} Rows");
                                        if ui.button(edit).clicked() {
                                            events.push(Event::EditTop(node.id.clone()));
                                            ui.close();
                                        }
                                    }
                                    if node.kind.scriptable() {
                                        let label = format!("Script {} as", node.kind.label());
                                        ui.menu_button(label, |ui| {
                                            for (text, action) in [
                                                (
                                                    "CREATE To New Query Window",
                                                    ScriptAction::Create,
                                                ),
                                                ("DROP To New Query Window", ScriptAction::Drop),
                                            ] {
                                                if ui.button(text).clicked() {
                                                    events.push(Event::Script(
                                                        node.id.clone(),
                                                        action,
                                                    ));
                                                    ui.close();
                                                }
                                            }
                                        });
                                    } else {
                                        ui.add_enabled_ui(false, |ui| {
                                            let _ = ui.button("Script as");
                                        });
                                    }
                                    ui.separator();
                                    if ui
                                        .add_enabled(
                                            node.has_children,
                                            egui::Button::new("Refresh"),
                                        )
                                        .clicked()
                                    {
                                        refresh = Some(node.id.clone());
                                        ui.close();
                                    }
                                    ui.separator();
                                    ui.add_enabled_ui(false, |ui| {
                                        let _ = ui.button("Properties");
                                    });
                                });
                            }
                            Row::Loading { .. } => {
                                ui.weak(format!("{} Loading...", icon::SPINNER));
                            }
                            Row::Failed { error, .. } => {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    format!("{} {error}", icon::WARNING),
                                );
                            }
                        }
                    });
                }
            },
        );
        if let Some(id) = select {
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
        NodeKind::Function => icon::FUNCTION,
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
    fn expanding_shows_loading_until_the_reply_arrives() {
        let mut oe = ObjectExplorer::default();
        oe.connect(Arc::new(FakeProvider::new(Duration::from_millis(50))));
        assert_eq!(oe.roots(), &Load::Loading);
        assert!(matches!(oe.visible_rows()[0], Row::Loading { .. }));
        assert!(oe.wait_idle(WAIT));
        assert!(matches!(oe.roots(), Load::Loaded(_)));
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
