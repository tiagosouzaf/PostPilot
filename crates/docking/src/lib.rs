//! Docking de painéis no estilo do SSMS.
//!
//! Invólucro fino sobre o `egui_dock`: o resto do PGStudio só conhece
//! [`Workspace`], [`Pane`] e [`PaneViewer`]. Isso permite trocar ou estender o
//! docking sem mexer na casca.
//!
//! O auto-hide do SSMS (alfinete) é feito aqui, por fora do egui_dock: um
//! painel desafixado sai do docking e vira um botão numa faixa na borda; ao
//! clicar ou passar o mouse, ele desliza por cima dos documentos e some ao
//! perder o foco (ou quando o mouse sai, se abriu só com o mouse por cima).
//! A borda interna do painel aberto pode ser arrastada para mudar o tamanho.

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

/// Largura (ou altura, embaixo) da faixa de auto-hide.
pub const STRIP: f32 = 24.0;
/// Tamanho padrão do painel aberto da faixa (largura nas laterais, altura
/// embaixo).
const FLYOUT_SIZE: f32 = 300.0;
/// Menor tamanho do painel aberto ao redimensionar.
const FLYOUT_MIN: f32 = 120.0;
/// Fração do espaço que o painel aberto pode ocupar ao redimensionar.
const FLYOUT_MAX: f32 = 0.9;
/// Espessura da alça de redimensionar, na borda interna do painel aberto.
const GRIP: f32 = 6.0;
/// Tempo com o mouse sobre o botão da faixa até o painel abrir sozinho.
pub const HOVER_OPEN: f64 = 0.3;
/// Tempo com o mouse fora até um painel aberto pelo mouse se esconder.
pub const HOVER_CLOSE: f64 = 0.5;
/// Duração da animação de deslizar (zero se o tema desliga animações).
const SLIDE: f32 = 0.15;

/// Mouse parado sobre um botão da faixa.
struct Hover<P> {
    pane: P,
    since: f64,
    /// Se ainda vai abrir o painel; desarma ao abrir, ao clicar e ao fechar
    /// o painel com o mouse ainda em cima, até o mouse sair e voltar.
    armed: bool,
}

/// Painel da faixa sendo desenhado, com o progresso da animação (0 é
/// escondido na faixa, 1 é aberto).
struct Slide<P> {
    pane: P,
    progress: f32,
}

