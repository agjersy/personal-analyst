//! Kernel client (PROTOCOL 1): transports, JSON-RPC framing, request/response matching,
//! notifications, typed helpers, and reconnect with `run.attach`.

pub mod client;
pub mod transport;

pub use client::{KernelClient, KernelError};
pub use transport::{
    resolve_tooling_python, KernelArgs, KernelTransport, LocalStdio, SpawnedKernel, SshStdio,
};
