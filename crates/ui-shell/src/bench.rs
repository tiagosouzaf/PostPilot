//! Cenários de medição de `cargo xtask bench`, rodados dentro do app de
//! verdade (release, com a GPU ou o renderizador por software do sistema):
//!
//! - **Grade:** uma consulta sintética de [`GRID_ROWS`] linhas chega pela
//!   mesma execução que uma consulta real usa; depois a grade rola do começo
//!   ao fim em [`GRID_SCROLL_SECONDS`] segundos (cerca de 600 frames a 60
//!   FPS; menos numa máquina lenta, para o cenário não demorar demais).
//! - **IntelliSense:** uma janela de consulta no banco `stress` do servidor
//!   de demonstração ([`STRESS_TABLES`] tabelas) recebe [`INTELLISENSE_SCRIPT`]
//!   uma tecla por frame, com a lista de sugestões aberta.
//!
//! - **Diagrama:** um Database Diagram com [`DIAGRAM_TABLES`] tabelas
//!   sintéticas (com FKs) passeia de um canto a outro com o zoom indo e
//!   voltando entre [`DIAGRAM_ZOOM`] em [`DIAGRAM_SECONDS`] segundos.
//!
//! - **Diagrama grande:** o mesmo passeio com [`DIAGRAM_LARGE_TABLES`]
//!   tabelas.
//! - **Table Designer:** num banco de [`DESIGNER_TABLES`] tabelas, abre o
//!   designer de uma delas e acrescenta [`DESIGNER_EDITS`] colunas, uma por
//!   frame (cada uma é uma edição com undo); no fim, mede o tempo de gerar
//!   o script de alterações.
//!
//! Ao terminar, o app escreve uma linha [`RESULT_PREFIX`] com os números
//! (`chave=valor`) e fica parado para a memória ser lida de fora.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use pgstudio_core::query::{QueryEvent, QuerySession, QuerySink};
pub use pgstudio_object_explorer::fake::STRESS_TABLES;

/// Linhas da consulta sintética do cenário da grade.
pub const GRID_ROWS: u64 = 1_000_000;
/// Duração da rolagem do começo ao fim da grade.
pub const GRID_SCROLL_SECONDS: f64 = 10.0;
/// Banco do servidor de demonstração com [`STRESS_TABLES`] tabelas.
pub const INTELLISENSE_DATABASE: &str = "stress";
/// Texto digitado no cenário do IntelliSense, uma tecla por frame. Cada
/// linha abre a lista de tabelas depois de `FROM` e a de colunas depois de
/// `t.`.
pub const INTELLISENSE_SCRIPT: &str = "select t.id from table_0123 t where t.na\n\
select t.email from table_0456 t where t.ac\n\
select t.balance from table_0789 t where t.cr\n\
select t.name from table_09 t join table_08 u on u.id = t.id\n";
/// Tabelas do cenário do diagrama.
pub const DIAGRAM_TABLES: usize = 500;
/// Tabelas do cenário do diagrama grande.
pub const DIAGRAM_LARGE_TABLES: usize = 2_000;
/// Tabelas do banco no cenário do Table Designer.
pub const DESIGNER_TABLES: usize = 2_000;
/// Tabela aberta no Table Designer (a primeira do modelo sintético).
pub const DESIGNER_TABLE: u64 = 1;
/// Colunas acrescentadas no Table Designer, uma por frame.
pub const DESIGNER_EDITS: usize = 300;
/// Duração do passeio pelo diagrama.
pub const DIAGRAM_SECONDS: f64 = 10.0;
/// Zoom mínimo e máximo do passeio (de tudo na tela a colunas legíveis).
pub const DIAGRAM_ZOOM: (f32, f32) = (0.08, 1.2);
/// Início da linha com os resultados de um cenário.
pub const RESULT_PREFIX: &str = "pgstudio-bench: result";

/// Um cenário de medição.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Grid,
    IntelliSense,
    Diagram,
    /// O diagrama com [`DIAGRAM_LARGE_TABLES`] tabelas.
    DiagramLarge,
    /// Table Designer num banco de [`DESIGNER_TABLES`] tabelas.
    Designer,
}

