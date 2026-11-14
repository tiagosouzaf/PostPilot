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
    assert_eq!(harness.state().theme(), Theme::Dark, "o escuro é o padrão");
    let ctx = harness.ctx.clone();
    harness.state_mut().set_theme(&ctx, Theme::Light);
    harness.run();
    harness.get_by_label("View").click();
    harness.run();
    harness.get_by_label("Color Theme ⏵").hover();
    harness.run();
    harness.get_by_label("Dark").click();
    harness.run();
    assert_eq!(harness.state().theme(), Theme::Dark);
}

#[test]
fn menu_bar_button_toggles_the_theme() {
    let mut harness = harness();
    harness.get_by_label(egui_phosphor::regular::SUN).click();
    harness.run();
    assert_eq!(harness.state().theme(), Theme::Light);
    harness.get_by_label(egui_phosphor::regular::MOON).click();
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
    assert_eq!(harness.state().status(), "No query window is open.");

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
        Some("> Execute")
    );
    press(&mut harness, Modifiers::CTRL, Key::F5);
    assert_eq!(
        harness.state().output().last().map(String::as_str),
        Some("> Parse (not available yet)")
    );
}

use pgstudio_core::metadata::NodeId;
use pgstudio_object_explorer::fake::{FAKE_OWNER, FakeProvider, SERVER_NAME};
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
fn properties_window_shows_what_the_provider_reads() {
    let mut harness = connected(&["srv", "srv/Databases"]);
    let db = NodeId("srv/Databases/postgres".into());
    harness.state_mut().explorer_mut().select(&db);
    harness.run();
    assert!(
        harness
            .state_mut()
            .explorer_mut()
            .wait_idle(Duration::from_secs(5))
    );
    harness.run();
    assert!(harness.query_all_by_value(FAKE_OWNER).next().is_some());
    assert!(harness.query_all_by_value("Database").next().is_some());
}

#[test]
fn f5_in_object_explorer_refreshes_the_selected_node() {
    let mut harness = connected(&["srv"]);
    // Com atraso, para ver o nó carregando de novo depois do F5.
    let oe = harness.state_mut().explorer_mut();
    oe.connect(Arc::new(FakeProvider::new(Duration::from_millis(300))));
    assert!(oe.wait_idle(Duration::from_secs(5)));
    oe.expand(&NodeId("srv".into()));
    assert!(oe.wait_idle(Duration::from_secs(5)));
    harness.run();
    harness.get(by().label_contains(SERVER_NAME)).click();
    harness.run();
    let srv = NodeId("srv".into());
    assert!(!harness.state().explorer().is_loading());
    harness.key_press(Key::F5);
    harness.step();
    assert!(
        harness.state().explorer().is_loading(),
        "F5 should reload the children of the selected node"
    );
    assert!(
        harness
            .state_mut()
            .explorer_mut()
            .wait_idle(Duration::from_secs(5))
    );
    harness.run();
    assert!(harness.state().explorer().children(&srv).is_some());
    assert!(
        !harness
            .state()
            .output()
            .iter()
            .any(|l| l.contains("not available")),
        "{:?}",
        harness.state().output()
    );
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

// Janelas de consulta (marco 1.6).

use eframe::egui::Event;
use pgstudio_ui_shell::documents::FileDialogs;
use std::path::{Path, PathBuf};

/// Diálogos com respostas prontas.
struct Answers {
    save: Option<PathBuf>,
    open: Option<PathBuf>,
}

impl FileDialogs for Answers {
    fn save_path(&mut self, _name: &str, _current: Option<&Path>) -> Option<PathBuf> {
        self.save.clone()
    }

    fn open_path(&mut self) -> Option<PathBuf> {
        self.open.clone()
    }
}

fn with_dialogs(save: Option<PathBuf>, open: Option<PathBuf>) -> Harness<'static, ShellApp> {
    let app = ShellApp::default().with_file_dialogs(Answers { save, open });
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

fn temp_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pgstudio-shell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn type_text(harness: &mut Harness<'static, ShellApp>, text: &str) {
    harness.event(Event::Text(text.to_string()));
    harness.run();
}

fn active_text(harness: &Harness<'static, ShellApp>) -> String {
    let app = harness.state();
    let id = app.active_document().expect("no active query");
    app.documents().get(id).unwrap().text.clone()
}

#[test]
fn ctrl_n_opens_a_query_window_in_the_document_area() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::N);
    let id = harness.state().active_document().expect("query opened");
    let app = harness.state();
    assert_eq!(app.documents().get(id).unwrap().name(), "SQLQuery1.sql");
    assert!(app.is_panel_open(Panel::Query(id)));
    let query = app.panel_rect(Panel::Query(id)).unwrap();
    let explorer = app.panel_rect(Panel::ObjectExplorer).unwrap();
    let props = app.panel_rect(Panel::Properties).unwrap();
    let output = app.panel_rect(Panel::Output).unwrap();
    assert!(query.left() >= explorer.right() - 1.0);
    assert!(query.right() <= props.left() + 1.0);
    assert!(query.bottom() <= output.top() + 1.0);

    press(&mut harness, Modifiers::CTRL, Key::N);
    let second = harness.state().active_document().unwrap();
    assert_ne!(second, id);
    assert!(
        harness
            .state()
            .panels_grouped(Panel::Query(id), Panel::Query(second))
    );
}

