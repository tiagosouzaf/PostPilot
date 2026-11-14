//! Desenho do plano como no SSMS: a raiz à esquerda, os filhos à direita,
//! um ícone por operação com o nome, a tabela e o custo embaixo, e setas
//! dos filhos para o pai, mais grossas quanto mais linhas passam.
//!
//! O layout é calculado uma vez por plano; a cada frame só os nós e setas
//! que caem na área visível são desenhados.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2,
    vec2,
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
    /// Nós desenhados no último frame.
    drawn: usize,
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
        PlanView {
            positions,
            size,
            zoom: 1.0,
            selected: None,
            drawn: 0,
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
        egui::ScrollArea::both()
            .id_salt(id_salt)
            .auto_shrink(false)
            .show(ui, |ui| self.paint(ui, plan));
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

        // Setas primeiro, para os nós ficarem por cima.
        for (id, node) in plan.nodes.iter().enumerate() {
            let Some(parent) = node.parent else { continue };
            let child = to_screen(self.node_rect(id));
            let to = to_screen(self.node_rect(parent));
            let from = pos2(
                child.left(),
                child.top() + ICON_SIZE * 0.5 * zoom + 6.0 * zoom,
            );
            let end = pos2(to.right(), to.top() + ICON_SIZE * 0.5 * zoom + 6.0 * zoom);
            let bounds = Rect::from_two_pos(from, end).expand(8.0);
            if !bounds.intersects(clip) {
                continue;
            }
            let width = arrow_width(node) * zoom.max(0.5);
            let mid = (from.x + end.x) / 2.0;
            let stroke = Stroke::new(width, line);
            painter.line(vec![from, pos2(mid, from.y), pos2(mid, end.y), end], stroke);
            let head = 5.0 * zoom + width;
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
        for id in 0..plan.nodes.len() {
            let r = to_screen(self.node_rect(id));
            if !r.intersects(clip) {
                continue;
            }
            drawn += 1;
            let response = ui.interact(r, ui.id().with(("plan_node", id)), Sense::click());
            // Para leitores de tela (e testes): o que o nó diz.
            let node = &plan.nodes[id];
            let label = format!(
                "{}{}, Cost: {:.0} %{}",
                node.title(),
                node.subtitle()
                    .map(|s| format!(", {s}"))
                    .unwrap_or_default(),
                plan.cost_share(id) * 100.0,
                if plan.misestimate(id).is_some() {
                    ", Warning: estimated rows differ from actual"
                } else {
                    ""
                }
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone())
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
            paint_node(painter, r, zoom, plan, id, &visuals);
            if response.clicked() {
                clicked = Some(id);
            }
            response.on_hover_ui(|ui| tooltip(ui, plan, id));
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

fn paint_node(
    painter: &egui::Painter,
    r: Rect,
    zoom: f32,
    plan: &Plan,
    id: usize,
    v: &egui::Visuals,
) {
    let node = &plan.nodes[id];
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
    painter.text(
        icon_rect.center(),
        Align2::CENTER_CENTER,
        operator_icon(&node.kind),
        FontId::proportional((22.0 * zoom).round().max(6.0)),
        text,
    );
    if plan.misestimate(id).is_some() {
        painter.text(
            icon_rect.right_top() + vec2(2.0 * zoom, -2.0 * zoom),
            Align2::CENTER_CENTER,
            icon::WARNING,
            FontId::proportional((14.0 * zoom).round().max(6.0)),
            v.warn_fg_color,
        );
    }
    if zoom < 0.45 {
        return;
    }
    let size = (11.0 * zoom).round().max(6.0);
    let width_chars = ((NODE_WIDTH - 8.0) / 6.0) as usize;
    let mut lines: Vec<(String, Color32)> = vec![(node.title(), text)];
    if let Some(s) = node.subtitle() {
        lines.push((s, weak));
    }
    lines.push((format!("Cost: {:.0} %", plan.cost_share(id) * 100.0), text));
    if let Some(a) = node.actual_total_rows() {
        let est = node.plan_rows * node.actual.map_or(1.0, |a| a.loops);
        let pct = if est > 0.0 { a / est * 100.0 } else { 0.0 };
        lines.push((format!("{} of {} ({pct:.0}%)", count(a), count(est)), weak));
    }
    let mut y = icon_rect.bottom() + 4.0 * zoom;
    for (line, color) in lines {
        painter.text(
            pos2(r.center().x, y),
            Align2::CENTER_TOP,
            truncate(&line, width_chars),
            FontId::proportional(size),
            color,
        );
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
                "{} Estimated rows differ from the actual rows by {r:.0}x. The statistics may be out of date; try ANALYZE on the table.",
                icon::WARNING
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
                "Estimated Operator Cost",
                format!(
                    "{:.2} ({:.0}%)",
                    plan.own_cost(id),
                    plan.cost_share(id) * 100.0
                ),
            );
            row("Estimated Subtree Cost", format!("{:.2}", node.total_cost));
            row("Estimated Number of Rows", count(node.plan_rows));
            if let Some(a) = node.actual {
                row("Actual Number of Rows", count(a.rows * a.loops));
                row("Actual Executions", count(a.loops));
                row("Actual Time (ms)", format!("{:.3}", a.total_ms * a.loops));
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
}
