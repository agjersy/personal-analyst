//! Desktop IPC types (docs/IPC.md sections 1 to 3): command results, session events, and the
//! structured error. Every type here, and every kernel/transcript type they reuse, derives
//! `ts_rs::TS`; the test `export_typescript_bindings` writes the bindings to
//! `apps/desktop/src/ipc/generated/` together with an `index.ts` that re-exports them.

use crate::config::{
    AskPolicy, EditPolicy, NetworkPolicy, PolicyConfig, ProviderKind, RunPolicy, TargetKind,
    ToolSupport,
};
use crate::evals::Mode;
use crate::kernel::{Artifact, Diagnostic, FigureSidecar, LibFunction, RunMeta, ScriptInfo};
use crate::transcript::{DecidedBy, Decision, ProposalKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub type SessionId = String;
pub type ConversationId = String;
pub type RunId = String;
pub type TerminalId = String;
pub type ProposalId = String;
pub type TaskId = String;
pub type TurnId = String;

// ---- section 1: errors ----

/// Error codes of IPC.md section 1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    NotFound,
    Conflict,
    PolicyDenied,
    KernelOffline,
    TargetOffline,
    Invalid,
    Io,
    Internal,
}

impl IpcErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            IpcErrorCode::NotFound => "not_found",
            IpcErrorCode::Conflict => "conflict",
            IpcErrorCode::PolicyDenied => "policy_denied",
            IpcErrorCode::KernelOffline => "kernel_offline",
            IpcErrorCode::TargetOffline => "target_offline",
            IpcErrorCode::Invalid => "invalid",
            IpcErrorCode::Io => "io",
            IpcErrorCode::Internal => "internal",
        }
    }
}

/// The structured error every command may return.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct IpcError {
    pub code: IpcErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub detail: Option<Value>,
}

