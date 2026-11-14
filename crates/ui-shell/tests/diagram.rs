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
use pgstudio_ui_shell::diagrams::{DiagramFile, DiagramView, Notice};
use pgstudio_ui_shell::documents::FileDialogs;
use pgstudio_ui_shell::panels::Panel;
use std::path::{Path, PathBuf};

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

/// "Save As" sempre responde `path`.
struct SaveTo(Option<PathBuf>);

impl FileDialogs for SaveTo {
    fn save_path(&mut self, _name: &str, _current: Option<&Path>) -> Option<PathBuf> {
        self.0.clone()
    }

    fn open_path(&mut self) -> Option<PathBuf> {
        None
    }
}

fn harness() -> (Harness<'static, ShellApp>, FakeProvider) {
    harness_saving_to(None)
}

fn harness_saving_to(path: Option<PathBuf>) -> (Harness<'static, ShellApp>, FakeProvider) {
    let provider = FakeProvider::new(Duration::ZERO);
    let mut app = ShellApp::default()
        .with_connector(Arc::new(FakeConnector(provider.clone())))
        .with_file_dialogs(SaveTo(path));
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

    // Fechar a aba com tabelas novas no desenho pergunta antes.
    harness.state_mut().close_panel_request(Panel::Diagram(id));
    harness.run_steps(3);
    assert!(diagram(&harness).1.is_confirming_close());
    harness.get_by_label("Close Without Saving").click();
    harness.run_steps(3);
    assert!(harness.state().diagrams().get(id).is_none());
    assert!(!harness.state().is_panel_open(Panel::Diagram(id)));
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

/// "New Database Diagram" com as tabelas `names` (pelo "Add Table").
fn new_diagram(harness: &mut Harness<'static, ShellApp>, names: &[&str]) -> u32 {
    harness
        .get_all_by_label_contains("Database Diagrams")
        .next()
        .unwrap()
        .click_secondary();
    harness.run();
    harness.get_by_label("New Database Diagram").click();
    harness.step();
    until(harness, "picker never opened", |h| {
        h.query_by_label("Select All").is_some()
    });
    let id = diagram(harness).0;
    for name in names {
        harness
            .state_mut()
            .diagrams_mut()
            .get_mut(id)
            .unwrap()
            .picker_mut()
            .unwrap()
            .filter = (*name).into();
        harness.run_steps(3);
        harness.get_by_label(name).click();
        harness.run();
    }
    harness.get_by_label("Add").click();
    harness.run_steps(5);
    id
}

fn table_id(d: &DiagramView, name: &str) -> u64 {
    d.diagram()
        .nodes
        .iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the diagram"))
        .table
}

#[test]
fn new_table_column_and_relationship_are_saved_with_the_script_shown() {
    let dir = std::env::temp_dir().join(format!("pgstudio-diagram-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loja.pgsdiagram");
    let (mut harness, provider) = harness_saving_to(Some(path.clone()));
    let id = new_diagram(&mut harness, &["public.table_00001"]);

    let new_table = format!("{} New Table", egui_phosphor::regular::TABLE);
    harness.get_by_label(&new_table).click();
    harness.run_steps(3);
    let d = diagram(&harness).1;
    let novo = table_id(d, "Table_1");
    assert_eq!(d.diagram().selected_table(), Some(novo));
    assert!(d.title().ends_with('*'));
    let add_column = format!("{} Add Column", egui_phosphor::regular::COLUMNS);
    harness.get_by_label(&add_column).click();
    harness.run_steps(3);
    let d = diagram(&harness).1;
    assert!(d.model().table(novo).unwrap().column("Column_1").is_some());

    // Arrastar table_00001.id até Table_1.id pede o relacionamento.
    let old = table_id(d, "table_00001");
    harness
        .state_mut()
        .diagrams_mut()
        .get_mut(id)
        .unwrap()
        .link(old, "id", novo, "id");
    harness.run_steps(3);
    let d = diagram(&harness).1;
    assert_eq!(d.diagram().edges.len(), 1);
    let script = d.change_script();
    assert!(
        script[0].starts_with("CREATE TABLE public.\"Table_1\""),
        "{script:?}"
    );
    assert!(
        script
            .iter()
            .any(|s| s.contains("FOREIGN KEY (id) REFERENCES public.table_00001 (id)")),
        "{script:?}"
    );

    // Undo e redo do relacionamento.
    harness
        .state_mut()
        .diagrams_mut()
        .get_mut(id)
        .unwrap()
        .undo();
    harness.run();
    assert!(diagram(&harness).1.diagram().edges.is_empty());
    harness
        .state_mut()
        .diagrams_mut()
        .get_mut(id)
        .unwrap()
        .redo();
    harness.run();
    assert_eq!(diagram(&harness).1.diagram().edges.len(), 1);

    // Ctrl+S com o diagrama em foco: mostra o script e salva ao confirmar.
    let ctx = harness.ctx.clone();
    harness.state_mut().focus_panel(Panel::Diagram(id));
    harness
        .state_mut()
        .execute(&ctx, pgstudio_core::CommandId::Save);
    harness.run_steps(3);
    assert_eq!(diagram(&harness).1.shown_script(), Some(script.as_slice()));
    assert!(provider.applied().is_empty());
    harness.get_by_label("Save").click();
    harness.step();
    until(&mut harness, "save never finished", |h| {
        !diagram(h).1.is_saving()
    });
    assert_eq!(provider.applied(), [("app_01".to_string(), script.clone())]);
    let d = diagram(&harness).1;
    assert!(!d.is_dirty(), "{:?}", d.notice());
    assert_eq!(d.title(), "app_01 - loja");
    assert!(d.change_script().is_empty());
    let file: DiagramFile = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file.database, "app_01");
    let names: Vec<&str> = file.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["table_00001", "Table_1"]);

    // Abrir o arquivo de novo: o servidor falso não guardou o Table_1,
    // então ele fica de fora com um aviso; o resto volta onde estava.
    let saved = d.diagram().nodes[0].pos;
    assert!(harness.state_mut().open_file(&path));
    harness.step();
    until(&mut harness, "file never loaded", |h| {
        h.state().diagrams().ids().len() == 2 && !diagram(h).1.is_loading()
    });
    let d = diagram(&harness).1;
    assert_eq!(d.title(), "app_01 - loja");
    assert_eq!(d.diagram().nodes.len(), 1);
    assert_eq!(d.diagram().nodes[0].pos, saved);
    assert!(
        matches!(d.notice(), Some(Notice::Info(t)) if t.contains("public.Table_1")),
        "{:?}",
        d.notice()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_save_keeps_the_edits() {
    let dir = std::env::temp_dir().join(format!("pgstudio-diagram-r{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.pgsdiagram");
    let (mut harness, provider) = harness_saving_to(Some(path.clone()));
    let id = new_diagram(&mut harness, &["public.table_00002"]);
    let t = table_id(diagram(&harness).1, "table_00002");
    // Uma coluna com um nome que o servidor falso recusa.
    {
        let view = harness.state_mut().diagrams_mut().get_mut(id).unwrap();
        view.diagram_mut().selected = Some(0);
        view.add_column();
    }
    let col = diagram(&harness)
        .1
        .model()
        .table(t)
        .unwrap()
        .columns
        .last()
        .unwrap()
        .id;
    harness
        .state_mut()
        .diagrams_mut()
        .get_mut(id)
        .unwrap()
        .edit(|d| d.rename_column(t, col, pgstudio_object_explorer::fake::FAILING_DDL));
    let ctx = harness.ctx.clone();
    harness.state_mut().focus_panel(Panel::Diagram(id));
    harness
        .state_mut()
        .execute(&ctx, pgstudio_core::CommandId::Save);
    harness.run_steps(3);
    harness.get_by_label("Save").click();
    harness.step();
    until(&mut harness, "save never finished", |h| {
        !diagram(h).1.is_saving()
    });
    assert!(provider.applied().is_empty());
    let d = diagram(&harness).1;
    assert!(matches!(d.notice(), Some(Notice::Error(e)) if e.contains("Nothing was saved")));
    assert!(d.is_dirty());
    assert!(!d.change_script().is_empty());
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
