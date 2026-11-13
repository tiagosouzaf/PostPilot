//! Spike da Fase 0: protótipo SSMS em egui + egui_dock.
//!
//! Exercita o que mais pesa num app tipo SSMS: menus, toolbar, status bar,
//! painéis encaixáveis com abas e janelas flutuantes, árvore com 10 mil nós
//! carregados sob demanda, grade virtualizada de 1 milhão de linhas e editor
//! SQL com realce e popup de autocompletar.
//!
//! Modo benchmark: `SPIKE_BENCH=1` mede o tempo até o primeiro frame, o tempo
//! de CPU por frame rolando a grade e a RAM em repouso, imprime e sai.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, FontId, Galley, TextFormat, text::LayoutJob};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};

const GRID_ROWS: usize = 1_000_000;
const DATABASES: usize = 10;
const SCHEMAS: usize = 10;
const TABLES: usize = 100; // 10 x 10 x 100 = 10 mil tabelas

const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP", "BY",
    "ORDER", "HAVING", "LIMIT", "OFFSET", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE",
    "CREATE", "TABLE", "INDEX", "VIEW", "AND", "OR", "NOT", "NULL", "AS", "DISTINCT", "WITH",
    "UNION", "ALL", "CASE", "WHEN", "THEN", "ELSE", "END", "IN", "EXISTS", "BETWEEN", "LIKE",
];

fn main() -> eframe::Result {
    let started = Instant::now();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Spike egui — PGStudio")
            .with_inner_size([1280.0, 800.0]),
        renderer: match std::env::var("SPIKE_RENDERER").as_deref() {
            Ok("glow") => eframe::Renderer::Glow,
            _ => eframe::Renderer::Wgpu,
        },
        ..Default::default()
    };
    if std::env::var("SPIKE_VSYNC").as_deref() == Ok("0") {
        // sem vsync: separa o custo de renderizar da espera pela apresentação
        options.wgpu_options.surface.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native(
        "spike-egui",
        options,
        Box::new(move |cc| Ok(Box::new(SpikeApp::new(cc, started)))),
    )
}

// ---------------------------------------------------------------- árvore ----

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
    fn icon(self) -> &'static str {
        match self {
            NodeId::Server => "🖧",
            NodeId::Database(_) => "🛢",
            NodeId::Schema(..) => "📁",
            NodeId::Tables(..) => "📂",
            NodeId::Table(..) => "▦",
        }
    }
    fn has_children(self) -> bool {
        !matches!(self, NodeId::Table(..))
    }
}

/// Provedor de metadados falso: responde numa thread com atraso, como um banco.
fn fake_children(id: NodeId) -> Vec<NodeId> {
    std::thread::sleep(Duration::from_millis(120));
    match id {
        NodeId::Server => (0..DATABASES).map(NodeId::Database).collect(),
        NodeId::Database(d) => (0..SCHEMAS).map(|s| NodeId::Schema(d, s)).collect(),
        NodeId::Schema(d, s) => vec![NodeId::Tables(d, s)],
        NodeId::Tables(d, s) => (0..TABLES).map(|t| NodeId::Table(d, s, t)).collect(),
        NodeId::Table(..) => vec![],
    }
}

enum Children {
    Loading,
    Loaded(Vec<NodeId>),
}

struct Tree {
    expanded: HashMap<NodeId, bool>,
    children: HashMap<NodeId, Children>,
    tx: Sender<(NodeId, Vec<NodeId>)>,
    rx: Receiver<(NodeId, Vec<NodeId>)>,
    selected: Option<NodeId>,
}

/// Linha visível da árvore achatada: o que a ScrollArea virtualiza.
enum Row {
    Node(NodeId, usize),
    Loading(usize),
}