impl IpcError {
    pub fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        IpcError {
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

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for IpcError {}

// ---- section 2: types ----

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "lowercase")]
pub enum KernelState {
    Stopped,
    Starting,
    Ready,
    Error,
    Offline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct AppInfo {
    pub version: String,
    pub home_abs: String,
    pub platform: Platform,
    pub dev_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct SessionSummary {
    pub id: SessionId,
    /// Repo directory name unless renamed.
    pub name: String,
    pub repo_abs: String,
    /// Target name, `local` by default.
    pub target: String,
    /// `provider/id`.
    pub model: Option<String>,
    pub created_at: String,
    pub last_opened_at: String,
    pub open: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
pub struct PolicyView {
    pub edits: EditPolicy,
    pub runs: RunPolicy,
    pub packages: AskPolicy,
    pub shell: AskPolicy,
    pub network: NetworkPolicy,
    pub git: AskPolicy,
    pub protected: Vec<String>,
    pub max_steps: u32,
}

impl From<&PolicyConfig> for PolicyView {
    fn from(c: &PolicyConfig) -> Self {
        PolicyView {
            edits: c.edits,
            runs: c.runs,
            packages: c.packages,
            shell: c.shell,
            network: c.network,
            git: c.git,
            protected: c.protected.clone(),
            max_steps: c.max_steps,
        }
    }
}

impl From<&PolicyView> for PolicyConfig {
    fn from(v: &PolicyView) -> Self {
        PolicyConfig {
            edits: v.edits,
            runs: v.runs,
            packages: v.packages,
            shell: v.shell,
            network: v.network,
            git: v.git,
            protected: v.protected.clone(),
            max_steps: v.max_steps,
        }
    }
}

/// `Partial<PolicyView>` (argument of `session_set_policy`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(default)]
pub struct PolicyPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub edits: Option<EditPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub runs: Option<RunPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub packages: Option<AskPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shell: Option<AskPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub network: Option<NetworkPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub git: Option<AskPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub protected: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_steps: Option<u32>,
}

impl PolicyPatch {
    /// Apply the patch to a policy config.
    pub fn apply(&self, c: &mut PolicyConfig) {
        if let Some(v) = self.edits {
            c.edits = v;
        }
        if let Some(v) = self.runs {
            c.runs = v;
        }
        if let Some(v) = self.packages {
            c.packages = v;
        }
        if let Some(v) = self.shell {
            c.shell = v;
        }
        if let Some(v) = self.network {
            c.network = v;
        }
        if let Some(v) = self.git {
            c.git = v;
        }
        if let Some(v) = &self.protected {
            c.protected = v.clone();
        }
        if let Some(v) = self.max_steps {
            c.max_steps = v;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct ConversationSummary {
    pub id: ConversationId,
    pub title: String,
    pub model: Option<String>,
    pub mode: Mode,
    pub created_at: String,
    pub turns: u32,
    pub busy: bool,
}

/// `SessionInfo extends SessionSummary`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct SessionInfo {
    #[serde(flatten)]
    pub summary: SessionSummary,
    pub policy: PolicyView,
    pub kernel: KernelState,
    pub sandbox: Option<String>,
    pub python_abs: Option<String>,
    pub conversations: Vec<ConversationSummary>,
    pub active_conversation: ConversationId,
    pub target_kind: TargetKind,
    pub target_host: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Dir,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    pub size: Option<u64>,
    /// `None` = not expanded.
    pub children: Option<Vec<FileEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    /// SHA-256 hex of the bytes.
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct WriteResult {
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct SearchHit {
    pub path: String,
    pub line: u64,
    pub col: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct IndexResult {
    pub scripts: Vec<ScriptInfo>,
    pub library: Vec<LibFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct RunView {
    pub meta: RunMeta,
    pub target: String,
    pub artifacts: Vec<Artifact>,
    /// Last 4 KiB.
    pub stdout_tail: String,
    /// Last 4 KiB.
    pub stderr_tail: String,
}

/// `RunView & { stdout: string; stderr: string }` (result of `run_get`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct RunDetail {
    #[serde(flatten)]
    pub view: RunView,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
pub enum FigureFormat {
    Png,
    Svg,
    Pdf,
}

impl FigureFormat {
    pub fn from_path(path: &str) -> Option<FigureFormat> {
        let ext = path.rsplit('.').next()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(FigureFormat::Png),
            "svg" => Some(FigureFormat::Svg),
            "pdf" => Some(FigureFormat::Pdf),
            _ => None,
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            FigureFormat::Png => "image/png",
            FigureFormat::Svg => "image/svg+xml",
            FigureFormat::Pdf => "application/pdf",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct FigureView {
    pub run_id: RunId,
    pub script: String,
    /// Repo-relative, e.g. `out/fit_decay/<run>/figures/residuals.png`.
    pub path: String,
    pub name: String,
    pub format: FigureFormat,
    pub sidecar: Option<FigureSidecar>,
    pub created_at: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct FigureGroup {
    pub run: RunView,
    pub figures: Vec<FigureView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct FigureData {
    pub mime: String,
    pub base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct ModelCapabilitiesView {
    pub context_window: u32,
    pub tools: ToolSupport,
    pub vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct ModelEntry {
    /// `provider/id`.
    pub name: String,
    pub provider: String,
    pub id: String,
    pub kind: ProviderKind,
    pub local: bool,
    pub capabilities: Option<ModelCapabilitiesView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct TargetInfo {
    pub name: String,
    pub kind: TargetKind,
    pub host: Option<String>,
    pub remote_root: Option<String>,
    pub sandbox: String,
    pub data_dirs: Vec<String>,
    pub bootstrapped: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProposalReason {
    Review,
    Ask,
    OutOfScope,
    Protected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct ProposalView {
    pub id: ProposalId,
    pub conversation: ConversationId,
    pub kind: ProposalKind,
    pub path: Option<String>,
    pub summary: String,
    /// Full file contents for edit (null for create).
    pub original: Option<String>,
    pub proposed: Option<String>,
    /// Unified diff for display.
    pub diff: Option<String>,
    pub command: Option<String>,
    pub packages: Option<Vec<String>>,
    pub message: Option<String>,
    pub in_scope: bool,
    pub reason: ProposalReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum ProposalDecision {
    /// `content`: the file as it should be written after per-hunk review; omitted = proposed as is.
    Accept {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        content: Option<String>,
    },
    Reject {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Attachment {
    File {
        path: String,
    },
    Selection {
        path: String,
        start_line: u64,
        end_line: u64,
        text: String,
    },
    Figure {
        path: String,
    },
}

// ---- section 3: events ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct TurnSummaryView {
    pub text: String,
    pub files_changed: Vec<String>,
    pub runs: Vec<RunId>,
    pub figures: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

// A wire type: every event is serialized as soon as it is produced, so the size spread between
// `session_changed` (a whole `SessionInfo`) and `index_changed` does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    AgentText {
        conversation: ConversationId,
        turn: TurnId,
        delta: String,
    },
    AgentToolCall {
        conversation: ConversationId,
        turn: TurnId,
        id: String,
        name: String,
        #[ts(type = "unknown")]
        arguments: Value,
    },
    AgentToolResult {
        conversation: ConversationId,
        turn: TurnId,
        id: String,
        ok: bool,
        summary: String,
    },
    Proposal {
        proposal: ProposalView,
    },
    ProposalResolved {
        proposal_id: ProposalId,
        decision: Decision,
        by: DecidedBy,
    },
    TurnStarted {
        conversation: ConversationId,
        turn: TurnId,
    },
    TurnDone {
        conversation: ConversationId,
        turn: TurnId,
        summary: TurnSummaryView,
    },
    TurnError {
        conversation: ConversationId,
        turn: TurnId,
        message: String,
    },
    Usage {
        conversation: ConversationId,
        input_tokens: u64,
        output_tokens: u64,
        context_window: u64,
        context_used: u64,
    },
    RunStarted {
        run: RunView,
    },
    RunOutput {
        run_id: RunId,
        stream: OutputStream,
        chunk: String,
    },
    RunArtifact {
        run_id: RunId,
        artifact: Artifact,
        figure: Option<FigureView>,
    },
    RunDone {
        run: RunView,
    },
    /// Replaces diagnostics for the listed paths.
    Diagnostics {
        paths: Vec<String>,
        diagnostics: Vec<Diagnostic>,
    },
    /// Created, modified, or deleted on disk by anyone.
    FilesChanged {
        paths: Vec<String>,
    },
    IndexChanged,
    /// UTF-8 text; invalid sequences replaced.
    TerminalData {
        terminal_id: TerminalId,
        data: String,
    },
    TerminalExit {
        terminal_id: TerminalId,
        code: Option<i64>,
    },
    KernelState {
        state: KernelState,
        message: Option<String>,
    },
    PackagesChanged,
    TaskLog {
        task_id: TaskId,
        line: String,
    },
    TaskDone {
        task_id: TaskId,
        ok: bool,
        message: Option<String>,
    },
    /// After target, model, mode, or policy changes.
    SessionChanged {
        info: SessionInfo,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{EqCheckResult, OutListResult, OutReadResult, PackageInfo};
    use crate::transcript::Event;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn events_use_documented_tags_and_shapes() {
        let e = SessionEvent::RunOutput {
            run_id: "r".into(),
            stream: OutputStream::Stderr,
            chunk: "x".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            json!({"type":"run_output","run_id":"r","stream":"stderr","chunk":"x"})
        );
        let v = serde_json::to_value(SessionEvent::IndexChanged).unwrap();
        assert_eq!(v, json!({"type":"index_changed"}));
        let d: ProposalDecision = serde_json::from_value(json!({"decision":"accept"})).unwrap();
        assert_eq!(d, ProposalDecision::Accept { content: None });
        let d: ProposalDecision =
            serde_json::from_value(json!({"decision":"reject","reason":"no"})).unwrap();
        assert!(matches!(d, ProposalDecision::Reject { reason: Some(r) } if r == "no"));
        let a: Attachment = serde_json::from_value(
            json!({"kind":"selection","path":"a.py","start_line":1,"end_line":2,"text":"t"}),
        )
        .unwrap();
        assert!(matches!(a, Attachment::Selection { .. }));
        let err = IpcError::conflict("changed").with_detail(json!({"hash":"h"}));
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "conflict");
        assert_eq!(v["detail"]["hash"], "h");
        let plain = serde_json::to_value(IpcError::io("x")).unwrap();
        assert!(plain.get("detail").is_none());
        let p: PolicyPatch = serde_json::from_value(json!({"edits":"auto"})).unwrap();
        let mut c = PolicyConfig::default();
        p.apply(&mut c);
        assert_eq!(c.edits, EditPolicy::Auto);
        assert_eq!(c.runs, RunPolicy::Auto);
    }

    /// Writes every binding to `apps/desktop/src/ipc/generated/` plus `index.ts`.
    #[test]
    fn export_typescript_bindings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/ipc/generated")
            .canonicalize()
            .or_else(|_| {
                let d = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../apps/desktop/src/ipc/generated");
                std::fs::create_dir_all(&d).unwrap();
                d.canonicalize()
            })
            .unwrap();
        // The directory holds ts-rs output only: start clean so stale files never linger.
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "ts").unwrap_or(false) {
                std::fs::remove_file(&p).unwrap();
            }
        }
        let cfg = ts_rs::Config::new()
            .with_out_dir(&dir)
            .with_large_int("number");
        // Root types; dependencies are exported alongside.
        AppInfo::export_all(&cfg).unwrap();
        SessionSummary::export_all(&cfg).unwrap();
        SessionInfo::export_all(&cfg).unwrap();
        PolicyView::export_all(&cfg).unwrap();
        PolicyPatch::export_all(&cfg).unwrap();
        ConversationSummary::export_all(&cfg).unwrap();
        FileEntry::export_all(&cfg).unwrap();
        FileContent::export_all(&cfg).unwrap();
        WriteResult::export_all(&cfg).unwrap();
        SearchHit::export_all(&cfg).unwrap();
        IndexResult::export_all(&cfg).unwrap();
        RunView::export_all(&cfg).unwrap();
        RunDetail::export_all(&cfg).unwrap();
        FigureView::export_all(&cfg).unwrap();
        FigureGroup::export_all(&cfg).unwrap();
        FigureData::export_all(&cfg).unwrap();
        EqCheckResult::export_all(&cfg).unwrap();
        PackageInfo::export_all(&cfg).unwrap();
        ModelEntry::export_all(&cfg).unwrap();
        TargetInfo::export_all(&cfg).unwrap();
        ProposalView::export_all(&cfg).unwrap();
        ProposalDecision::export_all(&cfg).unwrap();
        Attachment::export_all(&cfg).unwrap();
        SessionEvent::export_all(&cfg).unwrap();
        IpcError::export_all(&cfg).unwrap();
        Event::export_all(&cfg).unwrap();
        OutListResult::export_all(&cfg).unwrap();
        OutReadResult::export_all(&cfg).unwrap();
        crate::kernel::SimilarPair::export_all(&cfg).unwrap();
        crate::kernel::Equation::export_all(&cfg).unwrap();

        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let stem = p.file_stem()?.to_string_lossy().to_string();
                (p.extension()? == "ts" && stem != "index").then_some(stem)
            })
            .collect();
        names.sort();
        let mut index = String::from(
            "// This file was generated by analyst-protocol (`cargo test -p analyst-protocol`). Do not edit it manually.\n\n",
        );
        for alias in [
            "SessionId",
            "ConversationId",
            "RunId",
            "TerminalId",
            "ProposalId",
            "TaskId",
            "TurnId",
        ] {
            index.push_str(&format!("export type {alias} = string;\n"));
        }
        index.push('\n');
        for n in &names {
            index.push_str(&format!("export type {{ {n} }} from \"./{n}\";\n"));
        }
        std::fs::write(dir.join("index.ts"), index).unwrap();
        assert!(names.iter().any(|n| n == "SessionEvent"));
        assert!(names.iter().any(|n| n == "TranscriptEvent"));
        assert!(names.iter().any(|n| n == "EqCheck"));
    }
}
