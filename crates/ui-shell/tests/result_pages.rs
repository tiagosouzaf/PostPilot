//! Resultados em páginas e cópia no formato escolhido, na janela de
//! consulta: a faixa embaixo da grade troca de página, muda o tamanho da
//! página e o formato do Ctrl+C, e as escolhas ficam gravadas nas
//! preferências (voltam iguais ao reabrir o app).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_phosphor::regular as icons;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_result_grid::{CopyFormat, GridState};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::options::Page;
use pgstudio_ui_shell::preferences::{Preferences, PreferencesStore};

const ROWS: u64 = 1234;

struct FakeQueries;

impl QueryService for FakeQueries {
    fn open(&self, _: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession)
    }
}

/// Toda consulta devolve [`ROWS`] pedidos.
struct FakeSession;

impl QuerySession for FakeSession {
    fn execute(&self, _: String, mut sink: QuerySink) {
        std::thread::spawn(move || {
            sink(QueryEvent::Columns(vec![
                "id".into(),
                "cliente".into(),
                "total".into(),
            ]));
            for i in 1..=ROWS {
                sink(QueryEvent::Row(vec![
                    Some(i.to_string()),
                    Some(format!("Cliente {i}")),
                    (i % 7 != 0).then(|| format!("{}.90", i * 3)),
                ]));
            }
            sink(QueryEvent::CommandComplete { rows: ROWS });
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        "loja".into()
    }
}

struct FakeConnector;

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries),
        })
    }
}

fn settings_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("postpilot-pages-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("settings.json")
}

/// O app conectado, com as preferências gravadas em `file`.
fn harness(file: &std::path::Path) -> Harness<'static, ShellApp> {
    let (store, prefs) = PreferencesStore::open(file);
    let mut app = ShellApp::default()
        .with_connector(Arc::new(FakeConnector))
        .with_preferences(Some(store), prefs);
    app.connect_to(
        ConnectionSettings {
            database: "loja".into(),
            ..Default::default()
        },
        "secret".into(),
    );
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    let start = Instant::now();
    while harness.state().connection().is_none() {
        harness.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    harness
}

/// Abre uma janela, executa e espera as linhas todas.
fn run_query(harness: &mut Harness<'static, ShellApp>) -> u32 {
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    harness.event(Event::Text("select * from pedidos".into()));
    harness.run();
    let id = harness.state().active_document().unwrap();
    harness.key_press(Key::F5);
    let start = Instant::now();
    loop {
        harness.step();
        let done = harness
            .state()
            .documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running() && e.results.first().is_some_and(|r| r.len() == ROWS));
        if done && !harness.state().is_loading_catalog() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "query never finished"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
    id
}

fn grid<'a>(harness: &'a Harness<'static, ShellApp>, doc: u32) -> &'a GridState {
    &harness
        .state()
        .documents()
        .get(doc)
        .unwrap()
        .execution
        .as_ref()
        .unwrap()
        .results[0]
        .grid
}

fn click(harness: &mut Harness<'static, ShellApp>, pos: egui::Pos2) {
    harness.run_steps(40);
    harness.event(Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
}

/// O que o Ctrl+C manda para a área de transferência: a seleção da grade
/// no formato em uso (a própria tecla é conferida nos testes da grade).
fn copied(harness: &Harness<'static, ShellApp>, doc: u32) -> Option<String> {
    let prefs = harness.state().preferences().results.grid();
    let set = &harness
        .state()
        .documents()
        .get(doc)
        .unwrap()
        .execution
        .as_ref()
        .unwrap()
        .results[0];
    set.grid
        .copy_as(&mut set.store(), prefs.copy_format, prefs.copy_headers)
}

/// O botão da faixa das páginas com este ícone (o mais embaixo: a árvore
/// do Object Explorer usa setas parecidas).
fn page_button<'a>(
    harness: &'a Harness<'static, ShellApp>,
    icon: &'a str,
) -> egui_kittest::Node<'a> {
    harness
        .get_all_by_label(icon)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("page button")
}

fn corner(harness: &Harness<'static, ShellApp>, doc: u32) -> egui::Pos2 {
    let l = grid(harness, doc).layout.unwrap();
    egui::pos2(l.header.left() + 4.0, l.header.center().y)
}