#[test]
fn typing_goes_to_the_editor_and_marks_the_query_changed() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "select 1;");
    assert_eq!(active_text(&harness), "select 1;");
    let id = harness.state().active_document().unwrap();
    let doc = harness.state().documents().get(id).unwrap();
    assert!(doc.is_dirty());
    assert_eq!(doc.title(), "SQLQuery1.sql*");
    // Atalhos de edição ficam com o editor, não viram comandos da casca.
    harness.event(Event::Copy);
    press(&mut harness, Modifiers::CTRL, Key::A);
    assert_eq!(harness.state().last_command(), Some(CommandId::NewQuery));
}

#[test]
fn ctrl_s_asks_for_a_file_and_saves() {
    let path = temp_file("save.sql");
    let mut harness = with_dialogs(Some(path.clone()), None);
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "select now();");
    press(&mut harness, Modifiers::CTRL, Key::S);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "select now();");
    let id = harness.state().active_document().unwrap();
    let doc = harness.state().documents().get(id).unwrap();
    assert!(!doc.is_dirty());
    assert_eq!(doc.name(), "save.sql");
    assert_eq!(harness.state().status(), "Item(s) Saved");
}

#[test]
fn cancelled_save_keeps_the_changes() {
    let mut harness = with_dialogs(None, None);
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "x");
    press(&mut harness, Modifiers::CTRL | Modifiers::SHIFT, Key::S);
    let id = harness.state().active_document().unwrap();
    assert!(harness.state().documents().get(id).unwrap().is_dirty());
}

#[test]
fn ctrl_o_reopens_a_saved_file_identical() {
    let text = "select 'olá';\r\n\tselect 2;\n";
    let path = temp_file("open.sql");
    std::fs::write(&path, text).unwrap();
    let mut harness = with_dialogs(None, Some(path.clone()));
    press(&mut harness, Modifiers::CTRL, Key::O);
    assert_eq!(active_text(&harness), text);
    let id = harness.state().active_document().unwrap();
    assert_eq!(
        harness.state().documents().get(id).unwrap().name(),
        "open.sql"
    );
    // Abrir de novo traz a mesma aba.
    press(&mut harness, Modifiers::CTRL, Key::O);
    assert_eq!(harness.state().documents().ids(), vec![id]);
}

#[test]
fn closing_an_unchanged_query_does_not_ask() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::N);
    press(&mut harness, Modifiers::CTRL, Key::F4);
    assert!(harness.state().confirm_pending().is_none());
    assert!(harness.state().documents().ids().is_empty());
    assert_eq!(harness.state().active_document(), None);
}

#[test]
fn closing_a_changed_query_asks_first() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "select 1;");
    press(&mut harness, Modifiers::CTRL, Key::F4);
    assert!(harness.state().confirm_pending().is_some());
    harness.get_by_label("Save changes to the following items?");
    harness.get_by_label("SQLQuery1.sql");

    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.state().confirm_pending().is_none());
    assert_eq!(harness.state().documents().ids().len(), 1);

    press(&mut harness, Modifiers::CTRL, Key::F4);
    harness.get_by_label("No").click();
    harness.run();
    assert!(harness.state().documents().ids().is_empty());
}

#[test]
fn answering_yes_saves_then_closes() {
    let path = temp_file("yes.sql");
    let mut harness = with_dialogs(Some(path.clone()), None);
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "select 3;");
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Close Ctrl+F4").click();
    harness.run();
    harness.get_by_label("Yes").click();
    harness.run();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "select 3;");
    assert!(harness.state().documents().ids().is_empty());
}

