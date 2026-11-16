//! Temas claro e escuro e o estilo comum da interface: cantos arredondados,
//! espaçamentos e a moldura dos diálogos.
//!
//! O estilo mora aqui, num ponto só: as telas não escolhem cor, canto nem
//! margem por conta própria. Os diálogos modais usam [`modal`] no lugar de
//! `egui::Modal::new`, para terem a mesma margem interna e os mesmos cantos.

use std::sync::RwLock;

use eframe::egui::{self, Color32, CornerRadius, Margin, Shadow, Stroke, Vec2};
use pgstudio_core::Theme;
use pgstudio_sql_editor::Colors as EditorColors;

/// Cores vindas do tema do sistema (hoje, do tema do Omarchy; veja
/// [`crate::system_theme`]). Só o fundo e o texto são obrigatórios: o resto
/// sai deles quando o tema não traz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    /// Fundo do tema.
    pub background: Color32,
    /// Texto do tema.
    pub foreground: Color32,
    /// Cor de destaque.
    pub accent: Option<Color32>,
    /// As 16 cores do terminal (`color0` a `color15`), na ordem ANSI.
    pub ansi: [Option<Color32>; 16],
}

impl SystemColors {
    /// Se o tema é escuro (fundo mais escuro que o texto).
    pub fn theme(&self) -> Theme {
        if luminance(self.background) < luminance(self.foreground) {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    fn accent(&self) -> Color32 {
        self.accent
            .or(self.ansi[4])
            .unwrap_or_else(|| palette_of(self.theme()).accent)
    }
}

/// Cores do sistema em uso, se houver. Com elas, [`palette`], [`visuals`] e
/// [`editor_colors`] deixam de usar as paletas próprias do app.
static SYSTEM_COLORS: RwLock<Option<SystemColors>> = RwLock::new(None);

/// Passa a usar (ou deixa de usar, com `None`) as cores do sistema. Chame
/// [`apply`] em seguida.
pub fn set_system_colors(colors: Option<SystemColors>) {
    if let Ok(mut current) = SYSTEM_COLORS.write() {
        *current = colors;
    }
}

/// Cores do sistema em uso.
pub fn system_colors() -> Option<SystemColors> {
    SYSTEM_COLORS.read().ok().and_then(|c| *c)
}

/// Luminância aproximada, de 0 a 255.
fn luminance(c: Color32) -> u32 {
    (u32::from(c.r()) * 299 + u32::from(c.g()) * 587 + u32::from(c.b()) * 114) / 1000
}

/// Mistura `a` com `b`: `t = 0` dá `a`, `t = 1` dá `b`.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

/// Preto ou branco, o que contrastar mais com `c`.
fn contrast(c: Color32) -> Color32 {
    if luminance(c) > 150 {
        Color32::from_rgb(0x12, 0x14, 0x18)
    } else {
        Color32::WHITE
    }
}

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
    /// Bordas discretas (painéis, separadores).
    border: Color32,
    /// Borda dos campos e botões parados: tem de aparecer tanto sobre o
    /// fundo do campo quanto sobre o do painel ou do diálogo em volta.
    field_border: Color32,
    /// Borda dos campos e botões sob o mouse.
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

/// Deriva as cores de base a partir do fundo, do texto e do destaque do
/// tema do sistema.
fn base_from(c: &SystemColors) -> Base {
    let (bg, fg) = (c.background, c.foreground);
    let dark = c.theme() == Theme::Dark;
    Base {
        panel: bg,
        window: mix(bg, fg, 0.05),
        extreme: if dark {
            mix(bg, Color32::BLACK, 0.22)
        } else {
            mix(bg, Color32::WHITE, 0.6)
        },
        faint: mix(bg, fg, 0.04),
        widget: [mix(bg, fg, 0.10), mix(bg, fg, 0.17), mix(bg, fg, 0.24)],
        border: mix(bg, fg, 0.13),
        field_border: mix(bg, fg, 0.38),
        border_hover: mix(bg, fg, 0.58),
        text: fg,
        weak_text: mix(fg, bg, 0.40),
        selection: mix(bg, c.accent(), if dark { 0.40 } else { 0.30 }),
        shadow: Color32::from_black_alpha(if dark { 110 } else { 40 }),
    }
}

fn palette_from(c: &SystemColors) -> Palette {
    let dark = c.theme() == Theme::Dark;
    let accent = c.accent();
    let status_bar = if dark {
        mix(c.background, accent, 0.45)
    } else {
        accent
    };
    Palette {
        accent,
        on_accent: contrast(accent),
        status_bar,
        on_status_bar: contrast(status_bar),
        document_well: if dark {
            mix(c.background, Color32::BLACK, 0.30)
        } else {
            mix(c.background, Color32::BLACK, 0.06)
        },
    }
}

/// Cores do editor de SQL a partir das cores ANSI do tema do sistema; o que
/// faltar vem das cores próprias do app.
fn editor_colors_from(c: &SystemColors) -> EditorColors {
    let own = editor_colors_of(c.theme());
    let ansi = |i: usize, fallback: Color32| c.ansi[i].unwrap_or(fallback);
    EditorColors {
        text: c.foreground,
        keyword: ansi(4, own.keyword),
        string: ansi(2, own.string),
        comment: mix(c.foreground, c.background, 0.45),
        number: ansi(3, own.number),
        operator: mix(c.foreground, c.background, 0.20),
        identifier_quoted: ansi(6, own.identifier_quoted),
    }
}

fn editor_colors_of(theme: Theme) -> EditorColors {
    match theme {
        Theme::Light => EditorColors::LIGHT,
        Theme::Dark => EditorColors::DARK,
    }
}

/// Cores do editor de SQL (e do JSON Viewer) para o tema.
pub fn editor_colors(theme: Theme) -> EditorColors {
    match system_colors() {
        Some(c) => editor_colors_from(&c),
        None => editor_colors_of(theme),
    }
}

fn base(theme: Theme) -> Base {
    match system_colors() {
        Some(c) => base_from(&c),
        None => base_of(theme),
    }
}

fn base_of(theme: Theme) -> Base {
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
            field_border: Color32::from_rgb(0xA6, 0xAD, 0xB8),
            border_hover: Color32::from_rgb(0x7A, 0x83, 0x90),
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
            field_border: Color32::from_rgb(0x56, 0x5F, 0x6C),
            border_hover: Color32::from_rgb(0x7E, 0x88, 0x96),
            text: Color32::from_rgb(0xD8, 0xDC, 0xE2),
            weak_text: Color32::from_rgb(0x8A, 0x92, 0x9E),
            selection: Color32::from_rgb(0x26, 0x4F, 0x8F),
            shadow: Color32::from_black_alpha(110),
        },
    }
}

