//! `ServiceError`: the service-level error, mapped one-to-one onto IPC.md error codes.

use crate::kernel::KernelError;
use analyst_protocol::rpc::codes;
use analyst_protocol::ui::{IpcError, IpcErrorCode};
use serde_json::Value;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct ServiceError {
    pub code: IpcErrorCode,
    pub message: String,
    pub detail: Option<Value>,
}

pub type ServiceResult<T> = Result<T, ServiceError>;

impl ServiceError {
    pub fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        ServiceError {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::NotFound, m)
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::Conflict, m)
    }
    pub fn policy_denied(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::PolicyDenied, m)
    }
    pub fn kernel_offline(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::KernelOffline, m)
    }
    pub fn target_offline(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::TargetOffline, m)
    }
    pub fn invalid(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::Invalid, m)
    }
    pub fn io(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::Io, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(IpcErrorCode::Internal, m)
    }
}

impl From<ServiceError> for IpcError {
    fn from(e: ServiceError) -> Self {
        IpcError {
            code: e.code,
            message: e.message,
            detail: e.detail,
        }
    }
}

impl From<std::io::Error> for ServiceError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => ServiceError::not_found(e.to_string()),
            _ => ServiceError::io(e.to_string()),
        }
    }
}

impl From<anyhow::Error> for ServiceError {
    fn from(e: anyhow::Error) -> Self {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::NotFound {
                return ServiceError::not_found(format!("{e:#}"));
            }
        }
        ServiceError::internal(format!("{e:#}"))
    }
}

impl From<serde_json::Error> for ServiceError {
    fn from(e: serde_json::Error) -> Self {
        ServiceError::internal(format!("serialization: {e}"))
    }
}

impl From<KernelError> for ServiceError {
    fn from(e: KernelError) -> Self {
        match e {
            KernelError::Rpc(r) => match r.code {
                codes::RUN_NOT_FOUND | codes::SCRIPT_NOT_FOUND => {
                    ServiceError::not_found(r.to_string())
                }
                codes::INVALID_PARAMS | codes::PATH_OUTSIDE_ROOT => {
                    ServiceError::invalid(r.to_string())
                }
                codes::SANDBOX_UNAVAILABLE | codes::TOOL_MISSING => {
                    ServiceError::internal(r.to_string())
                }
                _ => ServiceError::internal(r.to_string()),
            },
            KernelError::Disconnected | KernelError::Exited => {
                ServiceError::kernel_offline(e.to_string())
            }
            KernelError::Io(io) => ServiceError::io(io.to_string()),
            KernelError::Decode(d) => ServiceError::internal(d.to_string()),
            KernelError::Other(o) => ServiceError::internal(o),
        }
    }
}

impl From<crate::tools::ToolError> for ServiceError {
    fn from(e: crate::tools::ToolError) -> Self {
        ServiceError::internal(e.message)
    }
}
