//! Janela "Check for Updates" e o aviso na status bar, sem rede.

use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::update::{
    InstallKind, Release, State, UpdateSettings, Updater, parse_release,
};

fn release() -> Release {
    parse_release(
        r#"{"tag_name": "v99.0.0", "html_url": "https://example.test/r", "body": "Novidades",
            "assets": [
              {"name": "PostPilot-99.0.0-x86_64.AppImage", "size": 1, "browser_download_url": "https://example.test/a"},
              {"name": "SHA256SUMS", "size": 1, "browser_download_url": "https://example.test/s"}
            ]}"#,
    )
    .unwrap()
    .unwrap()
}

fn harness(kind: InstallKind) -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default();
    let mut updater = Updater::new(UpdateSettings::in_memory(), kind).without_network();
    updater.set_state(State::Available(release()));
    *app.updater_mut() = updater;
    Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app)
}

#[test]
fn status_bar_announces_the_new_version_and_opens_the_window() {
    let mut h = harness(InstallKind::AppImage("/tmp/PostPilot.AppImage".into()));
    h.get_by_label("Update available: 99.0.0").click();
    h.run();
    assert!(h.state_mut().updater_mut().is_open());
    h.get_by_label("Version 99.0.0 is available.");
    h.get_by_label("Novidades");
    h.get_by_label("Download and install");
}

#[test]
fn pacman_installs_get_the_command_instead_of_a_download() {
    let mut h = harness(InstallKind::Pacman("postpilot-bin".into()));
    h.get_by_label("Update available: 99.0.0").click();
    h.run();
    assert!(h.query_by_label("Download and install").is_none());
    h.get(by().label_contains("-Syu postpilot-bin"));
    h.get_by_label("Copy");
}

#[test]
fn skipping_a_version_hides_it() {
    let mut h = harness(InstallKind::Pacman("postpilot-bin".into()));
    h.get_by_label("Update available: 99.0.0").click();
    h.run();
    h.get_by_label("Skip this version").click();
    h.run();
    let updater = h.state_mut().updater_mut();
    assert_eq!(
        updater.settings().skipped_version.as_deref(),
        Some("99.0.0")
    );
    assert!(!updater.is_open());
    assert!(h.query_by_label("Update available: 99.0.0").is_none());
}

#[test]
fn help_menu_has_check_for_updates_and_it_stays_offline_in_tests() {
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), ShellApp::default());
    h.get_by_label("Help").click();
    h.run();
    h.get_by_label("Check for Updates…").click();
    h.run();
    assert!(h.state_mut().updater_mut().is_open());
    h.get_by_label("Update checks are off in this mode.");
    h.get_by_label("Check for updates when the app starts");
}
