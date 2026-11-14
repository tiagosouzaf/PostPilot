//! IntelliSense na janela de consulta, com o servidor de demonstração: o
//! catálogo é lido em segundo plano, as sugestões vêm dele e
//! Ctrl+Shift+R lê de novo.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;

fn harness() -> Harness<'static, ShellApp> {
    let mut app = ShellApp::default();
    app.explorer_mut()
        .connect(Arc::new(FakeProvider::new(Duration::ZERO)));
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run();
    wait_catalog(&mut h);
    h
}

fn wait_catalog(h: &mut Harness<'static, ShellApp>) {
    let start = Instant::now();
    loop {
        h.step();
        let id = h.state().active_document().unwrap();
        if !h.state().is_loading_catalog() && h.state().catalog_for(id).is_some() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "catalog never loaded"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run();
}

fn typed(h: &mut Harness<'static, ShellApp>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
        h.run();
    }
}

fn doc<'a>(h: &'a Harness<'static, ShellApp>) -> &'a pgstudio_ui_shell::documents::Document {
    let id = h.state().active_document().unwrap();
    h.state().documents().get(id).unwrap()
}

#[test]
fn the_catalog_loads_in_the_background_and_feeds_suggestions() {
    let mut h = harness();
    assert!(
        h.state().output().iter().any(
            |l| l.starts_with("IntelliSense: ") && l.ends_with("objects loaded from postgres.")
        ),
        "{:?}",
        h.state().output()
    );
    typed(&mut h, "select * from table_0004");
    let labels = doc(&h).editor.completion_labels();
    assert_eq!(&labels[..2], ["table_00040", "table_00041"]);
    h.key_press(Key::Tab);
    h.run();
    typed(&mut h, " t where t.");
    assert_eq!(
        doc(&h).editor.completion_labels(),
        ["active", "balance", "created_at", "email", "id", "name"]
    );
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(doc(&h).text, "select * from table_00040 t where t.active");
}

#[test]
fn edit_menu_list_members_opens_the_list_and_refresh_reloads() {
    let mut h = harness();
    typed(&mut h, "select ");
    h.key_press_modifiers(Modifiers::CTRL, Key::J);
    h.run();
    assert!(doc(&h).editor.is_completing());
    h.key_press(Key::Escape);
    h.run();
    assert!(!doc(&h).editor.is_completing());

    let before = h.state().output().len();
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::R);
    h.run();
    wait_catalog(&mut h);
    assert_eq!(
        h.state().output().len(),
        before + 2,
        "{:?}",
        h.state().output()
    );
    assert!(
        h.state()
            .output()
            .last()
            .unwrap()
            .ends_with("loaded from postgres.")
    );
}