/// Pedidos feitos de dentro do desenho, aplicados depois dele.
enum Request<P> {
    AutoHide(P),
    Dock(P),
    Close(P),
    Toggle(P),
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
    /// Painéis desafixados (auto-hide), na ordem das faixas.
    #[serde(default = "Vec::new")]
    pub hidden: Vec<P>,
    /// Tamanho (em pontos) escolhido para o painel aberto da faixa, por
    /// painel; os que não estão aqui usam o padrão.
    #[serde(default = "Vec::new")]
    pub flyout_sizes: Vec<(P, f32)>,
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
    /// Painéis desafixados, cada um na faixa da sua área.
    hidden: Vec<P>,
    /// Painel desafixado que está aberto por cima dos documentos.
    flyout: Option<P>,
    /// Se o painel aberto fica até perder o foco (abriu com clique ou pelo
    /// menu, ou foi usado); `false` se abriu só com o mouse por cima.
    sticky: bool,
    flyout_rect: Option<egui::Rect>,
    /// Tamanho escolhido pelo usuário para cada painel aberto da faixa.
    flyout_sizes: Vec<(P, f32)>,
    strip_buttons: Vec<(P, egui::Rect)>,
    hover: Option<Hover<P>>,
    /// Quando o mouse saiu de um painel aberto pelo mouse.
    left_at: Option<f64>,
    slide: Option<Slide<P>>,
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
        Workspace::with_state(state)
    }

    fn with_state(state: DockState<P>) -> Self {
        Workspace {
            state,
            hidden: Vec::new(),
            flyout: None,
            sticky: false,
            flyout_rect: None,
            flyout_sizes: Vec::new(),
            strip_buttons: Vec::new(),
            hover: None,
            left_at: None,
            slide: None,
        }
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
        let hidden = self.hidden.iter().filter(|t| keep(t)).cloned().collect();
        let flyout_sizes = self
            .flyout_sizes
            .iter()
            .filter(|(t, _)| keep(t))
            .cloned()
            .collect();
        Layout {
            root,
            floating,
            hidden,
            flyout_sizes,
        }
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
        let mut workspace = Workspace::with_state(state);
        for pane in layout.floating {
            if !seen.contains(&pane) {
                seen.push(pane.clone());
                workspace.open(pane);
            }
        }
        for pane in layout.hidden {
            if !seen.contains(&pane) && pane.area() != Area::Document {
                seen.push(pane.clone());
                workspace.hidden.push(pane);
            }
        }
        for (pane, size) in layout.flyout_sizes {
            if pane.area() != Area::Document && size.is_finite() && size > 0.0 {
                workspace.set_flyout_size(&pane, size);
            }
        }
        workspace
    }

    /// Se o painel está aberto: encaixado, flutuando ou desafixado.
    pub fn is_open(&self, pane: &P) -> bool {
        self.state.find_tab(pane).is_some() || self.is_hidden(pane)
    }

    /// Se o painel está desafixado (numa faixa de auto-hide).
    pub fn is_hidden(&self, pane: &P) -> bool {
        self.hidden.contains(pane)
    }

    /// Painéis desafixados, na ordem em que foram escondidos.
    pub fn hidden(&self) -> &[P] {
        &self.hidden
    }

    /// Painel desafixado que está aberto por cima dos documentos.
    pub fn flyout(&self) -> Option<&P> {
        self.flyout.as_ref()
    }

    /// Se o painel aberto da faixa abriu só com o mouse por cima (e some
    /// quando o mouse sai).
    pub fn flyout_is_transient(&self) -> bool {
        self.flyout.is_some() && !self.sticky
    }

    /// Retângulo do painel aberto da faixa no último frame, já aberto por
    /// inteiro (sem a animação de deslizar).
    pub fn flyout_rect(&self) -> Option<egui::Rect> {
        self.flyout_rect.filter(|_| self.flyout.is_some())
    }

    /// Tamanho escolhido para o painel aberto da faixa (largura nas
    /// laterais, altura embaixo), se o usuário redimensionou.
    pub fn flyout_size(&self, pane: &P) -> Option<f32> {
        self.flyout_sizes
            .iter()
            .find(|(p, _)| p == pane)
            .map(|(_, s)| *s)
    }

    /// Guarda o tamanho do painel aberto da faixa.
    pub fn set_flyout_size(&mut self, pane: &P, size: f32) {
        let size = size.max(FLYOUT_MIN);
        match self.flyout_sizes.iter_mut().find(|(p, _)| p == pane) {
            Some(entry) => entry.1 = size,
            None => self.flyout_sizes.push((pane.clone(), size)),
        }
    }

    /// Abre ou esconde o painel da faixa. Um painel aberto pelo mouse
    /// (`sticky` falso) some quando o mouse sai dele e do botão.
    fn set_flyout(&mut self, pane: Option<P>, sticky: bool) {
        self.sticky = sticky && pane.is_some();
        self.left_at = None;
        if pane.is_none() {
            // Com o mouse ainda sobre o botão, não reabre sozinho.
            if let Some(hover) = &mut self.hover {
                hover.armed = false;
            }
        }
        self.flyout = pane;
    }

    /// Retângulo do botão do painel na faixa no último frame.
    pub fn strip_button_rect(&self, pane: &P) -> Option<egui::Rect> {
        self.strip_buttons
            .iter()
            .find(|(p, _)| p == pane)
            .map(|(_, r)| *r)
    }

    /// Desafixa o painel: sai do docking e vai para a faixa da sua área.
    /// Documentos não podem ser desafixados. Devolve se escondeu.
    pub fn auto_hide(&mut self, pane: &P) -> bool {
        if pane.area() == Area::Document || self.is_hidden(pane) {
            return false;
        }
        let Some(path) = self.state.find_tab(pane) else {
            return false;
        };
        if let Some(pane) = self.state.remove_tab(path) {
            self.hidden.push(pane);
        }
        true
    }

    /// Desafixa todos os painéis que não são documentos (Window > Auto Hide All).
    pub fn auto_hide_all(&mut self) {
        let tools: Vec<P> = self
            .state
            .iter_all_tabs()
            .map(|(_, p)| p.clone())
            .filter(|p| p.area() != Area::Document)
            .collect();
        for pane in tools {
            self.auto_hide(&pane);
        }
        self.set_flyout(None, false);
    }

    /// Fixa de novo um painel desafixado, na área de origem.
    pub fn dock(&mut self, pane: &P) {
        if let Some(i) = self.hidden.iter().position(|p| p == pane) {
            let pane = self.hidden.remove(i);
            if self.flyout.as_ref() == Some(&pane) {
                self.set_flyout(None, false);
            }
            self.open(pane);
        }
    }

    /// Mostra um painel desafixado por cima dos documentos.
    pub fn show_flyout(&mut self, pane: &P) {
        if self.is_hidden(pane) {
            self.set_flyout(Some(pane.clone()), true);
        }
    }

    /// Esconde o painel desafixado que estiver aberto.
    pub fn hide_flyout(&mut self) {
        self.set_flyout(None, false);
    }

    /// Se os dois painéis estão abertos como abas do mesmo grupo.
    pub fn same_group(&self, a: &P, b: &P) -> bool {
        match (self.state.find_tab(a), self.state.find_tab(b)) {
            (Some(x), Some(y)) => x.node_path() == y.node_path(),
            _ => false,
        }
    }

    /// Retângulo do grupo de abas do painel no último frame desenhado (ou do
    /// painel aberto da faixa, se estiver desafixado e aberto).
    pub fn group_rect(&self, pane: &P) -> Option<egui::Rect> {
        if self.flyout.as_ref() == Some(pane) {
            return self.flyout_rect();
        }
        let path = self.state.find_tab(pane)?.node_path();
        self.state.leaf(path).ok().map(|leaf| leaf.rect())
    }

    /// Todos os painéis abertos, inclusive os desafixados.
    pub fn panes(&self) -> impl Iterator<Item = &P> {
        self.state
            .iter_all_tabs()
            .map(|(_, p)| p)
            .chain(self.hidden.iter())
    }

    /// Mostra o painel: se já está aberto, traz para a frente; se não, abre
    /// junto de outro painel da mesma área ou recria a área na borda. Em
    /// todo caso, o painel fica com o foco (é o que [`Self::focused`] vê).
    pub fn open(&mut self, pane: P) {
        self.insert(pane.clone());
        if let Some(path) = self.state.find_tab(&pane) {
            let _ = self.state.set_active_tab(path);
            self.state.set_focused_node_and_surface(path.node_path());
        }
    }

    fn insert(&mut self, pane: P) {
        if self.is_hidden(&pane) {
            self.set_flyout(Some(pane), true);
            return;
        }
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
            tree.split_above(node, BOTTOM, vec![pane]);
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
        self.hidden.retain(|p| p != pane);
        if self.flyout.as_ref() == Some(pane) {
            self.set_flyout(None, false);
        }
    }

    /// Desenha o docking ocupando todo o `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui, viewer: &mut impl PaneViewer<P>) {
        let mut requests = Vec::new();
        self.strip_buttons.clear();
        self.strips(ui, viewer, &mut requests);

        let dock_rect = ui.available_rect_before_wrap();
        let style = Style::from_egui(ui.style().as_ref());
        let mut adapter = Adapter {
            viewer: &mut *viewer,
            requests: &mut requests,
        };
        DockArea::new(&mut self.state)
            .id(egui::Id::new("pgstudio_dock"))
            .style(style)
            .show_leaf_collapse_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut adapter);

        self.flyout_ui(ui, dock_rect, viewer, &mut requests);

        for request in requests {
            match request {
                Request::AutoHide(p) => {
                    self.auto_hide(&p);
                }
                Request::Dock(p) => self.dock(&p),
                Request::Close(p) => self.close(&p),
                Request::Toggle(p) => {
                    if let Some(hover) = &mut self.hover {
                        hover.armed = false;
                    }
                    if self.flyout.as_ref() != Some(&p) {
                        self.set_flyout(Some(p), true);
                    } else if self.sticky {
                        self.set_flyout(None, false);
                    } else {
                        // Aberto pelo mouse: o clique só faz o painel ficar.
                        self.sticky = true;
                    }
                }
            }
        }
        self.track_hover(ui.ctx());
    }

    /// Abre o painel com o mouse parado sobre o botão da faixa e esconde o
    /// painel aberto assim quando o mouse sai dele e do botão.
    fn track_hover(&mut self, ctx: &egui::Context) {
        let (now, pos) = ctx.input(|i| (i.time, i.pointer.hover_pos()));
        let over = pos.and_then(|pos| {
            self.strip_buttons
                .iter()
                .find(|(_, rect)| rect.contains(pos))
                .map(|(p, _)| p.clone())
        });
        match (over, &self.hover) {
            (Some(p), Some(hover)) if hover.pane == p => {}
            (Some(p), _) => {
                self.hover = Some(Hover {
                    armed: self.flyout.as_ref() != Some(&p),
                    pane: p,
                    since: now,
                })
            }
            (None, _) => self.hover = None,
        }
        if let Some(hover) = self.hover.as_mut().filter(|h| h.armed) {
            let waited = now - hover.since;
            if waited >= HOVER_OPEN {
                hover.armed = false;
                let pane = hover.pane.clone();
                self.set_flyout(Some(pane), false);
                // O painel é desenhado no próximo frame.
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after_secs((HOVER_OPEN - waited) as f32);
            }
        }

        let Some(pane) = self.flyout.clone() else {
            return;
        };
        if self.sticky || ctx.dragged_id().is_some() {
            self.left_at = None;
            return;
        }
        let inside = pos.is_some_and(|pos| {
            self.flyout_rect.is_some_and(|r| r.contains(pos))
                || self
                    .strip_button_rect(&pane)
                    .is_some_and(|r| r.contains(pos))
        });
        if inside {
            self.left_at = None;
            return;
        }
        let left_at = *self.left_at.get_or_insert(now);
        if now - left_at >= HOVER_CLOSE {
            self.set_flyout(None, false);
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after_secs((HOVER_CLOSE - (now - left_at)) as f32);
        }
    }

    /// Faixas de auto-hide nas bordas, com um botão por painel desafixado.
    fn strips(
        &mut self,
        ui: &mut egui::Ui,
        viewer: &mut impl PaneViewer<P>,
        requests: &mut Vec<Request<P>>,
    ) {
        let on = |area: Area| -> Vec<P> {
            self.hidden
                .iter()
                .filter(|p| p.area() == area)
                .cloned()
                .collect()
        };
        let (left, right, bottom) = (on(Area::Left), on(Area::Right), on(Area::Bottom));
        let open = self.flyout.clone();
        let mut buttons = Vec::new();
        let mut strip = |ui: &mut egui::Ui, panes: &[P], vertical: bool| {
            for pane in panes {
                let title = viewer.title(pane);
                let selected = open.as_ref() == Some(pane);
                let response = strip_button(ui, &title, vertical, selected);
                if response.clicked() {
                    requests.push(Request::Toggle(pane.clone()));
                }
                buttons.push((pane.clone(), response.rect));
            }
        };
        let frame = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(egui::Margin::same(2));
        if !bottom.is_empty() {
            egui::Panel::bottom("pgstudio_autohide_bottom")
                .exact_size(STRIP)
                .resizable(false)
                .frame(frame)
                .show(ui, |ui| {
                    ui.horizontal(|ui| strip(ui, &bottom, false));
                });
        }
        if !left.is_empty() {
            egui::Panel::left("pgstudio_autohide_left")
                .exact_size(STRIP)
                .resizable(false)
                .frame(frame)
                .show(ui, |ui| {
                    ui.vertical(|ui| strip(ui, &left, true));
                });
        }
        if !right.is_empty() {
            egui::Panel::right("pgstudio_autohide_right")
                .exact_size(STRIP)
                .resizable(false)
                .frame(frame)
                .show(ui, |ui| {
                    ui.vertical(|ui| strip(ui, &right, true));
                });
        }
        self.strip_buttons = buttons;
    }

    /// Painel desafixado aberto por cima dos documentos, encostado na faixa.
    /// Desliza para dentro ao abrir e para fora ao esconder; a borda interna
    /// pode ser arrastada para mudar o tamanho.
    fn flyout_ui(
        &mut self,
        ui: &mut egui::Ui,
        dock: egui::Rect,
        viewer: &mut impl PaneViewer<P>,
        requests: &mut Vec<Request<P>>,
    ) {
        let ctx = ui.ctx().clone();
        if self.flyout.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.set_flyout(None, false);
        }
        if self.flyout.is_none() {
            self.flyout_rect = None;
        }
        // Painel fixado de novo ou fechado não desliza para fora: já está em
        // outro lugar (ou em lugar nenhum).
        if self
            .slide
            .as_ref()
            .is_some_and(|s| !self.hidden.contains(&s.pane))
        {
            self.slide = None;
        }

        // Avança a animação na direção do estado atual.
        let duration = if ui.style().animation_time > 0.0 {
            SLIDE
        } else {
            0.0
        };
        let step = if duration > 0.0 {
            ctx.input(|i| i.stable_dt).min(0.1) / duration
        } else {
            1.0
        };
        match (&self.flyout, &mut self.slide) {
            (Some(p), Some(slide)) if slide.pane == *p => {
                slide.progress = (slide.progress + step).min(1.0);
            }
            (Some(p), _) => {
                self.slide = Some(Slide {
                    pane: p.clone(),
                    progress: if duration > 0.0 { 0.0 } else { 1.0 },
                });
            }
            (None, Some(slide)) => {
                slide.progress -= step;
                if slide.progress <= 0.0 {
                    self.slide = None;
                }
            }
            (None, None) => {}
        }
        let Some(slide) = &self.slide else {
            return;
        };
        let mut pane = slide.pane.clone();
        let progress = slide.progress;
        let open = self.flyout.as_ref() == Some(&pane);
        if progress < 1.0 {
            ctx.request_repaint();
        }

        let area = pane.area();
        let extent = match area {
            Area::Bottom => dock.height(),
            _ => dock.width(),
        };
        let max = (extent * FLYOUT_MAX).max(FLYOUT_MIN);
        let size = match self.flyout_size(&pane) {
            Some(size) => size.clamp(FLYOUT_MIN.min(max), max),
            None => FLYOUT_SIZE.min(extent * 0.8),
        };
        let rect = flyout_rect(area, dock, size);
        if open {
            self.flyout_rect = Some(rect);
        }
        // Desliza a partir da faixa: 0 escondido, 1 encostado nela.
        let eased = 1.0 - (1.0 - progress).powi(3);
        let hidden_by = (1.0 - eased) * size;
        let offset = match area {
            Area::Right => egui::vec2(hidden_by, 0.0),
            Area::Bottom => egui::vec2(0.0, hidden_by),
            _ => egui::vec2(-hidden_by, 0.0),
        };
        let shown = rect.translate(offset);

        let title = viewer.title(&pane);
        let mut resize = 0.0;
        egui::Area::new(egui::Id::new("pgstudio_autohide_flyout"))
            .order(egui::Order::Foreground)
            .fixed_pos(shown.min)
            .interactable(open)
            .show(&ctx, |ui| {
                ui.set_clip_rect(dock);
                let frame = egui::Frame::window(ui.style())
                    .corner_radius(0)
                    .inner_margin(egui::Margin::same(4));
                let inner = rect.size() - frame.total_margin().sum();
                frame.show(ui, |ui| {
                    ui.set_min_size(inner);
                    ui.set_max_size(inner);
                    ui.horizontal(|ui| {
                        ui.strong(&title);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if viewer.closeable(&pane)
                                && ui.small_button("✕").on_hover_text("Close").clicked()
                                && viewer.on_close(&pane)
                            {
                                requests.push(Request::Close(pane.clone()));
                            }
                            if ui
                                .small_button(egui_phosphor::regular::PUSH_PIN_SLASH)
                                .on_hover_text("Auto Hide (click to dock)")
                                .clicked()
                            {
                                requests.push(Request::Dock(pane.clone()));
                            }
                        });
                    });
                    ui.separator();
                    viewer.ui(ui, &mut pane);
                });
                if open {
                    resize = resize_grip(ui, area, shown);
                }
            });
        if resize != 0.0 {
            self.set_flyout_size(&pane, (size + resize).clamp(FLYOUT_MIN.min(max), max));
            self.sticky = true;
        }
        if !open {
            return;
        }

        // Clicar nos documentos ou nos outros painéis esconde o painel, como
        // no SSMS. Menus, toolbar e janelas por cima (como um menu aberto
        // sobre os documentos) não contam, para Window > Auto Hide funcionar.
        // Clicar dentro do painel faz um painel aberto pelo mouse ficar.
        let press = ctx.input(|i| {
            i.pointer
                .any_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        let Some(press) = press else {
            return;
        };
        if rect.contains(press) {
            self.sticky = true;
            return;
        }
        let over_popup = ctx
            .layer_id_at(press)
            .is_some_and(|layer| layer.order != egui::Order::Background);
        if dock.contains(press) && !over_popup {
            self.set_flyout(None, false);
        }
    }
}

