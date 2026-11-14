//! Desenho do plano como no SSMS: a raiz à esquerda, os filhos à direita,
//! um ícone por operação com o nome, a tabela e o custo embaixo, e setas
//! dos filhos para o pai, mais grossas quanto mais linhas passam.
//!
//! O layout, os textos de cada nó, as frações de custo e a espessura das
//! setas são calculados uma vez por plano. A cada frame só os nós e setas
//! que caem na área visível são desenhados, e os textos já diagramados
//! (galleys) de um nó são reaproveitados enquanto o tamanho da fonte não
//! muda.

use pgstudio_i18n::{tr, trf};
use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Galley, Pos2, Rect, Sense, Stroke, StrokeKind,
    Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;

use crate::plan::{Node, Plan};

pub const NODE_WIDTH: f32 = 160.0;
pub const NODE_HEIGHT: f32 = 104.0;
const GAP_X: f32 = 56.0;
const GAP_Y: f32 = 20.0;
const ICON_SIZE: f32 = 36.0;
const MARGIN: f32 = 12.0;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 2.0;

/// Onde cada nó fica e o zoom do desenho de um plano.
#[derive(Clone, Debug)]
pub struct PlanView {
    /// Canto de cima à esquerda de cada nó, sem zoom.
    pub positions: Vec<Pos2>,
    /// Tamanho do desenho inteiro, sem zoom.
    pub size: Vec2,
    pub zoom: f32,
    pub selected: Option<usize>,
    /// Rolagem a aplicar no próximo frame (a medição passeia pelo plano).
    pub scroll_to: Option<Vec2>,
    /// Nós desenhados no último frame.
    drawn: usize,
    /// Nós em ordem de altura, para achar os visíveis sem passar por todos.
    by_y: Vec<usize>,
    /// Textos de cada nó, montados uma vez.
    texts: Vec<NodeText>,
    /// Espessura da seta que sai de cada nó.
    widths: Vec<f32>,
    /// Galleys dos nós desenhados no último frame.
    galleys: HashMap<usize, NodeGalleys>,
}

/// O que um nó escreve, montado uma vez por plano.
#[derive(Clone, Debug)]
struct NodeText {
    /// O que o nó diz a leitores de tela.
    label: String,
    icon: &'static str,
    warning: bool,
    /// Linhas embaixo do ícone; `true` nas que vão em cor fraca.
    lines: Vec<(String, bool)>,
}

/// Tamanhos de fonte (pixels inteiros) de um zoom: o do ícone, o do
/// aviso, o das linhas e se as linhas aparecem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sizes {
    icon: u16,
    warning: u16,
    text: u16,
    lines: bool,
}

impl Sizes {
    fn at(zoom: f32) -> Self {
        let px = |s: f32| (s * zoom).round().max(6.0) as u16;
        Sizes {
            icon: px(22.0),
            warning: px(14.0),
            text: px(11.0),
            // Abaixo de 45% as linhas teriam menos de 5 px.
            lines: zoom >= 0.45,
        }
    }
}

/// Textos de um nó já diagramados num tamanho de fonte. A cor fica de
/// fora (`Color32::PLACEHOLDER`) e entra na hora de desenhar, então trocar
/// o tema não invalida nada.
#[derive(Clone, Debug)]
struct NodeGalleys {
    /// Para que zoom os textos servem.
    sizes: Sizes,
    icon: Arc<Galley>,
    warning: Option<Arc<Galley>>,
    lines: Vec<Arc<Galley>>,
    /// Quais linhas vão em cor fraca.
    weak: Vec<bool>,
}

