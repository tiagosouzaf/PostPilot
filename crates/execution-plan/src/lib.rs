//! Planos de execução do PGStudio.
//!
//! [`plan`] lê a saída do `EXPLAIN (FORMAT JSON)` do PostgreSQL e
//! [`showplan`] o Showplan XML do SQL Server; [`view`]
//! desenha o plano como no SSMS, com um ícone por operação, o custo
//! relativo e setas que engrossam com as linhas.

pub mod plan;
pub mod showplan;
pub mod view;

pub use plan::{Actual, Node, ParseError, Plan, batch_shares, parse};
pub use showplan::{SHOWPLAN_COLUMN, parse_showplan};
pub use view::PlanView;
