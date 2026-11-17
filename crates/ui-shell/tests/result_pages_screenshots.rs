//! Capturas da grade de resultados em páginas e do formato de cópia,
//! desenhadas sem janela. Só com `--features screenshots`; os PNGs vão
//! para `PGSTUDIO_SHOTS` (ou a pasta temporária). `PGSTUDIO_SHOT_SIZE=LxA`
//! muda o tamanho da janela e `PGSTUDIO_SHOT_THEME=light` usa o tema claro.
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_phosphor::regular as icons;
use pgstudio_core::CommandId;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{Pace, QueryEvent, QueryService, QuerySession, QuerySink};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_ui_shell::ShellApp;
use pgstudio_ui_shell::options::Page;

const ROWS: u64 = 12_340;

/// `true`: a sessão sabe buscar o resultado uma página por vez.
struct FakeQueries(bool);

impl QueryService for FakeQueries {
    fn open(&self, _: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession(self.0))
    }
}

struct FakeSession(bool);

/// Espera a linha estar liberada (a thread dorme até ser acordada).
fn allowed(pace: &Pace) -> bool {
    use std::future::Future;
    use std::task::{Context, Poll, Wake, Waker};
    struct Park(std::thread::Thread);
    impl Wake for Park {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Park(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut row = std::pin::pin!(pace.row());
    loop {
        if let Poll::Ready(go) = row.as_mut().poll(&mut cx) {
            return go;
        }
        std::thread::park();
    }
}

impl FakeSession {
    fn start(mut sink: QuerySink, pace: Option<Pace>) {
        std::thread::spawn(move || {
            sink(QueryEvent::Columns(
                ["id", "cliente", "cidade", "status", "total", "criado_em"]
                    .map(String::from)
                    .to_vec(),
            ));
            let cities = ["São Paulo", "Curitiba", "Recife", "Porto Alegre", "Belém"];
            let names = ["Ana Souza", "Bruno Lima", "Carla Dias", "Diego Reis"];
            for i in 1..=ROWS {
                if let Some(pace) = &pace
                    && !allowed(pace)
                {
                    sink(QueryEvent::Finished { canceled: true });
                    return;
                }
                sink(QueryEvent::Row(vec![
                    Some(i.to_string()),
                    Some(format!("{} {}", names[(i % 4) as usize], i % 97)),
                    (i % 11 != 0).then(|| cities[(i % 5) as usize].to_string()),
                    Some(["aberto", "pago", "enviado"][(i % 3) as usize].into()),
                    Some(format!("{}.{:02}", 40 + i * 7 % 900, i % 100)),
                    Some(format!(
                        "2026-11-{:02} {:02}:{:02}:00",
                        1 + i % 28,
                        i % 24,
                        i % 60
                    )),
                ]));
            }
            sink(QueryEvent::CommandComplete { rows: ROWS });
            sink(QueryEvent::Finished { canceled: false });
        });
    }
}

impl QuerySession for FakeSession {
    fn execute(&self, _: String, sink: QuerySink) {
        Self::start(sink, None);
    }

    fn execute_paced(&self, sql: String, sink: QuerySink, pace: Pace) {
        if self.0 {
            Self::start(sink, Some(pace));
        } else {
            self.execute(sql, sink);
        }
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        "loja".into()
    }
}

struct FakeConnector(bool);

impl Connector for FakeConnector {
    fn connect(&self, s: &ConnectionSettings, _: &str) -> Result<Connected, ConnectError> {
        Ok(Connected {
            settings: s.clone(),
            server_version: "17.2".into(),
            metadata: Arc::new(FakeProvider::new(Duration::ZERO)),
            queries: Arc::new(FakeQueries(self.0)),
        })
    }
}

fn window() -> [f32; 2] {
    std::env::var("PGSTUDIO_SHOT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1360.0, 960.0])
}

fn save(h: &mut Harness<'static, ShellApp>, name: &str) {
    let out = std::env::var_os("PGSTUDIO_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&out).unwrap();
    h.render()
        .expect("render")
        .save(out.join(name))
        .expect("save png");
}

fn click(h: &mut Harness<'static, ShellApp>, pos: egui::Pos2) {
    h.run_steps(30);
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    h.run_steps(5);
}

#[test]
fn screenshots_of_pages_and_copy_formats() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(false)));
    app.connect_to(
        ConnectionSettings {
            host: "homologacao.loja.com.br".into(),
            user: "app".into(),
            database: "loja".into(),
            ..Default::default()
        },
        "x".into(),
    );
    let mut h = Harness::builder()
        .with_size(window())
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    let theme = match std::env::var("PGSTUDIO_SHOT_THEME").as_deref() {
        Ok("light") => pgstudio_core::Theme::Light,
        _ => pgstudio_core::Theme::Dark,
    };
    pgstudio_ui_shell::theme::apply(&h.ctx, theme);
    let start = Instant::now();
    while h.state().connection().is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(5);
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run_steps(3);
    h.event(Event::Text(
        "select id, cliente, cidade, status, total, criado_em\nfrom pedidos".into(),
    ));
    h.run_steps(3);
    let doc = h.state().active_document().unwrap();
    h.key_press(Key::F5);
    let start = Instant::now();
    loop {
        h.step();
        let done = h
            .state()
            .documents()
            .get(doc)
            .and_then(|d| d.execution.as_ref())
            .is_some_and(|e| !e.is_running() && e.results.first().is_some_and(|r| r.len() == ROWS));
        if done && !h.state().is_loading_catalog() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(5);
    let grid = |h: &Harness<'static, ShellApp>| {
        h.state()
            .documents()
            .get(doc)
            .unwrap()
            .execution
            .as_ref()
            .unwrap()
            .results[0]
            .grid
            .clone()
    };

    // 1: a segunda página, com tudo selecionado pelo canto e copiado.
    let next = h
        .get_all_by_label(icons::CARET_RIGHT)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .rect()
        .center();
    click(&mut h, next);
    let l = grid(&h).layout.unwrap();
    click(
        &mut h,
        egui::pos2(l.header.left() + 4.0, l.header.center().y),
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run_steps(5);
    save(&mut h, "1-paginas.png");

    // 2: o seletor do formato de cópia aberto, com uma coluna selecionada.
    let header = grid(&h).header_center(1).unwrap();
    h.event(Event::PointerMoved(header));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: header,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::SHIFT,
        });
    }
    h.run_steps(5);
    h.get_by_value("TSV").click();
    h.run_steps(5);
    save(&mut h, "2-formato-de-copia.png");
    h.key_press(Key::Escape);
    h.run_steps(5);

    // 3: o seletor de linhas por página.
    h.get_by_value("500").click();
    h.run_steps(5);
    save(&mut h, "3-linhas-por-pagina.png");
    h.key_press(Key::Escape);
    h.run_steps(5);

    // 4: a página das opções.
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::Options);
    h.state_mut().options_mut().unwrap().page = Page::ResultsGrid;
    h.run_steps(8);
    save(&mut h, "4-opcoes.png");
}

