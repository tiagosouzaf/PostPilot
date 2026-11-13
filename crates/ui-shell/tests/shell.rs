//! Testes de interação da casca, sem janela real (egui_kittest).

use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::{CommandId, Theme};
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
    harness.get(by().label_contains("New Query"));
    harness.get(by().label_contains("Execute"));
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

fn press(harness: &mut Harness<'static, ShellApp>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run();
}

#[test]
fn f5_and_ctrl_e_trigger_execute() {
    let mut harness = harness();
    press(&mut harness, Modifiers::NONE, Key::F5);
    assert_eq!(harness.state().last_command(), Some(CommandId::Execute));
    assert_eq!(
        harness.state().status(),
        "\"Execute\" is not available yet."
    );

    let mut harness = self::harness();
    press(&mut harness, Modifiers::CTRL, Key::E);
    assert_eq!(harness.state().last_command(), Some(CommandId::Execute));
}

#[test]
fn chord_sequence_triggers_its_command() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::K);
    assert_eq!(harness.state().last_command(), None);
    assert_eq!(
        harness.state().status(),
        "(Ctrl+K) was pressed. Waiting for second key of chord..."
    );
    press(&mut harness, Modifiers::CTRL, Key::U);
    assert_eq!(
        harness.state().last_command(),
        Some(CommandId::UncommentSelection)
    );
}

#[test]
fn unknown_chord_sequence_is_reported() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::K);
    press(&mut harness, Modifiers::CTRL, Key::Q);
    assert_eq!(harness.state().last_command(), None);
    assert_eq!(
        harness.state().status(),
        "The key combination (Ctrl+K, Ctrl+Q) is not a command."
    );
}

#[test]
fn menus_show_shortcuts_from_the_keymap() {
    let mut keymap = pgstudio_core::keymap::Keymap::ssms();
    assert!(
        keymap
            .apply_overrides("Query.Execute = Ctrl+Enter")
            .is_empty()
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(
            |ui, app: &mut ShellApp| app.show(ui),
            ShellApp::default().with_keymap(keymap),
        );
    harness.get_by_label("Query").click();
    harness.run();
    harness.get_by_label("Execute Ctrl+Enter");
    press(&mut harness, Modifiers::CTRL, Key::Enter);
    assert_eq!(harness.state().last_command(), Some(CommandId::Execute));
}
