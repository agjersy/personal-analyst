//! JSON-RPC 2.0 envelope types (PROTOCOL 1.1, 1.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request id: string or integer per the JSON-RPC spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl From<i64> for RequestId {
    fn from(v: i64) -> Self {
        RequestId::Number(v)
    }
}

impl From<String> for RequestId {
    fn from(v: String) -> Self {
        RequestId::String(v)
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(n) => write!(f, "{n}"),
            RequestId::String(s) => write!(f, "{s}"),
        }
    }
}

/// Core -> kernel request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Value) -> Self {
        Request {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params: Some(params),
        }
    }
}

/// Error object carried by a response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// The `detail` field of `error.data`, if present.
    pub fn detail(&self) -> Option<&str> {
        self.data.as_ref()?.get("detail")?.as_str()
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rpc error {}: {}", self.code, self.message)?;
        if let Some(d) = self.detail() {
            write!(f, " ({d})")?;
        }
        Ok(())
    }
}

impl std::error::Error for RpcError {}

/// Application error codes (PROTOCOL 1.2).
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const RUN_NOT_FOUND: i64 = 1000;
    pub const SANDBOX_UNAVAILABLE: i64 = 1001;
    pub const PATH_OUTSIDE_ROOT: i64 = 1002;
    pub const TIMEOUT: i64 = 1003;
    pub const SCRIPT_NOT_FOUND: i64 = 1004;
    pub const TOOL_MISSING: i64 = 1005;
}

/// Any line the kernel writes on stdout: a response to a request, or a notification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Incoming {
    Response(Response),
    Notification(Notification),
}

/// Kernel -> core response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Kernel -> core notification (no id).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_distinguishes_response_and_notification() {
        let r: Incoming =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(r, Incoming::Response(_)));
        let n: Incoming = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"run.stdout","params":{"run_id":"x","chunk":"a","offset":0}}"#,
        )
        .unwrap();
        assert!(matches!(n, Incoming::Notification(_)));
        let e: Incoming = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":"abc","error":{"code":1000,"message":"run not found","data":{"detail":"no such run"}}}"#,
        )
        .unwrap();
        match e {
            Incoming::Response(r) => {
                let err = r.error.unwrap();
                assert_eq!(err.code, codes::RUN_NOT_FOUND);
                assert_eq!(err.detail(), Some("no such run"));
            }
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn request_round_trip() {
        let req = Request::new(
            7,
            "kernel.hello",
            serde_json::json!({"core_version":"0.0.1"}),
        );
        let s = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(req, back);
    }
}
