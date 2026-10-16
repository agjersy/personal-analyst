//! Agent tools (PROTOCOL 6): registry with JSON schemas, proposals for edits, dispatch to the
//! local filesystem, ripgrep, the kernel, `uv`, and git, with 16 KiB result capping.

pub mod dispatch;
pub mod proposals;
pub mod registry;

pub use dispatch::{ToolContext, ToolError, ToolOutcome};
pub use registry::{tool_class, ToolClass, ToolSpec};
