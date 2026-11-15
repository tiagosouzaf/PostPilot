//! A interface em português do Brasil (Tools > Options > International
//! Settings).
//!
//! O idioma é global no processo; por isso tudo fica num teste só, e os
//! demais arquivos de teste (outros processos) continuam em inglês.

use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::CommandId;
use pgstudio_i18n::{Language, set_language};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::preferences::{LanguageSetting, Preferences, PreferencesStore};

#[test]
fn switches_the_interface_to_portuguese_and_back() {
    let dir = std::env::temp_dir().join(format!("pgstudio-i18n-{}", std::process::id()));
    let file = dir.join("settings.json");
    let (store, prefs) = PreferencesStore::open(&file);
    // Arquivo novo: "Same as system". O teste fixa o inglês para não
    // depender do idioma da máquina.
    assert_eq!(prefs, Preferences::default());
    set_language(Language::English);
    let app = ShellApp::default().with_preferences(Some(store), prefs);
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);

    // Em inglês, como no SSMS.
    harness.get_by_label("File");
    harness.get_by_label("Tools").click();
    harness.run();
    harness.get_by_label("Options…").click();
    harness.run();
    assert!(harness.state().options_open());

    // Escolhe Português (Brasil) e confirma.
    assert_eq!(
        harness.query_all_by_label("International Settings").count(),
        2
    );
    language_combo(&harness).click();
    harness.run();
    harness.get_by_label("Português (Brasil)").click();
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();
    harness.run();
    assert!(!harness.state().options_open());
    assert_eq!(
        harness.state().preferences().language,
        LanguageSetting::Fixed(Language::PortugueseBrazil)
    );
    assert_eq!(pgstudio_i18n::language(), Language::PortugueseBrazil);

    // Menus, toolbar e status bar em português.
    for title in [
        "Arquivo",
        "Editar",
        "Exibir",
        "Consulta",
        "Ferramentas",
        "Janela",
        "Ajuda",
    ] {
        harness.get_by_label(title);
    }
    harness.get(by().label_contains("Nova Consulta"));
    harness.get(by().label_contains("Executar"));
    harness.get_by_label("Não conectado");
    harness.get_by_label("Arquivo").click();
    harness.run();
    harness.get_by_label("Salvar Ctrl+S");
    harness.get_by_label("Sair Alt+F4");
    harness.key_press(eframe::egui::Key::Escape);
    harness.run();
    let ctx = harness.ctx.clone();
    harness.state_mut().execute(&ctx, CommandId::Execute);
    harness.run();
    assert_eq!(
        harness.state().status(),
        "Nenhuma janela de consulta aberta."
    );

    // A escolha foi gravada.
    let (_, saved) = PreferencesStore::open(&file);
    assert_eq!(
        saved.language,
        LanguageSetting::Fixed(Language::PortugueseBrazil)
    );

    // E volta para o inglês pelo mesmo caminho.
    harness.state_mut().execute(&ctx, CommandId::Options);
    harness.run();
    assert_eq!(
        harness
            .query_all_by_label("Configurações Internacionais")
            .count(),
        2
    );
    language_combo(&harness).click();
    harness.run();
    harness.get_by_label("English").click();
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();
    harness.run();
    harness.get_by_label("File");
    assert_eq!(pgstudio_i18n::language(), Language::English);

    // O tema escolhido pelo menu também fica gravado.
    harness.state_mut().execute(&ctx, CommandId::ThemeLight);
    harness.run();
    let (_, saved) = PreferencesStore::open(&file);
    assert_eq!(saved.theme(None), pgstudio_core::Theme::Light);
    assert_eq!(saved.language, LanguageSetting::Fixed(Language::English));
    let _ = std::fs::remove_dir_all(dir);
    set_language(Language::English);
}

/// O seletor de idioma: o último combo da tela (o da toolbar vem antes).
fn language_combo<'a>(harness: &'a Harness<'static, ShellApp>) -> egui_kittest::Node<'a> {
    harness
        .query_all(by().role(Role::ComboBox))
        .last()
        .expect("seletor de idioma")
}
