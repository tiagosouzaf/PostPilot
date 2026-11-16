//! Tools > Options: árvore de páginas à esquerda e a página escolhida à
//! direita, como no SSMS. OK aplica e grava; Cancel descarta.
//!
//! Para uma página nova: uma variante em [`Page`], uma linha em
//! [`Page::ALL`] (com a categoria) e o desenho em `OptionsDialog::page`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;
use pgstudio_automation::llm::{self, Provider, ProviderKind};
use pgstudio_i18n::{Language, tr, trf};

use crate::ai::{AiSettings, CLAUDE_CODE, Keys};

use crate::preferences::{LanguageSetting, Preferences, ThemeSetting, UiScale};
use pgstudio_core::Theme;

/// Uma página da árvore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Environment > General (tema).
    General,
    /// Environment > International Settings.
    International,
    /// AI > Providers.
    AiProviders,
}

impl Page {
    /// As páginas, com a categoria da árvore, na ordem de exibição.
    pub const ALL: &[(&'static str, Page)] = &[
        ("Environment", Page::General),
        ("Environment", Page::International),
        ("AI", Page::AiProviders),
    ];

    /// Nome da página na árvore (em inglês; passa por `tr` ao exibir).
    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::International => "International Settings",
            Page::AiProviders => "AI Providers",
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

/// O resultado de "Test connection": a resposta do modelo (ou o erro) e
/// os modelos que a chave enxerga.
type Check = (Result<String, String>, Vec<String>);

/// A página dos provedores de IA em edição.
#[derive(Default)]
pub struct AiDraft {
    /// Provedores e a escolha, em edição.
    pub settings: AiSettings,
    /// Chaves digitadas agora, por `id` (vazia: apagar a chave).
    pub new_keys: HashMap<String, String>,
    /// Chaves já guardadas (para saber quem tem chave e para testar).
    saved: Keys,
    /// Provedor aberto para edição.
    pub selected: Option<usize>,
    /// Onde está o Claude Code, se instalado.
    pub claude_code: Option<PathBuf>,
    /// Teste em andamento, com o `id` do provedor.
    check: Option<(String, mpsc::Receiver<Check>)>,
    /// Resultado do último teste.
    pub checked: Option<(String, Result<String, String>)>,
    /// Modelos listados, por `id`.
    models: HashMap<String, Vec<String>>,
}

impl std::fmt::Debug for AiDraft {
    /// Sem as chaves.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiDraft")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl AiDraft {
    /// Uma cópia para editar.
    pub fn new(settings: &AiSettings, saved: Keys, claude_code: Option<PathBuf>) -> Self {
        AiDraft {
            settings: settings.clone(),
            saved,
            selected: settings
                .providers
                .iter()
                .position(|p| p.id == settings.active)
                .or((!settings.providers.is_empty()).then_some(0)),
            claude_code,
            ..AiDraft::default()
        }
    }

    /// Acrescenta um provedor do tipo `kind` e abre para edição.
    pub fn add(&mut self, kind: ProviderKind) {
        let id = self.settings.new_id(kind);
        let mut provider = Provider::new(kind, &id);
        if kind == ProviderKind::Custom {
            provider.name = tr("Other provider").to_string();
        }
        // O primeiro provedor acrescentado passa a ser o usado quando não
        // há Claude Code: é o que a pessoa veio fazer aqui.
        if self.settings.active == CLAUDE_CODE && self.claude_code.is_none() {
            self.settings.active = id;
        }
        self.settings.providers.push(provider);
        self.selected = Some(self.settings.providers.len() - 1);
    }

    /// A chave em uso para o provedor `id`: a digitada agora ou a guardada.
    fn key(&self, id: &str) -> Option<String> {
        match self.new_keys.get(id) {
            Some(k) if k.is_empty() => None,
            Some(k) => Some(k.clone()),
            None => self.saved.lock().expect("ai keys").get(id).cloned(),
        }
    }

    /// Testa o provedor `index` numa thread: uma pergunta curta ao modelo
    /// e a lista de modelos.
    pub fn test(&mut self, ctx: &egui::Context, index: usize) {
        let Some(provider) = self.settings.providers.get(index) else {
            return;
        };
        let conn = provider.connect(&self.key(&provider.id).unwrap_or_default());
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let models = llm::list_models(&conn).unwrap_or_default();
            let answer = llm::ask(&conn, "Answer with the single word OK.", "ping");
            let _ = tx.send((answer, models));
            ctx.request_repaint();
        });
        self.checked = None;
        self.check = Some((provider.id.clone(), rx));
    }

    /// Se há um teste rodando.
    pub fn is_testing(&self) -> bool {
        self.check.is_some()
    }