impl Tree {
    fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            expanded: HashMap::new(),
            children: HashMap::new(),
            tx,
            rx,
            selected: None,
        }
    }

    fn poll(&mut self) {
        while let Ok((id, kids)) = self.rx.try_recv() {
            self.children.insert(id, Children::Loaded(kids));
        }
    }

    fn toggle(&mut self, id: NodeId, ctx: &egui::Context) {
        let open = !self.expanded.get(&id).copied().unwrap_or(false);
        self.expanded.insert(id, open);
        if open && !self.children.contains_key(&id) {
            self.children.insert(id, Children::Loading);
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let kids = fake_children(id);
                let _ = tx.send((id, kids));
                ctx.request_repaint();
            });
        }
    }

    fn flatten(&self, id: NodeId, depth: usize, out: &mut Vec<Row>) {
        out.push(Row::Node(id, depth));
        if !self.expanded.get(&id).copied().unwrap_or(false) {
            return;
        }
        match self.children.get(&id) {
            Some(Children::Loaded(kids)) => {
                for k in kids {
                    self.flatten(*k, depth + 1, out);
                }
            }
            _ => out.push(Row::Loading(depth + 1)),
        }
    }

    fn expand_all_loaded(&mut self, ctx: &egui::Context) {
        // Usado pelo benchmark: abre servidor, bancos, schemas e "Tables".
        let mut queue = vec![NodeId::Server];
        while let Some(id) = queue.pop() {
            if !id.has_children() || matches!(id, NodeId::Table(..)) {
                continue;
            }
            if !self.expanded.get(&id).copied().unwrap_or(false) {
                self.toggle(id, ctx);
            }
            if let Some(Children::Loaded(kids)) = self.children.get(&id) {
                queue.extend(kids.iter().copied());
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let mut rows = Vec::new();
        self.flatten(NodeId::Server, 0, &mut rows);
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let mut toggle = None;
        ui.label(format!("{} linhas visíveis", rows.len()));
        egui::ScrollArea::both().auto_shrink(false).show_rows(
            ui,
            row_h,
            rows.len(),
            |ui, range| {
                for row in &rows[range] {
                    match row {
                        Row::Node(id, depth) => {
                            ui.horizontal(|ui| {
                                ui.add_space(*depth as f32 * 14.0);
                                if id.has_children() {
                                    let open = self.expanded.get(id).copied().unwrap_or(false);
                                    if ui.small_button(if open { "▼" } else { "▶" }).clicked() {
                                        toggle = Some(*id);
                                    }
                                } else {
                                    ui.add_space(18.0);
                                }
                                let text = format!("{} {}", id.icon(), id.label());
                                let resp = ui.selectable_label(self.selected == Some(*id), text);
                                if resp.clicked() {
                                    self.selected = Some(*id);
                                }
                                if resp.double_clicked() && id.has_children() {
                                    toggle = Some(*id);
                                }
                                resp.context_menu(|ui| {
                                    for item in
                                        ["New Query", "Script as CREATE", "Refresh", "Properties"]
                                    {
                                        if ui.button(item).clicked() {
                                            ui.close();
                                        }
                                    }
                                });
                            });
                        }
                        Row::Loading(depth) => {
                            ui.horizontal(|ui| {
                                ui.add_space(*depth as f32 * 14.0 + 18.0);
                                ui.spinner();
                                ui.weak("Carregando…");
                            });
                        }
                    }
                }
            },
        );
        if let Some(id) = toggle {
            self.toggle(id, ui.ctx());
        }
    }
}

// ---------------------------------------------------------------- editor ----

fn highlight(text: &str, font: FontId, dark: bool) -> LayoutJob {
    let kw = if dark {
        Color32::from_rgb(86, 156, 214)
    } else {
        Color32::from_rgb(0, 0, 255)
    };
    let string = if dark {
        Color32::from_rgb(206, 145, 120)
    } else {
        Color32::from_rgb(163, 21, 21)
    };
    let comment = Color32::from_rgb(87, 166, 74);
    let normal = if dark {
        Color32::from_gray(220)
    } else {
        Color32::from_gray(20)
    };
    let mut job = LayoutJob::default();
    let fmt = |c| TextFormat::simple(font.clone(), c);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        let start = i;
        let c = bytes[i];
        let color = if text[i..].starts_with("--") {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
            comment
        } else if c == b'\'' {
            i += 1;
            while i < text.len() && bytes[i] != b'\'' {
                i += 1;
            }
            i = (i + 1).min(text.len());
            string
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < text.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                kw
            } else {
                normal
            }
        } else {
            // avança um caractere UTF-8 inteiro
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            normal
        };
        job.append(&text[start..i], 0.0, fmt(color));
    }
    job
}

