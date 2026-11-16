//! Captura da faixa de status da janela de consulta em três larguras, para
//! conferir que o servidor encurta e nada se sobrepõe. Só com
//! `--features screenshots`; o PNG vai para `PGSTUDIO_SHOTS` (ou a pasta
//! temporária).
#![cfg(feature = "screenshots")]

use eframe::egui;
use egui_kittest::Harness;
use pgstudio_ui_shell::registered::{AiAccess, Rgb, ServerTag};
use pgstudio_ui_shell::results;

const HOST: &str = "trust-saas-cluster.cluster-cloqzcfusrjr.sa-east-1.rds.amazonaws.com (tiibuser)";

fn production() -> ServerTag {
    ServerTag {
        environment: Some("Produção".into()),
        color: Rgb([0xE5, 0x48, 0x4D]),
        confirm_changes: true,
        ai: AiAccess::default(),
    }
}

/// A faixa em três larguras, numa imagem só (`--features screenshots`; o
/// PNG vai para `PGSTUDIO_SHOTS` ou a pasta temporária).
#[test]
fn screenshot_of_the_strip_at_three_widths() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let tag = production();
    let mut h = Harness::builder()
        .with_size([1200.0, 150.0])
        .build_ui(move |ui| {
            for width in [940.0, 700.0, 440.0] {
                ui.allocate_ui(egui::vec2(width, 26.0), |ui| {
                    ui.set_width(width);
                    results::strip_frame(ui, Some(&tag)).show(ui, |ui| {
                        results::connected_strip(ui, "tiib", HOST, Some(&tag));
                    });
                });
                ui.add_space(14.0);
            }
        });
    pgstudio_ui_shell::install_fonts(&h.ctx);
    pgstudio_ui_shell::theme::apply(&h.ctx, pgstudio_core::Theme::Dark);
    h.ctx.set_zoom_factor(1.25);
    h.run_steps(4);
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.render()
        .expect("render")
        .save(out.join("faixa-de-status.png"))
        .expect("save png");
}
