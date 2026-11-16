//! Junta os eventos de uma execução numa resposta para o Claude, com
//! limite de linhas e de bytes: o que vai para o Claude sai da máquina, e
//! uma tabela grande não cabe na conversa.

use pgstudio_core::query::{QueryEvent, ServerMessage};
use serde_json::{Value, json};

/// Linhas por resultado, por padrão.
pub const DEFAULT_ROWS: usize = 200;
/// Teto de linhas que o Claude pode pedir.
pub const MAX_ROWS: usize = 1000;
/// Teto de bytes de valores numa resposta.
pub const MAX_BYTES: usize = 64 * 1024;
/// Maior valor de uma célula; o resto é cortado.
pub const MAX_CELL: usize = 2000;

/// Um resultado sendo montado.
#[derive(Debug, Default)]
struct Set {
    label: Option<String>,
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    row_count: u64,
    truncated: bool,
}

/// Recebe os [`QueryEvent`] de uma execução e monta o JSON da resposta.
#[derive(Debug)]
pub struct Collector {
    max_rows: usize,
    bytes: usize,
    sets: Vec<Set>,
    open: bool,
    /// Linhas afetadas de comandos sem resultado.
    commands: Vec<u64>,
    notices: Vec<String>,
    error: Option<ServerMessage>,
    canceled: bool,
    /// Comandos do embrulho READ ONLY a ignorar no começo e no fim.
    skip_leading: usize,
    skip_trailing: usize,
    completes: usize,
    labels: Vec<String>,
    finished: bool,
}

impl Collector {
    /// Até `max_rows` linhas por resultado (limitado a [`MAX_ROWS`]).
    pub fn new(max_rows: usize) -> Self {
        Collector {
            max_rows: max_rows.clamp(1, MAX_ROWS),
            bytes: 0,
            sets: Vec::new(),
            open: false,
            commands: Vec::new(),
            notices: Vec::new(),
            error: None,
            canceled: false,
            skip_leading: 0,
            skip_trailing: 0,
            completes: 0,
            labels: Vec::new(),
            finished: false,
        }
    }

    /// O lote veio de [`crate::safety::read_only_batch`]: os comandos do
    /// embrulho não aparecem na resposta.
    pub fn wrapped(mut self) -> Self {
        self.skip_leading = 2;
        self.skip_trailing = 1;
        self
    }

    /// Como [`Collector::wrapped`], para o embrulho do motor
    /// ([`crate::safety::read_only_batch_for`]). No SQL Server os comandos
    /// do embrulho não geram resposta, então não há o que esconder.
    pub fn wrapped_for(self, engine: pgstudio_core::connection::Engine) -> Self {
        match engine {
            pgstudio_core::connection::Engine::PostgreSql => self.wrapped(),
            pgstudio_core::connection::Engine::SqlServer
            | pgstudio_core::connection::Engine::Redis
            | pgstudio_core::connection::Engine::MongoDb
            | pgstudio_core::connection::Engine::Elasticsearch => self,
        }
    }

    /// Nomes dos resultados, na ordem (ferramentas com várias consultas).
    pub fn labels(mut self, labels: &[&str]) -> Self {
        self.labels = labels.iter().map(|l| l.to_string()).collect();
        self
    }

    /// Se já chegou o [`QueryEvent::Finished`].
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Se a execução terminou com erro.
    pub fn failed(&self) -> bool {
        self.error.is_some()
    }

    pub fn push(&mut self, event: QueryEvent) {
        match event {
            QueryEvent::Columns(columns) => {
                let label = self.labels.get(self.sets.len()).cloned();
                self.sets.push(Set {
                    label,
                    columns,
                    ..Default::default()
                });
                self.open = true;
            }
            QueryEvent::Row(row) => {
                let Some(set) = self.sets.last_mut().filter(|_| self.open) else {
                    return;
                };
                set.row_count += 1;
                if set.rows.len() >= self.max_rows || self.bytes >= MAX_BYTES {
                    set.truncated = true;
                    return;
                }
                let row: Vec<Option<String>> = row.into_iter().map(|v| v.map(cut)).collect();
                self.bytes += row
                    .iter()
                    .map(|v| v.as_ref().map_or(4, |s| s.len()) + 3)
                    .sum::<usize>();
                set.rows.push(row);
            }
            QueryEvent::CommandComplete { rows } => {
                self.completes += 1;
                if self.open {
                    if let Some(set) = self.sets.last_mut() {
                        set.row_count = set.row_count.max(rows);
                    }
                    self.open = false;
                } else if self.completes > self.skip_leading {
                    self.commands.push(rows);
                }
            }
            QueryEvent::Notice(m) => {
                // O ROLLBACK do embrulho não avisa nada; o resto vai.
                self.notices.push(message(&m));
            }
            QueryEvent::Error(m) => self.error = Some(m),
            QueryEvent::Finished { canceled } => {
                self.canceled = canceled;
                self.finished = true;
                if self.error.is_none() {
                    for _ in 0..self.skip_trailing {
                        self.commands.pop();
                    }
                }
            }
        }
    }

