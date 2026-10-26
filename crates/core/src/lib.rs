//! analyst-core: sessions, agent loop, policy, providers, kernel client, targets, sync,
//! checkpoints, tools, and the evaluation harness. See docs/PROTOCOL.md and docs/ARCHITECTURE.md.

pub mod agent;
pub mod checkpoint;
pub mod config;
pub mod evals;
pub mod init;
pub mod kernel;
pub mod policy;
pub mod providers;
pub mod service;
pub mod session;
pub mod target;
pub mod tools;
pub mod util;

pub use analyst_protocol as protocol;

/// Version string reported in `kernel.hello`.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