/// Cores do tema (as do sistema, quando em uso).
pub fn palette(theme: Theme) -> Palette {
    match system_colors() {
        Some(c) => palette_from(&c),
        None => palette_of(theme),
    }
}

fn palette_of(theme: Theme) -> Palette {
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
    visuals_of(theme, &base(theme), &palette(theme))
}

fn visuals_of(theme: Theme, b: &Base, p: &Palette) -> egui::Visuals {
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
    // Também é a borda do campo de texto com o foco: mais grossa que a
    // do campo parado, para o campo em uso saltar aos olhos.
    v.selection.stroke = Stroke::new(FOCUS_BORDER, b.text);
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
    // Botões e campos parados, sob o mouse, pressionados e abertos. A
    // borda vale para tudo em que se digita ou escolhe (campos de texto,
    // listas, caixas de seleção, números): sem ela o campo some no fundo.
    w.inactive.weak_bg_fill = b.widget[0];
    w.inactive.bg_fill = b.widget[0];
    w.inactive.bg_stroke = Stroke::new(BORDER, b.field_border);
    w.inactive.expansion = BORDER;
    w.inactive.fg_stroke = Stroke::new(1.0, b.text);
    w.hovered.weak_bg_fill = b.widget[1];
    w.hovered.bg_fill = b.widget[1];
    w.hovered.bg_stroke = Stroke::new(BORDER, b.border_hover);
    w.hovered.fg_stroke = Stroke::new(1.5, b.text);
    w.hovered.expansion = BORDER;
    w.active.weak_bg_fill = b.widget[2];
    w.active.bg_fill = b.widget[2];
    w.active.bg_stroke = Stroke::new(BORDER, p.accent);
    w.active.fg_stroke = Stroke::new(2.0, b.text);
    w.active.expansion = BORDER;
    w.open.weak_bg_fill = b.widget[1];
    w.open.bg_fill = b.widget[1];
    w.open.bg_stroke = Stroke::new(BORDER, b.border_hover);
    w.open.fg_stroke = Stroke::new(1.0, b.text);
    w.open.expansion = BORDER;
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

/// Espessura da borda de campos e botões. O egui desconta a borda da
/// margem interna, e um item sem moldura quando parado (linha de árvore,
/// item de menu) ficaria menor que sob o mouse; a `expansion` igual à
/// borda devolve esse tanto, e nada muda de tamanho ao passar o mouse.
const BORDER: f32 = 1.0;

/// Espessura da borda do campo de texto com o foco. Inteira: o egui
/// arredonda a margem, e com meio ponto o campo mudaria de tamanho ao
/// ganhar o foco.
const FOCUS_BORDER: f32 = 2.0;

/// Quanto a interface inteira é ampliada no tamanho "100%" das opções: as
/// letras do egui (12,5) ficam um pouco maiores, e ícones e espaços
/// acompanham.
pub const BASE_ZOOM: f32 = 1.08;

/// A menor área (em pontos) que a janela precisa ter para os diálogos e as
/// listas caberem: o zoom não passa do que deixa esse tanto visível.
pub const MIN_AREA: Vec2 = Vec2::new(640.0, 400.0);

/// O zoom do egui para o tamanho de interface escolhido nas opções.
pub fn zoom_factor(scale: crate::preferences::UiScale) -> f32 {
    BASE_ZOOM * scale.factor()
}

/// Espaçamentos: mais ar que o padrão do egui em volta do texto de menus,
/// botões e linhas, sem perder a densidade de uma ferramenta de banco de
/// dados.
fn spacing(spacing: &mut egui::Spacing) {
    spacing.item_spacing = Vec2::new(8.0, 5.0);
    spacing.button_padding = Vec2::new(10.0, 4.0);
    spacing.interact_size.y = 22.0;
    // Também é a margem do conteúdo das abas encaixadas (egui_dock); os
    // diálogos usam a margem maior de `dialog_frame`.
    spacing.window_margin = Margin::same(10);
    spacing.menu_margin = Margin::same(8);
    spacing.combo_height = 260.0;
}

/// Moldura dos diálogos modais: a mesma da janela, com margem interna
/// generosa e cantos arredondados.
pub fn dialog_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::window(style)
        .inner_margin(Margin::same(DIALOG_MARGIN))
        .corner_radius(CornerRadius::same(WINDOW_RADIUS))
}

