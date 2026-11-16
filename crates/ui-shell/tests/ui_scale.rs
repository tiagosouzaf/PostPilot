//! Tamanho da interface: Ctrl++, Ctrl+- e Ctrl+0 (e o menu View) mudam o
//! zoom do aplicativo inteiro, a escolha fica em `settings.json` e volta ao
//! reabrir, e o zoom não passa do que a janela comporta.

use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::CommandId;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::preferences::{PreferencesStore, UiScale};
use pgstudio_ui_shell::theme::BASE_ZOOM;

fn dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-scale-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct NoServer;

impl pgstudio_core::connection::Connector for NoServer {
    fn connect(
        &self,
        _: &pgstudio_core::connection::ConnectionSettings,
        _: &str,
    ) -> Result<pgstudio_core::connection::Connected, pgstudio_core::connection::ConnectError> {
        Err(pgstudio_core::connection::ConnectError::Other(
            "offline".into(),
        ))
    }
}

fn open(settings: &std::path::Path, size: [f32; 2]) -> Harness<'static, ShellApp> {
    open_with(settings, size, false)
}

/// Com `connector`, o "Connect to Server" pode ser aberto.
fn open_with(
    settings: &std::path::Path,
    size: [f32; 2],
    connector: bool,
) -> Harness<'static, ShellApp> {
    let (store, prefs) = PreferencesStore::open(settings);
    let mut app = ShellApp::default().with_preferences(Some(store), prefs);
    if connector {
        app = app.with_connector(std::sync::Arc::new(NoServer));
    }
    let mut h = Harness::builder()
        .with_size(size)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.run_steps(3);
    h
}

fn zoom(h: &Harness<'static, ShellApp>) -> f32 {
    h.ctx.zoom_factor()
}

fn close_to(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn the_default_size_is_a_little_larger_than_egui() {
    let dir = dir("default");
    let h = open(&dir.join("settings.json"), [3000.0, 2000.0]);
    assert_eq!(h.state().ui_scale(), UiScale::default());
    assert!(close_to(zoom(&h), BASE_ZOOM), "{}", zoom(&h));
}

#[test]
fn shortcuts_change_the_size_and_it_comes_back_on_reopen() {
    let dir = dir("keys");
    let settings = dir.join("settings.json");
    {
        let mut h = open(&settings, [3000.0, 2000.0]);
        h.key_press_modifiers(Modifiers::CTRL, Key::Plus);
        h.run_steps(2);
        // O "=" é a mesma tecla sem Shift; com Shift é o "+" de vários
        // teclados.
        h.key_press_modifiers(Modifiers::CTRL, Key::Equals);
        h.run_steps(2);
        h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::Plus);
        h.run_steps(2);
        assert_eq!(h.state().ui_scale().percent(), 130);
        assert!(close_to(zoom(&h), BASE_ZOOM * 1.3), "{}", zoom(&h));
        assert!(
            h.state().status().contains("130%"),
            "{}",
            h.state().status()
        );
        h.key_press_modifiers(Modifiers::CTRL, Key::Minus);
        h.run_steps(2);
        assert_eq!(h.state().ui_scale().percent(), 120);
    }
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains(r#""ui_scale": 120"#), "{text}");
    // O app abre de novo no mesmo tamanho.
    let mut h = open(&settings, [3000.0, 2000.0]);
    assert_eq!(h.state().ui_scale().percent(), 120);
    assert!(close_to(zoom(&h), BASE_ZOOM * 1.2), "{}", zoom(&h));
    h.key_press_modifiers(Modifiers::CTRL, Key::Num0);
    h.run_steps(2);
    assert_eq!(h.state().ui_scale(), UiScale::default());
    assert!(close_to(zoom(&h), BASE_ZOOM));
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains(r#""ui_scale": 100"#), "{text}");
}

#[test]
fn the_view_menu_changes_the_size() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::English);
    let dir = dir("menu");
    let mut h = open(&dir.join("settings.json"), [3000.0, 2000.0]);
    h.get_by_label("View").click();
    h.run_steps(4);
    h.get_by_label_contains("Zoom In").click();
    h.run_steps(4);
    assert_eq!(h.state().ui_scale().percent(), 110);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::ZoomOut);
    h.state_mut().execute(&ctx, CommandId::ZoomOut);
    assert_eq!(h.state().ui_scale().percent(), 90);
    h.state_mut().execute(&ctx, CommandId::ZoomReset);
    assert_eq!(h.state().ui_scale().percent(), 100);
}

