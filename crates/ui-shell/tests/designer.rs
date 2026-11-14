//! Table Designer com o servidor falso: abrir pelo Object Explorer, editar
//! colunas, ver o script e salvar (o que o servidor recebe é o script
//! mostrado), erro ao salvar e fechar com alterações.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::accesskit::Role;
use eframe::egui::{self, Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::Offline;
use pgstudio_object_explorer::fake::{FAILING_DDL, FakeProvider};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::designer::{Notice, TableDesigner};
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

fn designer<'a>(harness: &'a Harness<'static, ShellApp>) -> (u32, &'a TableDesigner) {
    let designers = harness.state().designers();
    let id = *designers.ids().last().expect("no designer open");
    (id, designers.get(id).unwrap())
}

/// Abre o designer pelo menu de contexto e espera carregar.
fn open(harness: &mut Harness<'static, ShellApp>, node: &str, item: &str) -> u32 {
    harness
        .get_all_by_label_contains(node)
        .next()
        .expect("no such node")
        .click_secondary();
    harness.run();
    harness.get_by_label(item).click();
    harness.step();
    until(harness, "designer never loaded", |h| {
        h.state()
            .designers()
            .ids()
            .last()
            .and_then(|id| h.state().designers().get(*id))
            .is_some_and(|d| !d.is_loading())
    });
    designer(harness).0
}

/// Troca o texto de um campo da grade (clica, seleciona tudo, digita, Enter).
fn retype(harness: &mut Harness<'static, ShellApp>, current: &str, text: &str) {
    harness
        .get_all(by().value(current).role(Role::TextInput))
        .next()
        .expect("no such field")
        .click();
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.event(Event::Text(text.into()));
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
}

fn save(harness: &mut Harness<'static, ShellApp>) {
    let label = format!("{} Save", egui_phosphor::regular::FLOPPY_DISK);
    let b = harness.get_by_label(&label);
    b.click();
    harness.step();
    until(harness, "save never finished", |h| {
        !designer(h).1.is_saving()
    });
}

#[test]
fn design_renames_a_column_and_saves_the_script_it_shows() {
    let (mut harness, provider) = harness();
    let id = open(&mut harness, "public.table_00003", "Design");
    assert_eq!(designer(&harness).1.title(), "app_01 - public.table_00003");
    assert!(
        harness
            .state()
            .designers()
            .get(id)
            .unwrap()
            .model()
            .tables
            .len()
            > 1
    );

    retype(&mut harness, "email", "mail");
    let d = designer(&harness).1;
    assert_eq!(d.title(), "app_01 - public.table_00003*");
    let t = d.model().table(d.table_id().unwrap()).unwrap();
    assert!(t.column("mail").is_some());
    // O índice da coluna acompanha.
    assert_eq!(t.indexes[0].columns, ["mail"]);

    harness
        .get_by_label_contains("Generate Change Script")
        .click();
    harness.run();
    let script = designer(&harness).1.shown_script().unwrap().to_string();
    assert_eq!(
        script,
        "ALTER TABLE public.table_00003 RENAME COLUMN email TO mail;"
    );
    harness.get_by_label("Close").click();
    harness.run();
    assert!(designer(&harness).1.shown_script().is_none());

    save(&mut harness);
    let d = designer(&harness).1;
    assert_eq!(
        provider.applied(),
        [("app_01".to_string(), vec![script.clone()])]
    );
    assert_eq!(d.title(), "app_01 - public.table_00003");
    assert!(d.change_script().is_empty());
    assert!(harness.state().output().contains(&script));
}

#[test]
fn new_table_gets_a_name_columns_and_a_primary_key() {
    let (mut harness, provider) = harness();
    let id = open(&mut harness, "Tables", "New Table...");
    assert_eq!(designer(&harness).1.title(), "app_01 - public.Table_1*");

    // Nome da tabela e uma coluna nova pela linha vazia do fim da grade.
    retype(&mut harness, "Table_1", "produto");
    harness
        .ctx
        .memory_mut(|m| m.request_focus(egui::Id::new("designer_new_column")));
    harness.run();
    harness.event(Event::Text("preco".into()));
    harness.key_press(Key::Enter);
    harness.run();
    {
        let d = harness.state_mut().designers_mut().get_mut(id).unwrap();
        let t = d.model().find("public", "produto").unwrap();
        let preco = t.column("preco").unwrap().id;
        assert_eq!(d.selected(), Some(preco));
        d.update_column(preco, |c| c.ty = "numeric(10,2)".into());
        d.toggle_primary_key(preco);
    }
    harness.run();
    assert_eq!(
        designer(&harness).1.change_script(),
        [
            "CREATE TABLE public.produto (\n    id integer GENERATED ALWAYS AS IDENTITY NOT NULL,\n    preco numeric(10,2) NOT NULL,\n    CONSTRAINT \"Table_1_pkey\" PRIMARY KEY (preco)\n);"
        ]
    );
    save(&mut harness);
    assert_eq!(provider.applied().len(), 1);
    assert_eq!(designer(&harness).1.title(), "app_01 - public.produto");
    // Uma segunda tabela nova ganha o próximo número.
    open(&mut harness, "Tables", "New Table...");
    assert_eq!(designer(&harness).1.title(), "app_01 - public.Table_2*");
}

#[test]
fn a_failed_save_applies_nothing_and_keeps_the_edits() {
    let (mut harness, provider) = harness();
    open(&mut harness, "public.table_00003", "Design");
    retype(&mut harness, "email", FAILING_DDL);
    save(&mut harness);
    let d = designer(&harness).1;
    assert!(provider.applied().is_empty());
    assert!(
        matches!(d.notice(), Some(Notice::Error(e)) if e.contains("statement 1 of 1")),
        "{:?}",
        d.notice()
    );
    assert!(d.is_dirty());
    assert_eq!(d.change_script().len(), 1);
}

#[test]
fn undo_and_redo_follow_the_edits() {
    let (mut harness, _) = harness();
    let id = open(&mut harness, "public.table_00003", "Design");
    retype(&mut harness, "email", "mail");
    retype(&mut harness, "name", "nome");
    let d = harness.state_mut().designers_mut().get_mut(id).unwrap();
    d.undo();
    let t = d.model().table(d.table_id().unwrap()).unwrap();
    assert!(t.column("name").is_some() && t.column("mail").is_some());
    d.undo();
    assert!(!d.is_dirty());
    d.redo();
    assert_eq!(d.change_script().len(), 1);
}

#[test]
fn closing_a_changed_designer_asks_first() {
    let (mut harness, provider) = harness();
    let id = open(&mut harness, "public.table_00003", "Design");
    retype(&mut harness, "email", "mail");
    harness
        .state_mut()
        .close_panel_request(Panel::TableDesigner(id));
    harness.run();
    assert!(designer(&harness).1.is_confirming_close());
    harness.get_by_label("No").click();
    harness.run();
    harness.run();
    assert!(harness.state().designers().get(id).is_none());
    assert!(!harness.state().is_panel_open(Panel::TableDesigner(id)));
    assert!(provider.applied().is_empty());
}

#[test]
fn exit_asks_about_a_changed_designer() {
    let (mut harness, _) = harness();
    open(&mut harness, "public.table_00003", "Design");
    retype(&mut harness, "email", "mail");
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .execute(&ctx, pgstudio_core::CommandId::Exit);
    harness.run();
    assert!(designer(&harness).1.is_confirming_close());
    harness.get_by_label("Cancel").click();
    harness.run();
    let d = designer(&harness).1;
    assert!(!d.is_confirming_close() && d.is_dirty());
}
