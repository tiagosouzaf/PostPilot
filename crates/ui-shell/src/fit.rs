//! Linhas que se ajustam à largura: o que não cabe é encurtado com
//! reticências (o texto inteiro fica na dica), em vez de ser desenhado
//! por cima do vizinho.

use eframe::egui::{self, Align, Layout, Ui, UiBuilder, WidgetText};

/// Abaixo desta largura não vale a pena desenhar um texto encurtado.
const MIN_TEXT: f32 = 24.0;

/// Uma linha com um lado esquerdo e um lado direito que nunca se
/// sobrepõem. O lado direito é desenhado primeiro, da direita para a
/// esquerda, e fica com o espaço que pedir; o esquerdo recebe o que
/// sobrar e é cortado nesse limite. Use [`text`] nos dois lados para os
/// textos encurtarem.
pub fn ends(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            right(ui);
            rest(ui, left);
        });
    });
}

/// Dentro de uma linha da direita para a esquerda: desenha `left` da
/// esquerda para a direita no espaço que sobrou, cortado nele.
pub fn rest(ui: &mut Ui, left: impl FnOnce(&mut Ui)) {
    let rect = ui.available_rect_before_wrap();
    if rect.width() < 1.0 {
        return;
    }
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(rect));
            left(ui);
        },
    );
}

/// Um texto que encurta com reticências para caber no espaço que resta
/// na linha; some quando não há espaço nem para isso.
pub fn text(ui: &mut Ui, text: impl Into<WidgetText>) -> Option<egui::Response> {
    text_within(ui, text, f32::INFINITY)
}

/// Como [`text`], com um teto de largura.
pub fn text_within(
    ui: &mut Ui,
    text: impl Into<WidgetText>,
    max_width: f32,
) -> Option<egui::Response> {
    let width = ui.available_width().min(max_width);
    if width < MIN_TEXT {
        return None;
    }
    let layout = *ui.layout();
    let mut rect = ui.available_rect_before_wrap();
    if layout.main_dir() == egui::Direction::RightToLeft {
        rect.min.x = rect.max.x - width;
    } else {
        rect.max.x = rect.min.x + width;
    }
    let inner = ui.scope_builder(UiBuilder::new().max_rect(rect).layout(layout), |ui| {
        ui.add(egui::Label::new(text).truncate())
    });
    Some(inner.inner)
}

/// A largura que um texto ocuparia inteiro, numa linha só.
pub fn text_width(ui: &Ui, text: impl Into<WidgetText>) -> f32 {
    text.into()
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
}

/// Um separador vertical, só quando ainda há espaço na linha.
pub fn separator(ui: &mut Ui) {
    if ui.available_width() >= MIN_TEXT {
        ui.separator();
    }
}

/// A largura de um diálogo: a desejada, ou a da janela do app quando ela
/// é mais estreita que isso (janela pequena ou zoom grande).
pub fn dialog_width(ui: &mut Ui, wanted: f32) {
    let screen = ui.ctx().content_rect().width();
    ui.set_width(wanted.min(screen - DIALOG_MARGIN).max(MIN_DIALOG));
}

/// A altura que o miolo de um diálogo (uma lista, uma área de rolagem)
/// pode ter: a desejada, ou a que cabe na janela do app descontado
/// `around`, a altura do resto do diálogo (título, campos e botões).
pub fn dialog_height(ui: &Ui, wanted: f32, around: f32) -> f32 {
    let screen = ui.ctx().content_rect().height();
    wanted.min(screen - around).max(MIN_TEXT * 3.0)
}

/// A largura de uma coluna lateral (lista, árvore) ao lado de um conteúdo:
/// a desejada, ou no máximo esta fração do espaço que há.
pub fn side_width(ui: &Ui, wanted: f32, fraction: f32) -> f32 {
    wanted.min(ui.available_width() * fraction)
}

/// Moldura, margens e sombra de um diálogo, dos dois lados.
const DIALOG_MARGIN: f32 = 48.0;
/// Um diálogo nunca fica mais estreito que isto.
const MIN_DIALOG: f32 = 160.0;

/// Os limites de um painel redimensionável dentro de uma área com este
/// tamanho: o mínimo desejado e, no máximo, o que deixa `keep` para o
/// vizinho. Numa área pequena demais para os dois, o painel fica com a
/// metade.
pub fn pane_range(available: f32, min: f32, keep: f32) -> egui::Rangef {
    let max = (available - keep).max(available * 0.5);
    egui::Rangef::new(min.min(max), max)
}