#[test]
fn the_size_stops_at_the_limits() {
    let dir = dir("limits");
    let mut h = open(&dir.join("settings.json"), [3000.0, 2000.0]);
    let ctx = h.ctx.clone();
    for _ in 0..40 {
        h.state_mut().execute(&ctx, CommandId::ZoomIn);
    }
    assert_eq!(h.state().ui_scale(), UiScale::MAX);
    for _ in 0..40 {
        h.state_mut().execute(&ctx, CommandId::ZoomOut);
    }
    assert_eq!(h.state().ui_scale(), UiScale::MIN);
    h.run_steps(2);
    assert!(close_to(zoom(&h), BASE_ZOOM * 0.7), "{}", zoom(&h));
}

#[test]
fn a_small_window_limits_the_zoom_but_keeps_the_choice() {
    let dir = dir("small");
    let settings = dir.join("settings.json");
    // 960 x 600 pixels: cabe até 1,5 de zoom (640 x 400 pontos).
    let mut h = open(&settings, [960.0, 600.0]);
    let ctx = h.ctx.clone();
    h.state_mut().set_ui_scale(UiScale::from_percent(200));
    h.run_steps(3);
    assert_eq!(h.state().ui_scale().percent(), 200);
    assert!(h.state().zoom_limited());
    assert!(close_to(zoom(&h), 1.5), "{}", zoom(&h));
    // Reduzir parte do que está na tela (138%), não dos 200%.
    h.state_mut().execute(&ctx, CommandId::ZoomOut);
    h.run_steps(3);
    assert_eq!(h.state().ui_scale().percent(), 130);
    assert!(!h.state().zoom_limited());
    assert!(close_to(zoom(&h), BASE_ZOOM * 1.3), "{}", zoom(&h));
}

#[test]
fn the_size_keys_work_with_a_dialog_open() {
    let dir = dir("dialog");
    let mut h = open_with(&dir.join("settings.json"), [3000.0, 2000.0], true);
    h.state_mut().open_connect_dialog();
    h.run_steps(2);
    assert!(h.state().connect_dialog().is_some());
    h.key_press_modifiers(Modifiers::CTRL, Key::Plus);
    h.run_steps(2);
    assert_eq!(h.state().ui_scale().percent(), 110);
    h.key_press_modifiers(Modifiers::CTRL, Key::Num0);
    h.run_steps(2);
    assert_eq!(h.state().ui_scale().percent(), 100);
}

#[test]
fn options_edits_the_size_and_ok_applies_it() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::English);
    let dir = dir("options");
    let settings = dir.join("settings.json");
    let mut h = open(&settings, [3000.0, 2000.0]);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::Options);
    h.run_steps(2);
    h.state_mut().options_mut().expect("options").page = pgstudio_ui_shell::options::Page::General;
    h.run_steps(2);
    assert!(h.query_by_label("Interface size:").is_some());
    h.state_mut().options_mut().expect("options").draft.ui_scale = UiScale::from_percent(150);
    h.run_steps(2);
    // Só o OK aplica.
    assert_eq!(h.state().ui_scale().percent(), 100);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().ui_scale().percent(), 150);
    assert!(close_to(zoom(&h), BASE_ZOOM * 1.5), "{}", zoom(&h));
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains(r#""ui_scale": 150"#), "{text}");
}