impl Scenario {
    /// Pelo nome usado em `PGSTUDIO_BENCH`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "grid" => Some(Scenario::Grid),
            "intellisense" => Some(Scenario::IntelliSense),
            "diagram" => Some(Scenario::Diagram),
            "diagram2000" => Some(Scenario::DiagramLarge),
            "designer" => Some(Scenario::Designer),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Grid => "grid",
            Scenario::IntelliSense => "intellisense",
            Scenario::Diagram => "diagram",
            Scenario::DiagramLarge => "diagram2000",
            Scenario::Designer => "designer",
        }
    }

    /// Tabelas do modelo sintético do cenário.
    pub fn tables(self) -> usize {
        match self {
            Scenario::DiagramLarge => DIAGRAM_LARGE_TABLES,
            Scenario::Designer => DESIGNER_TABLES,
            _ => DIAGRAM_TABLES,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Phase {
    /// Esperando o servidor de demonstração.
    Start,
    /// Esperando as linhas (grade) ou o catálogo (IntelliSense).
    Loading {
        since: Instant,
        doc: u32,
    },
    /// Medindo; o número é o frame (grade) ou a tecla (IntelliSense).
    Running {
        doc: u32,
        step: usize,
    },
    Done,
}

/// Estado de um cenário em andamento.
pub(crate) struct Run {
    pub scenario: Scenario,
    pub phase: Phase,
    /// Tempo de carga: linhas na grade ou catálogo do IntelliSense.
    pub load_ms: f64,
    /// Tempo de `ShellApp::show` em cada frame medido.
    pub ui_ms: Vec<f64>,
    /// Intervalo entre frames medidos (o que a tela mostra).
    pub frame_ms: Vec<f64>,
    pub last_frame: Option<Instant>,
    /// Teclas depois das quais a lista de sugestões estava aberta.
    pub popup_keys: usize,
    /// Mais tabelas desenhadas num frame (diagrama).
    pub drawn_max: usize,
    /// Tempo de cada edição (Table Designer), fora do desenho.
    pub edit_ms: Vec<f64>,
    /// Tecla a mandar no próximo frame (IntelliSense).
    pub next_key: Option<char>,
    /// Frames esperando o editor receber o foco antes de digitar.
    pub warmup: usize,
    /// Quando a rolagem da grade começou.
    pub scroll_start: Option<Instant>,
}

impl Run {
    pub fn new(scenario: Scenario) -> Self {
        Run {
            scenario,
            phase: Phase::Start,
            load_ms: 0.0,
            ui_ms: Vec::new(),
            frame_ms: Vec::new(),
            last_frame: None,
            popup_keys: 0,
            drawn_max: 0,
            edit_ms: Vec::new(),
            next_key: None,
            warmup: 0,
            scroll_start: None,
        }
    }

    /// Guarda o tempo de um frame medido.
    pub fn record(&mut self, ui_ms: f64) {
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            self.frame_ms.push((now - last).as_secs_f64() * 1000.0);
        }
        self.last_frame = Some(now);
        self.ui_ms.push(ui_ms);
    }

    /// A linha de resultado: `pgstudio-bench: result scenario=grid ...`.
    pub fn result_line(&self, extra: &[(&str, f64)]) -> String {
        let mut ui = self.ui_ms.clone();
        let mut frames = self.frame_ms.clone();
        let mut line = format!(
            "{RESULT_PREFIX} scenario={} load_ms={:.1} ui_p50={:.2} ui_p95={:.2} ui_max={:.2} \
             frame_p50={:.2} frame_p95={:.2} frame_max={:.2} samples={}",
            self.scenario.name(),
            self.load_ms,
            percentile(&mut ui, 50.0),
            percentile(&mut ui, 95.0),
            percentile(&mut ui, 100.0),
            percentile(&mut frames, 50.0),
            percentile(&mut frames, 95.0),
            percentile(&mut frames, 100.0),
            self.ui_ms.len(),
        );
        for (k, v) in extra {
            line.push_str(&format!(" {k}={v}"));
        }
        line
    }
}

/// Percentil `p` (0 a 100) pelo método do vizinho mais próximo.
pub fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let rank = (p / 100.0 * values.len() as f64).ceil() as usize;
    values[rank.clamp(1, values.len()) - 1]
}