impl PlanView {
    /// Calcula o layout: a profundidade dá a coluna; cada folha ganha uma
    /// linha nova e o pai fica na linha do primeiro filho, como no SSMS.
    pub fn new(plan: &Plan) -> Self {
        let n = plan.nodes.len();
        let depths = plan.depths();
        let mut rows = vec![0usize; n];
        let mut next_row = 0;
        // Pós-ordem sem recursão (planos podem ser fundos).
        let mut stack = vec![(0usize, false)];
        while let Some((id, visited)) = stack.pop() {
            let children = &plan.nodes[id].children;
            if children.is_empty() {
                rows[id] = next_row;
                next_row += 1;
            } else if visited {
                rows[id] = rows[children[0]];
            } else {
                stack.push((id, true));
                for &c in children.iter().rev() {
                    stack.push((c, false));
                }
            }
        }
        let positions: Vec<Pos2> = (0..n)
            .map(|i| {
                pos2(
                    MARGIN + depths[i] as f32 * (NODE_WIDTH + GAP_X),
                    MARGIN + rows[i] as f32 * (NODE_HEIGHT + GAP_Y),
                )
            })
            .collect();
        let columns = depths.iter().max().map_or(1, |d| d + 1);
        let size = vec2(
            2.0 * MARGIN + columns as f32 * NODE_WIDTH + (columns - 1) as f32 * GAP_X,
            2.0 * MARGIN + next_row.max(1) as f32 * (NODE_HEIGHT + GAP_Y) - GAP_Y,
        );
        let mut by_y: Vec<usize> = (0..n).collect();
        by_y.sort_by(|&a, &b| positions[a].y.total_cmp(&positions[b].y));
        let shares = cost_shares(plan);
        let texts = (0..n).map(|id| node_text(plan, id, shares[id])).collect();
        let widths = plan.nodes.iter().map(arrow_width).collect();
        PlanView {
            positions,
            size,
            zoom: 1.0,
            selected: None,
            scroll_to: None,
            drawn: 0,
            by_y,
            texts,
            widths,
            galleys: HashMap::new(),
        }
    }

    /// Nós desenhados no último frame (para os testes e a medição).
    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// Retângulo do nó `id`, sem zoom, a partir da origem do desenho.
    pub fn node_rect(&self, id: usize) -> Rect {
        Rect::from_min_size(self.positions[id], vec2(NODE_WIDTH, NODE_HEIGHT))
    }