#[test]
fn exit_with_changed_queries_asks_first() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "select 4;");
    press(&mut harness, Modifiers::ALT, Key::F4);
    assert!(harness.state().confirm_pending().is_some());
    harness.get_by_label("Cancel").click();
    harness.run();
    assert_eq!(harness.state().documents().ids().len(), 1);
}

#[test]
fn reset_window_layout_keeps_query_windows() {
    let mut harness = harness();
    press(&mut harness, Modifiers::CTRL, Key::N);
    type_text(&mut harness, "select 5;");
    let id = harness.state().active_document().unwrap();
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Reset Window Layout").click();
    harness.run();
    assert!(harness.state().is_panel_open(Panel::Query(id)));
    assert_eq!(active_text(&harness), "select 5;");
}

// Layout salvo entre sessões (marco 1.7).

use pgstudio_ui_shell::layout::LayoutStore;

fn restarted(path: &Path) -> Harness<'static, ShellApp> {
    let app = ShellApp::default().with_layout_store(LayoutStore::open(path));
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

#[test]
fn restart_restores_the_layout() {
    let path = temp_file("layout-restore.json");
    let mut harness = restarted(&path);
    // Leva o Properties para junto do Output e fecha o Object Explorer.
    let tab = |harness: &Harness<'static, ShellApp>, panel| {
        let rect = harness.state().panel_rect(panel).unwrap();
        rect.left_top() + eframe::egui::vec2(30.0, 12.0)
    };
    let from = tab(&harness, Panel::Properties);
    let to = tab(&harness, Panel::Output) + eframe::egui::vec2(150.0, 0.0);
    harness.drag_at(from);
    harness.run_steps(2);
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
    harness.state_mut().close_panel(Panel::ObjectExplorer);
    harness.run();
    press(&mut harness, Modifiers::CTRL, Key::N);
    harness.state_mut().save_layout();

    let harness = restarted(&path);
    let app = harness.state();
    assert!(!app.is_panel_open(Panel::ObjectExplorer));
    assert!(app.panels_grouped(Panel::Properties, Panel::Output));
    // Janelas de consulta não são salvas.
    assert!(app.documents().ids().is_empty());
    assert!(
        !app.current_layout()
            .docking
            .floating
            .contains(&Panel::Query(1))
    );
    assert_eq!(app.status(), "Ready");
}

#[test]
fn corrupt_layout_file_falls_back_to_default() {
    let path = temp_file("layout-corrupt.json");
    std::fs::write(&path, "{\"version\": 1, \"docking\": {\"root\": 42").unwrap();
    let harness = restarted(&path);
    let app = harness.state();
    for panel in [Panel::ObjectExplorer, Panel::Properties, Panel::Output] {
        assert!(app.is_panel_open(panel), "{panel:?}");
    }
    assert!(app.status().contains("default layout"));
}

#[test]
fn reset_after_restore_brings_back_the_default() {
    let path = temp_file("layout-reset.json");
    let mut harness = restarted(&path);
    harness.state_mut().close_panel(Panel::Output);
    harness.state_mut().save_layout();
    let mut harness = restarted(&path);
    assert!(!harness.state().is_panel_open(Panel::Output));
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Reset Window Layout").click();
    harness.run();
    assert!(harness.state().is_panel_open(Panel::Output));
}

// Auto-hide de painéis (ADR 0002).

use eframe::egui::accesskit::Role;

fn strip_button<'a>(
    harness: &'a Harness<'static, ShellApp>,
    title: &'a str,
) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Button, title)
}

fn auto_hide_all(harness: &mut Harness<'static, ShellApp>) {
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Auto Hide All").click();
    harness.run();
}

#[test]
fn auto_hide_all_moves_tool_windows_to_the_edges() {
    let mut harness = harness();
    auto_hide_all(&mut harness);
    for panel in [Panel::ObjectExplorer, Panel::Properties, Panel::Output] {
        assert!(harness.state().is_panel_hidden(panel), "{panel:?}");
        assert!(
            harness.state().strip_button_rect(panel).is_some(),
            "{panel:?}"
        );
    }
    let left = harness
        .state()
        .strip_button_rect(Panel::ObjectExplorer)
        .unwrap();
    let right = harness
        .state()
        .strip_button_rect(Panel::Properties)
        .unwrap();
    let bottom = harness.state().strip_button_rect(Panel::Output).unwrap();
    assert!(left.center().x < 40.0);
    assert!(right.center().x > 1240.0);
    assert!(bottom.center().y > left.center().y && bottom.height() < 30.0);
}

