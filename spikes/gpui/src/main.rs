//! Spike da Fase 0: protótipo SSMS em GPUI.
//!
//! GPUI não traz campo de texto editável, docking nem barra de menus no Linux e
//! no Windows (o `set_menus` só vira menu nativo no macOS). Este protótipo
//! desenha menus e painéis fixos à mão, mostra o SQL como texto somente
//! leitura com realce, e mede o mesmo que os outros spikes (`SPIKE_BENCH=1`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    App, Application, Bounds, Context, FontWeight, HighlightStyle, Hsla, SharedString, StyledText,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
    uniform_list,
};

const GRID_ROWS: usize = 1_000_000;
const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "ORDER", "BY", "LIMIT", "AS", "JOIN", "ON", "AND", "OR",
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NodeId {
    Server,
    Database(usize),
    Schema(usize, usize),
    Tables(usize, usize),
    Table(usize, usize, usize),
}

impl NodeId {
    fn label(self) -> String {
        match self {
            NodeId::Server => "localhost (PostgreSQL 17)".into(),
            NodeId::Database(d) => format!("db_{d:02}"),
            NodeId::Schema(_, s) => format!("schema_{s:02}"),
            NodeId::Tables(..) => "Tables".into(),
            NodeId::Table(_, _, t) => format!("tabela_{t:03}"),
        }
    }
    fn has_children(self) -> bool {
        !matches!(self, NodeId::Table(..))
    }
}

fn fake_children(id: NodeId) -> Vec<NodeId> {
    match id {
        NodeId::Server => (0..10).map(NodeId::Database).collect(),
        NodeId::Database(d) => (0..10).map(|s| NodeId::Schema(d, s)).collect(),
        NodeId::Schema(d, s) => vec![NodeId::Tables(d, s)],
        NodeId::Tables(d, s) => (0..100).map(|t| NodeId::Table(d, s, t)).collect(),
        NodeId::Table(..) => vec![],
    }
}

struct Bench {
    started: Instant,
    first_frame_ms: Option<f64>,
    step: usize,
    intervals: Vec<f64>,
    last: Option<Instant>,
    idle_secs: u64,
}

struct Spike {
    expanded: HashMap<NodeId, bool>,
    children: HashMap<NodeId, Option<Vec<NodeId>>>,
    rows: Vec<(NodeId, usize, bool)>, // id, profundidade, carregando
    sql: Vec<SharedString>,
    grid_scroll: UniformListScrollHandle,
    status: SharedString,
    bench: Option<Bench>,
}

impl Spike {
    fn new(started: Instant) -> Self {
        let lines = std::env::var("SPIKE_SQL_LINES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5_000);
        let sql = (0..lines)
            .map(|n| match n % 4 {
                0 => format!("-- consulta {n}"),
                1 => "SELECT t.id, t.nome, 'texto' AS rotulo".to_string(),
                2 => format!("FROM schema_01.tabela_{:03} t WHERE t.id > {n}", n % 100),
                _ => "ORDER BY t.id LIMIT 100;".to_string(),
            })
            .map(SharedString::from)
            .collect();
        let bench = std::env::var("SPIKE_BENCH").ok().map(|_| Bench {
            started,
            first_frame_ms: None,
            step: 0,
            intervals: vec![],
            last: None,
            idle_secs: std::env::var("SPIKE_IDLE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
        });
        let mut s = Self {
            expanded: HashMap::new(),
            children: HashMap::new(),
            rows: vec![],
            sql,
            grid_scroll: UniformListScrollHandle::new(),
            status: "Pronto".into(),
            bench,
        };
        s.flatten();
        s
    }

    fn flatten(&mut self) {
        let mut rows = vec![];
        self.walk(NodeId::Server, 0, &mut rows);
        self.rows = rows;
    }

    fn walk(&self, id: NodeId, depth: usize, out: &mut Vec<(NodeId, usize, bool)>) {
        let open = self.expanded.get(&id).copied().unwrap_or(false);
        let loading = open && matches!(self.children.get(&id), Some(None));
        out.push((id, depth, loading));
        if open && let Some(Some(kids)) = self.children.get(&id) {
            for k in kids {
                self.walk(*k, depth + 1, out);
            }
        }
    }

    fn toggle(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let open = !self.expanded.get(&id).copied().unwrap_or(false);
        self.expanded.insert(id, open);
        if open && !self.children.contains_key(&id) {
            self.children.insert(id, None);
            // provedor falso: atraso de 120 ms numa task de fundo
            let task = cx.background_executor().spawn(async move {
                std::thread::sleep(Duration::from_millis(120));
                fake_children(id)
            });
            cx.spawn(async move |this, cx| {
                let kids = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.children.insert(id, Some(kids));
                    this.flatten();
                    cx.notify();
                });
            })
            .detach();
        }
        self.flatten();
        cx.notify();
    }

