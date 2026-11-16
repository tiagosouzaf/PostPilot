//! O app se ajusta à largura: em janelas estreitas (ou com zoom grande) os
//! textos encurtam com reticências em vez de serem desenhados uns por cima
//! dos outros.

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_ui_shell::registered::{AiAccess, Rgb, ServerTag};
use pgstudio_ui_shell::{fit, results};

const HOST: &str = "trust-saas-cluster.cluster-cloqzcfusrjr.sa-east-1.rds.amazonaws.com (tiibuser)";

fn production() -> ServerTag {
    ServerTag {
        environment: Some("Produção".into()),
        color: Rgb([0xE5, 0x48, 0x4D]),
        confirm_changes: true,
        ai: AiAccess::default(),
    }
}

/// A faixa de status de uma janela de consulta, numa janela desta largura.
fn strip(width: f32) -> Harness<'static> {
    let tag = production();
    let mut h = Harness::builder()
        .with_size([width, 60.0])
        .build_ui(move |ui| {
            egui::Frame::new().inner_margin(6).show(ui, |ui| {
                results::connected_strip(ui, "tiib", HOST, Some(&tag));
            });
        });
    h.run();
    h
}

/// Os retângulos dos textos da faixa, da esquerda para a direita.
fn rects(h: &Harness<'_>, labels: &[&str]) -> Vec<(String, egui::Rect)> {
    let mut found: Vec<_> = labels
        .iter()
        .filter_map(|l| Some(((*l).to_string(), h.query_by_label(l)?.rect())))
        .collect();
    found.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    found
}

fn assert_side_by_side(h: &Harness<'_>, width: f32, labels: &[&str]) {
    let found = rects(h, labels);
    for (name, rect) in &found {
        assert!(
            rect.left() >= 0.0 && rect.right() <= width + 0.5,
            "{name} leaves the {width} px window: {rect:?}"
        );
    }
    for pair in found.windows(2) {
        assert!(
            pair[0].1.right() <= pair[1].1.left() + 0.5,
            "at {width} px, {} ({:?}) is drawn over {} ({:?})",
            pair[0].0,
            pair[0].1,
            pair[1].0,
            pair[1].1
        );
    }
}

#[test]
fn the_status_strip_never_draws_one_part_over_another() {
    // Da janela larga, em que tudo cabe, à mais estreita de um tiling.
    for width in [1400.0, 900.0, 700.0, 520.0, 380.0, 240.0, 120.0] {
        let h = strip(width);
        assert_side_by_side(&h, width, &["Connected.", "Produção", HOST, "tiib"]);
    }
}

#[test]
fn a_wide_strip_shows_everything_and_a_narrow_one_keeps_the_environment() {
    let h = strip(1400.0);
    let all = rects(&h, &["Connected.", "Produção", HOST, "tiib"]);
    assert_eq!(all.len(), 4, "everything fits in 1400 px");
    let whole = h.get_by_label(HOST).rect().width();

    // Estreita: o servidor encurta, a mensagem e o ambiente continuam.
    let h = strip(520.0);
    let host = h.get_by_label(HOST).rect();
    assert!(
        host.width() < whole * 0.7,
        "the host name ({whole} px) is shortened: {host:?}"
    );
    h.get_by_label("Produção");
    h.get_by_label("Connected.");
}

#[test]
fn ends_gives_the_left_side_only_what_the_right_side_left() {
    let mut h = Harness::builder().with_size([300.0, 40.0]).build_ui(|ui| {
        fit::ends(
            ui,
            |ui| {
                fit::text(ui, "a long message that does not fit in what is left");
            },
            |ui| {
                ui.label("right side");
            },
        );
    });
    h.run();
    let left = h
        .get_by_label("a long message that does not fit in what is left")
        .rect();
    let right = h.get_by_label("right side").rect();
    assert!(left.right() <= right.left(), "{left:?} over {right:?}");
    assert!(right.right() <= 300.0);
}
