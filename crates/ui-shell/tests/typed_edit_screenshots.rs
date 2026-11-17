//! Capturas dos editores por tipo na grade de resultados (calendário das
//! datas, lista dos booleanos e dos enums, e a confirmação), desenhadas
//! sem janela. Só com `--features screenshots`; os PNGs vão para
//! `PGSTUDIO_SHOTS` (ou a pasta temporária). `PGSTUDIO_SHOT_SIZE=LxA` muda
//! o tamanho da janela e `PGSTUDIO_SHOT_THEME=light` usa o tema claro.
#![cfg(feature = "screenshots")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pgstudio_core::connection::{ConnectError, Connected, ConnectionSettings, Connector};
use pgstudio_core::query::{
    ColumnSource, DescribeDone, QueryEvent, QueryService, QuerySession, QuerySink,
};
use pgstudio_object_explorer::fake::FakeProvider;
use pgstudio_result_grid::{Cell, GridState};
use pgstudio_ui_shell::ShellApp;

struct FakeQueries;

impl QueryService for FakeQueries {
    fn open(&self, _: &str) -> Arc<dyn QuerySession> {
        Arc::new(FakeSession)
    }
}

struct FakeSession;

/// `nome`, tipo, aceita NULL e valores do enum de cada coluna de `pedidos`.
const COLUMNS: [(&str, &str, bool, &str); 7] = [
    ("id", "integer", false, ""),
    ("cliente", "text", false, ""),
    ("pago", "boolean", true, ""),
    (
        "status",
        "pedido_status",
        false,
        "aberto\u{1f}pago\u{1f}enviado\u{1f}cancelado",
    ),
    ("criado_em", "timestamp with time zone", false, ""),
    ("entrega", "date", true, ""),
    ("total", "numeric(10,2)", false, ""),
];

impl QuerySession for FakeSession {
    fn execute(&self, sql: String, mut sink: QuerySink) {
        std::thread::spawn(move || {
            let text = |v: &str| Some(v.to_string());
            if sql.contains("pg_attribute") {
                // O catálogo da tabela, como `result_edit::table_sql` pede.
                sink(QueryEvent::Columns(vec!["c".into(); 11]));
                for (i, (name, ty, nullable, labels)) in COLUMNS.iter().enumerate() {
                    sink(QueryEvent::Row(vec![
                        text(&(i + 1).to_string()),
                        text(name),
                        text(if i == 0 { "t" } else { "f" }),
                        text("f"),
                        text("public.pedidos"),
                        text("public.pedidos"),
                        text("r"),
                        text("t"),
                        text(ty),
                        text(if *nullable { "t" } else { "f" }),
                        Some(labels.to_string()).filter(|l| !l.is_empty()),
                    ]));
                }
            } else {
                sink(QueryEvent::Columns(
                    COLUMNS.iter().map(|c| c.0.to_string()).collect(),
                ));
                let names = ["Ana Souza", "Bruno Lima", "Carla Dias", "Diego Reis"];
                let status = ["aberto", "pago", "enviado", "cancelado"];
                for i in 1..=14u32 {
                    sink(QueryEvent::Row(vec![
                        text(&(1000 + i).to_string()),
                        text(names[(i % 4) as usize]),
                        (i % 5 != 0).then(|| if i % 2 == 0 { "t" } else { "f" }.to_string()),
                        text(status[(i % 4) as usize]),
                        text(&format!(
                            "2026-11-{:02} {:02}:46:35.187386+00",
                            11 + i,
                            9 + i % 12
                        )),
                        (i % 3 != 0).then(|| format!("2026-12-{i:02}")),
                        text(&format!("{}.{:02}", 40 + i * 37 % 900, i * 7 % 100)),
                    ]));
                }
                sink(QueryEvent::CommandComplete { rows: 14 });
            }
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {}

    fn database(&self) -> String {
        "loja".into()
    }

    fn describe(&self, _: String, done: DescribeDone) {
        done(Ok((1..=COLUMNS.len() as i16)
            .map(|column| Some(ColumnSource { table: 1, column }))
            .collect()));
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

fn press(h: &mut Harness<'static, ShellApp>, pos: egui::Pos2) {
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

fn click(h: &mut Harness<'static, ShellApp>, pos: egui::Pos2) {
    h.run_steps(30);
    h.event(Event::PointerMoved(pos));
    h.step();
    press(h, pos);
    h.run_steps(5);
}

fn double_click(h: &mut Harness<'static, ShellApp>, pos: egui::Pos2) {
    h.run_steps(30);
    h.event(Event::PointerMoved(pos));
    h.step();
    press(h, pos);
    h.step();
    press(h, pos);
    h.run_steps(6);
}

#[test]
fn screenshots_of_the_typed_editors() {
    pgstudio_i18n::set_language(pgstudio_i18n::Language::PortugueseBrazil);
    let mut app = ShellApp::default().with_connector(Arc::new(FakeConnector));
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
        "select id, cliente, pago, status, criado_em, entrega, total\nfrom pedidos".into(),
    ));
    h.run_steps(3);
    let doc = h.state().active_document().unwrap();
    h.key_press(Key::F5);
    let grid = |h: &Harness<'static, ShellApp>| -> Option<GridState> {
        let d = h.state().documents().get(doc)?;
        let e = d.execution.as_ref()?;
        Some(e.results.first()?.grid.clone())
    };
    let start = Instant::now();
    loop {
        h.step();
        let ready = grid(&h).is_some_and(|g| g.kinds.len() == COLUMNS.len());
        if ready && !h.state().is_loading_catalog() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "never editable");
        std::thread::sleep(Duration::from_millis(2));
    }
    h.run_steps(5);
    let cell = |h: &Harness<'static, ShellApp>, row, col| {
        grid(h).unwrap().cell_center(Cell { row, col }).unwrap()
    };

    // 1: data e hora com fuso; outro dia escolhido no calendário.
    let p = cell(&h, 1, 4);
    double_click(&mut h, p);
    let day = h.get_by_label("2026-11-27").rect().center();
    click(&mut h, day);
    save(&mut h, "1-data-e-hora.png");
    h.key_press(Key::Escape);
    h.run_steps(5);

    // 2: só data, numa célula NULL (o calendário abre em hoje).
    let p = cell(&h, 2, 5);
    double_click(&mut h, p);
    save(&mut h, "2-data.png");
    h.key_press(Key::Escape);
    h.run_steps(5);

    // 3: booleano, com NULL porque a coluna aceita.
    let p = cell(&h, 1, 2);
    double_click(&mut h, p);
    save(&mut h, "3-booleano.png");

    // 4: escolher pede a confirmação de sempre.
    let no = h.get_by_label("false").rect().center();
    click(&mut h, no);
    save(&mut h, "4-confirmacao.png");
    h.key_press(Key::Escape);
    h.run_steps(5);

    // 5: enum.
    let p = cell(&h, 2, 3);
    double_click(&mut h, p);
    h.key_press(Key::ArrowDown);
    h.run_steps(3);
    save(&mut h, "5-enum.png");
}
