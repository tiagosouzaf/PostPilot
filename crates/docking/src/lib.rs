//! Docking de painéis no estilo do SSMS.
//!
//! Invólucro fino sobre o `egui_dock`: o resto do PGStudio só conhece
//! [`Workspace`], [`Pane`] e [`PaneViewer`]. Isso permite trocar ou estender o
//! docking (por exemplo, para ter auto-hide) sem mexer na casca.

use eframe::egui;
use egui_dock::tab_viewer::OnCloseResponse;
use egui_dock::{
    DockArea, DockState, Node, NodeIndex, NodePath, Split, Style, SurfaceIndex, TabViewer, Tree,
};
use serde::{Deserialize, Serialize};

/// Onde um painel mora no layout padrão e para onde volta ao ser reaberto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// Coluna da esquerda (Object Explorer).
    Left,
    /// Coluna da direita (Properties).
    Right,
    /// Faixa de baixo, sob os documentos (Output).
    Bottom,
    /// Área central de documentos (janelas de consulta).
    Document,
}

/// Um painel que pode ser encaixado.
pub trait Pane: Clone + PartialEq + std::hash::Hash + std::fmt::Debug {
    /// Área de origem do painel.
    fn area(&self) -> Area;
}

/// Desenha o conteúdo e o título de cada painel.
pub trait PaneViewer<P> {
    /// Texto da aba.
    fn title(&mut self, pane: &P) -> String;
    /// Conteúdo do painel.
    fn ui(&mut self, ui: &mut egui::Ui, pane: &mut P);
    /// Se a aba mostra o botão de fechar.
    fn closeable(&mut self, _pane: &P) -> bool {
        true
    }
    /// Chamado quando o usuário fecha a aba; `false` mantém a aba aberta
    /// (por exemplo, para perguntar antes de descartar alterações).
    fn on_close(&mut self, _pane: &P) -> bool {
        true
    }
}

/// Retrato do layout que pode ser salvo e restaurado.
///
/// É um formato próprio, independente do formato interno do egui_dock, para
/// que um arquivo salvo continue legível quando o egui_dock mudar e para que
/// um arquivo estragado nunca produza uma árvore inválida.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout<P> {
    /// Árvore da janela principal; `None` se tudo foi fechado.
    pub root: Option<LayoutNode<P>>,
    /// Painéis que estavam flutuando; voltam encaixados na área de origem.
    #[serde(default = "Vec::new")]
    pub floating: Vec<P>,
}

/// Um nó do layout salvo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutNode<P> {
    /// Grupo de abas.
    Tabs {
        tabs: Vec<P>,
        #[serde(default)]
        active: usize,
    },
    /// Divisão em duas partes; `fraction` é a parte da primeira.
    Split {
        direction: Direction,
        fraction: f32,
        first: Box<LayoutNode<P>>,
        second: Box<LayoutNode<P>>,
    },
}

/// Direção de uma divisão.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Lado a lado (primeira à esquerda).
    Horizontal,
    /// Empilhadas (primeira em cima).
    Vertical,
}

/// Profundidade máxima aceita ao restaurar. O egui_dock guarda a árvore num
/// vetor de 2^profundidade nós, então um arquivo patológico pesaria muito.
const MAX_DEPTH: usize = 10;