/// Um diálogo modal com o estilo do PostPilot. Use no lugar de
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

    /// Contraste WCAG entre duas cores (1 = iguais, 21 = preto e branco).
    fn contrast_ratio(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// A borda dos campos parados aparece sobre o fundo do campo, do painel
    /// e do diálogo; sob o mouse e com o foco ela fica mais forte.
    fn fields_have_a_visible_border(name: &str, v: &egui::Visuals) {
        let idle = v.widgets.inactive.bg_stroke;
        let hover = v.widgets.hovered.bg_stroke;
        let focus = v.selection.stroke;
        assert!(idle.width >= 1.0, "{name}: no border");
        let field = v.text_edit_bg_color();
        for (what, bg) in [
            ("field", field),
            ("panel", v.panel_fill),
            ("dialog", v.window_fill),
            ("button", v.widgets.inactive.weak_bg_fill),
        ] {
            let c = contrast_ratio(idle.color, bg);
            assert!(c >= 1.5, "{name}: border on the {what} is {c:.2}");
        }
        assert!(
            contrast_ratio(hover.color, field) > contrast_ratio(idle.color, field),
            "{name}: hover is not stronger"
        );
        assert!(
            contrast_ratio(focus.color, field) > contrast_ratio(hover.color, field),
            "{name}: focus is not stronger"
        );
        assert!(focus.width > idle.width, "{name}: focus is not thicker");
        assert_eq!(focus.width.fract(), 0.0, "{name}: focus resizes the field");
        // Mesma borda e mesma folga em todos os estados: passar o mouse
        // não muda o tamanho de nada.
        for w in [
            &v.widgets.inactive,
            &v.widgets.hovered,
            &v.widgets.active,
            &v.widgets.open,
        ] {
            assert_eq!(w.bg_stroke.width, idle.width, "{name}: border width");
            assert_eq!(w.expansion, w.bg_stroke.width, "{name}: expansion");
        }
    }

    #[test]
    fn fields_have_a_visible_border_in_every_theme() {
        for theme in Theme::ALL {
            let v = visuals_of(theme, &base_of(theme), &palette_of(theme));
            fields_have_a_visible_border(&format!("{theme:?}"), &v);
        }
        for (name, c) in [("tokyo night", tokyo_night()), ("latte", latte())] {
            let v = visuals_of(c.theme(), &base_from(&c), &palette_from(&c));
            fields_have_a_visible_border(name, &v);
        }
    }

    #[test]
    fn dark_text_contrasts_with_the_panels() {
        let b = base_of(Theme::Dark);
        // Luminância aproximada: o texto tem de ficar bem acima do fundo.
        let lum = |c: Color32| (u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b())) / 3;
        assert!(lum(b.text) > lum(b.panel) + 150);
        assert!(lum(b.text) > lum(b.extreme) + 150);
    }

    fn hex(n: u32) -> Color32 {
        Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
    }

    fn tokyo_night() -> SystemColors {
        let mut ansi = [None; 16];
        ansi[2] = Some(hex(0x9ece6a));
        ansi[4] = Some(hex(0x7aa2f7));
        SystemColors {
            background: hex(0x1a1b26),
            foreground: hex(0xa9b1d6),
            accent: Some(hex(0x7aa2f7)),
            ansi,
        }
    }

    fn latte() -> SystemColors {
        SystemColors {
            background: hex(0xeff1f5),
            foreground: hex(0x4c4f69),
            accent: None,
            ansi: [None; 16],
        }
    }

    #[test]
    fn system_colors_know_if_they_are_dark() {
        assert_eq!(tokyo_night().theme(), Theme::Dark);
        assert_eq!(latte().theme(), Theme::Light);
    }

    #[test]
    fn system_colors_become_the_base() {
        let c = tokyo_night();
        let b = base_from(&c);
        assert_eq!(b.panel, c.background);
        assert_eq!(b.text, c.foreground);
        // Campos mais escuros que o painel; botões mais claros.
        assert!(luminance(b.extreme) < luminance(b.panel));
        assert!(luminance(b.widget[0]) > luminance(b.panel));
        assert!(luminance(b.widget[2]) > luminance(b.widget[0]));
        let p = palette_from(&c);
        assert_eq!(p.accent, hex(0x7aa2f7));
        // No claro, os campos são mais claros que o painel.
        let b = base_from(&latte());
        assert!(luminance(b.extreme) > luminance(b.panel));
    }

    #[test]
    fn missing_colors_fall_back_to_the_app_ones() {
        let c = latte();
        assert_eq!(palette_from(&c).accent, palette_of(Theme::Light).accent);
        let e = editor_colors_from(&c);
        assert_eq!(e.text, c.foreground);
        assert_eq!(e.keyword, EditorColors::LIGHT.keyword);
        let e = editor_colors_from(&tokyo_night());
        assert_eq!(e.keyword, hex(0x7aa2f7));
        assert_eq!(e.string, hex(0x9ece6a));
        assert_eq!(e.number, EditorColors::DARK.number);
    }

    #[test]
    fn text_on_the_status_bar_is_readable() {
        for c in [tokyo_night(), latte()] {
            let p = palette_from(&c);
            let diff = luminance(p.status_bar).abs_diff(luminance(p.on_status_bar));
            assert!(diff > 90, "{diff}");
        }
    }

    #[test]
    fn toggle_switches_between_the_two_themes() {
        assert_eq!(toggled(Theme::Light), Theme::Dark);
        assert_eq!(toggled(Theme::Dark), Theme::Light);
    }
}
