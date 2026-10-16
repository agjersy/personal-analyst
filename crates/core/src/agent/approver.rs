//! Approval of policy `Ask` decisions.

use crate::tools::proposals::Proposal;
use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub tool: String,
    pub args: Value,
    pub reason: String,
    /// Present for edit-class tools: summary and diff to show.
    pub proposal: Option<Proposal>,
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, req: &ApprovalRequest) -> bool;
}

/// Accepts everything (evals, `--auto`-style runs).
pub struct AutoApprover;

#[async_trait]
impl Approver for AutoApprover {
    async fn approve(&self, _req: &ApprovalRequest) -> bool {
        true
    }
}

/// Rejects everything (tests).
pub struct RejectAllApprover;

#[async_trait]
impl Approver for RejectAllApprover {
    async fn approve(&self, _req: &ApprovalRequest) -> bool {
        false
    }
}

/// Records every request and answers with a fixed decision (tests).
pub struct RecordingApprover {
    pub decision: bool,
    pub seen: std::sync::Mutex<Vec<ApprovalRequest>>,
}

impl RecordingApprover {
    pub fn new(decision: bool) -> Self {
        RecordingApprover {
            decision,
            seen: std::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl Approver for RecordingApprover {
    async fn approve(&self, req: &ApprovalRequest) -> bool {
        if let Ok(mut s) = self.seen.lock() {
            s.push(req.clone());
        }
        self.decision
    }
}
