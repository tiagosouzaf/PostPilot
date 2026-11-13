//! Temas claro e escuro, com as cores do SSMS (que herda as do Visual Studio).

use eframe::egui::{self, Color32, CornerRadius, Stroke};
use pgstudio_core::Theme;

/// Cores usadas fora do `egui::Visuals`, como a status bar.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Cor de destaque (status bar, seleção).
    pub accent: Color32,
    /// Texto sobre a cor de destaque.
    pub on_accent: Color32,
    /// Fundo da área de documentos vazia.
    pub document_well: Color32,
}

/// Cores do tema.
pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Light => Palette {
            accent: Color32::from_rgb(0x00, 0x7A, 0xCC),
            on_accent: Color32::WHITE,
            document_well: Color32::from_rgb(0xEE, 0xEE, 0xF2),
        },
        Theme::Dark => Palette {
            accent: Color32::from_rgb(0x00, 0x7A, 0xCC),
            on_accent: Color32::WHITE,
            document_well: Color32::from_rgb(0x1E, 0x1E, 0x1E),
        },
    }
}

/// Monta o `egui::Visuals` do tema.
pub fn visuals(theme: Theme) -> egui::Visuals {
    let p = palette(theme);
    let mut v = match theme {
        Theme::Light => {
            let mut v = egui::Visuals::light();
            v.panel_fill = Color32::from_rgb(0xF5, 0xF5, 0xF5);
            v.window_fill = Color32::from_rgb(0xF5, 0xF5, 0xF5);
            v.extreme_bg_color = Color32::WHITE;
            v.faint_bg_color = Color32::from_rgb(0xF0, 0xF0, 0xF0);
            v.override_text_color = Some(Color32::from_rgb(0x1E, 0x1E, 0x1E));
            v
        }
        Theme::Dark => {
            let mut v = egui::Visuals::dark();
            v.panel_fill = Color32::from_rgb(0x2D, 0x2D, 0x30);
            v.window_fill = Color32::from_rgb(0x25, 0x25, 0x26);
            v.extreme_bg_color = Color32::from_rgb(0x1E, 0x1E, 0x1E);
            v.faint_bg_color = Color32::from_rgb(0x33, 0x33, 0x37);
            v.override_text_color = Some(Color32::from_rgb(0xF1, 0xF1, 0xF1));
            v
        }
    };
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.accent;
    // Cantos retos, como nas ferramentas da Microsoft.
    v.window_corner_radius = CornerRadius::same(2);
    v.menu_corner_radius = CornerRadius::same(0);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(2);
    }
    v
}

/// Aplica o tema ao contexto do egui.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let egui_theme = match theme {
        Theme::Light => egui::Theme::Light,
        Theme::Dark => egui::Theme::Dark,
    };
    ctx.set_theme(egui_theme);
    ctx.set_visuals_of(egui_theme, visuals(theme));
}
