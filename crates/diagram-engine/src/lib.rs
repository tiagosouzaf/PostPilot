//! Diagrama do banco: caixas de tabela com as colunas (chave primária e
//! estrangeira marcadas) e linhas de relacionamento, num canvas com zoom e
//! pan, como o Database Diagram do SSMS.
//!
//! - [`Diagram::from_model`] monta o diagrama de algumas tabelas de um
//!   [`Model`]; as FKs entre elas viram linhas.
//! - [`Diagram::auto_layout`] põe as tabelas em colunas: as referenciadas à
//!   esquerda das que apontam para elas.
//! - [`Diagram::show`] desenha só o que está na tela e, com o zoom bem
//!   afastado, só o contorno e o nome de cada tabela, para manter 60 FPS
//!   com centenas de tabelas.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Vec2, pos2, vec2,
};
use pgstudio_schema_model::{Id, Model};

/// Largura de uma caixa de tabela (em unidades do diagrama).
pub const NODE_WIDTH: f32 = 220.0;
/// Altura do título da caixa.
pub const HEADER_HEIGHT: f32 = 24.0;
/// Altura de cada coluna na caixa.
pub const ROW_HEIGHT: f32 = 18.0;
/// Espaço entre colunas de tabelas no layout automático.
const GAP_X: f32 = 120.0;
/// Espaço entre tabelas empilhadas.
const GAP_Y: f32 = 40.0;
/// Zoom mínimo e máximo.
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 4.0;
/// Abaixo deste zoom as colunas não são desenhadas (só a caixa e o nome).
const DETAIL_ZOOM: f32 = 0.5;
/// Abaixo deste zoom nem o nome é desenhado (ficaria com menos de 5 px).
const TITLE_ZOOM: f32 = 0.2;

/// Uma coluna dentro da caixa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeColumn {
    pub name: String,
    pub ty: String,
    pub primary_key: bool,
    pub foreign_key: bool,
    pub nullable: bool,
}

/// Uma tabela no diagrama.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Id da tabela no [`Model`].
    pub table: Id,
    pub schema: String,
    pub name: String,
    pub columns: Vec<NodeColumn>,
    /// Canto superior esquerdo, em unidades do diagrama.
    pub pos: Pos2,
}

impl Node {
    pub fn size(&self) -> Vec2 {
        vec2(
            NODE_WIDTH,
            HEADER_HEIGHT + self.columns.len().max(1) as f32 * ROW_HEIGHT + 6.0,
        )
    }

    /// Retângulo da caixa, em unidades do diagrama.
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos, self.size())
    }

    /// `schema.tabela`.
    pub fn title(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

/// Um relacionamento (FK) entre duas tabelas do diagrama.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub name: String,
    /// Índice em [`Diagram::nodes`] da tabela com a FK.
    pub from: usize,
    /// Índice da tabela referenciada.
    pub to: usize,
}

/// Como o diagrama aparece na tela: deslocamento e zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Posição na tela (relativa ao canto do canvas) do ponto (0, 0) do
    /// diagrama.
    pub pan: Vec2,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// Ponto do diagrama para a tela, num canvas que começa em `origin`.
    pub fn to_screen(&self, origin: Pos2, p: Pos2) -> Pos2 {
        origin + self.pan + p.to_vec2() * self.zoom
    }

    /// Ponto da tela para o diagrama.
    pub fn to_world(&self, origin: Pos2, p: Pos2) -> Pos2 {
        ((p - origin - self.pan) / self.zoom).to_pos2()
    }

    pub fn rect_to_screen(&self, origin: Pos2, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(origin, r.min), self.to_screen(origin, r.max))
    }

    /// Muda o zoom mantendo o ponto `anchor` da tela no lugar.
    pub fn zoom_at(&mut self, origin: Pos2, anchor: Pos2, factor: f32) {
        let world = self.to_world(origin, anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = anchor - origin - world.to_vec2() * self.zoom;
    }
}

