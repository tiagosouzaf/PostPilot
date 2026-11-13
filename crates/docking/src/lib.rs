//! Docking de painéis no estilo do SSMS.
//!
//! Invólucro fino sobre o `egui_dock`: o resto do PGStudio só conhece
//! [`Workspace`], [`Pane`] e [`PaneViewer`]. Isso permite trocar ou estender o
//! docking (por exemplo, para ter auto-hide) sem mexer na casca.

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};

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
pub trait Pane: Clone + PartialEq {
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
            Area::Document => self.state.push_to_focused_leaf(pane),
        }
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
        egui::Id::new(("pgstudio_pane", self.0.title(tab)))
    }

    fn closeable(&mut self, tab: &mut P) -> bool {
        self.0.closeable(tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
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
    fn documents_join_the_document_area() {
        let mut ws = ssms();
        ws.open(P::Doc(2));
        assert!(ws.same_group(&P::Doc(1), &P::Doc(2)));
        assert!(!ws.same_group(&P::Doc(1), &P::Explorer));
    }
}