    fn expand_all(&mut self, cx: &mut Context<Self>) {
        let mut queue = vec![NodeId::Server];
        while let Some(id) = queue.pop() {
            if !id.has_children() {
                continue;
            }
            if !self.expanded.get(&id).copied().unwrap_or(false) {
                self.toggle(id, cx);
            }
            if let Some(Some(kids)) = self.children.get(&id) {
                queue.extend(
                    kids.iter()
                        .copied()
                        .filter(|k| !matches!(k, NodeId::Table(..))),
                );
            }
        }
    }

    fn bench_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = &mut self.bench else { return };
        if b.first_frame_ms.is_none() {
            b.first_frame_ms = Some(b.started.elapsed().as_secs_f64() * 1000.0);
        }
        b.step += 1;
        let step = b.step;
        if step < 60 {
            // ~1 s para a árvore carregar
            self.expand_all(cx);
            window.request_animation_frame();
        } else if step < 360 {
            if let Some(t) = b.last {
                b.intervals.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            b.last = Some(Instant::now());
            self.grid_scroll
                .scroll_to_item_strict((step - 60) * 3_000, gpui::ScrollStrategy::Top);
            window.request_animation_frame();
        } else if step == 360 {
            let mut iv = b.intervals.clone();
            iv.sort_by(|a, b| a.total_cmp(b));
            let p = |q: f64| {
                iv.get(((iv.len() as f64 - 1.0) * q) as usize)
                    .copied()
                    .unwrap_or(f64::NAN)
            };
            println!("toolkit=gpui");
            println!("first_frame_ms={:.1}", b.first_frame_ms.unwrap_or(f64::NAN));
            println!("frame_interval_ms_p50={:.2}", p(0.5));
            println!("frame_interval_ms_p95={:.2}", p(0.95));
            let idle = Duration::from_secs(b.idle_secs);
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(idle).await;
                println!(
                    "idle_rss_mb={}",
                    rss_mb().map_or("n/a".into(), |m| format!("{m:.1}"))
                );
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        }
    }
}

#[cfg(not(windows))]
fn rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// Working set do processo no Windows (equivalente ao RSS).
#[cfg(windows)]
#[allow(unsafe_code)]
fn rss_mb() -> Option<f64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: handle do próprio processo e struct com o tamanho informado em `cb`.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then(|| c.WorkingSetSize as f64 / (1024.0 * 1024.0))
}

fn highlight(line: &SharedString) -> StyledText {
    let kw = HighlightStyle {
        color: Some(Hsla::from(rgb(0x569cd6))),
        ..Default::default()
    };
    let st = HighlightStyle {
        color: Some(Hsla::from(rgb(0xce9178))),
        ..Default::default()
    };
    let cm = HighlightStyle {
        color: Some(Hsla::from(rgb(0x57a64a))),
        ..Default::default()
    };
    let mut runs = vec![];
    let s = line.as_ref();
    if s.starts_with("--") {
        runs.push((0..s.len(), cm));
    } else {
        let b = s.as_bytes();
        let mut i = 0;
        while i < s.len() {
            let start = i;
            if b[i] == b'\'' {
                i += 1;
                while i < s.len() && b[i] != b'\'' {
                    i += 1;
                }
                i = (i + 1).min(s.len());
                runs.push((start..i, st));
            } else if b[i].is_ascii_alphabetic() {
                while i < s.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if KEYWORDS.contains(&&s[start..i]) {
                    runs.push((start..i, kw));
                }
            } else {
                i += 1;
            }
        }
    }
    StyledText::new(line.clone()).with_highlights(runs)
}