/// O diagrama: tabelas, relacionamentos e a câmera.
#[derive(Clone, Debug, Default)]
pub struct Diagram {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub camera: Camera,
    /// Tabela selecionada (índice em `nodes`).
    pub selected: Option<usize>,
    /// Enquadrar tudo no próximo frame (ao abrir).
    fit_pending: bool,
    /// Caixas e linhas desenhadas no último frame.
    drawn: (usize, usize),
}

impl Diagram {
    /// Diagrama das tabelas `tables` do modelo (na ordem dada), com as FKs
    /// entre elas, já com layout automático.
    pub fn from_model(model: &Model, tables: &[Id]) -> Diagram {
        let mut nodes = Vec::new();
        for id in tables {
            let Some(t) = model.table(*id) else {
                continue;
            };
            let pk: &[String] = t.primary_key.as_ref().map_or(&[], |k| &k.columns);
            let columns = t
                .columns
                .iter()
                .map(|c| NodeColumn {
                    name: c.name.clone(),
                    ty: c.ty.clone(),
                    primary_key: pk.contains(&c.name),
                    foreign_key: t.foreign_keys.iter().any(|fk| fk.columns.contains(&c.name)),
                    nullable: c.nullable,
                })
                .collect();
            nodes.push(Node {
                table: t.id,
                schema: t.schema.clone(),
                name: t.name.clone(),
                columns,
                pos: Pos2::ZERO,
            });
        }
        let mut edges = Vec::new();
        for (from, node) in nodes.iter().enumerate() {
            let Some(t) = model.table(node.table) else {
                continue;
            };
            for fk in &t.foreign_keys {
                let to = nodes
                    .iter()
                    .position(|n| n.schema == fk.ref_schema && n.name == fk.ref_table);
                if let Some(to) = to {
                    edges.push(Edge {
                        name: fk.name.clone(),
                        from,
                        to,
                    });
                }
            }
        }
        let mut d = Diagram {
            nodes,
            edges,
            fit_pending: true,
            ..Default::default()
        };
        d.auto_layout();
        d
    }

    /// Põe as tabelas em colunas: quem não aponta para ninguém fica na
    /// primeira; as outras, uma coluna à direita da mais à direita que elas
    /// referenciam. Colunas altas demais quebram em várias.
    pub fn auto_layout(&mut self) {
        let n = self.nodes.len();
        if n == 0 {
            return;
        }
        // Camada de cada tabela (o caminho mais longo pelas FKs, limitado
        // a n passos para ciclos não rodarem para sempre).
        let mut layer = vec![0usize; n];
        for _ in 0..n {
            let mut changed = false;
            for e in &self.edges {
                if e.from != e.to && layer[e.from] < layer[e.to] + 1 && layer[e.to] + 1 < n {
                    layer[e.from] = layer[e.to] + 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let area: f32 = self
            .nodes
            .iter()
            .map(|n| (n.size().x + GAP_X) * (n.size().y + GAP_Y))
            .sum();
        let max_height = (area.sqrt() * 0.8).max(600.0);
        let layers = layer.iter().max().copied().unwrap_or(0) + 1;
        let mut x = 0.0;
        for l in 0..layers {
            let mut members: Vec<usize> = (0..n).filter(|i| layer[*i] == l).collect();
            members.sort_by(|a, b| self.nodes[*a].title().cmp(&self.nodes[*b].title()));
            let mut y = 0.0;
            let mut used = false;
            for i in members {
                let h = self.nodes[i].size().y;
                if used && y + h > max_height {
                    x += NODE_WIDTH + GAP_X;
                    y = 0.0;
                }
                self.nodes[i].pos = pos2(x, y);
                y += h + GAP_Y;
                used = true;
            }
            if used {
                x += NODE_WIDTH + GAP_X;
            }
        }
        self.fit_pending = true;
    }

    /// Retângulo com todas as tabelas, em unidades do diagrama.
    pub fn bounds(&self) -> Rect {
        self.nodes
            .iter()
            .map(Node::rect)
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0)))
    }

