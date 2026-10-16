//! Kernel method parameters, results, and notifications (PROTOCOL 1.3, 1.4, 1.6).
//!
//! All paths are strings relative to the repo root with forward slashes.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---- lifecycle ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelloParams {
    pub core_version: String,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelloResult {
    pub kernel_version: String,
    pub protocol_version: u32,
    pub host: String,
    pub os: String,
    pub sandbox: String,
    pub python: String,
    pub root: String,
    pub runs_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OkResult {
    pub ok: bool,
}

// ---- environment ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvInfo {
    pub python: String,
    pub version: String,
    pub venv: Option<String>,
    #[serde(default)]
    pub site_packages: Vec<String>,
    pub sandbox: String,
    pub host: String,
    pub os: String,
    #[serde(default)]
    pub uv: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackagesResult {
    pub packages: Vec<PackageInfo>,
}

// ---- indexes ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ScriptsIndexParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub globs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScriptsIndexResult {
    pub scripts: Vec<ScriptInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScriptInfo {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub has_main: bool,
    pub has_main_guard: bool,
    #[serde(default)]
    pub options: Vec<OptionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptionInfo {
    pub flags: Vec<String>,
    pub dest: String,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub choices: Option<Vec<Value>>,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LibIndexParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibIndexResult {
    pub functions: Vec<LibFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Location {
    pub path: String,
    pub line: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibFunction {
    pub module: String,
    pub qualname: String,
    pub path: String,
    pub line: u64,
    pub signature: String,
    #[serde(default)]
    pub doc_summary: Option<String>,
    pub has_math: bool,
    #[serde(default)]
    pub callers: Vec<Location>,
}

// ---- runs ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RunStartParams {
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunStartResult {
    pub run_id: String,
    pub out_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RunAttachParams {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunAttachResult {
    pub status: RunStatus,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunIdParams {
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RunsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunsListResult {
    pub runs: Vec<RunMeta>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Done,
    Cancelled,
    Timeout,
    Failed,
}

impl RunStatus {
    pub fn is_finished(self) -> bool {
        !matches!(self, RunStatus::Running)
    }
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RunStatus::Running => "running",
            RunStatus::Done => "done",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Timeout => "timeout",
            RunStatus::Failed => "failed",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunMeta {
    pub run_id: String,
    pub script: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub status: RunStatus,
    pub exit_code: Option<i64>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub out_dir: String,
    pub sandbox: String,
    #[serde(default)]
    pub tree_hash: Option<String>,
    #[serde(default)]
    pub pid: Option<i64>,
    pub host: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Figure,
    Table,
    Array,
    Json,
    Text,
    File,
}

impl ArtifactKind {
    /// Artifact kind by extension (PROTOCOL 1.4 step 5).
    pub fn from_path(path: &str) -> ArtifactKind {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" | "svg" | "pdf" => ArtifactKind::Figure,
            "csv" | "tsv" | "parquet" => ArtifactKind::Table,
            "npy" | "npz" => ArtifactKind::Array,
            "json" => ArtifactKind::Json,
            "txt" | "md" | "log" => ArtifactKind::Text,
            _ => ArtifactKind::File,
        }
    }

    /// Kinds the core pulls back from remote targets.
    pub fn is_rendered(self) -> bool {
        matches!(
            self,
            ArtifactKind::Figure | ArtifactKind::Json | ArtifactKind::Text
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: String,
    pub size: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar: Option<FigureSidecar>,
    pub rendered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxesInfo {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub xlabel: Option<String>,
    #[serde(default)]
    pub ylabel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FigureSidecar {
    pub run_id: String,
    pub script: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub axes: Vec<AxesInfo>,
    pub created_at: String,
    pub width_in: f64,
    pub height_in: f64,
    pub dpi: f64,
}

/// `run.stdout` / `run.stderr` notification params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunOutput {
    pub run_id: String,
    pub chunk: String,
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunArtifact {
    pub run_id: String,
    pub artifact: Artifact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunDone {
    pub run_id: String,
    pub status: RunStatus,
    pub exit_code: Option<i64>,
    pub duration_s: f64,
}

/// Typed view of every kernel -> core notification.
#[derive(Debug, Clone, PartialEq)]
pub enum KernelNotification {
    Stdout(RunOutput),
    Stderr(RunOutput),
    Artifact(RunArtifact),
    Done(RunDone),
    /// A notification with a method this version does not know.
    Other {
        method: String,
        params: Value,
    },
}

impl KernelNotification {
    /// Parse a raw notification. Unknown methods become `Other`; malformed params for a
    /// known method are an error.
    pub fn parse(method: &str, params: Value) -> Result<Self, serde_json::Error> {
        Ok(match method {
            "run.stdout" => KernelNotification::Stdout(serde_json::from_value(params)?),
            "run.stderr" => KernelNotification::Stderr(serde_json::from_value(params)?),
            "run.artifact" => KernelNotification::Artifact(serde_json::from_value(params)?),
            "run.done" => KernelNotification::Done(serde_json::from_value(params)?),
            other => KernelNotification::Other {
                method: other.to_string(),
                params,
            },
        })
    }

    pub fn run_id(&self) -> Option<&str> {
        match self {
            KernelNotification::Stdout(o) | KernelNotification::Stderr(o) => Some(&o.run_id),
            KernelNotification::Artifact(a) => Some(&a.run_id),
            KernelNotification::Done(d) => Some(&d.run_id),
            KernelNotification::Other { .. } => None,
        }
    }
}

// ---- quick execution ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecSnippetParams {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecSnippetResult {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub value_repr: Option<String>,
}

// ---- outputs ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OutListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutEntry {
    pub path: String,
    pub size: u64,
    pub kind: ArtifactKind,
    pub mtime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutListResult {
    pub entries: Vec<OutEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutReadParams {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutReadResult {
    pub kind: ArtifactKind,
    pub preview: String,
    pub truncated: bool,
    #[serde(default)]
    pub summary: Option<Value>,
}

// ---- equations ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqExtractParams {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqExtractResult {
    pub equations: Vec<Equation>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EquationTier {
    Derived,
    Declared,
    Latex,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EquationStatus {
    Ok,
    Unsupported,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Equation {
    pub function: String,
    pub qualname: String,
    pub line: u64,
    pub tier: EquationTier,
    #[serde(default)]
    pub latex: Option<String>,
    #[serde(default)]
    pub source_expr: Option<String>,
    pub status: EquationStatus,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqCheckParams {
    pub path: String,
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trials: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges: Option<BTreeMap<String, [f64; 2]>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EqCheckStatus {
    Agree,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Counterexample {
    pub inputs: BTreeMap<String, f64>,
    pub expected: f64,
    pub actual: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqCheckResult {
    pub status: EqCheckStatus,
    pub trials: u64,
    #[serde(default)]
    pub max_abs_err: Option<f64>,
    #[serde(default)]
    pub max_rel_err: Option<f64>,
    #[serde(default)]
    pub counterexample: Option<Counterexample>,
    #[serde(default)]
    pub reason: Option<String>,
}

// ---- contract and lint ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PathsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagnosticsResult {
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub path: String,
    pub line: u64,
    pub col: u64,
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub related: Vec<Location>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SimilarParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimilarResult {
    pub pairs: Vec<SimilarPair>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum SimilarKind {
    Duplicate,
    NearDuplicate,
    UnusedTwin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionRef {
    pub path: String,
    pub qualname: String,
    pub line: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimilarPair {
    pub kind: SimilarKind,
    pub score: f64,
    pub a: FunctionRef,
    pub b: FunctionRef,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_meta_round_trip() {
        let v = json!({
            "run_id": "20261016-181502-3fa2c1", "script": "scripts/fit.py", "args": ["--n", "3"],
            "status": "running", "exit_code": null, "started_at": "2026-10-16T18:15:02Z",
            "finished_at": null, "out_dir": "out/fit/20261016-181502-3fa2c1", "sandbox": "bwrap",
            "tree_hash": null, "pid": 1234, "host": "lab"
        });
        let m: RunMeta = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(m.status, RunStatus::Running);
        assert_eq!(serde_json::to_value(&m).unwrap(), v);
    }

    #[test]
    fn artifact_with_sidecar_round_trip() {
        let a = Artifact {
            kind: ArtifactKind::Figure,
            path: "out/fit/r1/figures/residuals.png".into(),
            size: 1024,
            created_at: "2026-10-16T18:15:03Z".into(),
            sidecar: Some(FigureSidecar {
                run_id: "r1".into(),
                script: "scripts/fit.py".into(),
                args: vec![],
                title: Some("Residuals".into()),
                axes: vec![AxesInfo {
                    title: Some("Residuals".into()),
                    xlabel: Some("t".into()),
                    ylabel: None,
                }],
                created_at: "2026-10-16T18:15:03Z".into(),
                width_in: 6.4,
                height_in: 4.8,
                dpi: 100.0,
            }),
            rendered: true,
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: Artifact = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
        assert_eq!(ArtifactKind::from_path("x/y.NPZ"), ArtifactKind::Array);
        assert_eq!(ArtifactKind::from_path("x/y"), ArtifactKind::File);
        assert!(ArtifactKind::Json.is_rendered());
        assert!(!ArtifactKind::Table.is_rendered());
    }

    #[test]
    fn notification_parse() {
        let n = KernelNotification::parse(
            "run.done",
            json!({"run_id":"r","status":"done","exit_code":0,"duration_s":1.5}),
        )
        .unwrap();
        assert_eq!(n.run_id(), Some("r"));
        assert!(matches!(n, KernelNotification::Done(_)));
        let o = KernelNotification::parse("future.thing", json!({})).unwrap();
        assert!(matches!(o, KernelNotification::Other { .. }));
        assert!(KernelNotification::parse("run.done", json!({"bogus": 1})).is_err());
    }

    #[test]
    fn equation_and_diagnostic_round_trip() {
        let e: Equation = serde_json::from_value(json!({
            "function":"f","qualname":"f","line":3,"tier":"derived","latex":"x^2",
            "source_expr":"x**2","status":"ok","reason":null
        }))
        .unwrap();
        assert_eq!(e.tier, EquationTier::Derived);
        let d: Diagnostic = serde_json::from_value(json!({
            "path":"scripts/a.py","line":1,"col":0,"rule":"cli.main-guard","severity":"error",
            "message":"m","hint":null,"related":[]
        }))
        .unwrap();
        assert_eq!(d.severity, Severity::Error);
        let p: SimilarPair = serde_json::from_value(json!({
            "kind":"near-duplicate","score":0.9,
            "a":{"path":"a.py","qualname":"f","line":1},"b":{"path":"b.py","qualname":"g","line":2}
        }))
        .unwrap();
        assert_eq!(p.kind, SimilarKind::NearDuplicate);
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(back["kind"], "near-duplicate");
    }

    #[test]
    fn eq_check_result_round_trip() {
        let r: EqCheckResult = serde_json::from_value(json!({
            "status":"mismatch","trials":32,"max_abs_err":0.1,"max_rel_err":0.01,
            "counterexample":{"inputs":{"x":1.0},"expected":1.0,"actual":1.1},"reason":null
        }))
        .unwrap();
        assert_eq!(r.status, EqCheckStatus::Mismatch);
        assert_eq!(r.counterexample.as_ref().unwrap().inputs["x"], 1.0);
    }
}
