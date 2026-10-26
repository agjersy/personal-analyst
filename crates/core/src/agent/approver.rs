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

/// The outcome of an approval. `Accept { content: Some(..) }` carries the file content the user
/// wants written instead of the proposed one (per-hunk review in the desktop app).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Accept { content: Option<String> },
    Reject { reason: Option<String> },
}

impl ApprovalDecision {
    pub fn accepted(&self) -> bool {
        matches!(self, ApprovalDecision::Accept { .. })
    }
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, req: &ApprovalRequest) -> bool;

    /// Richer decision used by the agent loop; the default delegates to [`Approver::approve`].
    async fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision {
        if self.approve(req).await {
            ApprovalDecision::Accept { content: None }
        } else {
            ApprovalDecision::Reject { reason: None }
        }
    }
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
