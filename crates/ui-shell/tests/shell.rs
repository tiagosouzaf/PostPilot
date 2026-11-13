//! Testes de interação da casca, sem janela real (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::Theme;
use pgstudio_ui_shell::ShellApp;

fn harness() -> Harness<'static, ShellApp> {
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), ShellApp::default())
}

#[test]
fn menu_bar_has_ssms_menus() {
    let harness = harness();
    for title in ["File", "Edit", "View", "Query", "Tools", "Window", "Help"] {
        harness.get_by_label(title);
    }
}

#[test]
fn toolbar_and_status_bar_are_shown() {
    let harness = harness();
    harness.get_by_label("New Query");
    harness.get_by_label("Execute");
    harness.get_by_label("Ready");
    harness.get_by_label("Not connected");
}

#[test]
fn file_menu_lists_items() {
    let mut harness = harness();
    harness.get_by_label("File").click();
    harness.run();
    // O rótulo acessível junta o texto e o atalho.
    harness.get_by_label("Save Ctrl+S");
    harness.get_by_label("Exit Alt+F4");
}

#[test]
fn view_menu_switches_to_dark_theme() {
    let mut harness = harness();
    assert_eq!(harness.state().theme(), Theme::Light);
    harness.get_by_label("View").click();
    harness.run();
    harness.get_by_label("Color Theme ⏵").hover();
    harness.run();
    harness.get_by_label("Dark").click();
    harness.run();
    assert_eq!(harness.state().theme(), Theme::Dark);
}

#[test]
fn help_menu_opens_about() {
    let mut harness = harness();
    harness.get_by_label("Help").click();
    harness.run();
    harness.get_by_label("About PGStudio").click();
    harness.run();
    assert!(harness.state().about_open());
}