/// Sessão sem banco que devolve [`GRID_ROWS`] linhas parecidas com as de
/// uma tabela de clientes, o mais rápido possível.
pub(crate) struct SyntheticSession {
    rows: u64,
    cancel: Arc<AtomicBool>,
}

impl SyntheticSession {
    pub fn new(rows: u64) -> Self {
        SyntheticSession {
            rows,
            cancel: Arc::default(),
        }
    }
}

impl QuerySession for SyntheticSession {
    fn execute(&self, _sql: String, mut sink: QuerySink) {
        let rows = self.rows;
        let cancel = self.cancel.clone();
        cancel.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            sink(QueryEvent::Columns(
                ["id", "name", "email", "created_at", "balance", "active"]
                    .map(String::from)
                    .into(),
            ));
            for i in 0..rows {
                if cancel.load(Ordering::Relaxed) {
                    sink(QueryEvent::Finished { canceled: true });
                    return;
                }
                sink(QueryEvent::Row(vec![
                    Some(i.to_string()),
                    Some(format!("Customer {i}")),
                    (i % 7 != 0).then(|| format!("customer{i}@example.com")),
                    Some(format!(
                        "2026-{:02}-{:02} 12:{:02}:00+00",
                        i % 12 + 1,
                        i % 28 + 1,
                        i % 60
                    )),
                    Some(format!("{}.{:02}", i % 100_000, i % 100)),
                    Some((i % 2 == 0).to_string()),
                ]));
            }
            sink(QueryEvent::CommandComplete { rows });
            sink(QueryEvent::Finished { canceled: false });
        });
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn database(&self) -> String {
        "bench".to_string()
    }
}

/// Modelo com `n` tabelas de 4 a 11 colunas; a maioria aponta para uma ou
/// duas tabelas anteriores, como num sistema de verdade.
pub fn synthetic_model(n: usize) -> pgstudio_core::metadata::Model {
    use pgstudio_core::schema::{Column, ForeignKey, Key, Table};
    let tables = (0..n)
        .map(|i| {
            let id = i as u64 + 1;
            let mut columns = vec![Column {
                id: id << 16 | 1,
                name: "id".into(),
                ty: "bigint".into(),
                ..Default::default()
            }];
            let mut foreign_keys = Vec::new();
            let parents = [i / 2, i / 3 + i % 7]
                .into_iter()
                .filter(|p| *p < i)
                .take(1 + i % 2);
            for p in parents {
                let col = format!("table_{p:04}_id");
                if columns.iter().any(|c| c.name == col) {
                    continue;
                }
                columns.push(Column {
                    id: id << 16 | (columns.len() as u64 + 1),
                    name: col.clone(),
                    ty: "bigint".into(),
                    ..Default::default()
                });
                foreign_keys.push(ForeignKey {
                    name: format!("table_{i:04}_{col}_fkey"),
                    columns: vec![col],
                    ref_schema: "public".into(),
                    ref_table: format!("table_{p:04}"),
                    ref_columns: vec!["id".into()],
                    ..Default::default()
                });
            }
            for k in 0..(3 + i % 8) {
                columns.push(Column {
                    id: id << 16 | (columns.len() as u64 + 1),
                    name: format!("field_{k}"),
                    ty: [
                        "text",
                        "integer",
                        "numeric(12,2)",
                        "timestamp with time zone",
                    ][k % 4]
                        .into(),
                    nullable: true,
                    ..Default::default()
                });
            }
            Table {
                id,
                schema: "public".into(),
                name: format!("table_{i:04}"),
                columns,
                primary_key: Some(Key {
                    name: format!("table_{i:04}_pkey"),
                    columns: vec!["id".into()],
                }),
                foreign_keys,
                ..Default::default()
            }
        })
        .collect();
    pgstudio_core::metadata::Model::new(tables)
}

