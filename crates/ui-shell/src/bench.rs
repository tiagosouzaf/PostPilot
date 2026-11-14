//! Cenários de medição de `cargo xtask bench`, rodados dentro do app de
//! verdade (release, com a GPU ou o renderizador por software do sistema):
//!
//! - **Grade:** uma consulta sintética de [`GRID_ROWS`] linhas chega pela
//!   mesma execução que uma consulta real usa; depois a grade rola do começo
//!   ao fim em [`GRID_FRAMES`] frames.
//! - **IntelliSense:** uma janela de consulta no banco `stress` do servidor
//!   de demonstração ([`STRESS_TABLES`] tabelas) recebe [`INTELLISENSE_SCRIPT`]
//!   uma tecla por frame, com a lista de sugestões aberta.
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
/// Frames da rolagem do começo ao fim da grade.
pub const GRID_FRAMES: usize = 600;
/// Banco do servidor de demonstração com [`STRESS_TABLES`] tabelas.
pub const INTELLISENSE_DATABASE: &str = "stress";
/// Texto digitado no cenário do IntelliSense, uma tecla por frame. Cada
/// linha abre a lista de tabelas depois de `FROM` e a de colunas depois de
/// `t.`.
pub const INTELLISENSE_SCRIPT: &str = "select t.id from table_0123 t where t.na\n\
select t.email from table_0456 t where t.ac\n\
select t.balance from table_0789 t where t.cr\n\
select t.name from table_09 t join table_08 u on u.id = t.id\n";
/// Início da linha com os resultados de um cenário.
pub const RESULT_PREFIX: &str = "pgstudio-bench: result";

/// Um cenário de medição.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Grid,
    IntelliSense,
}

impl Scenario {
    /// Pelo nome usado em `PGSTUDIO_BENCH`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "grid" => Some(Scenario::Grid),
            "intellisense" => Some(Scenario::IntelliSense),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Grid => "grid",
            Scenario::IntelliSense => "intellisense",
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
    /// Tecla a mandar no próximo frame (IntelliSense).
    pub next_key: Option<char>,
    /// Frames esperando o editor receber o foco antes de digitar.
    pub warmup: usize,
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
            next_key: None,
            warmup: 0,
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
}
