//! Janela "Check for Updates" e o aviso na status bar, sem rede.

use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::update::{
    InstallKind, Installed, Release, State, UpdateSettings, Updater, parse_release,
};

fn release() -> Release {
    parse_release(
        r#"{"tag_name": "v99.0.0", "html_url": "https://example.test/r", "body": "Novidades",
            "assets": [
              {"name": "PostPilot-99.0.0-x86_64.AppImage", "size": 1, "browser_download_url": "https://example.test/a"},
              {"name": "postpilot-99.0.0-1-x86_64.pkg.tar.zst", "size": 1, "browser_download_url": "https://example.test/p"},
              {"name": "postpilot_99.0.0_amd64.deb", "size": 1, "browser_download_url": "https://example.test/d"},
              {"name": "SHA256SUMS", "size": 1, "browser_download_url": "https://example.test/s"}
            ]}"#,
    )
    .unwrap()
    .unwrap()
}

fn harness(kind: InstallKind) -> Harness<'static, ShellApp> {
    harness_with(kind, None, State::Available(release()))
}

fn harness_with(
    kind: InstallKind,
    pkexec: Option<&str>,
    state: State,
) -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default();
    let mut updater = Updater::new(UpdateSettings::in_memory(), kind)
        .with_pkexec(pkexec.map(Into::into))
        .without_network();
    updater.set_state(state);
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
    let mut h = harness(InstallKind::Pacman("tf-postpilot-bin".into()));
    h.get_by_label("Update available: 99.0.0").click();
    h.run();
    assert!(h.query_by_label("Download and install").is_none());
    h.get(by().label_contains("-Syu tf-postpilot-bin"));
    h.get_by_label("Copy");
}

#[test]
fn release_packages_point_to_the_installer() {
    for kind in [InstallKind::Pacman("postpilot".into()), InstallKind::Dpkg] {
        let mut h = harness(kind);
        h.get_by_label("Update available: 99.0.0").click();
        h.run();
        h.get(by().label_contains("install.sh | bash"));
        assert!(h.query_by_label_contains("-Syu postpilot").is_none());
    }
}

#[test]
fn skipping_a_version_hides_it() {
    let mut h = harness(InstallKind::Pacman("tf-postpilot-bin".into()));
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

#[test]
fn a_new_version_shows_a_banner_with_one_click_update() {
    // Pacote do Release com pkexec: o botão instala; sem pkexec, a faixa
    // leva à janela com o comando.
    for kind in [InstallKind::Pacman("postpilot".into()), InstallKind::Dpkg] {
        let h = harness_with(
            kind.clone(),
            Some("/usr/bin/pkexec"),
            State::Available(release()),
        );
        h.get_by_label("PostPilot 99.0.0 is available.");
        h.get_by_label("Update now");
        h.get_by_label("What's new");
        assert!(h.query_by_label("How to update").is_none());

        let mut h = harness(kind);
        assert!(h.query_by_label("Update now").is_none());
        h.get_by_label("How to update").click();
        h.run();
        h.get(by().label_contains("install.sh | bash"));
    }
    // O AUR fica com o yay mesmo com pkexec.
    let h = harness_with(
        InstallKind::Pacman("tf-postpilot-bin".into()),
        Some("/usr/bin/pkexec"),
        State::Available(release()),
    );
    assert!(h.query_by_label("Update now").is_none());
    h.get_by_label("How to update");
}

#[test]
fn package_installs_explain_the_password_prompt_in_the_window() {
    let mut h = harness_with(
        InstallKind::Pacman("postpilot".into()),
        Some("/usr/bin/pkexec"),
        State::Available(release()),
    );
    h.get_by_label("What's new").click();
    h.run();
    h.get_by_label("Download and install");
    h.get_by_label("The system asks for your password to install the package.");
    assert!(h.query_by_label_contains("install.sh | bash").is_none());
}

#[test]
fn later_hides_the_banner_but_not_the_status_bar() {
    let mut h = harness(InstallKind::AppImage("/tmp/PostPilot.AppImage".into()));
    h.get_by_label("Later").click();
    h.run();
    assert!(h.query_by_label("PostPilot 99.0.0 is available.").is_none());
    h.get_by_label("Update available: 99.0.0");
}

#[test]
fn the_banner_offers_the_restart_and_the_retry() {
    let kind = InstallKind::Pacman("postpilot".into());
    let mut h = harness_with(
        kind.clone(),
        Some("/usr/bin/pkexec"),
        State::Installed(release(), Installed::Restart("/usr/bin/postpilot".into())),
    );
    h.get_by_label("Version 99.0.0 is installed.");
    h.get_by_label("Restart now").click();
    h.run();
    assert_eq!(
        h.state_mut().updater_mut().take_restart().as_deref(),
        Some(std::path::Path::new("/usr/bin/postpilot"))
    );

    let mut h = harness_with(
        kind,
        Some("/usr/bin/pkexec"),
        State::InstallFailed(
            release(),
            "The update was cancelled at the password prompt.".into(),
        ),
    );
    h.get_by_label("The update to 99.0.0 failed: The update was cancelled at the password prompt.");
    h.get_by_label("Try again");
    h.get_by_label("Details").click();
    h.run();
    h.get_by_label("Or update it in a terminal with:");
    h.get(by().label_contains("install.sh | bash"));
}

#[test]
fn update_now_starts_the_download_without_opening_the_window() {
    let mut h = harness_with(
        InstallKind::Dpkg,
        Some("/usr/bin/pkexec"),
        State::Available(release()),
    );
    h.get_by_label("Update now").click();
    h.step();
    let updater = h.state_mut().updater_mut();
    // O download (para example.test) falha depois, numa thread; aqui só
    // importa que o clique começou a instalação.
    assert!(matches!(
        updater.state(),
        State::Downloading(_) | State::InstallFailed(..)
    ));
    assert!(!updater.is_open());
}

#[test]
fn a_server_failure_is_explained_instead_of_showing_the_http_code_alone() {
    let h = harness_with(
        InstallKind::Dpkg,
        Some("/usr/bin/pkexec"),
        State::InstallFailed(
            release(),
            "GitHub's download server is temporarily unavailable (HTTP 504)".into(),
        ),
    );
    h.get_by_label(
        "The update to 99.0.0 failed: GitHub's download server is temporarily unavailable \
         (HTTP 504). The app tried 5 times; try again in a few minutes.",
    );
    h.get_by_label("Try again");

    let h = harness_with(
        InstallKind::Dpkg,
        Some("/usr/bin/pkexec"),
        State::InstallFailed(
            release(),
            "The connection failed during the download (timeout)".into(),
        ),
    );
    h.get(by().label_contains(
        "The connection failed during the download, after 5 tries. Check your internet",
    ));
}