    /// Enquadra tudo num canvas do tamanho `size`.
    pub fn zoom_to_fit(&mut self, size: Vec2) {
        let b = self.bounds().expand(GAP_Y);
        let zoom = (size.x / b.width())
            .min(size.y / b.height())
            .clamp(MIN_ZOOM, 1.0);
        self.camera.zoom = zoom;
        self.camera.pan = size / 2.0 - b.center().to_vec2() * zoom;
        self.fit_pending = false;
    }

    /// Enquadra tudo no próximo frame.
    pub fn fit(&mut self) {
        self.fit_pending = true;
    }

    /// Ids das tabelas no diagrama, na ordem.
    pub fn tables(&self) -> Vec<Id> {
        self.nodes.iter().map(|n| n.table).collect()
    }

    /// Caixas e linhas desenhadas no último frame (o resto estava fora da
    /// tela).
    pub fn drawn(&self) -> (usize, usize) {
        self.drawn
    }

    /// Tabela na posição `p` do diagrama, a de cima primeiro.
    pub fn node_at(&self, p: Pos2) -> Option<usize> {
        self.nodes.iter().rposition(|n| n.rect().contains(p))
    }

    /// Desenha o diagrama ocupando o espaço disponível e trata zoom (Ctrl +
    /// roda ou pinça), pan (roda, arrastar o fundo ou o botão do meio) e
    /// seleção (clique).
    pub fn show(&mut self, ui: &mut egui::Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if self.fit_pending {
            self.zoom_to_fit(rect.size());
        }
        let origin = rect.min;
        self.handle_input(ui, &response, origin);

        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
        let visible = Rect::from_min_max(
            self.camera.to_world(origin, rect.min),
            self.camera.to_world(origin, rect.max),
        );
        let colors = Colors::from(visuals);
        // Longe, tudo vira uma malha só de retângulos sem anti-aliasing:
        // centenas de caixas e linhas com cantos e bordas suavizadas pesam
        // mais no desenho (sobretudo sem GPU) do que se enxerga a essa
        // distância.
        let mut mesh = (self.camera.zoom < DETAIL_ZOOM).then(egui::Mesh::default);

        let mut edges = 0;
        for e in &self.edges {
            let (a, b) = (self.nodes[e.from].rect(), self.nodes[e.to].rect());
            if !a.union(b).intersects(visible) {
                continue;
            }
            edges += 1;
            match &mut mesh {
                Some(mesh) => {
                    let points = edge_points(a, b);
                    for w in points.windows(2) {
                        let (p, q) = (
                            self.camera.to_screen(origin, w[0]),
                            self.camera.to_screen(origin, w[1]),
                        );
                        mesh.add_colored_rect(Rect::from_two_pos(p, q).expand(0.5), colors.line);
                    }
                }
                None => self.paint_edge(&painter, origin, a, b, &colors),
            }
        }
        let mut nodes = 0;
        let mut titles = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if !n.rect().intersects(visible) {
                continue;
            }
            nodes += 1;
            let selected = self.selected == Some(i);
            match &mut mesh {
                Some(mesh) => {
                    let r = self.camera.rect_to_screen(origin, n.rect());
                    let border = if selected {
                        colors.selection
                    } else {
                        colors.border
                    };
                    let width = if selected { 2.0 } else { 1.0 };
                    mesh.add_colored_rect(r, border);
                    let inner = r.shrink(width);
                    mesh.add_colored_rect(inner, colors.body);
                    let header = Rect::from_min_size(
                        inner.min,
                        vec2(inner.width(), HEADER_HEIGHT * self.camera.zoom),
                    );
                    mesh.add_colored_rect(header, colors.header);
                    if self.camera.zoom >= TITLE_ZOOM {
                        titles.push((header, n));
                    }
                }
                None => self.paint_node(&painter, origin, n, selected, &colors),
            }
        }
        if let Some(mesh) = mesh {
            painter.add(mesh);
            for (header, n) in titles {
                painter
                    .with_clip_rect(header.intersect(painter.clip_rect()))
                    .text(
                        header.left_center() + vec2(6.0 * self.camera.zoom, 0.0),
                        Align2::LEFT_CENTER,
                        n.title(),
                        font(6.5),
                        colors.header_text,
                    );
            }
        }
        self.drawn = (nodes, edges);
        response
    }

    fn handle_input(&mut self, ui: &egui::Ui, response: &Response, origin: Pos2) {
        if response.dragged_by(egui::PointerButton::Primary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.camera.pan += response.drag_delta();
        }
        if response.clicked() {
            self.selected = response
                .interact_pointer_pos()
                .and_then(|p| self.node_at(self.camera.to_world(origin, p)));
        }
        if response.hovered() {
            let (zoom, scroll, pointer, ctrl) = ui.input(|i| {
                (
                    i.zoom_delta(),
                    i.smooth_scroll_delta,
                    i.pointer.hover_pos(),
                    i.modifiers.command,
                )
            });
            let anchor = pointer.unwrap_or(response.rect.center());
            if zoom != 1.0 {
                self.camera.zoom_at(origin, anchor, zoom);
            } else if !ctrl && scroll != Vec2::ZERO {
                self.camera.pan += scroll;
            }
        }
    }

    fn paint_node(
        &self,
        painter: &egui::Painter,
        origin: Pos2,
        n: &Node,
        selected: bool,
        c: &Colors,
    ) {
        let zoom = self.camera.zoom;
        let r = self.camera.rect_to_screen(origin, n.rect());
        let radius = CornerRadius::same((3.0 * zoom).clamp(0.0, 6.0) as u8);
        painter.rect_filled(r, radius, c.body);
        let header = Rect::from_min_size(r.min, vec2(r.width(), HEADER_HEIGHT * zoom));
        painter.rect_filled(header, radius, c.header);
        let stroke = if selected {
            Stroke::new(2.0, c.selection)
        } else {
            Stroke::new(1.0, c.border)
        };
        painter.rect_stroke(r, radius, stroke, StrokeKind::Inside);
        let clip = painter.with_clip_rect(r.shrink(1.0).intersect(painter.clip_rect()));
        let text_font = font(13.0 * zoom);
        clip.text(
            header.left_center() + vec2(6.0 * zoom, 0.0),
            Align2::LEFT_CENTER,
            n.title(),
            text_font.clone(),
            c.header_text,
        );
        for (i, col) in n.columns.iter().enumerate() {
            let y = r.min.y + (HEADER_HEIGHT + 3.0 + ROW_HEIGHT * (i as f32 + 0.5)) * zoom;
            let mark = if col.primary_key {
                "🔑"
            } else if col.foreign_key {
                "↗"
            } else {
                ""
            };
            if !mark.is_empty() {
                clip.text(
                    pos2(r.min.x + 10.0 * zoom, y),
                    Align2::CENTER_CENTER,
                    mark,
                    font(11.0 * zoom),
                    c.key,
                );
            }
            clip.text(
                pos2(r.min.x + 20.0 * zoom, y),
                Align2::LEFT_CENTER,
                &col.name,
                text_font.clone(),
                c.text,
            );
            clip.text(
                pos2(r.max.x - 6.0 * zoom, y),
                Align2::RIGHT_CENTER,
                &col.ty,
                font(11.0 * zoom),
                c.weak,
            );
        }
    }

    /// Linha em cotovelo do lado de `a` (a tabela com a FK) para `b`, com
    /// uma bolinha na ponta da FK e um traço na da chave.
    fn paint_edge(&self, painter: &egui::Painter, origin: Pos2, a: Rect, b: Rect, c: &Colors) {
        let points = edge_points(a, b);
        let screen: Vec<Pos2> = points
            .iter()
            .map(|p| self.camera.to_screen(origin, *p))
            .collect();
        let zoom = self.camera.zoom;
        let stroke = Stroke::new(1.5, c.line);
        painter.line(screen.clone(), stroke);
        painter.circle_filled(screen[0], (3.5 * zoom).max(1.5), c.line);
        let end = screen[screen.len() - 1];
        let before = screen[screen.len() - 2];
        let dir = (end - before).normalized();
        let normal = vec2(-dir.y, dir.x) * (6.0 * zoom).max(2.0);
        let base = end - dir * (6.0 * zoom).max(2.0);
        painter.line_segment([base - normal, base + normal], stroke);
    }
}

