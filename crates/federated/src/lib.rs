//! Consulta entre servidores: um SELECT que junta dados de conexões
//! diferentes (PostgreSQL, SQL Server, MongoDB, Elasticsearch, Redis).
//!
//! [`plan`] lê o texto e acha as fontes; [`engine`] lê cada fonte no
//! servidor dela, guarda num SQLite temporário e roda o SQL do usuário lá.
//! O SQL de fora é, portanto, o do SQLite; o de dentro de cada
//! `servidor($$ ... $$)` é o do servidor.

pub mod engine;
pub mod plan;
pub mod pushdown;

pub use engine::{FederatedSession, Options, Remote};
pub use plan::{
    Home, Plan, PlanError, Server, Source, plan, plan_with, server_named, written_name,
};
