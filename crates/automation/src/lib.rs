//! Ferramentas que o Claude usa no PGStudio (Fase 7).
//!
//! O Claude chega ao app por duas portas, o servidor MCP (`pgstudio mcp`,
//! para o Claude Code) e o painel do Claude, e as duas usam as mesmas
//! ferramentas, descritas em [`tools`]. Quem chama manda a chamada por um
//! [`hub::Client`]; a interface recebe no [`hub::Inbox`] e atende na thread
//! dela.
//!
//! Regra de segurança: ler é livre, alterar pede confirmação. O
//! [`safety::classify`] decide; o que só lê roda numa transação
//! `READ ONLY` ([`safety::read_only_batch`]); o resto abre um diálogo no
//! PGStudio com o SQL. O que volta para o Claude passa pelo
//! [`collect::Collector`], com limite de linhas e de bytes.

pub mod agent;
pub mod catalog;
pub mod chat_agent;
pub mod codex;
pub mod collect;
pub mod commit;
pub mod endpoint;
pub mod hub;
pub mod llm;
pub mod mcp;
pub mod naming;
pub mod safety;
pub mod tools;

pub use hub::{Client, Inbox, Origin, Pending, Reply, Responder};
pub use safety::{Safety, classify};
pub use tools::{EditMode, ToolCall, ToolSpec};
