//! Resultados de consulta do PGStudio.
//!
//! - [`store`]: buffer paginado das linhas, com páginas em disco além do
//!   orçamento de memória.
//! - [`grid`]: a grade virtualizada (seleção, largura de coluna, cópia).
//! - [`text`]: "Results to Text" e o texto copiado.
//! - [`export`]: CSV, TSV, JSON e INSERT (Save Results As, Results to
//!   File, Copy as INSERT).
//! - [`json`]: reconhece e lê o JSON das células (para o JSON Viewer).

pub mod export;
pub mod grid;
pub mod json;
pub mod store;
pub mod text;

pub use grid::{Cell, GridOutput, GridState, Mode, Selection};
pub use store::ResultStore;