/// Retângulo do painel aberto da faixa de `area`, com `size` de largura (ou
/// altura, embaixo), encostado na borda de `dock`.
fn flyout_rect(area: Area, dock: egui::Rect, size: f32) -> egui::Rect {
    match area {
        Area::Right => {
            egui::Rect::from_min_max(egui::pos2(dock.right() - size, dock.top()), dock.max)
        }
        Area::Bottom => {
            egui::Rect::from_min_max(egui::pos2(dock.left(), dock.bottom() - size), dock.max)
        }
        _ => egui::Rect::from_min_size(dock.min, egui::vec2(size, dock.height())),
    }
}

/// Alça na borda interna do painel aberto da faixa (a que dá para os
/// documentos). Devolve quanto o painel cresceu neste frame.
fn resize_grip(ui: &mut egui::Ui, area: Area, rect: egui::Rect) -> f32 {
    let half = GRIP / 2.0;
    let (grip, cursor) = match area {
        Area::Right => (
            egui::Rect::from_x_y_ranges(rect.left()..=rect.left() + GRIP, rect.y_range()),
            egui::CursorIcon::ResizeHorizontal,
        ),
        Area::Bottom => (
            egui::Rect::from_x_y_ranges(rect.x_range(), rect.top()..=rect.top() + GRIP),
            egui::CursorIcon::ResizeVertical,
        ),
        _ => (
            egui::Rect::from_x_y_ranges(rect.right() - GRIP..=rect.right(), rect.y_range()),
            egui::CursorIcon::ResizeHorizontal,
        ),
    };
    let response = ui
        .interact(
            grip,
            egui::Id::new("pgstudio_autohide_resize"),
            egui::Sense::drag(),
        )
        .on_hover_cursor(cursor);
    let color = if response.dragged() {
        ui.visuals().widgets.active.bg_stroke.color
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_stroke.color
    } else {
        egui::Color32::TRANSPARENT
    };
    if color != egui::Color32::TRANSPARENT {
        let line = match area {
            Area::Bottom => grip.shrink2(egui::vec2(0.0, half - 1.0)),
            _ => grip.shrink2(egui::vec2(half - 1.0, 0.0)),
        };
        ui.painter().rect_filled(line, 0.0, color);
    }
    if !response.dragged() {
        return 0.0;
    }
    let delta = response.drag_delta();
    match area {
        Area::Right => -delta.x,
        Area::Bottom => -delta.y,
        _ => delta.x,
    }
}