    /// A resposta em JSON.
    pub fn to_json(&self) -> Value {
        let mut out = serde_json::Map::new();
        let results: Vec<Value> = self
            .sets
            .iter()
            .map(|s| {
                let mut r = serde_json::Map::new();
                if let Some(l) = &s.label {
                    r.insert("name".into(), json!(l));
                }
                r.insert("columns".into(), json!(s.columns));
                r.insert("rows".into(), json!(s.rows));
                r.insert("row_count".into(), json!(s.row_count));
                if s.truncated {
                    r.insert("truncated".into(), json!(true));
                }
                Value::Object(r)
            })
            .collect();
        if !results.is_empty() {
            out.insert("results".into(), Value::Array(results));
        }
        if !self.commands.is_empty() {
            out.insert("rows_affected".into(), json!(self.commands));
        }
        if !self.notices.is_empty() {
            out.insert("messages".into(), json!(self.notices));
        }
        if let Some(e) = &self.error {
            out.insert("error".into(), json!(message(e)));
        }
        if self.canceled {
            out.insert("canceled".into(), json!(true));
        }
        Value::Object(out)
    }

    /// Resumo de uma linha para a janela Output.
    pub fn summary(&self) -> String {
        if let Some(e) = &self.error {
            return format!("error: {}", e.message);
        }
        let rows: u64 = self.sets.iter().map(|s| s.row_count).sum();
        match (self.sets.len(), self.commands.len()) {
            (0, 0) => "done".into(),
            (0, _) => format!("{} row(s) affected", self.commands.iter().sum::<u64>()),
            (1, _) => format!("{rows} row(s)"),
            (n, _) => format!("{n} results, {rows} row(s)"),
        }
    }
}

fn cut(mut v: String) -> String {
    if v.len() > MAX_CELL {
        let mut end = MAX_CELL;
        while !v.is_char_boundary(end) {
            end -= 1;
        }
        v.truncate(end);
        v.push('…');
    }
    v
}

/// Mensagem do servidor com código, detalhe e dica.
pub fn message(m: &ServerMessage) -> String {
    let mut text = if m.code.is_empty() {
        format!("{}: {}", m.severity, m.message)
    } else {
        format!("{} {}: {}", m.severity, m.code, m.message)
    };
    if let Some(d) = &m.detail {
        text.push_str(&format!("\nDETAIL: {d}"));
    }
    if let Some(h) = &m.hint {
        text.push_str(&format!("\nHINT: {h}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(c: &mut Collector, events: Vec<QueryEvent>) {
        for e in events {
            c.push(e);
        }
    }

    #[test]
    fn wrapped_batch_hides_wrapper() {
        let mut c = Collector::new(2).wrapped();
        feed(
            &mut c,
            vec![
                QueryEvent::CommandComplete { rows: 0 },
                QueryEvent::CommandComplete { rows: 0 },
                QueryEvent::Columns(vec!["a".into()]),
                QueryEvent::Row(vec![Some("1".into())]),
                QueryEvent::Row(vec![None]),
                QueryEvent::Row(vec![Some("3".into())]),
                QueryEvent::CommandComplete { rows: 3 },
                QueryEvent::CommandComplete { rows: 0 },
                QueryEvent::Finished { canceled: false },
            ],
        );
        let v = c.to_json();
        assert_eq!(v["results"][0]["rows"], json!([["1"], [null]]));
        assert_eq!(v["results"][0]["row_count"], 3);
        assert_eq!(v["results"][0]["truncated"], true);
        assert!(v.get("rows_affected").is_none());
        assert_eq!(c.summary(), "3 row(s)");
    }

    #[test]
    fn writes_and_errors() {
        let mut c = Collector::new(10);
        feed(
            &mut c,
            vec![
                QueryEvent::CommandComplete { rows: 4 },
                QueryEvent::Error(ServerMessage {
                    severity: "ERROR".into(),
                    code: "42P01".into(),
                    message: "relation \"x\" does not exist".into(),
                    ..Default::default()
                }),
                QueryEvent::Finished { canceled: false },
            ],
        );
        let v = c.to_json();
        assert_eq!(v["rows_affected"], json!([4]));
        assert_eq!(
            v["error"],
            json!("ERROR 42P01: relation \"x\" does not exist")
        );
        assert!(c.failed());
    }

    #[test]
    fn labels_and_long_cells() {
        let mut c = Collector::new(10).labels(&["table", "columns"]);
        feed(
            &mut c,
            vec![
                QueryEvent::Columns(vec!["t".into()]),
                QueryEvent::Row(vec![Some("x".repeat(MAX_CELL + 10))]),
                QueryEvent::CommandComplete { rows: 1 },
                QueryEvent::Columns(vec!["c".into()]),
                QueryEvent::CommandComplete { rows: 0 },
                QueryEvent::Finished { canceled: false },
            ],
        );
        let v = c.to_json();
        assert_eq!(v["results"][0]["name"], "table");
        assert_eq!(v["results"][1]["name"], "columns");
        assert!(
            v["results"][0]["rows"][0][0]
                .as_str()
                .unwrap()
                .ends_with('…')
        );
    }
}
