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

use pgstudio_ui_shell::panels::Panel;

#[test]
fn default_layout_has_tool_windows() {
    let harness = harness();
    for panel in [Panel::ObjectExplorer, Panel::Properties, Panel::Output] {
        assert!(harness.state().is_panel_open(panel), "{panel:?}");
    }
    // As abas do egui_dock não aparecem na árvore de acessibilidade, então
    // conferimos o conteúdo de cada painel.
    assert!(
        harness
            .query_all_by_value("Not connected. Use File > Connect Object Explorer.")
            .next()
            .is_some()
    );
    assert!(
        harness
            .query_all_by_value("No object selected.")
            .next()
            .is_some()
    );
    assert!(
        harness
            .query_all_by_value("Show output from:")
            .next()
            .is_some()
    );
}

#[test]
fn closed_panels_come_back_from_the_view_menu() {
    let mut harness = harness();
    for (panel, item) in [
        (Panel::ObjectExplorer, "Object Explorer F8"),
        (Panel::Properties, "Properties Window F4"),
        (Panel::Output, "Output Ctrl+Alt+O"),
    ] {
        harness.state_mut().close_panel(panel);
        harness.run();
        assert!(!harness.state().is_panel_open(panel));
        harness.get_by_label("View").click();
        harness.run();
        harness.get_by_label(item).click();
        harness.run();
        assert!(harness.state().is_panel_open(panel), "{panel:?}");
    }
}

#[test]
fn shortcut_reopens_object_explorer() {
    let mut harness = harness();
    harness.state_mut().close_panel(Panel::ObjectExplorer);
    press(&mut harness, Modifiers::NONE, Key::F8);
    assert!(harness.state().is_panel_open(Panel::ObjectExplorer));
}

#[test]
fn reset_window_layout_restores_every_panel() {
    let mut harness = harness();
    for panel in [Panel::ObjectExplorer, Panel::Properties, Panel::Output] {
        harness.state_mut().close_panel(panel);
    }
    harness.run();
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Reset Window Layout").click();
    harness.run();
    for panel in [Panel::ObjectExplorer, Panel::Properties, Panel::Output] {
        assert!(harness.state().is_panel_open(panel), "{panel:?}");
    }
}

#[test]
fn dragging_a_tab_onto_another_group_moves_it() {
    let mut harness = harness();
    assert!(
        !harness
            .state()
            .panels_grouped(Panel::Properties, Panel::Output)
    );
    // A aba ativa fica no canto esquerdo da barra de abas do grupo.
    let tab = |panel| {
        let rect = harness.state().panel_rect(panel).unwrap();
        rect.left_top() + eframe::egui::vec2(30.0, 12.0)
    };
    let from = tab(Panel::Properties);
    // Solta na barra de abas do Output, à direita da aba existente.
    let to = tab(Panel::Output) + eframe::egui::vec2(150.0, 0.0);
    harness.drag_at(from);
    harness.run_steps(2);
    // Move aos poucos, como o mouse de verdade, até a barra de abas do Output.
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        harness.hover_at(from + (to - from) * t);
        harness.run_steps(2);
    }
    harness.drop_at(to);
    harness.run_steps(3);
    assert!(
        harness
            .state()
            .panels_grouped(Panel::Properties, Panel::Output)
    );
}

#[test]
fn output_logs_executed_commands() {
    let mut harness = harness();
    press(&mut harness, Modifiers::NONE, Key::F5);
    assert_eq!(
        harness.state().output().last().map(String::as_str),
        Some("> Execute (not available yet)")
    );
}

use pgstudio_core::metadata::NodeId;
use pgstudio_object_explorer::fake::{FakeProvider, SERVER_NAME};
use std::sync::Arc;
use std::time::Duration;

/// Shell conectado a um provedor falso sem atraso, com a árvore expandida
/// nos caminhos pedidos.
fn connected(paths: &[&str]) -> Harness<'static, ShellApp> {
    let mut harness = harness();
    let oe = harness.state_mut().explorer_mut();
    oe.connect(Arc::new(FakeProvider::new(Duration::ZERO)));
    assert!(oe.wait_idle(Duration::from_secs(5)));
    for p in paths {
        oe.expand(&NodeId((*p).into()));
        assert!(oe.wait_idle(Duration::from_secs(5)), "{p}");
    }
    harness.run();
    harness
}

#[test]
fn file_menu_connects_and_disconnects_object_explorer() {
    let mut harness = harness();
    assert!(!harness.state().explorer().is_connected());
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Connect Object Explorer…").click();
    harness.run();
    assert!(harness.state().explorer().is_connected());
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Disconnect Object Explorer").click();
    harness.run();
    assert!(!harness.state().explorer().is_connected());
}

#[test]
fn clicking_a_node_shows_it_in_properties() {
    let mut harness = connected(&[]);
    harness.get(by().label_contains(SERVER_NAME)).click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .explorer()
            .selected()
            .map(|n| n.name.as_str()),
        Some(SERVER_NAME)
    );
    assert!(harness.query_all_by_value("Server").next().is_some());
}

#[test]
fn expand_arrow_loads_children() {
    let mut harness = connected(&[]);
    // Só o servidor está na árvore, então só há uma seta.
    harness
        .get_by_label(egui_phosphor::regular::CARET_RIGHT)
        .click();
    harness.run();
    let oe = harness.state_mut().explorer_mut();
    assert!(oe.wait_idle(Duration::from_secs(5)));
    harness.run();
    harness.get(by().label_contains("Databases"));
}

#[test]
fn ten_thousand_tables_render_only_visible_rows() {
    let harness = connected(&[
        "srv",
        "srv/Databases",
        "srv/Databases/stress",
        "srv/Databases/stress/Schemas",
        "srv/Databases/stress/Schemas/public",
        "srv/Databases/stress/Schemas/public/Tables",
    ]);
    let drawn = harness
        .query_all(by().label_contains("public.table_"))
        .count();
    assert!(drawn > 0 && drawn < 200, "{drawn} linhas desenhadas");
}
