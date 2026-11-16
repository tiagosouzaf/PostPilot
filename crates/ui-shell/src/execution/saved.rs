//! O último resultado de uma janela de consulta como a sessão guarda, para
//! reabrir o app com ele na tela (veja [`crate::session`]).
//!
//! Reabrir **nunca roda a consulta de novo**: o que volta é o que estava
//! na tela (grade, Messages, tempo e situação), só leitura, com um aviso de
//! que veio da sessão anterior. Resultados grandes voltam cortados em
//! [`MAX_ROWS`] linhas ou [`MAX_BYTES`] de texto; planos de execução e
//! Results to File não são guardados.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use pgstudio_core::connection::Engine;
use pgstudio_core::query::{QuerySession, QuerySink};
use pgstudio_i18n::{tr, trf};
use pgstudio_result_grid::ResultStore;
use serde::{Deserialize, Serialize};

use super::{Batch, Execution, Message, MessageKind, Outcome, ResultSet, ResultsMode, Tab};
use crate::result_edit::ResultEditor;

/// Linhas guardadas por janela, somando os resultados.
pub const MAX_ROWS: u64 = 5_000;

/// Texto guardado por janela, somando as células.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

/// Um resultado guardado.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// Linhas que o resultado tinha (mais que `rows` se foi cortado).
    pub total: u64,
}

/// Uma linha da aba Messages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedMessage {
    pub text: String,
    /// `info`, `notice` ou `error`.
    pub kind: String,
    pub position: Option<usize>,
}

/// A execução guardada.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedExecution {
    pub results: Vec<SavedResult>,
    pub messages: Vec<SavedMessage>,
    /// `succeeded`, `failed` ou `canceled`.
    pub outcome: String,
    pub elapsed_ms: u64,
    pub database: String,
    pub engine: Engine,
    /// Quando terminou (segundos desde 1970).
    pub finished_at: u64,
    /// Results to Text.
    pub text_mode: bool,
    /// A aba Messages estava na frente.
    pub messages_tab: bool,
}

/// De onde veio um resultado reaberto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restored {
    /// Quando a consulta rodou (segundos desde 1970).
    pub finished_at: u64,
    /// Linhas mostradas e linhas que o resultado tinha, se foi cortado.
    pub cut: Option<(u64, u64)>,
}

