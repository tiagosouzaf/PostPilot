//! O resultado vem do servidor uma página por vez: o F5 de um SELECT traz
//! só a primeira página, a faixa embaixo da grade busca as seguintes, e a
//! consulta que fica aberta no servidor é encerrada quando a janela roda
//! outra coisa ou quando espera demais.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_phosphor::regular as icons;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{Pace, QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_result_grid::{GridPrefs, GridState, prefs};
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::execution::Execution;

const ROWS: u64 = 1234;

/// O que o "servidor" viu.
#[derive(Default)]
struct Server {
    /// Linhas mandadas pela última execução.
    sent: AtomicU64,
    /// Execuções que buscaram aos poucos e as que buscaram tudo de uma vez.
    paced: AtomicU64,
    whole: AtomicU64,
    /// Uma execução parada foi encerrada sem trazer o resto.
    abandoned: AtomicBool,
}

struct FakeQueries(Arc<Server>);

impl QueryService for FakeQueries {
    fn open(&self, _: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession {
            server: self.0.clone(),
            current: Mutex::default(),
        })
    }
}

/// Toda consulta devolve [`ROWS`] pedidos; como as sessões de verdade, a
/// execução seguinte faz a que está parada desistir.
struct FakeSession {
    server: Arc<Server>,
    current: Mutex<Option<Pace>>,
}

fn block_on<F: Future>(f: F) -> F::Output {
    struct Park(std::thread::Thread);
    impl Wake for Park {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Park(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

impl FakeSession {
    fn start(&self, mut sink: QuerySink, pace: Option<Pace>) {
        if let Some(old) = self.current.lock().unwrap().take()
            && old.is_waiting()
        {
            old.abandon();
        }
        *self.current.lock().unwrap() = pace.clone();
        let server = self.server.clone();
        server.sent.store(0, Ordering::SeqCst);
        match pace {
            Some(_) => server.paced.fetch_add(1, Ordering::SeqCst),
            None => server.whole.fetch_add(1, Ordering::SeqCst),
        };
        std::thread::spawn(move || {
            sink(QueryEvent::Columns(vec!["id".into(), "cliente".into()]));
            for i in 1..=ROWS {
                if let Some(pace) = &pace
                    && !block_on(pace.row())
                {
                    server.abandoned.store(true, Ordering::SeqCst);
                    sink(QueryEvent::Finished { canceled: true });
                    return;
                }
                sink(QueryEvent::Row(vec![
                    Some(i.to_string()),
                    Some(format!("Cliente {i}")),
                ]));
                server.sent.fetch_add(1, Ordering::SeqCst);
            }
            sink(QueryEvent::CommandComplete { rows: ROWS });
            sink(QueryEvent::Finished { canceled: false });
        });
    }
}

impl QuerySession for FakeSession {
    fn execute(&self, _: String, sink: QuerySink) {
        self.start(sink, None);
    }

    fn execute_paced(&self, _: String, sink: QuerySink, pace: Pace) {
        self.start(sink, Some(pace));
    }

    fn cancel(&self) {
        if let Some(pace) = self.current.lock().unwrap().as_ref() {
            pace.abandon();
        }
    }

    fn database(&self) -> String {
        "loja".into()
    }
}

struct FakeConnector(Arc<Server>);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries(self.0.clone())),
        })
    }
}

fn harness() -> (Harness<'static, ShellApp>, Arc<Server>) {
    let server = Arc::new(Server::default());
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(server.clone())));
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
    wait(&mut harness, |h| h.state().connection().is_some());
    (harness, server)
}

fn wait(
    harness: &mut Harness<'static, ShellApp>,
    done: impl Fn(&Harness<'static, ShellApp>) -> bool,
) {
    let start = Instant::now();
    loop {
        harness.step();
        if done(harness) && !harness.state().is_loading_catalog() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "never happened");
        std::thread::sleep(Duration::from_millis(2));
    }
    harness.run();
}

fn exec<'a>(harness: &'a Harness<'static, ShellApp>, doc: u32) -> &'a Execution {
    harness
        .state()
        .documents()
        .get(doc)
        .unwrap()
        .execution
        .as_ref()
        .unwrap()
}

