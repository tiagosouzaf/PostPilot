//! Resultados de consulta do PGStudio.
//!
//! - [`store`]: buffer paginado das linhas, com páginas em disco além do
//!   orçamento de memória.
//! - [`grid`]: a grade virtualizada (seleção, largura de coluna, cópia).
//! - [`text`]: "Results to Text" e o texto copiado.

pub mod grid;
pub mod store;
pub mod text;

pub use grid::{Cell, GridOutput, GridState, Mode, Selection};
pub use store::ResultStore;