/// Pontos da linha em cotovelo de `a` para `b`, saindo do lado de `a` que
/// fica de frente para `b`.
fn edge_points(a: Rect, b: Rect) -> Vec<Pos2> {
    let side_by_side = b.min.x > a.max.x || b.max.x < a.min.x;
    if side_by_side {
        let (p1, p2) = if b.min.x > a.max.x {
            (a.right_center(), b.left_center())
        } else {
            (a.left_center(), b.right_center())
        };
        let mid = (p1.x + p2.x) / 2.0;
        vec![p1, pos2(mid, p1.y), pos2(mid, p2.y), p2]
    } else {
        let (p1, p2) = if b.min.y > a.max.y {
            (a.center_bottom(), b.center_top())
        } else {
            (a.center_top(), b.center_bottom())
        };
        let mid = (p1.y + p2.y) / 2.0;
        vec![p1, pos2(p1.x, mid), pos2(p2.x, mid), p2]
    }
}

/// Fonte em tamanho inteiro de pixels. Com o zoom mudando a cada frame,
/// tamanhos quebrados fariam o egui rasterizar glifos novos (e mandar a
/// textura de fontes de novo para a GPU) quase todo frame.
fn font(size: f32) -> FontId {
    FontId::proportional(size.round().max(6.0))
}