impl<P: Pane> LayoutNode<P> {
    /// Remove abas repetidas ou recusadas por `keep`, grupos vazios e
    /// divisões inúteis; corrige frações e índices. `None` se nada sobrou.
    fn sanitize(self, depth: usize, seen: &mut Vec<P>, keep: &impl Fn(&P) -> bool) -> Option<Self> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self {
            LayoutNode::Tabs { tabs, active } => {
                let active_tab = tabs.get(active).cloned();
                let mut kept = Vec::new();
                for tab in tabs {
                    if keep(&tab) && !seen.contains(&tab) {
                        seen.push(tab.clone());
                        kept.push(tab);
                    }
                }
                let active = active_tab
                    .and_then(|a| kept.iter().position(|t| *t == a))
                    .unwrap_or(0);
                (!kept.is_empty()).then_some(LayoutNode::Tabs { tabs: kept, active })
            }
            LayoutNode::Split {
                direction,
                fraction,
                first,
                second,
            } => {
                let first = first.sanitize(depth + 1, seen, keep);
                let second = second.sanitize(depth + 1, seen, keep);
                match (first, second) {
                    (Some(a), Some(b)) => Some(LayoutNode::Split {
                        direction,
                        fraction: if fraction.is_finite() {
                            fraction.clamp(0.05, 0.95)
                        } else {
                            0.5
                        },
                        first: Box::new(a),
                        second: Box::new(b),
                    }),
                    (a, b) => a.or(b),
                }
            }
        }
    }

    fn first_tab(&self) -> &P {
        match self {
            LayoutNode::Tabs { tabs, .. } => &tabs[0],
            LayoutNode::Split { first, .. } => first.first_tab(),
        }
    }

    /// Recria este nó na posição `index` da árvore, que já é uma folha.
    fn build(self, tree: &mut Tree<P>, index: NodeIndex) {
        match self {
            LayoutNode::Tabs { tabs, active } => {
                if let Some(leaf) = tree[index].get_leaf_mut() {
                    leaf.tabs = tabs;
                    let _ = leaf.set_active_tab(active);
                }
            }
            LayoutNode::Split {
                direction,
                fraction,
                first,
                second,
            } => {
                let split = match direction {
                    Direction::Horizontal => Split::Right,
                    Direction::Vertical => Split::Below,
                };
                let placeholder = Node::leaf(second.first_tab().clone());
                let [a, b] = tree.split(index, split, fraction, placeholder);
                first.build(tree, a);
                second.build(tree, b);
            }
        }
    }
}