    fn poll(&mut self) {
        let Some((id, rx)) = &self.check else {
            return;
        };
        match rx.try_recv() {
            Ok((answer, models)) => {
                if !models.is_empty() {
                    self.models.insert(id.clone(), models);
                }
                self.checked = Some((id.clone(), answer));
                self.check = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.check = None,
        }
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let active_label = |settings: &AiSettings| {
            settings
                .active_provider()
                .map_or_else(|| "Claude Code".to_string(), |p| p.name.clone())
        };
        ui.label(tr(
            "Assistant used by the AI panel, by Commit and to suggest file names:",
        ));
        egui::ComboBox::from_id_salt("options_ai_active")
            .width(300.0)
            .selected_text(active_label(&self.settings))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.settings.active,
                    CLAUDE_CODE.to_string(),
                    "Claude Code", // i18n-ignore
                );
                for p in &self.settings.providers {
                    ui.selectable_value(&mut self.settings.active, p.id.clone(), &p.name);
                }
            });
        match self.settings.active_provider() {
            None => match &self.claude_code {
                Some(path) => ui.weak(trf!(
                    "Claude Code is installed ({path}); it uses its own sign-in, no key needed.",
                    path = path.display()
                )),
                None => ui.colored_label(
                    ui.visuals().warn_fg_color,
                    tr("Claude Code is not installed on this computer."),
                ),
            },
            Some(p) if self.key(&p.id).is_none() && p.kind != ProviderKind::Custom => ui
                .colored_label(
                    ui.visuals().warn_fg_color,
                    tr("This provider has no API key yet."),
                ),
            Some(_) => ui.weak(tr("Ready to use.")),
        };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.strong(tr("Providers"));
            ui.menu_button(
                format!("{} {}", egui_phosphor::regular::PLUS, tr("Add")),
                |ui| {
                    for kind in ProviderKind::ALL {
                        let label = if kind == ProviderKind::Custom {
                            tr("Other (OpenAI-compatible)…").to_string()
                        } else {
                            kind.label().to_string()
                        };
                        if ui.button(label).clicked() {
                            self.add(kind);
                            ui.close();
                        }
                    }
                },
            );
        });
        if self.settings.providers.is_empty() {
            ui.weak(tr(
                "None yet. Add OpenAI (ChatGPT), OpenRouter, GLM or any OpenAI-compatible service with its API key.",
            ));
        }
        ui.horizontal_wrapped(|ui| {
            for (i, p) in self.settings.providers.iter().enumerate() {
                if ui
                    .selectable_label(self.selected == Some(i), &p.name)
                    .clicked()
                {
                    self.selected = Some(i);
                }
            }
        });
        let Some(index) = self.selected.filter(|i| *i < self.settings.providers.len()) else {
            return;
        };
        let id = self.settings.providers[index].id.clone();
        let kind = self.settings.providers[index].kind;
        let has_saved = self.saved.lock().expect("ai keys").contains_key(&id);
        let models = self.models.get(&id).cloned().unwrap_or_default();
        let mut test = false;
        let mut remove = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let p = &mut self.settings.providers[index];
            egui::Grid::new("options_ai_provider")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label(tr("Name:"));
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(280.0));
                    ui.end_row();
                    ui.label(tr("Base URL:"));
                    ui.add(egui::TextEdit::singleline(&mut p.base_url).desired_width(280.0));
                    ui.end_row();
                    ui.label(tr("Model:"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut p.model).desired_width(
                            if models.is_empty() { 280.0 } else { 240.0 },
                        ));
                        if !models.is_empty() {
                            egui::ComboBox::from_id_salt("options_ai_models")
                                .width(24.0)
                                .selected_text("")
                                .show_ui(ui, |ui| {
                                    for m in &models {
                                        ui.selectable_value(&mut p.model, m.clone(), m);
                                    }
                                });
                        }
                    });
                    ui.end_row();
                    ui.label(tr("API key:"));
                    let key = self.new_keys.entry(id.clone()).or_default();
                    let hint = if has_saved {
                        tr("Saved. Type to replace it.")
                    } else {
                        tr("Paste the key here")
                    };
                    ui.add(
                        egui::TextEdit::singleline(key)
                            .password(true)
                            .hint_text(hint)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });
            // Campo vazio não é "apagar a chave": só o que foi digitado vale.
            if self.new_keys.get(&id).is_some_and(String::is_empty) {
                self.new_keys.remove(&id);
            }
            if kind == ProviderKind::Glm {
                ui.weak(tr(
                    "This address is the one for the GLM Coding Plan subscription key. A pay-as-you-go key uses https://api.z.ai/api/paas/v4.",
                ));
            }
            if kind == ProviderKind::OpenAi {
                ui.weak(tr(
                    "Needs an API key from platform.openai.com; a ChatGPT subscription alone does not include one.",
                ));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.check.is_none(),
                        egui::Button::new(tr("Test connection")),
                    )
                    .clicked()
                {
                    test = true;
                }
                if ui.button(tr("Remove")).clicked() {
                    remove = true;
                }
                if self.check.as_ref().is_some_and(|(i, _)| *i == id) {
                    ui.spinner();
                }
            });
            if let Some((checked, result)) = &self.checked
                && *checked == id
            {
                match result {
                    Ok(_) => ui.colored_label(
                        egui::Color32::from_rgb(0x3f, 0xa8, 0x5f),
                        tr("It works: the model answered."),
                    ),
                    Err(e) => ui.colored_label(ui.visuals().error_fg_color, e),
                };
            }
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr(
                    "API keys are kept in the system keyring (or, when there is none, in a file only you can read). They never go into the workspace or to Git.",
                ))
                .small()
                .weak(),
            )
            .wrap(),
        );
        if test {
            self.test(ui.ctx(), index);
        }
        if remove {
            let gone = self.settings.providers.remove(index);
            self.new_keys.remove(&gone.id);
            if self.settings.active == gone.id {
                self.settings.active = CLAUDE_CODE.into();
            }
            self.selected = (!self.settings.providers.is_empty())
                .then(|| index.min(self.settings.providers.len() - 1));
        }
    }
}

