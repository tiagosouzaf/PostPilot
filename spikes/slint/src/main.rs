//! Spike da Fase 0: protótipo SSMS em Slint. Mesmo cenário e mesmo modo de
//! benchmark do spike egui (`SPIKE_BENCH=1`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

const GRID_ROWS: usize = 1_000_000;

type TreeState = (Rc<RefCell<Tree>>, Rc<VecModel<TreeRow>>);
thread_local! {
    static STATE: RefCell<Option<TreeState>> = const { RefCell::new(None) };
}

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
    std::thread::sleep(Duration::from_millis(120));
    match id {
        NodeId::Server => (0..10).map(NodeId::Database).collect(),
        NodeId::Database(d) => (0..10).map(|s| NodeId::Schema(d, s)).collect(),
        NodeId::Schema(d, s) => vec![NodeId::Tables(d, s)],
        NodeId::Tables(d, s) => (0..100).map(|t| NodeId::Table(d, s, t)).collect(),
        NodeId::Table(..) => vec![],
    }
}

#[derive(Default)]
struct Tree {
    expanded: HashMap<NodeId, bool>,
    children: HashMap<NodeId, Option<Vec<NodeId>>>, // None = carregando
    visible: Vec<NodeId>,
}

impl Tree {
    fn flatten(&mut self) -> Vec<TreeRow> {
        let mut rows = vec![];
        self.visible.clear();
        self.walk(NodeId::Server, 0, &mut rows);
        rows
    }
    fn walk(&mut self, id: NodeId, depth: i32, rows: &mut Vec<TreeRow>) {
        let open = self.expanded.get(&id).copied().unwrap_or(false);
        let loading = open && matches!(self.children.get(&id), Some(None));
        rows.push(TreeRow {
            text: id.label().into(),
            depth,
            expandable: id.has_children(),
            expanded: open,
            loading,
        });
        self.visible.push(id);
        if open && let Some(Some(kids)) = self.children.get(&id).cloned() {
            for k in kids {
                self.walk(k, depth + 1, rows);
            }
        }
    }
}

/// Modelo virtual: gera as linhas sob demanda, sem guardar 1 milhão de structs.
struct GridModel;

impl Model for GridModel {
    type Data = GridRow;
    fn row_count(&self) -> usize {
        GRID_ROWS
    }
    fn row_data(&self, i: usize) -> Option<GridRow> {
        (i < GRID_ROWS).then(|| GridRow {
            n: SharedString::from((i + 1).to_string()),
            id: SharedString::from(i.to_string()),
            nome: format!("Cliente {i}").into(),
            email: format!("cliente{i}@exemplo.com").into(),
            criado: format!("2026-{:02}-{:02}", i % 12 + 1, i % 28 + 1).into(),
            saldo: format!("{:.2}", (i % 10_000) as f64 * 1.37).into(),
        })
    }
    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &()
    }
}

