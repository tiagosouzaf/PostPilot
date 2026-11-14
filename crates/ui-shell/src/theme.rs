//! Temas claro e escuro e o estilo comum da interface: cantos arredondados,
//! espaçamentos e a moldura dos diálogos.
//!
//! O estilo mora aqui, num ponto só: as telas não escolhem cor, canto nem
//! margem por conta própria. Os diálogos modais usam [`modal`] no lugar de
//! `egui::Modal::new`, para terem a mesma margem interna e os mesmos cantos.

use eframe::egui::{self, Color32, CornerRadius, Margin, Shadow, Stroke, Vec2};
use pgstudio_core::Theme;

/// Raio dos cantos de botões, campos e abas.
pub const WIDGET_RADIUS: u8 = 5;
/// Raio dos cantos de janelas e diálogos.
pub const WINDOW_RADIUS: u8 = 10;
/// Raio dos cantos de menus, listas suspensas e dicas.
pub const MENU_RADIUS: u8 = 7;
/// Margem interna dos diálogos (o conteúdo não encosta na borda).
pub const DIALOG_MARGIN: i8 = 18;

/// Cores usadas fora do `egui::Visuals`, como a status bar.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Cor de destaque (seleção, links, item ativo).
    pub accent: Color32,
    /// Texto sobre a cor de destaque.
    pub on_accent: Color32,
    /// Fundo da status bar.
    pub status_bar: Color32,
    /// Texto da status bar.
    pub on_status_bar: Color32,
    /// Fundo da área de documentos vazia.
    pub document_well: Color32,
}

/// Cores de base de um tema; o resto do `Visuals` sai delas.
struct Base {
    /// Fundo dos painéis.
    panel: Color32,
    /// Fundo de janelas, diálogos e menus.
    window: Color32,
    /// Fundo de campos de texto, editor e grade.
    extreme: Color32,
    /// Linhas alternadas e realces discretos.
    faint: Color32,
    /// Botões: parado, sob o mouse e pressionado.
    widget: [Color32; 3],
    /// Bordas discretas (painéis, separadores, campos).
    border: Color32,
    /// Borda dos campos sob o mouse.
    border_hover: Color32,
    /// Texto.
    text: Color32,
    /// Texto secundário (desabilitado, dicas).
    weak_text: Color32,
    /// Fundo da seleção (texto e item selecionado).
    selection: Color32,
    /// Sombra de janelas e menus.
    shadow: Color32,
}

fn base(theme: Theme) -> Base {
    match theme {
        Theme::Light => Base {
            panel: Color32::from_rgb(0xF3, 0xF4, 0xF6),
            window: Color32::from_rgb(0xFB, 0xFB, 0xFC),
            extreme: Color32::WHITE,
            faint: Color32::from_rgb(0xF4, 0xF5, 0xF8),
            widget: [
                Color32::from_rgb(0xE6, 0xE8, 0xEC),
                Color32::from_rgb(0xDA, 0xDE, 0xE5),
                Color32::from_rgb(0xCB, 0xD1, 0xDA),
            ],
            border: Color32::from_rgb(0xDC, 0xDF, 0xE4),
            border_hover: Color32::from_rgb(0xB4, 0xBA, 0xC4),
            text: Color32::from_rgb(0x1F, 0x23, 0x28),
            weak_text: Color32::from_rgb(0x6B, 0x72, 0x7C),
            selection: Color32::from_rgb(0xC9, 0xDD, 0xF8),
            shadow: Color32::from_black_alpha(40),
        },
        Theme::Dark => Base {
            panel: Color32::from_rgb(0x1C, 0x1F, 0x24),
            window: Color32::from_rgb(0x24, 0x28, 0x2F),
            extreme: Color32::from_rgb(0x15, 0x17, 0x1B),
            faint: Color32::from_rgb(0x21, 0x25, 0x2B),
            widget: [
                Color32::from_rgb(0x2B, 0x30, 0x38),
                Color32::from_rgb(0x35, 0x3B, 0x45),
                Color32::from_rgb(0x40, 0x47, 0x53),
            ],
            border: Color32::from_rgb(0x2E, 0x33, 0x3B),
            border_hover: Color32::from_rgb(0x4A, 0x52, 0x5E),
            text: Color32::from_rgb(0xD8, 0xDC, 0xE2),
            weak_text: Color32::from_rgb(0x8A, 0x92, 0x9E),
            selection: Color32::from_rgb(0x26, 0x4F, 0x8F),
            shadow: Color32::from_black_alpha(110),
        },
    }
}

/// Cores do tema.
pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Light => Palette {
            accent: Color32::from_rgb(0x0B, 0x6B, 0xCB),
            on_accent: Color32::WHITE,
            status_bar: Color32::from_rgb(0x0B, 0x6B, 0xCB),
            on_status_bar: Color32::WHITE,
            document_well: Color32::from_rgb(0xE8, 0xEA, 0xEE),
        },
        Theme::Dark => Palette {
            accent: Color32::from_rgb(0x4C, 0x8D, 0xF6),
            on_accent: Color32::WHITE,
            status_bar: Color32::from_rgb(0x1E, 0x4C, 0x94),
            on_status_bar: Color32::from_rgb(0xE8, 0xEE, 0xF8),
            document_well: Color32::from_rgb(0x13, 0x15, 0x19),
        },
    }
}