fn grid<'a>(harness: &'a Harness<'static, ShellApp>, doc: u32) -> &'a GridState {
    &exec(harness, doc).results[0].grid
}

fn rows(harness: &Harness<'static, ShellApp>, doc: u32) -> u64 {
    exec(harness, doc).results.first().map_or(0, |r| r.len())
}

/// Abre uma janela com `sql`, executa e espera a sessão parar ou acabar.
fn run_query(harness: &mut Harness<'static, ShellApp>, sql: &str) -> u32 {
    harness.key_press_modifiers(Modifiers::CTRL, Key::N);
    harness.run();
    harness.event(Event::Text(sql.into()));
    harness.run();
    let id = harness.state().active_document().unwrap();
    press_f5(harness, id);
    id
}

fn press_f5(harness: &mut Harness<'static, ShellApp>, id: u32) {
    harness.key_press(Key::F5);
    harness.step();
    wait(harness, |h| {
        h.state()
            .documents()
            .get(id)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running())
    });
}

fn page_button<'a>(
    harness: &'a Harness<'static, ShellApp>,
    icon: &'a str,
) -> egui_kittest::Node<'a> {
    harness
        .get_all_by_label(icon)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .expect("page button")
}

/// O F5 de um SELECT traz só a primeira página; a faixa busca as outras.
#[test]
fn a_select_brings_the_first_page_and_the_bar_fetches_the_rest() {
    let (mut h, server) = harness();
    let doc = run_query(&mut h, "select * from pedidos");
    // Só a primeira página saiu do servidor, e a execução já conta como
    // pronta: a janela está livre e o histórico tem a consulta.
    assert_eq!(server.sent.load(Ordering::SeqCst), 500);
    assert_eq!(rows(&h, doc), 500);
    assert!(exec(&h, doc).has_more());
    assert!(!exec(&h, doc).is_running());
    assert_eq!(h.state().status(), "Query executed successfully.");
    h.get_by_label("1–500 of 500+");
    h.get_by_label("500+ rows");
    assert_eq!(h.state().history().store.len(), 1);

    // A página seguinte busca mais 500 e vai para ela.
    page_button(&h, icons::CARET_RIGHT).click();
    h.step();
    wait(&mut h, |h| {
        rows(h, doc) == 1000 && !exec(h, doc).is_fetching()
    });
    assert_eq!(server.sent.load(Ordering::SeqCst), 1000);
    assert_eq!(grid(&h, doc).shown, 500..1000);
    h.get_by_label("501–1000 of 1000+");
    // Voltar não busca nada.
    page_button(&h, icons::CARET_LEFT).click();
    h.run();
    assert_eq!(grid(&h, doc).shown, 0..500);
    assert_eq!(server.sent.load(Ordering::SeqCst), 1000);

    // A última página busca tudo, e a execução termina de verdade.
    page_button(&h, icons::CARET_DOUBLE_RIGHT).click();
    h.step();
    wait(&mut h, |h| !exec(h, doc).has_more());
    assert_eq!(rows(&h, doc), ROWS);
    assert_eq!(grid(&h, doc).shown, 1000..ROWS);
    h.get_by_label("1001–1234 of 1234");
    h.get_by_label("1234 rows");
    assert!(!server.abandoned.load(Ordering::SeqCst));
    // Um F5, uma linha no histórico.
    assert_eq!(h.state().history().store.len(), 1);
}

/// Rodar outra coisa na janela encerra a consulta que ficou aberta.
#[test]
fn running_again_closes_the_query_left_open() {
    let (mut h, server) = harness();
    let doc = run_query(&mut h, "select * from pedidos");
    assert!(exec(&h, doc).has_more());
    press_f5(&mut h, doc);
    assert!(server.abandoned.load(Ordering::SeqCst));
    assert_eq!(server.paced.load(Ordering::SeqCst), 2);
    assert_eq!(rows(&h, doc), 500);
    assert!(exec(&h, doc).has_more());
}