/// Câmera do passeio pelo diagrama no instante `t` (0 a 1): o centro vai do
/// canto de cima à esquerda ao de baixo à direita e o zoom afasta e
/// aproxima duas vezes.
pub fn diagram_camera(
    bounds: eframe::egui::Rect,
    canvas: eframe::egui::Vec2,
    t: f64,
) -> pgstudio_diagram_engine::Camera {
    let t = t.clamp(0.0, 1.0) as f32;
    let (lo, hi) = DIAGRAM_ZOOM;
    let wave = 0.5 - 0.5 * (t * std::f32::consts::TAU * 2.0).cos();
    let zoom = hi - (hi - lo) * wave;
    let center = bounds.min + bounds.size() * t;
    pgstudio_diagram_engine::Camera {
        pan: canvas / 2.0 - center.to_vec2() * zoom,
        zoom,
    }
}

/// Modelo do cenário do Table Designer: o sintético de
/// [`DESIGNER_TABLES`] tabelas.
pub fn designer_model() -> pgstudio_core::metadata::Model {
    synthetic_model(DESIGNER_TABLES)
}

/// Servidor de mentira que só devolve um modelo pronto (medição do Table
/// Designer, sem PostgreSQL).
pub struct ModelProvider(pub pgstudio_core::metadata::Model);

impl pgstudio_core::metadata::MetadataProvider for ModelProvider {
    fn roots(
        &self,
    ) -> Result<Vec<pgstudio_core::metadata::ObjectNode>, pgstudio_core::metadata::MetadataError>
    {
        Ok(Vec::new())
    }

    fn children(
        &self,
        _parent: &pgstudio_core::metadata::NodeId,
    ) -> Result<Vec<pgstudio_core::metadata::ObjectNode>, pgstudio_core::metadata::MetadataError>
    {
        Ok(Vec::new())
    }

    fn load_model(
        &self,
        _database: &str,
    ) -> Result<pgstudio_core::metadata::Model, pgstudio_core::metadata::MetadataError> {
        Ok(self.0.clone())
    }

    fn apply_ddl(
        &self,
        _database: &str,
        _statements: &[String],
    ) -> Result<(), pgstudio_core::metadata::MetadataError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&mut v, 50.0), 50.0);
        assert_eq!(percentile(&mut v, 95.0), 95.0);
        assert_eq!(percentile(&mut v, 100.0), 100.0);
        assert_eq!(percentile(&mut [], 50.0), 0.0);
    }

    #[test]
    fn scenario_names_round_trip() {
        for s in [Scenario::Grid, Scenario::IntelliSense] {
            assert_eq!(Scenario::parse(s.name()), Some(s));
        }
        assert_eq!(Scenario::parse("1"), None);
    }

    #[test]
    fn synthetic_diagram_model_has_foreign_keys_to_existing_tables() {
        let m = synthetic_model(DIAGRAM_TABLES);
        assert_eq!(m.tables.len(), DIAGRAM_TABLES);
        let fks: usize = m.tables.iter().map(|t| t.foreign_keys.len()).sum();
        assert!(fks > DIAGRAM_TABLES, "{fks}");
        for t in &m.tables {
            for fk in &t.foreign_keys {
                assert!(m.find(&fk.ref_schema, &fk.ref_table).is_some());
                assert!(t.column(&fk.columns[0]).is_some());
            }
        }
    }

    #[test]
    fn diagram_tour_goes_corner_to_corner_zooming_out_and_in() {
        use eframe::egui::{Rect, pos2, vec2};
        let bounds = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 500.0));
        let canvas = vec2(800.0, 600.0);
        let center = |t| {
            let c = diagram_camera(bounds, canvas, t);
            (c.to_world(pos2(0.0, 0.0), (canvas / 2.0).to_pos2()), c.zoom)
        };
        let (start, z0) = center(0.0);
        let (end, z1) = center(1.0);
        let (_, z_mid) = center(0.25);
        assert!((start - bounds.min).length() < 1e-3);
        assert!((end - bounds.max).length() < 1e-2);
        assert_eq!((z0, z1), (DIAGRAM_ZOOM.1, DIAGRAM_ZOOM.1));
        assert!((z_mid - DIAGRAM_ZOOM.0).abs() < 1e-4);
    }
}
