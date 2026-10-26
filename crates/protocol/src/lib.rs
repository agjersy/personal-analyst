//! analyst-protocol: serde types for every boundary fixed in docs/PROTOCOL.md.
//!
//! - [`rpc`]: JSON-RPC 2.0 framing used between the core and the kernel (PROTOCOL 1.1, 1.2).
//! - [`kernel`]: kernel request parameters, results, and notifications (PROTOCOL 1.3, 1.6).
//! - [`config`]: global and project configuration files (PROTOCOL 2.1, 2.2).
//! - [`session`]: session store records (PROTOCOL 2.3).
//! - [`transcript`]: conversation transcript events (PROTOCOL 5).
//! - [`model`]: model provider types, `ModelEvent`, `ChatRequest` (PROTOCOL 4).
//! - [`evals`]: evaluation task and scorecard types (PROTOCOL 8).
//! - [`ui`]: desktop IPC types with TypeScript export (docs/IPC.md).

pub mod config;
pub mod evals;
pub mod kernel;
pub mod model;
pub mod rpc;
pub mod session;
pub mod transcript;
pub mod ui;

/// The protocol version both sides must agree on (`kernel.hello`).
pub const PROTOCOL_VERSION: u32 = 1;

/// Every tool result shown to the model is capped at this many bytes (PROTOCOL 6).
pub const TOOL_RESULT_CAP_BYTES: usize = 16 * 1024;
