//! Database Diagrams com o servidor falso: "New Database Diagram" no
//! Object Explorer, escolher tabelas no "Add Table" e ver o diagrama.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::Event;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::Offline;
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::diagrams::DiagramView;
use pgstudio_ui_shell::panels::Panel;

struct FakeConnector(FakeProvider);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(self.0.clone()),
            queries: Arc::new(Offline("no queries in these tests")),
        })
    }
}

fn harness() -> (Harness<'static, ShellApp>, FakeProvider) {
    let provider = FakeProvider::new(Duration::ZERO);
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(provider.clone())));
    app.connect_to(
        ConnectionSettings {
            database: "loja".into(),
            ..Default::default()
        },
        "secret".into(),
    );
    let schema = "srv/Databases/app_01/Schemas/public";
    let app = app.with_expanded(
        [
            "srv",
            "srv/Databases",
            "srv/Databases/app_01",
            "srv/Databases/app_01/Schemas",
            schema,
        ]
        .into_iter()
        .map(String::from)
        .chain([format!("{schema}/Tables")]),
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 900.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    until(&mut harness, "tree never loaded", |h| {
        h.query_by_label_contains("public.table_00003").is_some()
    });
    (harness, provider)
}

/// Roda frames até `done`.
fn until(
    harness: &mut Harness<'static, ShellApp>,
    what: &str,
    done: impl Fn(&Harness<'static, ShellApp>) -> bool,
) {
    let start = Instant::now();
    loop {
        harness.step();
        if done(harness) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run_steps(20);
    harness.run();
}

fn diagram<'a>(harness: &'a Harness<'static, ShellApp>) -> (u32, &'a DiagramView) {
    let diagrams = harness.state().diagrams();
    let id = *diagrams.ids().last().expect("no diagram open");
    (id, diagrams.get(id).unwrap())
}

#[test]
fn new_diagram_picks_tables_and_draws_them() {
    let (mut harness, _) = harness();
    harness
        .get_all_by_label_contains("Database Diagrams")
        .next()
        .expect("no Database Diagrams folder")
        .click_secondary();
    harness.run();
    harness.get_by_label("New Database Diagram").click();
    harness.step();
    until(&mut harness, "diagram never loaded", |h| {
        h.state().diagrams().ids().last().is_some_and(|id| {
            h.state()
                .diagrams()
                .get(*id)
                .is_some_and(|d| !d.is_loading())
        })
    });
    let (id, d) = diagram(&harness);
    assert_eq!(d.title(), "app_01 - Diagram_0");
    assert!(harness.state().is_panel_open(Panel::Diagram(id)));
    assert!(d.picker().is_some(), "Add Table opens with the diagram");
    assert!(d.diagram().nodes.is_empty());

    // Filtro e duas tabelas marcadas.
    harness
        .state_mut()
        .diagrams_mut()
        .get_mut(id)
        .unwrap()
        .picker_mut()
        .unwrap()
        .filter = "public.table_0000".into();
    harness.run_steps(5);
    assert_eq!(diagram(&harness).1.candidates().len(), 10);
    harness.get_by_label("public.table_00001").click();
    harness.run();
    harness.get_by_label("public.table_00002").click();
    harness.run();
    harness.get_by_label("Add").click();
    harness.run_steps(5);
    let d = diagram(&harness).1;
    assert!(d.picker().is_none());
    let names: Vec<&str> = d.diagram().nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["table_00001", "table_00002"]);
    assert_eq!(d.diagram().drawn().0, 2);

    // As que já estão no diagrama não aparecem de novo no "Add Table".
    let add = format!("{} Add Table...", egui_phosphor::regular::PLUS);
    harness.get_by_label(&add).click();
    harness.run();
    let d = diagram(&harness).1;
    assert_eq!(d.candidates().len(), d.model().tables.len() - 2);
    harness.get_by_label("Select All").click();
    harness.run();
    harness.get_by_label("Add").click();
    harness.run_steps(5);
    let d = diagram(&harness).1;
    assert_eq!(d.diagram().nodes.len(), d.model().tables.len());

    // Fechar a aba esquece o diagrama.
    harness.state_mut().close_panel_request(Panel::Diagram(id));
    harness.run_steps(3);
    assert!(harness.state().diagrams().get(id).is_none());
}

#[test]
fn typing_in_the_filter_narrows_the_list() {
    let (mut harness, _) = harness();
    harness
        .get_all_by_label_contains("Database Diagrams")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label("New Database Diagram").click();
    harness.step();
    until(&mut harness, "picker never opened", |h| {
        h.query_by_label("Select All").is_some()
    });
    let all = diagram(&harness).1.candidates().len();
    harness.event(Event::Text("00007".into()));
    harness.run_steps(3);
    let d = diagram(&harness).1;
    assert!(d.candidates().len() < all);
    assert!(
        d.candidates()
            .iter()
            .all(|(_, n)| n.ends_with("table_00007"))
    );
}