fn snapshot<P: Pane>(nodes: &[&Node<P>], index: usize) -> Option<LayoutNode<P>> {
    match nodes.get(index)? {
        Node::Leaf(leaf) => Some(LayoutNode::Tabs {
            tabs: leaf.tabs.clone(),
            active: leaf.active.0,
        }),
        Node::Horizontal(split) | Node::Vertical(split) => {
            let direction = match nodes[index] {
                Node::Horizontal(_) => Direction::Horizontal,
                _ => Direction::Vertical,
            };
            let first = snapshot(nodes, 2 * index + 1);
            let second = snapshot(nodes, 2 * index + 2);
            match (first, second) {
                (Some(a), Some(b)) => Some(LayoutNode::Split {
                    direction,
                    fraction: split.fraction,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (a, b) => a.or(b),
            }
        }
        Node::Empty => None,
    }
}

/// Proporções do layout padrão, como frações da janela.
const LEFT: f32 = 0.2;
const RIGHT: f32 = 0.78;
const BOTTOM: f32 = 0.72;

/// Os painéis abertos e como estão encaixados.
pub struct Workspace<P: Pane> {
    state: DockState<P>,
}

impl<P: Pane> Workspace<P> {
    /// Layout do SSMS: esquerda de altura total, documentos ao centro,
    /// direita de altura total e a faixa de baixo sob os documentos.
    pub fn new(documents: Vec<P>, left: Vec<P>, right: Vec<P>, bottom: Vec<P>) -> Self {
        let mut state = DockState::new(documents);
        let tree = state.main_surface_mut();
        let mut center = NodeIndex::root();
        if !left.is_empty() {
            [center, _] = tree.split_left(center, LEFT, left);
        }
        if !right.is_empty() {
            [center, _] = tree.split_right(center, RIGHT, right);
        }
        if !bottom.is_empty() {
            tree.split_below(center, BOTTOM, bottom);
        }
        Workspace { state }
    }

    /// Retrato do layout atual, só com os painéis aceitos por `keep`
    /// (as janelas de consulta, por exemplo, não são salvas).
    pub fn layout(&self, keep: impl Fn(&P) -> bool) -> Layout<P> {
        let nodes: Vec<&Node<P>> = self.state.main_surface().iter().collect();
        let mut seen = Vec::new();
        let root = snapshot(&nodes, 0).and_then(|n| n.sanitize(0, &mut seen, &keep));
        let floating = self
            .state
            .iter_surfaces_indexed()
            .filter(|(index, _)| !index.is_main())
            .flat_map(|(_, surface)| surface.iter_all_tabs().map(|(_, t)| t.clone()))
            .filter(|t| keep(t) && !seen.contains(t))
            .collect();
        Layout { root, floating }
    }

    /// Recria o docking a partir de um retrato. Abas repetidas, grupos vazios
    /// e frações inválidas são corrigidos; nunca falha.
    pub fn from_layout(layout: Layout<P>) -> Self {
        let mut seen = Vec::new();
        let root = layout
            .root
            .and_then(|n| n.sanitize(0, &mut seen, &|_| true));
        let state = match root {
            Some(root) => {
                let mut state = DockState::new(vec![root.first_tab().clone()]);
                root.build(state.main_surface_mut(), NodeIndex::root());
                state
            }
            None => DockState::new(Vec::new()),
        };
        let mut workspace = Workspace { state };
        for pane in layout.floating {
            if !seen.contains(&pane) {
                seen.push(pane.clone());
                workspace.open(pane);
            }
        }
        workspace
    }

    /// Se o painel está aberto, encaixado ou flutuando.
    pub fn is_open(&self, pane: &P) -> bool {
        self.state.find_tab(pane).is_some()
    }

    /// Se os dois painéis estão abertos como abas do mesmo grupo.
    pub fn same_group(&self, a: &P, b: &P) -> bool {
        match (self.state.find_tab(a), self.state.find_tab(b)) {
            (Some(x), Some(y)) => x.node_path() == y.node_path(),
            _ => false,
        }
    }

    /// Retângulo do grupo de abas do painel no último frame desenhado.
    pub fn group_rect(&self, pane: &P) -> Option<egui::Rect> {
        let path = self.state.find_tab(pane)?.node_path();
        self.state.leaf(path).ok().map(|leaf| leaf.rect())
    }

    /// Todos os painéis abertos.
    pub fn panes(&self) -> impl Iterator<Item = &P> {
        self.state.iter_all_tabs().map(|(_, p)| p)
    }

    /// Mostra o painel: se já está aberto, traz para a frente; se não, abre
    /// junto de outro painel da mesma área ou recria a área na borda.
    pub fn open(&mut self, pane: P) {
        if let Some(path) = self.state.find_tab(&pane) {
            let _ = self.state.set_active_tab(path);
            self.state.set_focused_node_and_surface(path.node_path());
            return;
        }
        let area = pane.area();
        let sibling = self
            .state
            .iter_leaves()
            .find(|(_, leaf)| leaf.tabs().iter().any(|t| t.area() == area))
            .map(|(path, _)| path);
        if let Some(path) = sibling {
            if let Ok(leaf) = self.state.leaf_mut(path) {
                leaf.append_tab(pane.clone());
                let index = leaf.len() - 1;
                let _ = leaf.set_active_tab(index);
            }
            self.state.set_focused_node_and_surface(path);
            return;
        }
        let tree = self.state.main_surface_mut();
        if tree.is_empty() {
            tree.push_to_first_leaf(pane);
            return;
        }
        let root = NodeIndex::root();
        match area {
            Area::Left => {
                tree.split_left(root, LEFT, vec![pane]);
            }
            Area::Right => {
                tree.split_right(root, RIGHT, vec![pane]);
            }
            Area::Bottom => {
                tree.split_below(root, BOTTOM, vec![pane]);
            }
            Area::Document => self.open_document_area(pane),
        }
    }

    /// Recria a área de documentos quando a última aba foi fechada: acima da
    /// faixa de baixo, à direita da coluna da esquerda ou à esquerda da
    /// coluna da direita, nessa ordem.
    fn open_document_area(&mut self, pane: P) {
        let find = |state: &DockState<P>, area: Area| {
            state
                .main_surface()
                .iter()
                .enumerate()
                .find(|(_, node)| {
                    node.tabs()
                        .is_some_and(|tabs| tabs.iter().any(|t| t.area() == area))
                })
                .map(|(i, _)| NodeIndex(i))
        };
        let empty = self
            .state
            .main_surface()
            .iter()
            .position(|node| node.is_leaf() && node.tabs_count() == 0);
        if let Some(i) = empty {
            if let Ok(leaf) = self.state.leaf_mut(NodePath {
                surface: SurfaceIndex::main(),
                node: NodeIndex(i),
            }) {
                leaf.append_tab(pane);
            }
            return;
        }
        let bottom = find(&self.state, Area::Bottom);
        let left = find(&self.state, Area::Left);
        let right = find(&self.state, Area::Right);
        let tree = self.state.main_surface_mut();
        if let Some(node) = bottom {
            tree.split_above(node, 1.0 - BOTTOM, vec![pane]);
        } else if let Some(node) = left {
            tree.split_right(node, LEFT / RIGHT, vec![pane]);
        } else if let Some(node) = right {
            tree.split_left(node, 0.5, vec![pane]);
        } else {
            tree.push_to_first_leaf(pane);
        }
    }

    /// Painel ativo do grupo com foco, se houver.
    pub fn focused(&mut self) -> Option<&P> {
        self.state.find_active_focused().map(|(_, p)| &*p)
    }

    /// Fecha o painel, se estiver aberto.
    pub fn close(&mut self, pane: &P) {
        if let Some(path) = self.state.find_tab(pane) {
            self.state.remove_tab(path);
        }
    }

    /// Desenha o docking ocupando todo o `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, viewer: &mut impl PaneViewer<P>) {
        let style = Style::from_egui(ui.style().as_ref());
        DockArea::new(&mut self.state)
            .id(egui::Id::new("pgstudio_dock"))
            .style(style)
            .show_leaf_collapse_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut Adapter(viewer, std::marker::PhantomData));
    }
}

