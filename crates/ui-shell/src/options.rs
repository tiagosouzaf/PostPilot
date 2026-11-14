//! Tools > Options: árvore de páginas à esquerda e a página escolhida à
//! direita, como no SSMS. OK aplica e grava; Cancel descarta.
//!
//! Para uma página nova: uma variante em [`Page`], uma linha em
//! [`Page::ALL`] (com a categoria) e o desenho em `OptionsDialog::page`.

use eframe::egui;
use pgstudio_i18n::{Language, tr};

use crate::preferences::{LanguageSetting, Preferences};

/// Uma página da árvore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Environment > International Settings.
    International,
}

impl Page {
    /// As páginas, com a categoria da árvore, na ordem de exibição.
    pub const ALL: &[(&'static str, Page)] = &[("Environment", Page::International)];

    /// Nome da página na árvore (em inglês; passa por `tr` ao exibir).
    pub fn title(self) -> &'static str {
        match self {
            Page::International => "International Settings",
        }
    }
}

/// O que o diálogo pede à casca.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Nada.
    None,
    /// OK: aplicar e gravar estas preferências.
    Apply(Preferences),
    /// Cancel ou fechar a janela.
    Cancel,
}

/// O diálogo aberto, com uma cópia das preferências em edição.
#[derive(Debug)]
pub struct OptionsDialog {
    /// Preferências em edição.
    pub draft: Preferences,
    /// Página aberta.
    pub page: Page,
}

impl OptionsDialog {
    /// Abre com uma cópia das preferências atuais.
    pub fn new(current: &Preferences) -> Self {
        OptionsDialog {
            draft: current.clone(),
            page: Page::International,
        }
    }

    /// Desenha o diálogo.
    pub fn show(&mut self, ctx: &egui::Context) -> Action {
        let mut action = Action::None;
        let mut open = true;
        egui::Window::new(tr("Options"))
            .id(egui::Id::new("options_dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(egui::vec2(200.0, 260.0), |ui| {
                        ui.set_min_size(egui::vec2(200.0, 260.0));
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_size(egui::vec2(190.0, 250.0));
                            ui.vertical(|ui| self.tree(ui));
                        });
                    });
                    ui.vertical(|ui| {
                        ui.set_min_width(380.0);
                        self.page(ui);
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("Cancel")).clicked() {
                            action = Action::Cancel;
                        }
                        if ui.button(tr("OK")).clicked() {
                            action = Action::Apply(self.draft.clone());
                        }
                    });
                });
            });
        if !open {
            action = Action::Cancel;
        }
        action
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        let mut category = None;
        for &(cat, page) in Page::ALL {
            if category != Some(cat) {
                category = Some(cat);
                ui.label(egui::RichText::new(tr(cat)).strong());
            }
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                if ui
                    .selectable_label(self.page == page, tr(page.title()))
                    .clicked()
                {
                    self.page = page;
                }
            });
        }
    }

    fn page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tr(self.page.title()));
        ui.add_space(6.0);
        match self.page {
            Page::International => self.international(ui),
        }
    }

    fn international(&mut self, ui: &mut egui::Ui) {
        let label = |s: LanguageSetting| match s {
            LanguageSetting::System => format!(
                "{} ({})",
                tr("Same as system"),
                pgstudio_i18n::system_language().native_name()
            ),
            LanguageSetting::Fixed(l) => l.native_name().to_string(),
        };
        ui.horizontal(|ui| {
            ui.label(tr("Language:"));
            egui::ComboBox::from_id_salt("options_language")
                .width(220.0)
                .selected_text(label(self.draft.language))
                .show_ui(ui, |ui| {
                    let choices = std::iter::once(LanguageSetting::System)
                        .chain(Language::ALL.into_iter().map(LanguageSetting::Fixed));
                    for s in choices {
                        ui.selectable_value(&mut self.draft.language, s, label(s));
                    }
                });
        });
        ui.add_space(6.0);
        ui.label(tr("SQL keywords and object names are not translated."));
        ui.label(tr(
            "Messages from PostgreSQL follow the lc_messages setting of the server.",
        ));
    }
}