fn sample_sql(lines: usize) -> String {
    let mut s = String::with_capacity(lines * 64);
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

struct Editor {
    text: String,
    cache: Option<(u64, bool, Arc<Galley>, f32)>,
    completions: Vec<String>,
    words: Vec<String>,
}

impl Editor {
    fn new(lines: usize) -> Self {
        let mut words: Vec<String> = KEYWORDS.iter().map(|k| k.to_string()).collect();
        words.extend((0..TABLES).map(|t| format!("tabela_{t:03}")));
        words.extend((0..SCHEMAS).map(|s| format!("schema_{s:02}")));
        Self {
            text: sample_sql(lines),
            cache: None,
            completions: vec![],
            words,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let cache = &mut self.cache;
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            buf.as_str().hash(&mut h);
            let key = h.finish();
            if let Some((k, d, g, w)) = cache
                && *k == key
                && *d == dark
                && *w == wrap
            {
                return g.clone();
            }
            let mut job = highlight(buf.as_str(), font.clone(), dark);
            job.wrap.max_width = wrap;
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            *cache = Some((key, dark, galley.clone(), wrap));
            galley
        };
        let out = egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::TextEdit::multiline(&mut self.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;

        // Autocompletar: palavra antes do cursor -> sugestões num popup.
        self.completions.clear();
        if let Some(range) = out.cursor_range
            && out.response.has_focus()
        {
            let idx = range.primary.index.0;
            let before: String = self.text.chars().take(idx).collect();
            let prefix: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if prefix.len() >= 2 {
                self.completions = self
                    .words
                    .iter()
                    .filter(|w| {
                        w.to_lowercase().starts_with(&prefix.to_lowercase())
                            && w.len() > prefix.len()
                    })
                    .take(8)
                    .cloned()
                    .collect();
            }
            if !self.completions.is_empty() {
                let pos = out.galley_pos
                    + out
                        .galley
                        .pos_from_cursor(range.primary)
                        .left_bottom()
                        .to_vec2();
                let mut chosen = None;
                egui::Area::new(egui::Id::new("completion"))
                    .fixed_pos(pos)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            for w in &self.completions {
                                if ui.selectable_label(false, w).clicked() {
                                    chosen = Some(w.clone());
                                }
                            }
                        });
                    });
                if let Some(word) = chosen {
                    let byte_idx = self
                        .text
                        .char_indices()
                        .nth(idx)
                        .map_or(self.text.len(), |(b, _)| b);
                    let start = byte_idx - prefix.len();
                    self.text.replace_range(start..byte_idx, &word);
                }
            }
        }
    }
}

// ----------------------------------------------------------------- dock -----

#[derive(Debug, Clone, PartialEq)]
enum Tab {
    ObjectExplorer,
    Query(usize),
    Results,
    Messages,
    Properties,
}

struct Panes {
    tree: Tree,
    editors: HashMap<usize, Editor>,
    bench_scroll: Option<f32>,
}