    /// Desenha o plano no espaço disponível, com rolagem nos dois sentidos.
    /// Ctrl + roda do mouse muda o zoom.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        plan: &Plan,
        id_salt: impl std::hash::Hash + std::fmt::Debug,
    ) {
        if ui.rect_contains_pointer(ui.max_rect()) {
            // Ctrl + roda vira zoom_delta no egui.
            let factor = ui.input(|i| i.zoom_delta());
            if factor != 1.0 {
                self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            }
        }
        let mut area = egui::ScrollArea::both().id_salt(id_salt).auto_shrink(false);
        if let Some(offset) = self.scroll_to.take() {
            area = area.scroll_offset(offset);
        }
        area.show(ui, |ui| self.paint(ui, plan));
    }

    fn paint(&mut self, ui: &mut egui::Ui, plan: &Plan) {
        let zoom = self.zoom;
        let (rect, _) = ui.allocate_exact_size(self.size * zoom, Sense::hover());
        let origin = rect.min;
        let clip = ui.clip_rect();
        let to_screen =
            |r: Rect| Rect::from_min_size(origin + r.min.to_vec2() * zoom, r.size() * zoom);
        let painter = ui.painter();
        let visuals = ui.visuals().clone();
        let line = visuals.weak_text_color();

        // Setas primeiro, para os nós ficarem por cima. Os filhos de um nó
        // ficam na mesma coluna, então as setas dividem o tronco vertical
        // e a ponta: cada filho desenha só o seu trecho horizontal, e o
        // tronco e a ponta saem uma vez por pai, cortados ao visível (uma
        // `Append` de mil partições não desenha mil troncos sobrepostos).
        let arrow_y = |r: Rect| r.top() + ICON_SIZE * 0.5 * zoom + 6.0 * zoom;
        for (parent, node) in plan.nodes.iter().enumerate() {
            let Some(&last) = node.children.last() else {
                continue;
            };
            let to = to_screen(self.node_rect(parent));
            let end = pos2(to.right(), arrow_y(to));
            let first = to_screen(self.node_rect(node.children[0]));
            let bottom = arrow_y(to_screen(self.node_rect(last)));
            let mid = (first.left() + end.x) / 2.0;
            let trunk = Rect::from_min_max(
                pos2(end.x, end.y.min(bottom)),
                pos2(first.left(), bottom.max(end.y)),
            )
            .expand(8.0);
            if !trunk.intersects(clip) {
                continue;
            }
            // Os filhos ficam em linhas crescentes: só os que caem na
            // altura visível são percorridos.
            let widest = node
                .children
                .iter()
                .map(|&c| self.widths[c])
                .fold(0.0f32, f32::max)
                * zoom.max(0.5);
            let child_y = |c: usize| arrow_y(to_screen(self.node_rect(c)));
            let start = node
                .children
                .partition_point(|&c| child_y(c) + 8.0 < clip.top());
            for &child in &node.children[start..] {
                let c = to_screen(self.node_rect(child));
                let y = arrow_y(c);
                if y - 8.0 > clip.bottom() {
                    break;
                }
                let width = self.widths[child] * zoom.max(0.5);
                painter.line_segment([pos2(c.left(), y), pos2(mid, y)], Stroke::new(width, line));
            }
            let top = end.y.max(clip.top() - 8.0);
            let low = bottom.min(clip.bottom() + 8.0);
            if low > top {
                painter.line_segment([pos2(mid, top), pos2(mid, low)], Stroke::new(widest, line));
            }
            painter.line_segment([pos2(mid, end.y), end], Stroke::new(widest, line));
            let head = 5.0 * zoom + widest;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    end,
                    end + vec2(head, -head * 0.6),
                    end + vec2(head, head * 0.6),
                ],
                line,
                Stroke::NONE,
            ));
        }

        let mut drawn = 0;
        let mut clicked = None;
        // Nós cuja altura cruza a área visível (o resto nem é olhado).
        let top = (clip.top() - origin.y) / zoom - NODE_HEIGHT;
        let bottom = (clip.bottom() - origin.y) / zoom;
        let first = self.by_y.partition_point(|&i| self.positions[i].y < top);
        let last = self
            .by_y
            .partition_point(|&i| self.positions[i].y <= bottom);
        let sizes = Sizes::at(zoom);
        let mut visible = Vec::with_capacity(last.saturating_sub(first));
        let (by_y, texts, cache) = (&self.by_y, &self.texts, &mut self.galleys);
        let positions = &self.positions;
        for &id in &by_y[first..last.max(first)] {
            let r = to_screen(Rect::from_min_size(
                positions[id],
                vec2(NODE_WIDTH, NODE_HEIGHT),
            ));
            if !r.intersects(clip) {
                continue;
            }
            drawn += 1;
            visible.push(id);
            let response = ui.interact(r, ui.id().with(("plan_node", id)), Sense::click());
            let text = &texts[id];
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text.label.clone())
            });
            let selected = self.selected == Some(id);
            if selected || response.hovered() {
                let fill = if selected {
                    visuals.selection.bg_fill
                } else {
                    visuals.widgets.hovered.weak_bg_fill
                };
                painter.rect_filled(r, CornerRadius::same(4), fill);
            }
            let galleys = match cache.get(&id) {
                Some(g) if g.sizes == sizes => g,
                _ => {
                    let g = layout_node(painter, text, sizes);
                    cache.insert(id, g);
                    &cache[&id]
                }
            };
            paint_node(painter, r, zoom, galleys, &visuals);
            if response.clicked() {
                clicked = Some(id);
            }
            response.on_hover_ui(|ui| tooltip(ui, plan, id));
        }
        // Só fica guardado o que está na tela.
        if self.galleys.len() > visible.len() {
            visible.sort_unstable();
            self.galleys
                .retain(|id, _| visible.binary_search(id).is_ok());
        }
        if let Some(id) = clicked {
            self.selected = Some(id);
        }
        self.drawn = drawn;
    }
}

/// Espessura da seta: cresce com o logaritmo das linhas que passam.
fn arrow_width(node: &Node) -> f32 {
    let rows = node.actual_total_rows().unwrap_or(node.plan_rows).max(0.0);
    (1.0 + (rows + 1.0).log10() as f32).clamp(1.0, 8.0)
}