/// Liga o [`PaneViewer`] ao `TabViewer` do egui_dock.
struct Adapter<'a, P, V>(&'a mut V, std::marker::PhantomData<P>);

impl<P: Pane, V: PaneViewer<P>> TabViewer for Adapter<'_, P, V> {
    type Tab = P;

    fn title(&mut self, tab: &mut P) -> egui::WidgetText {
        self.0.title(tab).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut P) {
        self.0.ui(ui, tab);
    }

    fn id(&mut self, tab: &mut P) -> egui::Id {
        egui::Id::new(("pgstudio_pane", &*tab))
    }

    fn closeable(&mut self, tab: &mut P) -> bool {
        self.0.closeable(tab)
    }

    fn on_close(&mut self, tab: &mut P) -> OnCloseResponse {
        if self.0.on_close(tab) {
            OnCloseResponse::Close
        } else {
            OnCloseResponse::Ignore
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Hash, Serialize, Deserialize)]
    enum P {
        Explorer,
        Props,
        Output,
        Doc(u32),
    }

    impl Pane for P {
        fn area(&self) -> Area {
            match self {
                P::Explorer => Area::Left,
                P::Props => Area::Right,
                P::Output => Area::Bottom,
                P::Doc(_) => Area::Document,
            }
        }
    }

    fn ssms() -> Workspace<P> {
        Workspace::new(
            vec![P::Doc(1)],
            vec![P::Explorer],
            vec![P::Props],
            vec![P::Output],
        )
    }

    #[test]
    fn default_layout_has_every_pane() {
        let ws = ssms();
        for p in [P::Explorer, P::Props, P::Output, P::Doc(1)] {
            assert!(ws.is_open(&p), "{p:?}");
        }
    }

    #[test]
    fn closed_pane_reopens() {
        let mut ws = ssms();
        for p in [P::Explorer, P::Props, P::Output] {
            ws.close(&p);
            assert!(!ws.is_open(&p));
            ws.open(p);
            assert!(ws.is_open(&p), "{p:?}");
        }
    }

    #[test]
    fn opening_an_open_pane_does_not_duplicate_it() {
        let mut ws = ssms();
        ws.open(P::Explorer);
        assert_eq!(ws.panes().filter(|p| **p == P::Explorer).count(), 1);
    }

    #[test]
    fn reopens_even_when_everything_was_closed() {
        let mut ws = ssms();
        for p in [P::Explorer, P::Props, P::Output, P::Doc(1)] {
            ws.close(&p);
        }
        assert_eq!(ws.panes().count(), 0);
        ws.open(P::Output);
        ws.open(P::Explorer);
        ws.open(P::Props);
        ws.open(P::Doc(2));
        assert_eq!(ws.panes().count(), 4);
    }

    #[test]
    fn documents_open_between_the_tool_windows() {
        // Sem documentos no início, a área central fica vazia e recebe o
        // primeiro; depois de fechar o último, ela volta acima da faixa de baixo.
        let mut ws = Workspace::new(
            Vec::new(),
            vec![P::Explorer],
            vec![P::Props],
            vec![P::Output],
        );
        for round in 0..2 {
            ws.open(P::Doc(round));
            for tool in [P::Explorer, P::Props, P::Output] {
                assert!(!ws.same_group(&P::Doc(round), &tool), "{round} {tool:?}");
            }
            ws.open(P::Doc(10 + round));
            assert!(ws.same_group(&P::Doc(round), &P::Doc(10 + round)));
            assert_eq!(ws.state.iter_leaves().count(), 4);
            ws.close(&P::Doc(round));
            ws.close(&P::Doc(10 + round));
        }
    }

    #[test]
    fn documents_join_the_document_area() {
        let mut ws = ssms();
        ws.open(P::Doc(2));
        assert!(ws.same_group(&P::Doc(1), &P::Doc(2)));
        assert!(!ws.same_group(&P::Doc(1), &P::Explorer));
    }

    struct Viewer;

    impl PaneViewer<P> for Viewer {
        fn title(&mut self, pane: &P) -> String {
            format!("{pane:?}")
        }

        fn ui(&mut self, ui: &mut egui::Ui, pane: &mut P) {
            ui.label(format!("{pane:?}"));
        }
    }

    /// Desenha alguns frames, para garantir que o layout não entra em pânico.
    fn render(ws: &mut Workspace<P>) {
        let ctx = egui::Context::default();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| ws.show(ui, &mut Viewer));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn layout_round_trips_through_json() {
        let mut ws = ssms();
        ws.open(P::Doc(2));
        let layout = ws.layout(|_| true);
        let json = serde_json::to_string(&layout).unwrap();
        let back: Layout<P> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layout);
        let mut restored = Workspace::from_layout(back);
        assert_eq!(restored.layout(|_| true), layout);
        for p in [P::Explorer, P::Props, P::Output, P::Doc(1), P::Doc(2)] {
            assert!(restored.is_open(&p), "{p:?}");
        }
        assert!(restored.same_group(&P::Doc(1), &P::Doc(2)));
        render(&mut restored);
    }

    #[test]
    fn layout_can_leave_panes_out() {
        let ws = ssms();
        let restored = Workspace::from_layout(ws.layout(|p| !matches!(p, P::Doc(_))));
        assert!(!restored.is_open(&P::Doc(1)));
        assert!(restored.is_open(&P::Output));
    }

    #[test]
    fn broken_layouts_are_repaired_without_panicking() {
        let tabs = |tabs: Vec<P>, active| LayoutNode::Tabs { tabs, active };
        let split = |fraction, a, b| LayoutNode::Split {
            direction: Direction::Vertical,
            fraction,
            first: Box::new(a),
            second: Box::new(b),
        };
        let cases = vec![
            Layout {
                root: None,
                floating: vec![P::Props, P::Props],
            },
            Layout {
                root: Some(tabs(vec![], 3)),
                floating: vec![],
            },
            Layout {
                root: Some(split(
                    f32::NAN,
                    tabs(vec![P::Explorer], 9),
                    tabs(vec![P::Explorer], 0),
                )),
                floating: vec![P::Explorer],
            },
            Layout {
                root: Some(split(
                    -4.0,
                    tabs(vec![], 0),
                    tabs(vec![P::Output, P::Doc(1)], 1),
                )),
                floating: vec![],
            },
        ];
        for layout in cases {
            let mut ws = Workspace::from_layout(layout.clone());
            let panes: Vec<P> = ws.panes().copied().collect();
            for p in &panes {
                assert_eq!(panes.iter().filter(|q| *q == p).count(), 1, "{layout:?}");
            }
            render(&mut ws);
            ws.open(P::Doc(7));
            assert!(ws.is_open(&P::Doc(7)));
            render(&mut ws);
        }
        // Árvore profunda demais vira nada em vez de estourar a pilha.
        let mut deep = tabs(vec![P::Doc(0)], 0);
        for i in 1..200 {
            deep = split(0.5, tabs(vec![P::Doc(i)], 0), deep);
        }
        let mut ws = Workspace::from_layout(Layout {
            root: Some(deep),
            floating: vec![],
        });
        render(&mut ws);
    }
}