/// O resultado vem em páginas de 500 linhas, sem o usuário pedir; a faixa
/// embaixo troca de página, e a contagem da barra de status continua a do
/// resultado inteiro.
#[test]
fn a_query_result_comes_in_pages_by_default() {
    let file = settings_file("default");
    let mut h = harness(&file);
    let doc = run_query(&mut h);
    assert_eq!(grid(&h, doc).shown, 0..500);
    h.get_by_label("1–500 of 1234");
    h.get_by_label("1234 rows");

    page_button(&h, icons::CARET_RIGHT).click();
    h.run();
    assert_eq!(grid(&h, doc).shown, 500..1000);
    h.get_by_label("501–1000 of 1234");
    page_button(&h, icons::CARET_DOUBLE_RIGHT).click();
    h.run();
    assert_eq!(grid(&h, doc).shown, 1000..1234);

    // O canto seleciona a página à mostra e o Ctrl+C copia só ela.
    let p = corner(&h, doc);
    click(&mut h, p);
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    let text = copied(&h, doc).expect("copied");
    assert_eq!(text.lines().count(), 234);
    assert_eq!(text.lines().next(), Some("1001\tCliente 1001\tNULL"));
    h.run();
    h.get_by_label("234 rows copied as TSV");
}

/// O tamanho da página e o formato de cópia escolhidos na faixa valem na
/// hora, ficam gravados e voltam ao abrir o app de novo.
#[test]
fn the_choices_below_the_grid_are_saved_and_come_back() {
    let file = settings_file("saved");
    {
        let mut h = harness(&file);
        let doc = run_query(&mut h);
        // Linhas por página: 100.
        h.get_by_value("500").click();
        h.run();
        h.get_by_label("100").click();
        h.run();
        assert_eq!(grid(&h, doc).shown, 0..100);
        h.get_by_label("1–100 of 1234");
        assert_eq!(h.state().preferences().results.page_size, 100);

        // Formato de cópia: CSV com ponto e vírgula.
        h.get_by_value("TSV").click();
        h.run();
        h.get_by_label("CSV (semicolon)").click();
        h.run();
        assert_eq!(
            h.state().preferences().results.copy_format,
            CopyFormat::CsvSemicolon
        );
        let p = grid(&h, doc)
            .cell_center(pgstudio_result_grid::Cell { row: 1, col: 0 })
            .unwrap();
        click(&mut h, p);
        h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
        h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::C);
        h.run();
        assert_eq!(
            copied(&h, doc).as_deref(),
            Some("2;Cliente 2\n3;Cliente 3\n")
        );
        h.get_by_label("2 rows copied as CSV ;");
    }
    // O arquivo guarda as escolhas...
    let (_, prefs) = PreferencesStore::open(&file);
    assert_eq!(prefs.results.page_size, 100);
    assert!(prefs.results.paginate);
    assert_eq!(prefs.results.copy_format, CopyFormat::CsvSemicolon);
    // ...e o app reaberto já vem com elas.
    let mut h = harness(&file);
    let doc = run_query(&mut h);
    assert_eq!(grid(&h, doc).shown, 0..100);
    h.get_by_value("CSV ;");

    // "Todas as linhas" desliga a paginação (e também fica gravado).
    h.get_by_value("100").click();
    h.run();
    h.get_by_label("All rows").click();
    h.run();
    assert_eq!(grid(&h, doc).shown, 0..ROWS);
    assert!(!h.state().preferences().results.paginate);
    assert!(!PreferencesStore::open(&file).1.results.paginate);
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// Tools > Options > Query Results > Results to Grid escolhe o padrão.
#[test]
fn the_options_page_sets_the_default() {
    let file = settings_file("options");
    let mut h = harness(&file);
    let doc = run_query(&mut h);
    assert_eq!(grid(&h, doc).shown, 0..500);
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::Options);
    h.state_mut().options_mut().unwrap().page = Page::ResultsGrid;
    h.run();
    h.get_by_label("Show results in pages").click();
    h.run();
    h.get_by_label("OK").click();
    h.run();
    assert_eq!(grid(&h, doc).shown, 0..ROWS);
    let (_, prefs) = PreferencesStore::open(&file);
    assert_eq!(
        prefs,
        Preferences {
            results: pgstudio_ui_shell::preferences::ResultsPrefs {
                paginate: false,
                ..Default::default()
            },
            ..Preferences::default()
        }
    );
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}