/// O resultado vindo do servidor uma página por vez: a primeira página com
/// o total "500+", e a página das opções.
#[test]
fn screenshots_of_fetching_only_the_page() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector(true)));
    app.connect_to(
        ConnectionSettings {
            host: "homologacao.loja.com.br".into(),
            user: "app".into(),
            database: "loja".into(),
            ..Default::default()
        },
        "x".into(),
    );
    let mut h = Harness::builder()
        .with_size(window())
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(|ui, app: &mut ShellApp| app.show(ui), app);
    pgstudio_ui_shell::install_fonts(&h.ctx);
    let theme = match std::env::var("PGSTUDIO_SHOT_THEME").as_deref() {
        Ok("light") => pgstudio_core::Theme::Light,
        _ => pgstudio_core::Theme::Dark,
    };
    pgstudio_ui_shell::theme::apply(&h.ctx, theme);
    let start = Instant::now();
    while h.state().connection().is_none() {
        h.step();
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(5);
    h.key_press_modifiers(Modifiers::CTRL, Key::N);
    h.run_steps(3);
    h.event(Event::Text(
        "select id, cliente, cidade, status, total, criado_em\nfrom pedidos".into(),
    ));
    h.run_steps(3);
    let doc = h.state().active_document().unwrap();
    h.key_press(Key::F5);
    let rows = |h: &Harness<'static, ShellApp>| {
        h.state()
            .documents()
            .get(doc)
            .and_then(|d| d.execution.as_ref())
            .filter(|e| !e.is_running())
            .and_then(|e| e.results.first().map(|r| r.len()))
    };
    let wait = |h: &mut Harness<'static, ShellApp>, want: u64| {
        let start = Instant::now();
        loop {
            h.step();
            if rows(h) == Some(want) && !h.state().is_loading_catalog() {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(2));
        }
        h.run_steps(5);
    };
    // 5: só a primeira página veio; o resto está no servidor.
    wait(&mut h, 500);
    save(&mut h, "5-so-a-primeira-pagina.png");

    // 6: a página seguinte foi buscada.
    let next = h
        .get_all_by_label(icons::CARET_RIGHT)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .rect()
        .center();
    click(&mut h, next);
    wait(&mut h, 1000);
    save(&mut h, "6-pagina-seguinte.png");

    // 7: as opções.
    let ctx = h.ctx.clone();
    h.state_mut().execute(&ctx, CommandId::Options);
    h.state_mut().options_mut().unwrap().page = Page::ResultsGrid;
    h.run_steps(8);
    save(&mut h, "7-opcoes-busca.png");
}