/// Fração do custo de cada nó, numa passada só (`Plan::cost_share` soma
/// o plano inteiro a cada chamada).
fn cost_shares(plan: &Plan) -> Vec<f64> {
    let own: Vec<f64> = (0..plan.nodes.len()).map(|i| plan.own_cost(i)).collect();
    let sum: f64 = own.iter().sum();
    own.iter()
        .enumerate()
        .map(|(i, c)| {
            if sum > 0.0 {
                c / sum
            } else if i == 0 {
                1.0
            } else {
                0.0
            }
        })
        .collect()
}

/// Os textos do nó `id`, montados uma vez por plano.
fn node_text(plan: &Plan, id: usize, share: f64) -> NodeText {
    let node = &plan.nodes[id];
    let warning = plan.misestimate(id).is_some();
    let title = node.title();
    let subtitle = node.subtitle();
    let label = format!(
        "{title}{}, {}{}",
        subtitle
            .as_deref()
            .map(|s| format!(", {s}"))
            .unwrap_or_default(),
        trf!("Cost: {cost} %", cost = format!("{:.0}", share * 100.0)),
        if warning {
            format!(", {}", tr("Warning: estimated rows differ from actual"))
        } else {
            String::new()
        }
    );
    let width_chars = ((NODE_WIDTH - 8.0) / 6.0) as usize;
    let mut lines = vec![(truncate(&title, width_chars), false)];
    if let Some(s) = subtitle {
        lines.push((truncate(&s, width_chars), true));
    }
    lines.push((
        trf!("Cost: {cost} %", cost = format!("{:.0}", share * 100.0)),
        false,
    ));
    if let Some(a) = node.actual_total_rows() {
        let est = node.plan_rows * node.actual.map_or(1.0, |a| a.loops);
        let pct = if est > 0.0 { a / est * 100.0 } else { 0.0 };
        let rows = trf!(
            "{actual} of {estimated} ({percent}%)",
            actual = count(a),
            estimated = count(est),
            percent = format!("{pct:.0}")
        );
        lines.push((truncate(&rows, width_chars), true));
    }
    NodeText {
        label,
        icon: operator_icon(&node.kind),
        warning,
        lines,
    }
}

/// Diagrama os textos de um nó nos tamanhos de um zoom. Com o zoom
/// afastado as linhas embaixo do ícone não aparecem e nem são
/// diagramadas.
fn layout_node(painter: &egui::Painter, text: &NodeText, sizes: Sizes) -> NodeGalleys {
    let galley = |s: &str, size: u16| {
        let font = FontId::proportional(f32::from(size));
        painter.layout_no_wrap(s.to_owned(), font, Color32::PLACEHOLDER)
    };
    NodeGalleys {
        sizes,
        icon: galley(text.icon, sizes.icon),
        warning: text.warning.then(|| galley(icon::WARNING, sizes.warning)),
        lines: if sizes.lines {
            text.lines
                .iter()
                .map(|(l, _)| galley(l, sizes.text))
                .collect()
        } else {
            Vec::new()
        },
        weak: text.lines.iter().map(|&(_, w)| w).collect(),
    }
}

fn paint_node(painter: &egui::Painter, r: Rect, zoom: f32, g: &NodeGalleys, v: &egui::Visuals) {
    let text = v.text_color();
    let weak = v.weak_text_color();
    let icon_rect = Rect::from_center_size(
        pos2(r.center().x, r.top() + 6.0 * zoom + ICON_SIZE * zoom / 2.0),
        Vec2::splat(ICON_SIZE * zoom),
    );
    painter.rect(
        icon_rect,
        CornerRadius::same(4),
        v.extreme_bg_color,
        Stroke::new(1.0, weak),
        StrokeKind::Inside,
    );
    let place = |galley: &Arc<Galley>, anchor: Align2, pos: Pos2, color: Color32| {
        let rect = anchor.anchor_size(pos, galley.size());
        painter.galley(rect.min, galley.clone(), color);
    };
    place(&g.icon, Align2::CENTER_CENTER, icon_rect.center(), text);
    if let Some(w) = &g.warning {
        let pos = icon_rect.right_top() + vec2(2.0 * zoom, -2.0 * zoom);
        place(w, Align2::CENTER_CENTER, pos, v.warn_fg_color);
    }
    // As linhas fracas são as que `node_text` marcou; a ordem é a mesma.
    let size = f32::from(g.sizes.text);
    let mut y = icon_rect.bottom() + 4.0 * zoom;
    for (i, galley) in g.lines.iter().enumerate() {
        let color = if g.weak.get(i).copied().unwrap_or(false) {
            weak
        } else {
            text
        };
        place(galley, Align2::CENTER_TOP, pos2(r.center().x, y), color);
        y += (size + 2.0).max(8.0);
    }
}