impl TabViewer for Panes {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(format!("{tab:?}"))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::ObjectExplorer => "Object Explorer".into(),
            Tab::Query(n) => format!("SQLQuery{n}.sql").into(),
            Tab::Results => "Results".into(),
            Tab::Messages => "Messages".into(),
            Tab::Properties => "Properties".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::ObjectExplorer => self.tree.ui(ui),
            Tab::Query(n) => self
                .editors
                .entry(*n)
                .or_insert_with(|| Editor::new(5_000))
                .ui(ui),
            Tab::Results => results_grid(ui, self.bench_scroll),
            Tab::Messages => {
                ui.monospace("Commands completed successfully.");
            }
            Tab::Properties => {
                let sel = self
                    .tree
                    .selected
                    .map_or("(nada selecionado)".into(), |n| n.label());
                egui::Grid::new("props").striped(true).show(ui, |ui| {
                    ui.label("Nome");
                    ui.label(sel);
                    ui.end_row();
                    ui.label("Owner");
                    ui.label("postgres");
                    ui.end_row();
                });
            }
        }
    }

    fn closeable(&mut self, tab: &mut Tab) -> bool {
        matches!(tab, Tab::Query(_))
    }
}

fn results_grid(ui: &mut egui::Ui, scroll: Option<f32>) {
    use egui_extras::{Column, TableBuilder};
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    let mut table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .column(Column::auto().at_least(70.0))
        .columns(Column::initial(140.0).at_least(60.0), 5);
    if let Some(off) = scroll {
        table = table.vertical_scroll_offset(off);
    }
    table
        .header(20.0, |mut h| {
            for name in ["#", "id", "nome", "email", "criado_em", "saldo"] {
                h.col(|ui| {
                    ui.strong(name);
                });
            }
        })
        .body(|body| {
            body.rows(18.0, GRID_ROWS, |mut row| {
                let i = row.index();
                row.col(|ui| {
                    ui.weak((i + 1).to_string());
                });
                row.col(|ui| {
                    ui.label(i.to_string());
                });
                row.col(|ui| {
                    ui.label(format!("Cliente {i}"));
                });
                row.col(|ui| {
                    ui.label(format!("cliente{i}@exemplo.com"));
                });
                row.col(|ui| {
                    ui.label(format!("2026-{:02}-{:02}", i % 12 + 1, i % 28 + 1));
                });
                row.col(|ui| {
                    ui.label(format!("{:.2}", (i % 10_000) as f64 * 1.37));
                });
            });
        });
}

// ------------------------------------------------------------------ app -----

struct Bench {
    started: Instant,
    frame: u64,
    first_frame_ms: Option<f64>,
    scroll_cpu: Vec<f32>,
    scroll_wall: Vec<f64>,
    last_frame: Option<Instant>,
    idle_secs: u64,
    idle_from: Option<Instant>,
}

struct SpikeApp {
    dock: DockState<Tab>,
    panes: Panes,
    next_query: usize,
    bench: Option<Bench>,
    status: String,
}

impl SpikeApp {
    fn new(cc: &eframe::CreationContext<'_>, started: Instant) -> Self {
        let mut dock = DockState::new(vec![Tab::Query(1)]);
        let tree = dock.main_surface_mut();
        let [center, _left] = tree.split_left(NodeIndex::root(), 0.22, vec![Tab::ObjectExplorer]);
        let [center, _right] = tree.split_right(center, 0.78, vec![Tab::Properties]);
        let [_editor, _results] = tree.split_below(center, 0.5, vec![Tab::Results, Tab::Messages]);

        let bench = std::env::var("SPIKE_BENCH").ok().map(|_| Bench {
            started,
            frame: 0,
            first_frame_ms: None,
            scroll_cpu: vec![],
            scroll_wall: vec![],
            last_frame: None,
            idle_secs: std::env::var("SPIKE_IDLE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            idle_from: None,
        });
        let mut panes = Panes {
            tree: Tree::new(),
            editors: HashMap::new(),
            bench_scroll: None,
        };
        if bench.is_some() {
            panes.tree.toggle(NodeId::Server, &cc.egui_ctx);
        }
        Self {
            dock,
            panes,
            next_query: 2,
            bench,
            status: "Pronto".into(),
        }
    }

    fn new_query(&mut self) {
        self.dock.push_to_focused_leaf(Tab::Query(self.next_query));
        self.next_query += 1;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let new_query = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
        let execute = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
        let execute2 = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
        if ctx.input_mut(|i| i.consume_shortcut(&new_query)) {
            self.new_query();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&execute) || i.consume_shortcut(&execute2)) {
            self.status = "Executando (simulado)…".into();
        }
    }