/// O diálogo aberto, com uma cópia das preferências em edição.
#[derive(Debug)]
pub struct OptionsDialog {
    /// Preferências em edição.
    pub draft: Preferences,
    /// Página aberta.
    pub page: Page,
    /// Tema do sistema agora, para mostrar em "Same as system".
    pub system_theme: Option<Theme>,
    /// Provedores de IA em edição (a casca aplica junto com o OK).
    pub ai: AiDraft,
    /// O maior tamanho de interface que a janela comporta agora.
    pub max_scale: UiScale,
}

impl OptionsDialog {
    /// Abre com uma cópia das preferências atuais.
    pub fn new(current: &Preferences) -> Self {
        OptionsDialog {
            draft: current.clone(),
            page: Page::International,
            system_theme: None,
            ai: AiDraft::default(),
            max_scale: UiScale::MAX,
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
                        ui.set_max_width(420.0);
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
            Page::General => self.general(ui),
            Page::International => self.international(ui),
            Page::AiProviders => self.ai.show(ui),
        }
    }

    fn general(&mut self, ui: &mut egui::Ui) {
        let system = self.system_theme;
        let label = |s: ThemeSetting| match s {
            ThemeSetting::System => format!(
                "{} ({})",
                tr("Same as system"),
                tr(s.resolve(system).label())
            ),
            ThemeSetting::Fixed(t) => tr(t.label()).to_string(),
        };
        ui.horizontal(|ui| {
            ui.label(tr("Color theme:"));
            egui::ComboBox::from_id_salt("options_theme")
                .width(220.0)
                .selected_text(label(self.draft.theme))
                .show_ui(ui, |ui| {
                    let choices = std::iter::once(ThemeSetting::System)
                        .chain(Theme::ALL.into_iter().map(ThemeSetting::Fixed));
                    for s in choices {
                        ui.selectable_value(&mut self.draft.theme, s, label(s));
                    }
                });
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(tr("Interface size:"));
            let scale = &mut self.draft.ui_scale;
            if ui
                .add_enabled(
                    *scale != UiScale::MIN,
                    egui::Button::new(egui_phosphor::regular::MINUS),
                )
                .on_hover_text(tr("Zoom Out"))
                .clicked()
            {
                *scale = scale.stepped(-1);
            }
            let mut percent = scale.percent();
            if ui
                .add(
                    egui::DragValue::new(&mut percent)
                        .range(UiScale::MIN.percent()..=UiScale::MAX.percent())
                        .speed(1.0)
                        .suffix("%"),
                )
                .changed()
            {
                *scale = UiScale::from_percent(percent);
            }
            if ui
                .add_enabled(
                    *scale != UiScale::MAX,
                    egui::Button::new(egui_phosphor::regular::PLUS),
                )
                .on_hover_text(tr("Zoom In"))
                .clicked()
            {
                *scale = scale.stepped(1);
            }
            if ui
                .add_enabled(*scale != UiScale::default(), egui::Button::new(tr("Reset")))
                .clicked()
            {
                *scale = UiScale::default();
            }
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(tr(
                    "Makes text, icons and spacing larger or smaller in the whole application. Ctrl++ and Ctrl+- change it at any time; Ctrl+0 goes back to 100%.",
                ))
                .weak(),
            )
            .wrap(),
        );
        if self.draft.ui_scale.percent() > self.max_scale.percent() {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(trf!(
                        "This window fits up to {percent}% now. A larger size is saved and applies when the window is larger.",
                        percent = self.max_scale.percent()
                    ))
                    .color(ui.visuals().warn_fg_color),
                )
                .wrap(),
            );
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