#[test]
fn clicking_the_strip_shows_the_panel_over_the_documents() {
    let mut harness = harness();
    auto_hide_all(&mut harness);
    press(&mut harness, Modifiers::CTRL, Key::N);
    strip_button(&harness, "Object Explorer").click();
    harness.run();
    assert_eq!(harness.state().flyout_panel(), Some(Panel::ObjectExplorer));
    harness.get_by_label("Not connected. Use File > Connect Object Explorer.");
    let flyout = harness.state().panel_rect(Panel::ObjectExplorer).unwrap();
    // Clicar no documento, fora do painel, esconde o painel de novo.
    let outside = flyout.right_center() + eframe::egui::vec2(200.0, 0.0);
    harness.hover_at(outside);
    harness.run();
    harness.drag_at(outside);
    harness.drop_at(outside);
    harness.run();
    assert_eq!(harness.state().flyout_panel(), None);
    assert!(harness.state().is_panel_hidden(Panel::ObjectExplorer));
}

#[test]
fn hovering_the_strip_slides_the_panel_out_and_back() {
    let mut harness = harness();
    auto_hide_all(&mut harness);
    let button = harness
        .state()
        .strip_button_rect(Panel::ObjectExplorer)
        .unwrap();
    harness.hover_at(button.center());
    // Cada passo do harness avança 0,25 s; o painel abre depois de 0,3 s.
    harness.run_steps(2);
    assert_eq!(harness.state().flyout_panel(), None, "ainda não abriu");
    harness.run();
    assert_eq!(harness.state().flyout_panel(), Some(Panel::ObjectExplorer));
    let flyout = harness.state().panel_rect(Panel::ObjectExplorer).unwrap();
    harness.hover_at(flyout.right_center() + eframe::egui::vec2(200.0, 0.0));
    harness.run_steps(4);
    assert_eq!(harness.state().flyout_panel(), None);
    assert!(harness.state().is_panel_hidden(Panel::ObjectExplorer));
}

#[test]
fn escape_and_second_click_hide_the_panel() {
    let mut harness = harness();
    auto_hide_all(&mut harness);
    strip_button(&harness, "Output").click();
    harness.run();
    assert_eq!(harness.state().flyout_panel(), Some(Panel::Output));
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert_eq!(harness.state().flyout_panel(), None);
    strip_button(&harness, "Output").click();
    harness.run();
    strip_button(&harness, "Output").click();
    harness.run();
    assert_eq!(harness.state().flyout_panel(), None);
}

#[test]
fn pin_button_docks_the_panel_again() {
    let mut harness = harness();
    auto_hide_all(&mut harness);
    strip_button(&harness, "Properties").click();
    harness.run();
    harness
        .get_by_label(egui_phosphor::regular::PUSH_PIN_SLASH)
        .click();
    harness.run();
    assert!(!harness.state().is_panel_hidden(Panel::Properties));
    assert!(harness.state().is_panel_open(Panel::Properties));
    assert_eq!(harness.state().flyout_panel(), None);
    let props = harness.state().panel_rect(Panel::Properties).unwrap();
    assert!(props.right() > 1200.0, "voltou para a direita: {props:?}");
}

#[test]
fn window_auto_hide_toggles_the_open_panel() {
    let mut harness = harness();
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Auto Hide").click();
    harness.run();
    // Sem painel com foco, só avisa.
    assert_eq!(
        harness.state().status(),
        "Select a tool window to auto hide."
    );

    assert!(harness.state_mut().auto_hide_panel(Panel::Output));
    harness.run();
    // View > Output com o painel desafixado mostra o painel por cima.
    harness.get_by_label("View").click();
    harness.run();
    harness.get_by_label("Output Ctrl+Alt+O").click();
    harness.run();
    assert_eq!(harness.state().flyout_panel(), Some(Panel::Output));
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Auto Hide").click();
    harness.run();
    assert!(!harness.state().is_panel_hidden(Panel::Output));
}

#[test]
fn hidden_panels_survive_a_restart() {
    let path = temp_file("layout-autohide.json");
    let mut harness = restarted(&path);
    harness.state_mut().auto_hide_panel(Panel::ObjectExplorer);
    harness.run();
    harness.state_mut().save_layout();
    let harness = restarted(&path);
    assert!(harness.state().is_panel_hidden(Panel::ObjectExplorer));
    assert!(
        harness
            .state()
            .strip_button_rect(Panel::ObjectExplorer)
            .is_some()
    );
}
