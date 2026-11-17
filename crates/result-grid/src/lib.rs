//! Resultados de consulta do PGStudio.
//!
//! - [`store`]: buffer paginado das linhas, com páginas em disco além do
//!   orçamento de memória.
//! - [`grid`]: a grade virtualizada (seleção, largura de coluna, cópia).
//! - [`text`]: "Results to Text" e o texto copiado.
//! - [`export`]: CSV, TSV, JSON e INSERT (Save Results As, Results to
//!   File, Copy as INSERT).
//! - [`copy`]: o que o Ctrl+C manda para a área de transferência, no
//!   formato escolhido (TSV, CSV, JSON, INSERT, lista para IN, Markdown,
//!   HTML).
//! - [`prefs`]: as escolhas que valem para todas as grades (paginação e
//!   formato de cópia).
//! - [`view`]: ordenar e filtrar as linhas já recebidas, sem rodar a
//!   consulta de novo.
//! - [`json`]: reconhece e lê o JSON das células (para o JSON Viewer).

pub mod copy;
pub mod export;
pub mod grid;
pub mod json;
pub mod prefs;
pub mod store;
pub mod text;
pub mod view;
pub mod xlsx;

pub use copy::CopyFormat;
pub use grid::{Cell, GridOutput, GridState, Mode, Selection};
pub use prefs::GridPrefs;
pub use store::ResultStore;
pub use view::{Sort, ViewProgress, ViewSpec};