    fn run_bench(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(b) = &mut self.bench else { return };
        b.frame += 1;
        if b.frame == 2 {
            b.first_frame_ms = Some(b.started.elapsed().as_secs_f64() * 1000.0);
        }
        if b.frame > 5 {
            self.panes.tree.expand_all_loaded(ctx);
        }
        // frames 10..310: rola a grade 1 milhão de linhas, mede CPU do frame.
        if (10..310).contains(&b.frame) {
            let step = (b.frame - 10) as f32;
            self.panes.bench_scroll = Some(step * 18.0 * 3_000.0);
            if let Some(cpu) = frame.info().cpu_usage {
                b.scroll_cpu.push(cpu * 1000.0);
            }
            if let Some(t) = b.last_frame {
                b.scroll_wall.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            b.last_frame = Some(Instant::now());
            ctx.request_repaint();
        } else if b.frame < 10 {
            ctx.request_repaint();
        } else if b.frame == 310 {
            self.panes.bench_scroll = None;
            b.idle_from = Some(Instant::now());
            ctx.request_repaint_after(Duration::from_secs(b.idle_secs));
        } else if let Some(t) = b.idle_from
            && t.elapsed() >= Duration::from_secs(b.idle_secs)
        {
            let mut cpu = b.scroll_cpu.clone();
            cpu.sort_by(|a, b| a.total_cmp(b));
            let pct = |p: f32| {
                cpu.get(((cpu.len() as f32 - 1.0) * p) as usize)
                    .copied()
                    .unwrap_or(f32::NAN)
            };
            println!("toolkit=egui");
            println!("first_frame_ms={:.1}", b.first_frame_ms.unwrap_or(f64::NAN));
            println!("scroll_cpu_ms_p50={:.2}", pct(0.5));
            println!("scroll_cpu_ms_p95={:.2}", pct(0.95));
            let mut wall = b.scroll_wall.clone();
            wall.sort_by(|a, b| a.total_cmp(b));
            let wp = |p: f64| {
                wall.get(((wall.len() as f64 - 1.0) * p) as usize)
                    .copied()
                    .unwrap_or(f64::NAN)
            };
            println!("frame_interval_ms_p50={:.2}", wp(0.5));
            println!("frame_interval_ms_p95={:.2}", wp(0.95));
            println!(
                "idle_rss_mb={}",
                rss_mb().map_or("n/a".into(), |m| format!("{m:.1}"))
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.bench = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
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

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Query\tCtrl+N").clicked() {
                        self.new_query();
                        ui.close();
                    }
                    let _ = ui.button("Open…\tCtrl+O");
                    let _ = ui.button("Save\tCtrl+S");
                    let _ = ui.button("Save As…\tCtrl+Shift+S");
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                for menu in ["Edit", "View", "Query", "Tools", "Window", "Help"] {
                    ui.menu_button(menu, |ui| {
                        ui.weak("(spike)");
                    });
                }
            });
        });
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📄 New Query").clicked() {
                    self.new_query();
                }
                let _ = ui.button("▶ Execute");
                let _ = ui.button("■ Cancel");
                ui.separator();
                let _ = ui.button("🔍 Estimated Plan");
                egui::ComboBox::from_id_salt("db")
                    .selected_text("db_00")
                    .show_ui(ui, |ui| {
                        for d in 0..DATABASES {
                            let _ = ui.selectable_label(false, format!("db_{d:02}"));
                        }
                    });
            });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.separator();
                ui.label("localhost (17)");
                ui.separator();
                ui.label("postgres");
                ui.separator();
                ui.label(format!(
                    "{:.1} ms/frame",
                    frame.info().cpu_usage.unwrap_or(0.0) * 1000.0
                ));
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            DockArea::new(&mut self.dock)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_close_buttons(true)
                .show_inside(ui, &mut self.panes);
        });

        self.run_bench(&ctx, frame);
    }
}