/// Cores do diagrama, a partir do tema.
struct Colors {
    body: Color32,
    header: Color32,
    header_text: Color32,
    border: Color32,
    selection: Color32,
    text: Color32,
    weak: Color32,
    key: Color32,
    line: Color32,
}

impl From<&egui::Visuals> for Colors {
    fn from(v: &egui::Visuals) -> Self {
        Colors {
            body: v.window_fill,
            header: v.selection.bg_fill.gamma_multiply(0.35),
            header_text: v.strong_text_color(),
            border: v.widgets.noninteractive.fg_stroke.color.gamma_multiply(0.6),
            selection: v.selection.stroke.color,
            text: v.text_color(),
            weak: v.weak_text_color(),
            key: Color32::from_rgb(0xc8, 0x9b, 0x1e),
            line: v.widgets.noninteractive.fg_stroke.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgstudio_schema_model::edit::Document;
    use pgstudio_schema_model::{Column, ForeignKey};

    /// cliente <- pedido <- item, e produto solto.
    fn shop() -> Model {
        let mut d = Document::default();
        let cliente = d.add_table("public", "cliente").unwrap();
        let pedido = d.add_table("public", "pedido").unwrap();
        let item = d.add_table("public", "item").unwrap();
        d.add_table("public", "produto").unwrap();
        d.add_column(
            cliente,
            Column {
                name: "nome".into(),
                ty: "text".into(),
                nullable: true,
                ..Default::default()
            },
        )
        .unwrap();
        for (t, col, target) in [
            (pedido, "cliente_id", "cliente"),
            (item, "pedido_id", "pedido"),
        ] {
            d.add_column(
                t,
                Column {
                    name: col.into(),
                    ty: "integer".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            d.add_foreign_key(
                t,
                ForeignKey {
                    columns: vec![col.into()],
                    ref_schema: "public".into(),
                    ref_table: target.into(),
                    ref_columns: vec!["id".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        }
        d.model
    }

    fn ids(m: &Model) -> Vec<Id> {
        m.tables.iter().map(|t| t.id).collect()
    }

    #[test]
    fn foreign_keys_become_edges_between_the_chosen_tables() {
        let m = shop();
        let d = Diagram::from_model(&m, &ids(&m));
        assert_eq!(d.nodes.len(), 4);
        let names: Vec<(String, String)> = d
            .edges
            .iter()
            .map(|e| (d.nodes[e.from].name.clone(), d.nodes[e.to].name.clone()))
            .collect();
        assert_eq!(
            names,
            [
                ("pedido".to_string(), "cliente".to_string()),
                ("item".to_string(), "pedido".to_string())
            ]
        );
        let pedido = &d.nodes[1];
        assert!(pedido.columns[0].primary_key);
        assert!(pedido.columns[1].foreign_key);
        // Sem a tabela de destino, a FK não vira linha.
        let some: Vec<Id> = ids(&m).into_iter().skip(1).collect();
        assert_eq!(Diagram::from_model(&m, &some).edges.len(), 1);
    }

    #[test]
    fn layout_puts_referenced_tables_to_the_left_without_overlaps() {
        let m = shop();
        let d = Diagram::from_model(&m, &ids(&m));
        let x = |name: &str| d.nodes.iter().find(|n| n.name == name).unwrap().pos.x;
        assert!(x("cliente") < x("pedido"));
        assert!(x("pedido") < x("item"));
        assert_eq!(x("produto"), x("cliente"));
        for (i, a) in d.nodes.iter().enumerate() {
            for b in &d.nodes[i + 1..] {
                assert!(!a.rect().intersects(b.rect()), "{} {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn many_tables_wrap_into_columns_and_cycles_terminate() {
        let mut d = Diagram {
            nodes: (0..500)
                .map(|i| Node {
                    table: i,
                    schema: "public".into(),
                    name: format!("t{i:03}"),
                    columns: vec![
                        NodeColumn {
                            name: "id".into(),
                            ty: "integer".into(),
                            primary_key: true,
                            foreign_key: false,
                            nullable: false,
                        };
                        5
                    ],
                    pos: Pos2::ZERO,
                })
                .collect(),
            edges: vec![
                Edge {
                    name: "a".into(),
                    from: 0,
                    to: 1,
                },
                Edge {
                    name: "b".into(),
                    from: 1,
                    to: 0,
                },
            ],
            ..Default::default()
        };
        d.auto_layout();
        let b = d.bounds();
        // Mais ou menos quadrado, não uma coluna de 500 tabelas.
        assert!(
            b.width() / b.height() > 0.5 && b.width() / b.height() < 3.0,
            "{b:?}"
        );
    }

    #[test]
    fn camera_round_trips_and_zooms_around_the_pointer() {
        let mut c = Camera {
            pan: vec2(30.0, -10.0),
            zoom: 0.5,
        };
        let origin = pos2(100.0, 50.0);
        let p = pos2(400.0, 300.0);
        let w = c.to_world(origin, p);
        assert!((c.to_screen(origin, w) - p).length() < 1e-3);
        c.zoom_at(origin, p, 2.0);
        assert_eq!(c.zoom, 1.0);
        assert!((c.to_screen(origin, w) - p).length() < 1e-3);
        c.zoom_at(origin, p, 1000.0);
        assert_eq!(c.zoom, MAX_ZOOM);
    }

    #[test]
    fn only_what_is_on_screen_is_drawn() {
        let m = shop();
        let mut harness = egui_kittest::Harness::builder()
            .with_size([800.0, 600.0])
            .build_ui_state(
                |ui, d: &mut Diagram| {
                    d.show(ui);
                },
                Diagram::from_model(&m, &ids(&m)),
            );
        harness.run();
        assert_eq!(harness.state().drawn(), (4, 2));
        // Zoom grande no canto de cima: só a primeira tabela aparece.
        let d = harness.state_mut();
        d.camera = Camera {
            pan: vec2(0.0, 400.0),
            zoom: MAX_ZOOM,
        };
        let first = d.nodes.iter().position(|n| n.pos == Pos2::ZERO).unwrap();
        harness.run();
        assert_eq!(harness.state().drawn().0, 1);
        // Clique numa tabela seleciona; no fundo, tira a seleção.
        harness.event(egui::Event::PointerMoved(pos2(30.0, 430.0)));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: pos2(30.0, 430.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        harness.run();
        assert_eq!(harness.state().selected, Some(first));
    }
}