fn sample_sql(lines: usize) -> String {
    let mut s = String::new();
    for n in 0..lines {
        match n % 4 {
            0 => s.push_str(&format!("-- consulta {n}\n")),
            1 => s.push_str("SELECT t.id, t.nome, 'texto' AS rotulo\n"),
            2 => s.push_str(&format!(
                "FROM schema_01.tabela_{:03} t WHERE t.id > {n}\n",
                n % 100
            )),
            _ => s.push_str("ORDER BY t.id LIMIT 100;\n"),
        }
    }
    s
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

fn refresh(app: &AppWindow, tree: &Rc<RefCell<Tree>>, model: &Rc<VecModel<TreeRow>>) {
    let rows = tree.borrow_mut().flatten();
    model.set_vec(rows);
    let _ = app;
}

fn toggle(app: &AppWindow, tree: &Rc<RefCell<Tree>>, model: &Rc<VecModel<TreeRow>>, id: NodeId) {
    let open = {
        let mut t = tree.borrow_mut();
        let open = !t.expanded.get(&id).copied().unwrap_or(false);
        t.expanded.insert(id, open);
        open
    };
    if open && !tree.borrow().children.contains_key(&id) {
        tree.borrow_mut().children.insert(id, None);
        let weak = app.as_weak();
        std::thread::spawn(move || {
            let kids = fake_children(id);
            let _ = slint::invoke_from_event_loop(move || {
                STATE.with(|st| {
                    if let Some((tree, model)) = st.borrow().as_ref() {
                        tree.borrow_mut().children.insert(id, Some(kids));
                        if let Some(app) = weak.upgrade() {
                            refresh(&app, tree, model);
                        }
                    }
                });
            });
        });
    }
    refresh(app, tree, model);
}

fn main() -> Result<(), slint::PlatformError> {
    let started = Instant::now();
    let app = AppWindow::new()?;
    if std::env::var("SPIKE_BENCH").is_ok() {
        println!(
            "window_new_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    let tree = Rc::new(RefCell::new(Tree::default()));
    let tree_model = Rc::new(VecModel::<TreeRow>::default());
    app.set_tree(ModelRc::from(tree_model.clone()));
    app.set_grid(ModelRc::new(GridModel));
    let lines = std::env::var("SPIKE_SQL_LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000);
    app.set_sql(sample_sql(lines).into());
    refresh(&app, &tree, &tree_model);
    STATE.with(|st| *st.borrow_mut() = Some((tree.clone(), tree_model.clone())));

    {
        let weak = app.as_weak();
        let tree = tree.clone();
        let model = tree_model.clone();
        app.on_toggle(move |i| {
            let app = weak.unwrap();
            let id = tree.borrow().visible.get(i as usize).copied();
            if let Some(id) = id {
                toggle(&app, &tree, &model, id);
            }
        });
    }
    {
        let weak = app.as_weak();
        app.on_execute(move || weak.unwrap().set_status("Executando (simulado)…".into()));
    }

    if std::env::var("SPIKE_BENCH").is_ok() {
        let idle_secs: u64 = std::env::var("SPIKE_IDLE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let first_frame: Rc<RefCell<Option<f64>>> = Rc::default();
        let frame_times: Rc<RefCell<Vec<f64>>> = Rc::default();
        let frame_start: Rc<RefCell<Option<Instant>>> = Rc::default();
        let intervals: Rc<RefCell<Vec<f64>>> = Rc::default();
        let last_frame: Rc<RefCell<Option<Instant>>> = Rc::default();
        {
            let intervals = intervals.clone();
            let last_frame = last_frame.clone();
            let first_frame = first_frame.clone();
            let frame_times = frame_times.clone();
            let frame_start = frame_start.clone();
            app.window()
                .set_rendering_notifier(move |state, _| match state {
                    slint::RenderingState::BeforeRendering => {
                        *frame_start.borrow_mut() = Some(Instant::now());
                    }
                    slint::RenderingState::AfterRendering => {
                        let mut ff = first_frame.borrow_mut();
                        if ff.is_none() {
                            *ff = Some(started.elapsed().as_secs_f64() * 1000.0);
                        }
                        if let Some(t) = frame_start.borrow_mut().take() {
                            frame_times
                                .borrow_mut()
                                .push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                        let mut last = last_frame.borrow_mut();
                        if let Some(t) = *last {
                            intervals
                                .borrow_mut()
                                .push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                        *last = Some(Instant::now());
                    }
                    _ => {}
                })
                .expect("rendering notifier");
        }
        // expande servidor -> bancos -> schemas -> Tables, como no spike egui
        let weak = app.as_weak();
        let tree2 = tree.clone();
        let model2 = tree_model.clone();
        let expander = slint::Timer::default();
        expander.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                let app = weak.unwrap();
                let mut queue = vec![NodeId::Server];
                while let Some(id) = queue.pop() {
                    if !id.has_children() {
                        continue;
                    }
                    if !tree2.borrow().expanded.get(&id).copied().unwrap_or(false) {
                        toggle(&app, &tree2, &model2, id);
                    }
                    if let Some(Some(kids)) = tree2.borrow().children.get(&id).cloned() {
                        queue.extend(kids.into_iter().filter(|k| !matches!(k, NodeId::Table(..))));
                    }
                }
            },
        );
        std::mem::forget(expander);

        // rola a grade: 300 passos, mede o tempo de renderização de cada frame
        let weak = app.as_weak();
        let step = Rc::new(RefCell::new(0u32));
        let scroller = slint::Timer::default();
        let frame_times2 = frame_times.clone();
        let intervals2 = intervals.clone();
        let last_frame2 = last_frame.clone();
        let started_scroll: Rc<RefCell<Option<Instant>>> = Rc::default();
        let scroll_tick = move || {
            let app = weak.unwrap();
            let mut s = step.borrow_mut();
            if *s == 0 {
                frame_times2.borrow_mut().clear();
                intervals2.borrow_mut().clear();
                *last_frame2.borrow_mut() = None;
                *started_scroll.borrow_mut() = Some(Instant::now());
            }
            if *s < 300 {
                app.set_grid_scroll(-(*s as f32) * 20.0 * 3_000.0);
                *s += 1;
            } else if *s == 300 {
                *s += 1;
                let wall = started_scroll
                    .borrow()
                    .map(|t| t.elapsed().as_secs_f64() * 1000.0 / 300.0);
                let mut ft = frame_times2.borrow().clone();
                ft.sort_by(|a, b| a.total_cmp(b));
                let pct = |p: f64| {
                    ft.get(((ft.len() as f64 - 1.0) * p) as usize)
                        .copied()
                        .unwrap_or(f64::NAN)
                };
                println!("toolkit=slint");
                println!(
                    "first_frame_ms={:.1}",
                    first_frame.borrow().unwrap_or(f64::NAN)
                );
                println!("scroll_render_ms_p50={:.2}", pct(0.5));
                println!("scroll_render_ms_p95={:.2}", pct(0.95));
                let _ = wall;
                let mut iv = intervals2.borrow().clone();
                iv.sort_by(|a, b| a.total_cmp(b));
                let ip = |p: f64| {
                    iv.get(((iv.len() as f64 - 1.0) * p) as usize)
                        .copied()
                        .unwrap_or(f64::NAN)
                };
                println!("frame_interval_ms_p50={:.2}", ip(0.5));
                println!("frame_interval_ms_p95={:.2}", ip(0.95));
                println!("frames={}", iv.len());
                let weak = app.as_weak();
                slint::Timer::single_shot(Duration::from_secs(idle_secs), move || {
                    println!(
                        "idle_rss_mb={}",
                        rss_mb().map_or("n/a".into(), |m| format!("{m:.1}"))
                    );
                    let _ = weak.unwrap().hide();
                    let _ = slint::quit_event_loop();
                });
            }
        };
        // espera 1,5 s para a árvore carregar, sem bloquear o event loop
        slint::Timer::single_shot(Duration::from_millis(1500), move || {
            scroller.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(1),
                scroll_tick,
            );
            std::mem::forget(scroller);
        });
    }

    app.run()
}