/// A consulta parada no servidor é encerrada depois do limite, e o que já
/// veio continua na grade.
#[test]
fn a_query_left_waiting_is_closed_after_the_limit() {
    let (mut h, server) = harness();
    let doc = run_query(&mut h, "select * from pedidos");
    h.state_mut()
        .documents_mut()
        .get_mut(doc)
        .unwrap()
        .execution
        .as_mut()
        .unwrap()
        .set_idle_limit(Duration::from_millis(30));
    wait(&mut h, |h| {
        !exec(h, doc).has_more() && !exec(h, doc).is_running()
    });
    assert!(server.abandoned.load(Ordering::SeqCst));
    let e = exec(&h, doc);
    assert_eq!(rows(&h, doc), 500);
    assert_eq!(
        e.outcome(),
        pgstudio_ui_shell::execution::Outcome::Succeeded
    );
    assert!(
        e.messages
            .iter()
            .any(|m| m.text.contains("was closed on the server")),
        "{:?}",
        e.messages
    );
    h.get_by_label("1–500 of 500");
    h.get_by_label("500 rows");
}

/// O Cancel (Alt+Break) com a consulta parada encerra no servidor e fica
/// com as linhas que já vieram, que passam a poder ser usadas como um
/// resultado terminado.
#[test]
fn cancel_closes_the_query_left_open_and_keeps_the_rows() {
    let (mut h, server) = harness();
    let doc = run_query(&mut h, "select * from pedidos");
    let ctx = h.ctx.clone();
    h.state_mut()
        .execute(&ctx, pgstudio_core::CommandId::CancelQuery);
    wait(&mut h, |h| {
        !exec(h, doc).has_more() && !exec(h, doc).is_running()
    });
    assert!(server.abandoned.load(Ordering::SeqCst));
    assert_eq!(rows(&h, doc), 500);
    assert_eq!(
        exec(&h, doc).outcome(),
        pgstudio_ui_shell::execution::Outcome::Succeeded
    );
    h.get_by_label("1–500 of 500");
}

/// Sem páginas ("All rows") não há o que esperar: vem tudo. Um script e
/// um comando que altera dados também rodam inteiros.
#[test]
fn all_rows_scripts_and_changes_fetch_everything() {
    let (mut h, server) = harness();
    let doc = run_query(&mut h, "select * from pedidos");
    assert_eq!(rows(&h, doc), 500);
    // Página maior: busca o que falta para enchê-la.
    prefs::set(
        &h.ctx,
        GridPrefs {
            page_size: 1000,
            ..GridPrefs::default()
        },
    );
    h.step();
    wait(&mut h, |h| {
        rows(h, doc) == 1000 && !exec(h, doc).is_fetching()
    });
    assert!(exec(&h, doc).has_more());
    // Sem páginas: o resto todo.
    prefs::set(
        &h.ctx,
        GridPrefs {
            paginate: false,
            ..GridPrefs::default()
        },
    );
    h.step();
    wait(&mut h, |h| !exec(h, doc).has_more());
    assert_eq!(rows(&h, doc), ROWS);

    // Com as páginas desligadas, a execução nem busca aos poucos.
    let before = server.paced.load(Ordering::SeqCst);
    press_f5(&mut h, doc);
    assert_eq!(rows(&h, doc), ROWS);
    assert_eq!(server.paced.load(Ordering::SeqCst), before);

    prefs::set(&h.ctx, GridPrefs::default());
    h.run();
    for sql in [
        "select 1; select * from pedidos",
        "delete from pedidos returning *",
    ] {
        let doc = run_query(&mut h, sql);
        assert_eq!(rows(&h, doc), ROWS, "{sql}");
        assert!(!exec(&h, doc).has_more());
    }
    assert_eq!(server.paced.load(Ordering::SeqCst), before);

    // Buscar só a página desligado nas opções: vem tudo, em páginas na tela.
    prefs::set(
        &h.ctx,
        GridPrefs {
            fetch_on_demand: false,
            ..GridPrefs::default()
        },
    );
    h.run();
    let doc = run_query(&mut h, "select * from pedidos");
    assert_eq!(rows(&h, doc), ROWS);
    assert_eq!(grid(&h, doc).shown, 0..500);
    assert_eq!(server.paced.load(Ordering::SeqCst), before);
}
