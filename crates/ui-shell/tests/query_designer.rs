//! Query Designer com o servidor falso: Ctrl+Shift+Q numa janela de
//! consulta, tabelas pelo "Add Table", colunas marcadas no diagrama, filtro
//! e ordenação na grade, e o SQL indo para o editor no OK.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::Offline;
use pgstudio_diagram_engine::{HEADER_HEIGHT, ROW_HEIGHT};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::query_designer::SortType;

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

fn harness() -> Harness<'static, ShellApp> {
    let provider = FakeProvider::new(Duration::ZERO);
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(provider)));
    app.connect_to(ConnectionSettings::default(), "secret".into());
    let mut harness = Harness::builder()
        .with_size([1280.0, 900.0])
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    until(&mut harness, "never connected", |h| {
        h.state().explorer().is_connected()
    });
    harness
}

#[test]
fn designed_query_replaces_the_selection_in_the_editor() {
    let mut harness = harness();
    let doc = harness
        .state_mut()
        .documents_mut()
        .new_script("-- relatório\n".into(), Some("app_01".into()));
    harness.state_mut().activate_document(doc);
    harness.run();
    // O fim do texto fica selecionado (cursor no fim): o SQL entra ali.
    let editor = pgstudio_ui_shell::panels::editor_id(doc);
    pgstudio_sql_editor::set_selection(&harness.ctx, editor, (13, 13));
    harness.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::Q);
    harness.step();
    assert!(
        harness.state().query_designer().is_some(),
        "Ctrl+Shift+Q opens it"
    );
    until(&mut harness, "tables never loaded", |h| {
        h.query_by_label("Select All").is_some()
    });
    assert_eq!(
        harness.state().query_designer().unwrap().database(),
        "app_01"
    );

    for name in ["public.table_00001", "public.table_00002"] {
        harness
            .state_mut()
            .query_designer_mut()
            .unwrap()
            .picker_mut()
            .unwrap()
            .filter = name.into();
        harness.run_steps(3);
        harness.get_by_label(name).click();
        harness.run();
    }
    harness.get_by_label("Add").click();
    harness.run_steps(10);
    let qd = harness.state().query_designer().unwrap();
    assert_eq!(qd.query.tables.len(), 2);
    assert_eq!(qd.diagram().nodes.len(), 2);

    // Clica na caixinha de "name" (2ª coluna) da primeira tabela.
    let node = &qd.diagram().nodes[0];
    let row = node.columns.iter().position(|c| c.name == "name").unwrap();
    let at = qd
        .diagram()
        .screen_pos(node.pos + vec2(9.0, HEADER_HEIGHT + 3.0 + ROW_HEIGHT * (row as f32 + 0.5)));
    harness.event(egui::Event::PointerMoved(at));
    harness.run();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        harness.run();
    }
    harness.run_steps(3);
    let qd = harness.state().query_designer().unwrap();
    assert!(qd.query.is_output(0, "name"), "{:?}", qd.query.rows);
    assert_eq!(qd.diagram().nodes[0].columns[row].checked, Some(true));

    // JOIN, filtro e ordenação.
    {
        let qd = harness.state_mut().query_designer_mut().unwrap();
        qd.add_join(0, "id", 1, "id");
        qd.toggle_column(1, "email");
        qd.query.rows[1].sort = SortType::Ascending;
        qd.query.rows[0].filter = "LIKE 'A%'".into();
    }
    harness.run_steps(3);
    let expected = "SELECT table_00001.name,\n       table_00002.email\n\
        FROM public.table_00001\n    \
        INNER JOIN public.table_00002 ON table_00001.id = table_00002.id\n\
        WHERE table_00001.name LIKE 'A%'\n\
        ORDER BY table_00002.email;";
    assert_eq!(harness.state().query_designer().unwrap().sql(), expected);

    harness.get_by_label("OK").click();
    harness.run_steps(3);
    assert!(harness.state().query_designer().is_none());
    let text = &harness.state().documents().get(doc).unwrap().text;
    assert_eq!(text, &format!("-- relatório\n{expected}"));
}

#[test]
fn cancel_leaves_the_editor_alone() {
    let mut harness = harness();
    let doc = harness
        .state_mut()
        .documents_mut()
        .new_script("select 1;".into(), Some("app_01".into()));
    harness.state_mut().activate_document(doc);
    harness.run();
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .execute(&ctx, pgstudio_core::CommandId::DesignQuery);
    harness.step();
    until(&mut harness, "tables never loaded", |h| {
        h.query_by_label("Select All").is_some()
    });
    harness.get_by_label("Close").click();
    harness.run();
    harness.get_by_label("Cancel").click();
    harness.run_steps(3);
    assert!(harness.state().query_designer().is_none());
    assert_eq!(
        harness.state().documents().get(doc).unwrap().text,
        "select 1;"
    );
}

/// Abre o designer com `text` todo selecionado e espera as tabelas.
fn open_with_selection(text: &str) -> (Harness<'static, ShellApp>, u32) {
    let mut harness = harness();
    let doc = harness
        .state_mut()
        .documents_mut()
        .new_script(text.into(), Some("app_01".into()));
    harness.state_mut().activate_document(doc);
    harness.run();
    let editor = pgstudio_ui_shell::panels::editor_id(doc);
    pgstudio_sql_editor::set_selection(&harness.ctx, editor, (0, text.chars().count()));
    harness.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::Q);
    harness.step();
    until(&mut harness, "tables never loaded", |h| {
        h.state()
            .query_designer()
            .is_some_and(|qd| !qd.is_loading())
    });
    (harness, doc)
}

#[test]
fn the_selected_select_opens_in_the_designer() {
    let (mut harness, doc) = open_with_selection(
        "select a.name, b.email from table_00001 a \
         join table_00002 b on b.id = a.id where a.name like 'A%' order by b.email",
    );
    let qd = harness.state().query_designer().unwrap();
    assert_eq!(qd.notice(), None);
    assert!(qd.replaces_selection());
    assert_eq!(qd.query.tables.len(), 2);
    assert_eq!(qd.query.joins.len(), 1);
    assert_eq!(qd.diagram().nodes.len(), 2, "the tables are drawn");
    assert!(qd.query.is_output(0, "name"));
    assert!(harness.query_by_label("Select All").is_none(), "no picker");
    let expected = "SELECT a.name,\n       b.email\n\
        FROM public.table_00001 AS a\n    \
        INNER JOIN public.table_00002 AS b ON b.id = a.id\n\
        WHERE a.name LIKE 'A%'\n\
        ORDER BY b.email;";
    assert_eq!(qd.sql(), expected);
    harness.get_by_label("OK").click();
    harness.run_steps(3);
    assert_eq!(
        harness.state().documents().get(doc).unwrap().text,
        expected,
        "OK replaces the selection"
    );
}

#[test]
fn a_cte_shows_a_warning_and_keeps_the_selection() {
    let text = "with x as (select 1) select * from x;";
    let (mut harness, doc) = open_with_selection(text);
    let qd = harness.state().query_designer().unwrap();
    assert!(qd.notice().unwrap().contains("WITH"), "{:?}", qd.notice());
    assert!(!qd.replaces_selection());
    assert!(qd.query.tables.is_empty());
    assert!(
        harness
            .query_by_label_contains("common table expression")
            .is_some()
    );
    harness.get_by_label("OK").click();
    harness.run_steps(3);
    assert_eq!(
        harness.state().documents().get(doc).unwrap().text,
        format!("{text}\nSELECT *;")
    );
}