/// Monta o `egui::Visuals` do tema.
pub fn visuals(theme: Theme) -> egui::Visuals {
    let p = palette(theme);
    let b = base(theme);
    let mut v = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
    };
    v.panel_fill = b.panel;
    v.window_fill = b.window;
    v.extreme_bg_color = b.extreme;
    v.text_edit_bg_color = Some(b.extreme);
    v.code_bg_color = b.faint;
    v.faint_bg_color = b.faint;
    v.override_text_color = Some(b.text);
    v.weak_text_color = Some(b.weak_text);
    v.window_stroke = Stroke::new(1.0, b.border);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: b.shadow,
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: b.shadow,
    };
    v.selection.bg_fill = b.selection;
    v.selection.stroke = Stroke::new(1.0, b.text);
    v.hyperlink_color = p.accent;
    v.window_corner_radius = CornerRadius::same(WINDOW_RADIUS);
    v.menu_corner_radius = CornerRadius::same(MENU_RADIUS);
    v.window_highlight_topmost = false;

    let w = &mut v.widgets;
    // Fundo de painéis, separadores e bordas dos campos.
    w.noninteractive.bg_fill = b.panel;
    w.noninteractive.weak_bg_fill = b.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, b.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, b.text);
    // Botões e campos parados, sob o mouse, pressionados e abertos.
    w.inactive.weak_bg_fill = b.widget[0];
    w.inactive.bg_fill = b.widget[0];
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, b.text);
    w.hovered.weak_bg_fill = b.widget[1];
    w.hovered.bg_fill = b.widget[1];
    w.hovered.bg_stroke = Stroke::new(1.0, b.border_hover);
    w.hovered.fg_stroke = Stroke::new(1.5, b.text);
    w.hovered.expansion = 0.0;
    w.active.weak_bg_fill = b.widget[2];
    w.active.bg_fill = b.widget[2];
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(2.0, b.text);
    w.active.expansion = 0.0;
    w.open.weak_bg_fill = b.widget[1];
    w.open.bg_fill = b.widget[1];
    w.open.bg_stroke = Stroke::new(1.0, b.border_hover);
    w.open.fg_stroke = Stroke::new(1.0, b.text);
    for w in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        w.corner_radius = CornerRadius::same(WIDGET_RADIUS);
    }
    v
}

/// Espaçamentos: um pouco mais de ar que o padrão do egui, sem perder a
/// densidade de uma ferramenta de banco de dados.
fn spacing(spacing: &mut egui::Spacing) {
    spacing.item_spacing = Vec2::new(8.0, 4.0);
    spacing.button_padding = Vec2::new(8.0, 3.0);
    spacing.interact_size.y = 20.0;
    // Também é a margem do conteúdo das abas encaixadas (egui_dock); os
    // diálogos usam a margem maior de `dialog_frame`.
    spacing.window_margin = Margin::same(8);
    spacing.menu_margin = Margin::same(6);
    spacing.combo_height = 260.0;
}

/// Moldura dos diálogos modais: a mesma da janela, com margem interna
/// generosa e cantos arredondados.
pub fn dialog_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::window(style)
        .inner_margin(Margin::same(DIALOG_MARGIN))
        .corner_radius(CornerRadius::same(WINDOW_RADIUS))
}

/// Um diálogo modal com o estilo do PGStudio. Use no lugar de
/// `egui::Modal::new`.
pub fn modal(id: egui::Id) -> ModalBuilder {
    ModalBuilder(egui::Modal::new(id))
}

/// Diálogo modal que só ganha a moldura do tema na hora de aparecer (a
/// moldura depende do estilo do contexto).
pub struct ModalBuilder(egui::Modal);

impl ModalBuilder {
    /// Mostra o diálogo; o mesmo que [`egui::Modal::show`].
    pub fn show<T>(
        self,
        ctx: &egui::Context,
        content: impl FnOnce(&mut egui::Ui) -> T,
    ) -> egui::ModalResponse<T> {
        let frame = dialog_frame(&ctx.global_style());
        self.0.frame(frame).show(ctx, content)
    }
}

/// Aplica o tema ao contexto do egui.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    let egui_theme = match theme {
        Theme::Light => egui::Theme::Light,
        Theme::Dark => egui::Theme::Dark,
    };
    ctx.set_theme(egui_theme);
    ctx.set_visuals_of(egui_theme, visuals(theme));
    ctx.style_mut_of(egui_theme, |style| spacing(&mut style.spacing));
}

/// O outro tema, para o botão de trocar.
pub fn toggled(theme: Theme) -> Theme {
    match theme {
        Theme::Light => Theme::Dark,
        Theme::Dark => Theme::Light,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogs_have_room_around_the_content() {
        let style = egui::Style::default();
        let frame = dialog_frame(&style);
        assert!(frame.inner_margin.left >= 16);
        assert!(frame.inner_margin.top >= 16);
        assert_eq!(frame.corner_radius, CornerRadius::same(WINDOW_RADIUS));
    }

    #[test]
    fn both_themes_round_the_widgets() {
        for theme in Theme::ALL {
            let v = visuals(theme);
            assert_eq!(v.widgets.inactive.corner_radius.nw, WIDGET_RADIUS);
            assert_eq!(v.window_corner_radius.nw, WINDOW_RADIUS);
            assert_eq!(v.menu_corner_radius.nw, MENU_RADIUS);
        }
    }

    #[test]
    fn dark_text_contrasts_with_the_panels() {
        let b = base(Theme::Dark);
        // Luminância aproximada: o texto tem de ficar bem acima do fundo.
        let lum = |c: Color32| (u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b())) / 3;
        assert!(lum(b.text) > lum(b.panel) + 150);
        assert!(lum(b.text) > lum(b.extreme) + 150);
    }

    #[test]
    fn toggle_switches_between_the_two_themes() {
        assert_eq!(toggled(Theme::Light), Theme::Dark);
        assert_eq!(toggled(Theme::Dark), Theme::Light);
    }
}