/// Botão de uma faixa de auto-hide; nas laterais, com o texto na vertical.
fn strip_button(ui: &mut egui::Ui, text: &str, vertical: bool, selected: bool) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let color = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let pad = egui::vec2(8.0, 3.0);
    let text_size = galley.size();
    let size = if vertical {
        egui::vec2(text_size.y + 2.0 * pad.y, text_size.x + 2.0 * pad.x)
    } else {
        text_size + 2.0 * pad
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, text)
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter().rect_filled(rect, 2.0, visuals.weak_bg_fill);
        let shape = if vertical {
            egui::epaint::TextShape::new(
                egui::pos2(rect.right() - pad.y, rect.top() + pad.x),
                galley,
                visuals.text_color(),
            )
            .with_angle(std::f32::consts::FRAC_PI_2)
        } else {
            egui::epaint::TextShape::new(rect.min + pad, galley, visuals.text_color())
        };
        ui.painter().add(shape);
    }
    response
}

/// Liga o [`PaneViewer`] ao `TabViewer` do egui_dock.
struct Adapter<'a, P, V> {
    viewer: &'a mut V,
    requests: &'a mut Vec<Request<P>>,
}

impl<P: Pane, V: PaneViewer<P>> TabViewer for Adapter<'_, P, V> {
    type Tab = P;

    fn title(&mut self, tab: &mut P) -> egui::WidgetText {
        self.viewer.title(tab).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut P) {
        self.viewer.ui(ui, tab);
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut P, _path: NodePath) {
        if tab.area() != Area::Document && ui.button("Auto Hide").clicked() {
            self.requests.push(Request::AutoHide(tab.clone()));
            ui.close();
        }
    }

    fn id(&mut self, tab: &mut P) -> egui::Id {
        egui::Id::new(("pgstudio_pane", &*tab))
    }

    fn closeable(&mut self, tab: &mut P) -> bool {
        self.viewer.closeable(tab)
    }

    fn on_close(&mut self, tab: &mut P) -> OnCloseResponse {
        if self.viewer.on_close(tab) {
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
                hidden: vec![P::Props, P::Doc(3), P::Explorer],
                flyout_sizes: vec![],
            },
            Layout {
                root: Some(tabs(vec![], 3)),
                floating: vec![],
                hidden: vec![],
                flyout_sizes: vec![],
            },
            Layout {
                root: Some(split(
                    f32::NAN,
                    tabs(vec![P::Explorer], 9),
                    tabs(vec![P::Explorer], 0),
                )),
                floating: vec![P::Explorer],
                hidden: vec![],
                flyout_sizes: vec![],
            },
            Layout {
                root: Some(split(
                    -4.0,
                    tabs(vec![], 0),
                    tabs(vec![P::Output, P::Doc(1)], 1),
                )),
                floating: vec![],
                hidden: vec![],
                flyout_sizes: vec![],
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
            hidden: vec![],
            flyout_sizes: vec![],
        });
        render(&mut ws);
    }

    #[test]
    fn auto_hide_moves_tool_windows_to_the_strip_and_back() {
        let mut ws = ssms();
        assert!(ws.auto_hide(&P::Explorer));
        assert!(ws.is_hidden(&P::Explorer));
        assert!(ws.is_open(&P::Explorer));
        assert_eq!(ws.panes().filter(|p| **p == P::Explorer).count(), 1);
        // Documentos não são desafixados.
        assert!(!ws.auto_hide(&P::Doc(1)));
        // Abrir um painel desafixado mostra o painel por cima.
        ws.open(P::Explorer);
        assert_eq!(ws.flyout(), Some(&P::Explorer));
        ws.dock(&P::Explorer);
        assert!(!ws.is_hidden(&P::Explorer));
        assert_eq!(ws.flyout(), None);
        assert!(ws.state.find_tab(&P::Explorer).is_some());
    }

    #[test]
    fn auto_hide_all_keeps_documents_docked() {
        let mut ws = ssms();
        ws.auto_hide_all();
        assert_eq!(ws.hidden().len(), 3);
        assert!(ws.state.find_tab(&P::Doc(1)).is_some());
        ws.close(&P::Props);
        assert!(!ws.is_open(&P::Props));
        assert_eq!(ws.hidden().len(), 2);
    }

    #[test]
    fn hidden_panes_are_saved_in_the_layout() {
        let mut ws = ssms();
        ws.auto_hide(&P::Output);
        let layout = ws.layout(|_| true);
        assert_eq!(layout.hidden, vec![P::Output]);
        let json = serde_json::to_string(&layout).unwrap();
        let restored = Workspace::from_layout(serde_json::from_str(&json).unwrap());
        assert!(restored.is_hidden(&P::Output));
        // Arquivos da versão anterior, sem "hidden", continuam valendo.
        let old: Layout<P> = serde_json::from_str(r#"{"root":null}"#).unwrap();
        assert!(old.hidden.is_empty());
    }

    #[test]
    fn strips_and_flyout_sit_on_their_edges() {
        let mut ws = ssms();
        ws.auto_hide(&P::Explorer);
        ws.auto_hide(&P::Output);
        for (pane, check) in [
            (
                P::Explorer,
                (|r: egui::Rect| r.left() <= STRIP + 1.0) as fn(egui::Rect) -> bool,
            ),
            (P::Output, |r: egui::Rect| r.bottom() >= 800.0 - STRIP - 1.0),
        ] {
            ws.show_flyout(&pane);
            render(&mut ws);
            let button = ws.strip_button_rect(&pane).expect("botão na faixa");
            assert!(check(button), "{pane:?} {button:?}");
            let flyout = ws.flyout_rect().expect("painel aberto");
            assert!(
                flyout.width() > 100.0 && flyout.height() > 100.0,
                "{flyout:?}"
            );
        }
        // O painel de baixo encosta na faixa de baixo e cobre a largura.
        let flyout = ws.flyout_rect().unwrap();
        assert!(flyout.width() > 1000.0);
    }

    #[test]
    fn reopened_document_area_is_taller_than_the_bottom_strip() {
        let mut ws = ssms();
        ws.close(&P::Doc(1));
        ws.open(P::Doc(2));
        render(&mut ws);
        let doc = ws.group_rect(&P::Doc(2)).unwrap();
        let output = ws.group_rect(&P::Output).unwrap();
        assert!(doc.height() > 2.0 * output.height(), "{doc:?} {output:?}");
        assert!(doc.bottom() <= output.top() + 1.0);
    }

    /// Roda frames com relógio e eventos de mouse, para testar o auto-hide.
    struct Driver {
        ctx: egui::Context,
        time: f64,
        pointer: egui::Pos2,
    }

    impl Driver {
        fn new() -> Self {
            Driver {
                ctx: egui::Context::default(),
                time: 0.0,
                pointer: egui::pos2(640.0, 300.0),
            }
        }

        fn frame(&mut self, ws: &mut Workspace<P>, dt: f64, events: Vec<egui::Event>) {
            self.time += dt;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(self.time),
                predicted_dt: dt as f32,
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| ws.show(ui, &mut Viewer));
            output.textures_delta.clear();
        }

        /// Espera `secs` com o mouse parado, em frames de 1/60 s.
        fn wait(&mut self, ws: &mut Workspace<P>, secs: f64) {
            let frames = (secs * 60.0).ceil() as usize;
            for _ in 0..frames {
                self.frame(ws, 1.0 / 60.0, Vec::new());
            }
        }

        fn move_to(&mut self, ws: &mut Workspace<P>, pos: egui::Pos2) {
            self.pointer = pos;
            self.frame(ws, 1.0 / 60.0, vec![egui::Event::PointerMoved(pos)]);
        }

        fn button(&mut self, ws: &mut Workspace<P>, pressed: bool) {
            let event = egui::Event::PointerButton {
                pos: self.pointer,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(ws, 1.0 / 60.0, vec![event]);
        }

        fn click(&mut self, ws: &mut Workspace<P>) {
            self.button(ws, true);
            self.button(ws, false);
        }
    }

    #[test]
    fn hovering_the_strip_opens_the_panel_until_the_mouse_leaves() {
        let mut ws = ssms();
        ws.auto_hide(&P::Explorer);
        let mut d = Driver::new();
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        let button = ws.strip_button_rect(&P::Explorer).unwrap();
        d.move_to(&mut ws, button.center());
        d.wait(&mut ws, HOVER_OPEN / 2.0);
        assert_eq!(ws.flyout(), None, "passar rápido não abre");
        d.wait(&mut ws, HOVER_OPEN);
        assert_eq!(ws.flyout(), Some(&P::Explorer));
        assert!(ws.flyout_is_transient());

        // Ir do botão para o painel não esconde.
        let rect = ws.flyout_rect().unwrap();
        d.move_to(&mut ws, rect.center());
        d.wait(&mut ws, HOVER_CLOSE * 2.0);
        assert_eq!(ws.flyout(), Some(&P::Explorer));

        // Sair para os documentos esconde, depois de um tempo.
        d.move_to(&mut ws, egui::pos2(900.0, 300.0));
        d.wait(&mut ws, HOVER_CLOSE / 2.0);
        assert_eq!(ws.flyout(), Some(&P::Explorer));
        d.wait(&mut ws, HOVER_CLOSE);
        assert_eq!(ws.flyout(), None);

        // Voltar ao botão abre de novo.
        d.move_to(&mut ws, button.center());
        d.wait(&mut ws, HOVER_OPEN * 1.5);
        assert_eq!(ws.flyout(), Some(&P::Explorer));
    }

    #[test]
    fn using_a_hover_opened_panel_keeps_it_open() {
        let mut ws = ssms();
        ws.auto_hide(&P::Props);
        let mut d = Driver::new();
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        let button = ws.strip_button_rect(&P::Props).unwrap();
        d.move_to(&mut ws, button.center());
        d.wait(&mut ws, HOVER_OPEN * 1.5);
        let rect = ws.flyout_rect().unwrap();
        d.move_to(&mut ws, rect.center());
        d.click(&mut ws);
        assert!(!ws.flyout_is_transient());
        d.move_to(&mut ws, egui::Pos2::new(10.0, 10.0));
        d.wait(&mut ws, HOVER_CLOSE * 2.0);
        assert_eq!(ws.flyout(), Some(&P::Props));
    }

    #[test]
    fn closed_panel_does_not_reopen_under_a_resting_mouse() {
        let mut ws = ssms();
        ws.auto_hide(&P::Output);
        let mut d = Driver::new();
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        let button = ws.strip_button_rect(&P::Output).unwrap();
        d.move_to(&mut ws, button.center());
        d.click(&mut ws);
        assert_eq!(ws.flyout(), Some(&P::Output));
        assert!(!ws.flyout_is_transient(), "clique deixa o painel aberto");
        d.click(&mut ws);
        assert_eq!(ws.flyout(), None);
        d.wait(&mut ws, HOVER_OPEN * 3.0);
        assert_eq!(ws.flyout(), None);
    }

    #[test]
    fn the_panel_slides_in_and_out() {
        let mut ws = ssms();
        ws.auto_hide(&P::Explorer);
        let mut d = Driver::new();
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        ws.show_flyout(&P::Explorer);
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        let progress = ws.slide.as_ref().unwrap().progress;
        assert!(progress > 0.0 && progress < 1.0, "{progress}");
        d.wait(&mut ws, f64::from(SLIDE));
        assert_eq!(ws.slide.as_ref().unwrap().progress, 1.0);

        ws.hide_flyout();
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        assert!(ws.slide.is_some(), "ainda deslizando para fora");
        assert_eq!(ws.flyout_rect(), None);
        d.wait(&mut ws, f64::from(SLIDE) * 2.0);
        assert!(ws.slide.is_none());

        // Fixar de novo não anima: o painel já voltou para o docking.
        ws.show_flyout(&P::Explorer);
        d.wait(&mut ws, f64::from(SLIDE) * 2.0);
        ws.dock(&P::Explorer);
        d.frame(&mut ws, 1.0 / 60.0, Vec::new());
        assert!(ws.slide.is_none());
    }

    #[test]
    fn dragging_the_inner_edge_resizes_the_panel() {
        for (pane, grab, drag, grows) in [
            (
                P::Explorer,
                (|r: egui::Rect| r.right_center() - egui::vec2(2.0, 0.0))
                    as fn(egui::Rect) -> egui::Pos2,
                egui::vec2(100.0, 0.0),
                (|r: egui::Rect| r.width()) as fn(egui::Rect) -> f32,
            ),
            (
                P::Props,
                |r: egui::Rect| r.left_center() + egui::vec2(2.0, 0.0),
                egui::vec2(-100.0, 0.0),
                |r: egui::Rect| r.width(),
            ),
            (
                P::Output,
                |r: egui::Rect| r.center_top() + egui::vec2(0.0, 2.0),
                egui::vec2(0.0, -100.0),
                |r: egui::Rect| r.height(),
            ),
        ] {
            let mut ws = ssms();
            ws.auto_hide(&pane);
            ws.show_flyout(&pane);
            let mut d = Driver::new();
            d.wait(&mut ws, 0.5);
            let before = ws.flyout_rect().unwrap();
            d.move_to(&mut ws, grab(before));
            d.button(&mut ws, true);
            for _ in 0..4 {
                let to = d.pointer + drag / 4.0;
                d.move_to(&mut ws, to);
            }
            d.button(&mut ws, false);
            d.wait(&mut ws, 0.1);
            let after = ws.flyout_rect().unwrap();
            let grew = grows(after) - grows(before);
            assert!((grew - 100.0).abs() < 2.0, "{pane:?} {grew}");
            assert_eq!(ws.flyout(), Some(&pane), "arrastar não esconde");

            // O tamanho fica salvo no layout.
            let layout = ws.layout(|_| true);
            let json = serde_json::to_string(&layout).unwrap();
            let restored = Workspace::from_layout(serde_json::from_str(&json).unwrap());
            assert_eq!(restored.flyout_size(&pane), ws.flyout_size(&pane));
        }
    }

    #[test]
    fn resized_panel_stays_within_bounds() {
        let mut ws = ssms();
        ws.auto_hide(&P::Explorer);
        ws.set_flyout_size(&P::Explorer, 5000.0);
        ws.show_flyout(&P::Explorer);
        render(&mut ws);
        let rect = ws.flyout_rect().unwrap();
        assert!(rect.width() < 1280.0 * FLYOUT_MAX + 1.0, "{rect:?}");
        ws.set_flyout_size(&P::Explorer, 1.0);
        render(&mut ws);
        assert!(ws.flyout_rect().unwrap().width() >= FLYOUT_MIN);
        // Tamanho inválido num arquivo é ignorado.
        let mut layout = ws.layout(|_| true);
        layout.flyout_sizes = vec![(P::Explorer, f32::NAN), (P::Doc(1), 200.0)];
        let restored = Workspace::from_layout(layout);
        assert_eq!(restored.flyout_size(&P::Explorer), None);
        assert_eq!(restored.flyout_size(&P::Doc(1)), None);
    }
}