fn panel(title: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(rgb(0x3c3c3c))
        .child(div().px_2().py_1().bg(rgb(0x2d2d30)).child(title))
}

impl Render for Spike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::env::var("SPIKE_TRACE").is_ok() {
            eprintln!("render");
        }
        self.bench_tick(window, cx);
        let menu = div()
            .flex()
            .gap_4()
            .px_2()
            .py_1()
            .bg(rgb(0x2d2d30))
            .children(
                ["File", "Edit", "View", "Query", "Tools", "Window", "Help"]
                    .map(|m| div().child(m)),
            );
        let toolbar = div().flex().gap_2().px_2().py_1().children(
            [
                "New Query",
                "Execute",
                "Cancel",
                "Estimated Plan",
                "db_00 ▾",
            ]
            .map(|b| {
                div()
                    .px_2()
                    .border_1()
                    .border_color(rgb(0x555555))
                    .rounded_sm()
                    .child(b)
            }),
        );
        let rows = self.rows.len();
        let tree = uniform_list(
            "tree",
            rows,
            cx.processor(|this, range: std::ops::Range<usize>, _w, _cx| {
                range
                    .map(|ix| {
                        let (id, depth, loading) = this.rows[ix];
                        let open = this.expanded.get(&id).copied().unwrap_or(false);
                        let arrow = if loading {
                            "…"
                        } else if !id.has_children() {
                            " "
                        } else if open {
                            "▼"
                        } else {
                            "▶"
                        };
                        div()
                            .id(ix)
                            .pl(px(depth as f32 * 14.0 + 4.0))
                            .flex()
                            .gap_1()
                            .child(div().w(px(14.0)).child(arrow))
                            .child(id.label())
                            .on_click(_cx.listener(move |this, _e, _w, cx| {
                                if id.has_children() {
                                    this.toggle(id, cx);
                                }
                            }))
                    })
                    .collect()
            }),
        )
        .flex_grow();
        let editor = uniform_list(
            "editor",
            self.sql.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _w, _cx| {
                range
                    .map(|ix| {
                        div()
                            .font_family("DejaVu Sans Mono")
                            .child(highlight(&this.sql[ix]))
                    })
                    .collect()
            }),
        )
        .flex_grow();
        let header = div().flex().font_weight(FontWeight::BOLD).children(
            ["#", "id", "nome", "email", "criado_em", "saldo"].map(|h| div().w(px(130.0)).child(h)),
        );
        let grid = uniform_list(
            "grid",
            GRID_ROWS,
            cx.processor(|_this, range: std::ops::Range<usize>, _w, _cx| {
                range
                    .map(|i| {
                        let cells = [
                            (i + 1).to_string(),
                            i.to_string(),
                            format!("Cliente {i}"),
                            format!("cliente{i}@exemplo.com"),
                            format!("2026-{:02}-{:02}", i % 12 + 1, i % 28 + 1),
                            format!("{:.2}", (i % 10_000) as f64 * 1.37),
                        ];
                        div()
                            .flex()
                            .when(i % 2 == 1, |d| d.bg(rgb(0x252526)))
                            .children(cells.map(|c| {
                                div()
                                    .w(px(130.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(c)
                            }))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.grid_scroll.clone())
        .flex_grow();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xdcdcdc))
            .text_size(px(13.0))
            .child(menu)
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_grow()
                    .min_h_0()
                    .child(panel("Object Explorer").w(px(280.0)).child(tree))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_grow()
                            .child(
                                panel("SQLQuery1.sql (somente leitura)")
                                    .h(px(330.0))
                                    .child(editor),
                            )
                            .child(
                                panel("Results")
                                    .flex_grow()
                                    .min_h_0()
                                    .child(header)
                                    .child(grid),
                            ),
                    )
                    .child(panel("Properties").w(px(220.0)).child("Owner: postgres")),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .px_2()
                    .bg(rgb(0x007acc))
                    .child(self.status.clone())
                    .child("localhost (17)")
                    .child("postgres"),
            )
    }
}

fn main() {
    let started = Instant::now();
    env_logger::init();
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Spike::new(started)),
        )
        .unwrap();
        cx.activate(true);
    });
}
