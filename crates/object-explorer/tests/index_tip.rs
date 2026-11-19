//! A dica de um índice no Object Explorer: o ponteiro parado no índice
//! mostra o nome inteiro e os detalhes, lidos só nessa hora.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::metadata::{NodeId, Property};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_object_explorer::{Load, ObjectExplorer, Properties, index_tip_rows};

const WAIT: Duration = Duration::from_secs(5);

/// Expande o primeiro filho de `parent` cujo nome passa em `pick` e devolve
/// o id dele.
fn open(oe: &mut ObjectExplorer, parent: Option<&NodeId>, pick: impl Fn(&str) -> bool) -> NodeId {
    let kids = match parent {
        None => oe.roots().clone(),
        Some(id) => oe.children(id).cloned().expect("known node"),
    };
    let Load::Loaded(kids) = kids else {
        panic!("not loaded: {kids:?}");
    };
    let id = kids
        .into_iter()
        .find(|k| pick(&oe.node(k).expect("node").name))
        .expect("child");
    oe.expand(&id);
    assert!(oe.wait_idle(WAIT));
    id
}

/// A árvore aberta até os índices da primeira tabela; devolve o id do
/// índice do e-mail.
fn explorer() -> (ObjectExplorer, NodeId) {
    let mut oe = ObjectExplorer::default();
    oe.connect(Arc::new(FakeProvider::new(Duration::ZERO)));
    assert!(oe.wait_idle(WAIT));
    let mut at = open(&mut oe, None, |_| true);
    for name in ["Databases", "", "Schemas", "", "Tables", "", "Indexes"] {
        at = open(&mut oe, Some(&at), |n| name.is_empty() || n == name);
    }
    let Some(Load::Loaded(indexes)) = oe.children(&at).cloned() else {
        panic!("indexes not loaded");
    };
    let email = indexes
        .into_iter()
        .find(|i| i.0.ends_with("/email"))
        .expect("email index");
    (oe, email)
}

fn harness(size: [f32; 2]) -> (Harness<'static, ObjectExplorer>, NodeId) {
    let (oe, email) = explorer();
    let h = Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, oe: &mut ObjectExplorer| {
                oe.show(ui);
            },
            oe,
        );
    (h, email)
}

#[test]
fn hovering_an_index_shows_its_details() {
    let (mut h, email) = harness([420.0, 400.0]);
    h.run();
    // Nada é lido antes de o ponteiro parar no índice.
    assert!(!h.state().properties_loading());
    let name = h.state().node(&email).expect("index").name.clone();
    let row = h.get_by_label_contains(&name).rect().center();
    h.hover_at(row);
    h.run_steps(90);
    assert!(matches!(
        h.state_mut().properties(&email),
        Some(Properties::Loaded(_))
    ));
    // O essencial aparece; o comando inteiro e o que é "não" ficam de fora.
    h.get_by_label("lower(email), created_at DESC");
    h.get_by_label("btree");
    h.get_by_label("16 kB");
    assert!(h.query_by_label("Primary key").is_none());
    assert!(h.query_by_label("Definition").is_none());
    assert!(h.query_by_label_contains("CREATE UNIQUE INDEX").is_none());
}

#[test]
fn the_tip_fits_a_narrow_window() {
    let (mut h, email) = harness([200.0, 400.0]);
    h.run();
    let name = h.state().node(&email).expect("index").name.clone();
    let row = h.get_by_label_contains(&name).rect();
    h.hover_at(egui::pos2(row.left() + 4.0, row.center().y));
    h.run_steps(90);
    let screen = h.ctx.content_rect();
    let value = h.get_by_label("lower(email), created_at DESC").rect();
    assert!(screen.contains_rect(value), "{value:?} fora de {screen:?}");
}

#[test]
fn rows_keep_what_matters() {
    let long = "x".repeat(500);
    let props = [
        Property::new("Table", "t"),
        Property::new("Columns", "a, b"),
        Property::new("Include", ""),
        Property::new("Condition", long.as_str()),
        Property::new("Primary key", "no"),
        Property::new("Unique", "yes"),
        Property::new("Definition", "CREATE INDEX ..."),
    ];
    let rows = index_tip_rows(&props);
    let names: Vec<&str> = rows.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["Columns", "Condition", "Unique"]);
    assert_eq!(rows[0].1, "a, b");
    assert!(rows[1].1.chars().count() < 200 && rows[1].1.ends_with('…'));
    assert_eq!(rows[2].1, "yes");
}