/// Lista de propriedades do nó, como o tooltip do SSMS.
fn tooltip(ui: &mut egui::Ui, plan: &Plan, id: usize) {
    let node = &plan.nodes[id];
    ui.set_max_width(420.0);
    ui.strong(node.title());
    if let Some(s) = node.subtitle() {
        ui.label(s);
    }
    if let Some(r) = plan.misestimate(id) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "{} {}",
                icon::WARNING,
                trf!(
                    "Estimated rows differ from the actual rows by {ratio}x. The statistics may be out of date; try ANALYZE on the table.",
                    ratio = format!("{r:.0}")
                )
            ),
        );
    }
    ui.separator();
    egui::Grid::new(("plan_tooltip", id))
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            };
            row(
                tr("Estimated Operator Cost"),
                format!(
                    "{:.2} ({:.0}%)",
                    plan.own_cost(id),
                    plan.cost_share(id) * 100.0
                ),
            );
            row(
                tr("Estimated Subtree Cost"),
                format!("{:.2}", node.total_cost),
            );
            row(tr("Estimated Number of Rows"), count(node.plan_rows));
            if let Some(a) = node.actual {
                row(tr("Actual Number of Rows"), count(a.rows * a.loops));
                row(tr("Actual Executions"), count(a.loops));
                row(
                    tr("Actual Time (ms)"),
                    format!("{:.3}", a.total_ms * a.loops),
                );
            }
            for (k, v) in &node.properties {
                row(k, truncate(v, 200));
            }
        });
}

/// Ícone por tipo de operação.
pub fn operator_icon(kind: &str) -> &'static str {
    match kind {
        "Seq Scan" | "Parallel Seq Scan" | "Sample Scan" => icon::TABLE,
        "Index Scan" | "Index Only Scan" => icon::MAGNIFYING_GLASS,
        "Bitmap Index Scan" | "Bitmap Heap Scan" | "BitmapAnd" | "BitmapOr" => {
            icon::LIST_MAGNIFYING_GLASS
        }
        "Hash" => icon::HASH,
        "Hash Join" | "Merge Join" | "Nested Loop" => icon::INTERSECT,
        "Sort" | "Incremental Sort" => icon::SORT_ASCENDING,
        "Aggregate" | "GroupAggregate" | "HashAggregate" | "WindowAgg" | "Group" => icon::SIGMA,
        "Limit" => icon::SCISSORS,
        "Materialize" | "Memoize" => icon::STACK,
        "Append" | "Merge Append" | "Gather" | "Gather Merge" => icon::ARROWS_MERGE,
        "Unique" | "SetOp" => icon::FUNNEL,
        "ModifyTable" => icon::PENCIL_SIMPLE,
        "Function Scan" | "Table Function Scan" | "ProjectSet" => icon::BRACKETS_CURLY,
        "CTE Scan" | "WorkTable Scan" | "Subquery Scan" | "Recursive Union" => icon::TREE_STRUCTURE,
        "Values Scan" | "Result" => icon::ROWS,
        "LockRows" => icon::KEY,
        "Foreign Scan" | "Custom Scan" => icon::LINK,
        _ => icon::CUBE,
    }
}