impl Restored {
    /// O aviso em cima do resultado.
    pub fn notice(&self) -> String {
        let age = now().saturating_sub(self.finished_at);
        let age = match age {
            0..60 => tr("less than a minute").to_string(),
            60..3600 => trf!("{n} min", n = age / 60),
            3600..172_800 => trf!("{n} h", n = age / 3600),
            _ => trf!("{n} days", n = age / 86_400),
        };
        let mut text = trf!(
            "Results from the previous session ({age} ago), not run again. Press F5 to refresh.",
            age = age
        );
        if let Some((kept, total)) = self.cut {
            text.push(' ');
            text.push_str(&trf!(
                "Showing the first {kept} of {total} rows.",
                kept = kept,
                total = total
            ));
        }
        text
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Sessão de um resultado reaberto: não roda nada.
struct Detached {
    database: String,
    engine: Engine,
}

impl QuerySession for Detached {
    fn execute(&self, _sql: String, _sink: QuerySink) {}

    fn cancel(&self) {}

    fn database(&self) -> String {
        self.database.clone()
    }

    fn engine(&self) -> Engine {
        self.engine
    }
}

impl Execution {
    /// Identifica esta execução, para a sessão notar que há um resultado
    /// novo sem comparar as linhas.
    pub fn started(&self) -> Instant {
        self.started
    }

    /// Se o resultado veio da sessão anterior.
    pub fn restored(&self) -> Option<&Restored> {
        self.restored.as_ref()
    }

    /// O resultado como a sessão guarda, ou `None` se não é guardado
    /// (ainda rodando, plano de execução ou Results to File).
    pub fn snapshot(&self) -> Option<SavedExecution> {
        if self.is_running() || self.plan_mode || self.file.is_some() {
            return None;
        }
        let mut rows_left = MAX_ROWS;
        let mut bytes_left = MAX_BYTES;
        let mut results = Vec::new();
        for set in &self.results {
            let mut store = set.store();
            let mut saved = SavedResult {
                columns: store.columns().to_vec(),
                rows: Vec::new(),
                total: store.len(),
            };
            for r in 0..store.len() {
                if rows_left == 0 || bytes_left == 0 {
                    break;
                }
                let row = store.row(r);
                let size: usize = row.iter().flatten().map(String::len).sum::<usize>() + row.len();
                rows_left -= 1;
                bytes_left = bytes_left.saturating_sub(size);
                saved.rows.push(row);
            }
            results.push(saved);
        }
        // Um resultado já cortado na sessão anterior continua dizendo
        // quantas linhas tinha.
        if let (
            Some(Restored {
                cut: Some((_, total)),
                ..
            }),
            [only],
        ) = (&self.restored, results.as_mut_slice())
        {
            only.total = *total;
        }
        Some(SavedExecution {
            results,
            messages: self
                .messages
                .iter()
                .map(|m| SavedMessage {
                    text: m.text.clone(),
                    kind: match m.kind {
                        MessageKind::Info => "info",
                        MessageKind::Notice => "notice",
                        MessageKind::Error => "error",
                    }
                    .into(),
                    position: m.position,
                })
                .collect(),
            outcome: match self.outcome {
                Outcome::Failed => "failed",
                Outcome::Canceled => "canceled",
                Outcome::Succeeded | Outcome::Running => "succeeded",
            }
            .into(),
            elapsed_ms: u64::try_from(self.elapsed().as_millis()).unwrap_or(u64::MAX),
            database: self.database.clone(),
            engine: self.session.engine(),
            finished_at: self.restored.map_or_else(now, |r| r.finished_at),
            text_mode: self.mode == ResultsMode::Text,
            messages_tab: self.tab == Tab::Messages,
        })
    }

    /// Mostra de novo um resultado guardado, sem rodar nada.
    pub fn restore(saved: SavedExecution, ctx: &egui::Context) -> Self {
        let session: Arc<dyn QuerySession> = Arc::new(Detached {
            database: saved.database.clone(),
            engine: saved.engine,
        });
        let mut exec =
            Execution::start_batch(session.clone(), Batch::generated(""), String::new(), 0, ctx);
        let reason = tr(
            "The results are read-only: they came from the previous session. Run the query again to edit them.",
        );
        let (mut kept, mut total) = (0, 0);
        for r in saved.results {
            let mut store = ResultStore::new(r.columns);
            for row in &r.rows {
                store.push(row);
            }
            kept += r.rows.len() as u64;
            total += r.total.max(r.rows.len() as u64);
            let mut set = ResultSet::new(Arc::new(Mutex::new(store)));
            set.editor = Some(ResultEditor::read_only(session.clone(), reason, ctx));
            exec.results.push(set);
        }
        exec.messages = saved
            .messages
            .into_iter()
            .map(|m| Message {
                text: m.text,
                kind: match m.kind.as_str() {
                    "error" => MessageKind::Error,
                    "notice" => MessageKind::Notice,
                    _ => MessageKind::Info,
                },
                position: m.position,
            })
            .collect();
        exec.outcome = match saved.outcome.as_str() {
            "failed" => Outcome::Failed,
            "canceled" => Outcome::Canceled,
            _ => Outcome::Succeeded,
        };
        exec.elapsed = Some(Duration::from_millis(saved.elapsed_ms));
        exec.finished = true;
        exec.mode = if saved.text_mode {
            ResultsMode::Text
        } else {
            ResultsMode::Grid
        };
        exec.tab = if saved.messages_tab || exec.results.is_empty() {
            Tab::Messages
        } else {
            Tab::Results
        };
        exec.restored = Some(Restored {
            finished_at: saved.finished_at,
            cut: (kept < total).then_some((kept, total)),
        });
        exec
    }
}