/// Número de linhas sem casas decimais quando inteiro.
fn count(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n:.1}")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse;

    fn chain(depth: usize, leaves: usize) -> String {
        // Uma raiz com `leaves` filhos, cada um uma corrente de `depth` nós.
        let mut leaf =
            r#"{"Node Type": "Seq Scan", "Total Cost": 1.0, "Plan Rows": 10}"#.to_string();
        for _ in 1..depth {
            leaf = format!(r#"{{"Node Type": "Sort", "Total Cost": 2.0, "Plans": [{leaf}]}}"#);
        }
        let children = vec![leaf; leaves].join(",");
        format!(
            r#"[{{"Plan": {{"Node Type": "Append", "Total Cost": 5.0, "Plans": [{children}]}}}}]"#
        )
    }

    #[test]
    fn parent_sits_on_the_row_of_its_first_child() {
        let plan = &parse(&chain(3, 2), "").unwrap()[0];
        let view = PlanView::new(plan);
        // Raiz (0), primeiro ramo 1..=3, segundo 4..=6.
        assert_eq!(view.positions[0].y, view.positions[1].y);
        assert_eq!(view.positions[1].y, view.positions[3].y);
        assert!(view.positions[4].y > view.positions[3].y);
        assert!(view.positions[1].x > view.positions[0].x);
        assert_eq!(view.positions[3].x, view.positions[6].x);
        let right = view.positions[3].x + NODE_WIDTH + MARGIN;
        assert_eq!(view.size.x, right);
    }

    #[test]
    fn deep_plans_do_not_overflow_the_stack() {
        // O serde_json para em 128 níveis de JSON (dois por nó); o layout
        // não usa recursão, então aguenta o que o parser aceitar.
        let plan = &parse(&chain(60, 1), "").unwrap()[0];
        let view = PlanView::new(plan);
        assert_eq!(view.positions.len(), 61);
    }

    #[test]
    fn arrows_grow_with_rows() {
        let thin = &parse(r#"[{"Plan": {"Node Type": "Result", "Plan Rows": 1}}]"#, "").unwrap()[0];
        let thick = &parse(
            r#"[{"Plan": {"Node Type": "Result", "Plan Rows": 1000000}}]"#,
            "",
        )
        .unwrap()[0];
        assert!(arrow_width(thin.root()) < arrow_width(thick.root()));
        assert!((arrow_width(thick.root()) - 7.0).abs() < 0.01);
        assert_eq!(count(3.0), "3");
        assert_eq!(count(2.5), "2.5");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[test]
    fn only_visible_nodes_are_drawn() {
        let plan = parse(&chain(5, 200), "").unwrap().remove(0);
        assert_eq!(plan.nodes.len(), 1001);
        let mut view = PlanView::new(&plan);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(800.0, 600.0))
            .build_ui_state(
                |ui, view: &mut PlanView| view.show(ui, &plan, "plan"),
                view.clone(),
            );
        harness.run();
        let drawn = harness.state().drawn();
        assert!(drawn > 0 && drawn < 60, "{drawn}");
        view.zoom = 0.25;
        *harness.state_mut() = view;
        harness.run();
        assert!(harness.state().drawn() > drawn);
    }

    #[test]
    fn text_is_laid_out_once_and_only_for_what_is_on_screen() {
        let plan = parse(&chain(5, 200), "").unwrap().remove(0);
        let view = PlanView::new(&plan);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(800.0, 600.0))
            .build_ui_state(|ui, view: &mut PlanView| view.show(ui, &plan, "plan"), view);
        harness.run();
        let drawn = harness.state().drawn();
        assert_eq!(harness.state().galleys.len(), drawn);
        let first = harness.state().galleys[&0].clone();
        assert_eq!(first.lines.len(), 2, "título e custo");
        harness.run();
        // Mesmo zoom: o mesmo galley, sem diagramar de novo.
        assert!(Arc::ptr_eq(&first.icon, &harness.state().galleys[&0].icon));
        // Afastado as linhas somem; de volta, voltam.
        harness.state_mut().zoom = 0.4;
        harness.run();
        assert!(harness.state().galleys[&0].lines.is_empty());
        assert!(harness.state().galleys.len() <= harness.state().drawn());
        harness.state_mut().zoom = 1.0;
        harness.run();
        assert_eq!(harness.state().galleys[&0].lines.len(), 2);
    }
}
