//! Tool execution. Every result is capped at 16 KiB for the model; larger results are stored
//! under the session's `artifacts/` and the model receives the head plus
//! `{truncated: true, artifact: "<path>"}`.

use super::proposals;
use crate::kernel::KernelClient;
use crate::session::Session;
use crate::target::{self, Target};
use crate::util::{normalize_rel_path, now_rfc3339, truncate_bytes};
use analyst_protocol::config::ProjectConfig;
use analyst_protocol::kernel::*;
use analyst_protocol::transcript::Event;
use analyst_protocol::TOOL_RESULT_CAP_BYTES;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(m: impl Into<String>) -> Self {
        ToolError { message: m.into() }
    }
}

impl From<anyhow::Error> for ToolError {
    fn from(e: anyhow::Error) -> Self {
        ToolError::new(format!("{e:#}"))
    }
}

impl From<crate::kernel::KernelError> for ToolError {
    fn from(e: crate::kernel::KernelError) -> Self {
        ToolError::new(e.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(e: serde_json::Error) -> Self {
        ToolError::new(format!("serialization: {e}"))
    }
}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        ToolError::new(e.to_string())
    }
}

/// A capped tool result as shown to the model.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub ok: bool,
    pub result: Value,
    pub truncated: bool,
}

/// Callback for live run output: `(stream, chunk)` with stream `stdout` or `stderr`.
pub type OutputSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Everything tools need.
pub struct ToolContext {
    pub repo: PathBuf,
    pub home: PathBuf,
    pub session: Session,
    pub conv_id: String,
    pub kernel: Option<KernelClient>,
    pub target: Target,
    pub project: ProjectConfig,
    pub control_dir: Option<PathBuf>,
    pub last_sync_hash: Mutex<Option<String>>,
    pub output_sink: Option<OutputSink>,
}

impl ToolContext {
    pub fn new(
        repo: &Path,
        home: &Path,
        session: Session,
        conv_id: &str,
        project: ProjectConfig,
    ) -> Self {
        ToolContext {
            repo: repo.to_path_buf(),
            home: home.to_path_buf(),
            session,
            conv_id: conv_id.to_string(),
            kernel: None,
            target: Target::Local,
            project,
            control_dir: None,
            last_sync_hash: Mutex::new(None),
            output_sink: None,
        }
    }

    fn kernel(&self) -> Result<&KernelClient, ToolError> {
        self.kernel.as_ref().ok_or_else(|| {
            ToolError::new("the kernel is not connected on this target (target offline)")
        })
    }

    /// Execute a tool by internal id. Edit tools are built and applied directly here; the agent
    /// builds proposals separately when it needs approval first.
    pub async fn dispatch(
        &self,
        call_id: &str,
        name: &str,
        args: &Value,
    ) -> Result<Value, ToolError> {
        match name {
            "fs.read" => self.fs_read(args),
            "fs.list" => self.fs_list(args),
            "fs.search" => self.fs_search(args).await,
            "scripts.index" => {
                let globs = Some(self.project.project.scripts.clone());
                Ok(serde_json::to_value(
                    self.kernel()?.scripts_index(globs).await?,
                )?)
            }
            "lib.index" => {
                let pkg = Some(self.project.project.library.clone());
                Ok(serde_json::to_value(self.kernel()?.lib_index(pkg).await?)?)
            }
            "runs.list" => {
                let limit = args.get("limit").and_then(Value::as_u64);
                match self.kernel() {
                    Ok(k) => Ok(serde_json::to_value(k.runs_list(limit).await?)?),
                    Err(_) => {
                        let mut runs = self.session.read_runs()?;
                        runs.reverse();
                        runs.truncate(limit.unwrap_or(20) as usize);
                        Ok(json!({"runs": runs, "source": "session"}))
                    }
                }
            }
            "runs.get" => {
                let id = str_arg(args, "run_id")?;
                match self.kernel() {
                    Ok(k) => Ok(serde_json::to_value(k.run_status(id).await?)?),
                    Err(_) => match self.session.find_run(id)? {
                        Some(r) => Ok(serde_json::to_value(r)?),
                        None => Err(ToolError::new(format!(
                            "run `{id}` not found in this session"
                        ))),
                    },
                }
            }
            "out.list" => {
                let p: OutListParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(self.kernel()?.out_list(p).await?)?)
            }
            "out.read" => {
                let p: OutReadParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(self.kernel()?.out_read(p).await?)?)
            }
            "eq.extract" => {
                let p: EqExtractParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(self.kernel()?.eq_extract(p).await?)?)
            }
            "eq.check" => {
                let p: EqCheckParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(self.kernel()?.eq_check(p).await?)?)
            }
            "contract.check" => {
                let p: PathsParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(
                    self.kernel()?.contract_check(p.paths).await?,
                )?)
            }
            "contract.similar" => {
                let p: SimilarParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(
                    self.kernel()?.contract_similar(p.threshold).await?,
                )?)
            }
            "lint.run" => {
                let p: PathsParams = serde_json::from_value(args.clone()).map_err(bad_args)?;
                Ok(serde_json::to_value(
                    self.kernel()?.lint_run(p.paths).await?,
                )?)
            }
            "fig.list" => self.fig_list(args),
            "target.info" => self.target_info().await,
            "fs.propose_edit" | "fs.propose_create" | "fs.propose_delete" => {
                let p = proposals::build(&self.repo, call_id, name, args)?;
                proposals::apply(&self.repo, &p)?;
                Ok(json!({"applied": true, "path": p.path, "summary": p.summary}))
            }
            "run.script" => self.run_script(args).await,
            "exec.snippet" => {
                let code = str_arg(args, "code")?;
                let t = args.get("timeout_s").and_then(Value::as_f64);
                Ok(serde_json::to_value(
                    self.kernel()?.exec_snippet(code, t).await?,
                )?)
            }
            "pkg.add" | "pkg.remove" | "pkg.sync" => self.pkg(name, args).await,
            "shell.run" => self.shell_run(args).await,
            "git.status" => {
                let out = crate::checkpoint::git(
                    &self.repo,
                    &["status", "--porcelain=v1", "--branch"],
                    None,
                )?;
                Ok(json!({"status": out}))
            }
            "git.diff" => {
                let mut a = vec!["diff", "--no-color"];
                let p;
                if let Some(path) = args.get("path").and_then(Value::as_str) {
                    p = normalize_rel_path(path).map_err(ToolError::new)?;
                    a.push("--");
                    a.push(&p);
                }
                let out = crate::checkpoint::git(&self.repo, &a, None)?;
                Ok(json!({"diff": out}))
            }
            "git.commit" => {
                let msg = str_arg(args, "message")?;
                crate::checkpoint::git(&self.repo, &["add", "-A"], None)?;
                let out = crate::checkpoint::git(&self.repo, &["commit", "-q", "-m", msg], None)?;
                let sha =
                    crate::checkpoint::git(&self.repo, &["rev-parse", "--short", "HEAD"], None)?;
                Ok(json!({"committed": true, "commit": sha.trim(), "output": out}))
            }
            "declare_scope" | "pin_fact" => Err(ToolError::new(format!(
                "`{name}` is handled by the agent loop, not dispatch"
            ))),
            other => Err(ToolError::new(format!("unknown tool `{other}`"))),
        }
    }

    /// Cap a result for the model; large results are spilled to the session artifacts dir.
    pub fn cap_result(&self, call_id: &str, result: Result<Value, ToolError>) -> ToolOutcome {
        match result {
            Err(e) => ToolOutcome {
                ok: false,
                result: json!({"error": e.message}),
                truncated: false,
            },
            Ok(v) => {
                let text = serde_json::to_string(&v).unwrap_or_default();
                if text.len() <= TOOL_RESULT_CAP_BYTES {
                    return ToolOutcome {
                        ok: true,
                        result: v,
                        truncated: false,
                    };
                }
                let pretty = serde_json::to_string_pretty(&v).unwrap_or(text);
                let name = format!("{}/{}.json", self.conv_id, call_id);
                match self.session.spill_artifact(&name, pretty.as_bytes()) {
                    Ok(path) => ToolOutcome {
                        ok: true,
                        result: json!({
                            "head": truncate_bytes(&pretty, TOOL_RESULT_CAP_BYTES * 3 / 4),
                            "truncated": true,
                            "artifact": path.display().to_string(),
                            "hint": "page the full result with fs.read on the artifact path"
                        }),
                        truncated: true,
                    },
                    Err(e) => ToolOutcome {
                        ok: true,
                        result: json!({
                            "head": truncate_bytes(&pretty, TOOL_RESULT_CAP_BYTES * 3 / 4),
                            "truncated": true,
                            "error": format!("could not store full result: {e}")
                        }),
                        truncated: true,
                    },
                }
            }
        }
    }

    // ---- filesystem ----

    /// Resolve a path argument: repo-relative, or an absolute path inside the session
    /// artifacts directory (spilled results).
    fn resolve_read_path(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let p = Path::new(raw);
        if p.is_absolute() {
            let art = self.session.artifacts_dir();
            let canon =
                std::fs::canonicalize(p).map_err(|e| ToolError::new(format!("`{raw}`: {e}")))?;
            let art_canon = std::fs::canonicalize(&art).unwrap_or(art);
            if canon.starts_with(&art_canon) {
                return Ok(canon);
            }
            return Err(ToolError::new(format!(
                "`{raw}` is outside the repo and the session artifacts directory"
            )));
        }
        let rel = normalize_rel_path(raw).map_err(ToolError::new)?;
        Ok(self.repo.join(rel))
    }

    fn fs_read(&self, args: &Value) -> Result<Value, ToolError> {
        let raw = str_arg(args, "path")?;
        let full = self.resolve_read_path(raw)?;
        let bytes = std::fs::read(&full).map_err(|e| ToolError::new(format!("`{raw}`: {e}")))?;
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                return Ok(json!({"path": raw, "binary": true, "size": e.as_bytes().len()}));
            }
        };
        let total = text.lines().count();
        let start = args
            .get("start_line")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let end = args
            .get("end_line")
            .and_then(Value::as_u64)
            .map(|e| e as usize)
            .unwrap_or(total)
            .min(total);
        let content: String = if start > end {
            String::new()
        } else {
            text.lines()
                .enumerate()
                .filter(|(i, _)| *i + 1 >= start && *i < end)
                .map(|(i, l)| format!("{:>5}| {l}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(
            json!({"path": raw, "start_line": start, "end_line": end, "total_lines": total, "content": content}),
        )
    }

    fn fs_list(&self, args: &Value) -> Result<Value, ToolError> {
        let raw = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let rel = if raw == "." || raw.is_empty() {
            String::new()
        } else {
            normalize_rel_path(raw).map_err(ToolError::new)?
        };
        let depth = args
            .get("depth")
            .and_then(Value::as_u64)
            .unwrap_or(2)
            .clamp(1, 6) as usize;
        let root = self.repo.join(&rel);
        if !root.is_dir() {
            return Err(ToolError::new(format!("`{raw}` is not a directory")));
        }
        let mut entries = Vec::new();
        for e in walkdir::WalkDir::new(&root)
            .min_depth(1)
            .max_depth(depth)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                let n = e.file_name().to_string_lossy();
                !(n == ".git"
                    || n == ".venv"
                    || n == "__pycache__"
                    || n == "node_modules"
                    || n == ".ruff_cache")
            })
            .flatten()
        {
            let p = e.path().strip_prefix(&self.repo).unwrap_or(e.path());
            let mut s = p.to_string_lossy().replace('\\', "/");
            if e.file_type().is_dir() {
                s.push('/');
            }
            entries.push(s);
            if entries.len() >= 2000 {
                break;
            }
        }
        Ok(json!({"path": raw, "entries": entries}))
    }

    async fn fs_search(&self, args: &Value) -> Result<Value, ToolError> {
        let pattern = str_arg(args, "pattern")?;
        let max = args
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(50) as usize;
        let mut cmd = tokio::process::Command::new("rg");
        cmd.args(["--json", "--no-messages", "-e", pattern])
            .arg("--glob")
            .arg("!.git")
            .arg("--glob")
            .arg("!.venv")
            .current_dir(&self.repo)
            .stdin(std::process::Stdio::null());
        if let Some(g) = args.get("glob").and_then(Value::as_str) {
            cmd.arg("--glob").arg(g);
        }
        cmd.arg(".");
        let out = cmd
            .output()
            .await
            .map_err(|e| ToolError::new(format!("running rg: {e} (is ripgrep installed?)")))?;
        let mut matches = Vec::new();
        let mut truncated = false;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v["type"] != "match" {
                continue;
            }
            if matches.len() >= max {
                truncated = true;
                break;
            }
            let d = &v["data"];
            let path = d["path"]["text"]
                .as_str()
                .unwrap_or("")
                .trim_start_matches("./");
            let text = d["lines"]["text"].as_str().unwrap_or("").trim_end();
            matches.push(json!({
                "path": path,
                "line": d["line_number"],
                "text": truncate_bytes(text, 400),
            }));
        }
        Ok(json!({"pattern": pattern, "matches": matches, "truncated": truncated}))
    }

    fn fig_list(&self, args: &Value) -> Result<Value, ToolError> {
        let want = args.get("run_id").and_then(Value::as_str);
        let mut seen = std::collections::BTreeSet::new();
        let mut runs: Vec<_> = self.session.read_runs()?;
        runs.reverse();
        let mut figures = Vec::new();
        for r in runs {
            if !seen.insert(r.meta.run_id.clone()) {
                continue;
            }
            if let Some(w) = want {
                if r.meta.run_id != w {
                    continue;
                }
            }
            let dir = self.repo.join(&r.meta.out_dir);
            let session_dir = self.session.pulled_artifact_path(&r.meta.run_id, "");
            for base in [dir, session_dir] {
                if !base.is_dir() {
                    continue;
                }
                for e in walkdir::WalkDir::new(&base).into_iter().flatten() {
                    let p = e.path();
                    if !p.is_file() {
                        continue;
                    }
                    let rel = p.to_string_lossy().to_string();
                    if ArtifactKind::from_path(&rel) != ArtifactKind::Figure {
                        continue;
                    }
                    let sidecar = p.with_extension("json");
                    let meta: Option<FigureSidecar> = std::fs::read_to_string(&sidecar)
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok());
                    figures.push(json!({
                        "run_id": r.meta.run_id,
                        "script": r.meta.script,
                        "path": p.strip_prefix(&self.repo).unwrap_or(p).to_string_lossy(),
                        "title": meta.as_ref().and_then(|m| m.title.clone()),
                        "axes": meta.as_ref().map(|m| m.axes.clone()),
                    }));
                }
            }
        }
        Ok(json!({"figures": figures}))
    }

    async fn target_info(&self) -> Result<Value, ToolError> {
        let hello = match &self.kernel {
            Some(k) => k.hello().await,
            None => None,
        };
        let connected = match &self.kernel {
            Some(k) => k.is_connected().await,
            None => false,
        };
        Ok(json!({
            "name": self.target.name(),
            "kind": if self.target.is_remote() { "ssh" } else { "local" },
            "host": self.target.host(),
            "connected": connected,
            "sandbox": hello.as_ref().map(|h| h.sandbox.clone()),
            "python": hello.as_ref().map(|h| h.python.clone()),
            "data_dirs": crate::config::effective_project(&self.project, self.target.name()).data_dirs,
            "target_data_dirs": self.target.data_dirs(),
        }))
    }

    // ---- runs ----

    /// Sync the mirror when the target is remote and the tree changed. Returns the tree hash.
    pub async fn sync_if_needed(&self) -> Result<Option<String>, ToolError> {
        if !self.target.is_remote() {
            return Ok(None);
        }
        let last = self.last_sync_hash.lock().await.clone();
        let report = target::sync::push(
            &self.repo,
            &self.target,
            self.control_dir.as_deref(),
            last.as_deref(),
            false,
        )
        .await?;
        if let Some(w) = &report.warning {
            tracing::warn!("{w}");
        }
        *self.last_sync_hash.lock().await = Some(report.tree_hash.clone());
        Ok(Some(report.tree_hash))
    }

    async fn run_script(&self, args: &Value) -> Result<Value, ToolError> {
        let script = str_arg(args, "script")?;
        let script = normalize_rel_path(script).map_err(ToolError::new)?;
        let run_args: Vec<String> = args
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let eff = crate::config::effective_project(&self.project, self.target.name());
        let timeout_s = args
            .get("timeout_s")
            .and_then(Value::as_f64)
            .unwrap_or(eff.timeout_s as f64);
        let network = args.get("network").and_then(Value::as_bool);
        let kernel = self.kernel()?.clone();
        let tree_hash = match self.sync_if_needed().await {
            Ok(h) => h,
            Err(e) => return Err(ToolError::new(format!("sync before run failed: {e}"))),
        };
        let tree_hash = match tree_hash {
            Some(h) => Some(h),
            None => target::sync::file_list(&self.repo)
                .ok()
                .map(|l| target::sync::tree_hash(&l)),
        };
        let mut rx = kernel.subscribe();
        let started = kernel
            .run_start(RunStartParams {
                script: script.clone(),
                args: Some(run_args.clone()),
                env: None,
                timeout_s: Some(timeout_s),
                network,
                tree_hash,
            })
            .await?;
        let run_id = started.run_id.clone();
        self.session.append_event(
            &self.conv_id,
            &Event::Run {
                ts: now_rfc3339(),
                run_id: run_id.clone(),
                script: script.clone(),
                args: run_args.clone(),
                target: self.target.name().to_string(),
            },
        )?;
        if let Ok(meta) = kernel.run_status(&run_id).await {
            let _ = self.session.append_run(&meta);
        }
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut artifacts: Vec<Artifact> = Vec::new();
        let mut done: Option<RunDone> = None;
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs_f64(timeout_s + 60.0);
        loop {
            let n = match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(n)) => n,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(_)) => break,
                Err(_) => break,
            };
            if n.run_id() != Some(run_id.as_str()) {
                continue;
            }
            match n {
                KernelNotification::Stdout(o) => {
                    if let Some(s) = &self.output_sink {
                        s("stdout", &o.chunk);
                    }
                    stdout.push_str(&o.chunk);
                }
                KernelNotification::Stderr(o) => {
                    if let Some(s) = &self.output_sink {
                        s("stderr", &o.chunk);
                    }
                    stderr.push_str(&o.chunk);
                }
                KernelNotification::Artifact(a) => artifacts.push(a.artifact),
                KernelNotification::Done(d) => {
                    done = Some(d);
                    break;
                }
                KernelNotification::Other { .. } => {}
            }
        }
        let meta = kernel.run_status(&run_id).await.ok();
        if let Some(m) = &meta {
            let _ = self.session.append_run(m);
        }
        // Pull rendered artifacts from a remote target.
        let mut pull_error = None;
        if let (Target::Ssh { host, .. }, Some(mirror)) =
            (&self.target, self.target.mirror_dir(&self.repo))
        {
            if let Err(e) = target::sync::pull_run(
                host,
                &mirror,
                &started.out_dir,
                &self.repo,
                self.control_dir.as_deref(),
            )
            .await
            {
                pull_error = Some(e.to_string());
            } else {
                for a in artifacts.iter().filter(|a| a.rendered) {
                    let src = self.repo.join(&a.path);
                    let dst = self.session.pulled_artifact_path(&run_id, &a.path);
                    if let Some(p) = dst.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    let _ = std::fs::copy(&src, &dst);
                }
            }
        }
        let status = done
            .as_ref()
            .map(|d| d.status)
            .or(meta.as_ref().map(|m| m.status))
            .unwrap_or(RunStatus::Running);
        let exit_code = done
            .as_ref()
            .and_then(|d| d.exit_code)
            .or(meta.as_ref().and_then(|m| m.exit_code));
        let figures: Vec<&str> = artifacts
            .iter()
            .filter(|a| a.kind == ArtifactKind::Figure)
            .map(|a| a.path.as_str())
            .collect();
        let tail = |s: &str| tail_bytes(s, 6000);
        let mut result = json!({
            "run_id": run_id,
            "script": script,
            "args": run_args,
            "status": status,
            "exit_code": exit_code,
            "duration_s": done.as_ref().map(|d| d.duration_s),
            "out_dir": started.out_dir,
            "stdout": tail(&stdout),
            "stderr": tail(&stderr),
            "artifacts": artifacts.iter().map(|a| json!({"kind": a.kind, "path": a.path, "size": a.size})).collect::<Vec<_>>(),
            "figures": figures,
        });
        if status == RunStatus::Running {
            result["note"] =
                json!("the run did not finish before the wait deadline; use runs.get to poll it");
        } else if status != RunStatus::Done || exit_code.unwrap_or(0) != 0 {
            result["failed"] = json!(true);
            result["note"] =
                json!("the run failed; read stderr above, fix the cause, and run again");
        }
        if let Some(e) = pull_error {
            result["pull_error"] = json!(e);
        }
        Ok(result)
    }

    async fn pkg(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        let packages: Vec<String> = args
            .get("packages")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if name != "pkg.sync" && packages.is_empty() {
            return Err(ToolError::new("`packages` must list at least one package"));
        }
        let has_pyproject = self.repo.join("pyproject.toml").is_file();
        let sub: Vec<String> = match (name, has_pyproject) {
            ("pkg.add", true) => vec!["add".into()],
            ("pkg.add", false) => vec!["pip".into(), "install".into()],
            ("pkg.remove", true) => vec!["remove".into()],
            ("pkg.remove", false) => vec!["pip".into(), "uninstall".into()],
            ("pkg.sync", true) => vec!["sync".into()],
            ("pkg.sync", false) => {
                return Err(ToolError::new("pkg.sync needs a pyproject.toml / uv.lock"))
            }
            _ => return Err(ToolError::new(format!("unknown package tool `{name}`"))),
        };
        let out = match &self.target {
            Target::Local => {
                let mut line = vec!["uv".to_string()];
                line.extend(sub);
                line.extend(packages.clone());
                target::run_command(&line, Some(&self.repo)).await?
            }
            Target::Ssh { host, .. } => {
                let mirror = self.target.mirror_dir(&self.repo).unwrap_or_default();
                let mut parts = vec!["~/.analyst/bin/uv".to_string()];
                parts.extend(sub);
                parts.extend(packages.iter().map(|p| crate::util::sh_quote(p)));
                let remote = format!(
                    "cd {} && {}",
                    crate::util::sh_quote(&mirror),
                    parts.join(" ")
                );
                let s = target::run_ssh(host, self.control_dir.as_deref(), &remote).await?;
                return Ok(json!({"ok": true, "output": tail_bytes(&s, 4000)}));
            }
        };
        Ok(json!({
            "ok": true,
            "stdout": tail_bytes(&String::from_utf8_lossy(&out.stdout), 4000),
            "stderr": tail_bytes(&String::from_utf8_lossy(&out.stderr), 4000),
        }))
    }

    /// `shell.run` goes through the kernel's sandboxed REPL (`exec.snippet`) with a subprocess
    /// wrapper, since the kernel protocol has no dedicated shell method.
    async fn shell_run(&self, args: &Value) -> Result<Value, ToolError> {
        let command = str_arg(args, "command")?;
        let timeout = args
            .get("timeout_s")
            .and_then(Value::as_f64)
            .unwrap_or(120.0);
        let code = format!(
            "import subprocess as _sp, json as _json\n_r = _sp.run({cmd}, shell=True, capture_output=True, text=True, timeout={timeout})\nprint(_json.dumps({{'exit_code': _r.returncode, 'stdout': _r.stdout[-8000:], 'stderr': _r.stderr[-8000:]}}))\n",
            cmd = serde_json::to_string(command).unwrap_or_default(),
        );
        let r = self
            .kernel()?
            .exec_snippet(&code, Some(timeout + 5.0))
            .await?;
        if !r.ok {
            return Err(ToolError::new(format!(
                "shell wrapper failed: {}",
                tail_bytes(&r.stderr, 2000)
            )));
        }
        let last = r.stdout.lines().last().unwrap_or("");
        match serde_json::from_str::<Value>(last) {
            Ok(v) => Ok(v),
            Err(_) => Ok(json!({"stdout": r.stdout, "stderr": r.stderr})),
        }
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new(format!("`{key}` (string) is required")))
}

fn bad_args(e: serde_json::Error) -> ToolError {
    ToolError::new(format!("invalid arguments: {e}"))
}

fn tail_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("...[truncated]...{}", &s[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        crate::checkpoint::git(repo.path(), &["init", "-q"], None).unwrap();
        std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
        std::fs::write(
            repo.path().join("scripts/a.py"),
            "import argparse\n\ndef main():\n    pass\n",
        )
        .unwrap();
        let session = Session::create(home.path(), repo.path(), "local", "p/m").unwrap();
        let c = ToolContext::new(
            repo.path(),
            home.path(),
            session,
            "c1",
            ProjectConfig::default(),
        );
        (home, repo, c)
    }

    #[tokio::test]
    async fn fs_tools_and_capping() {
        let (_h, _r, c) = ctx();
        let r = c
            .dispatch(
                "1",
                "fs.read",
                &json!({"path":"scripts/a.py","start_line":3}),
            )
            .await
            .unwrap();
        assert_eq!(r["total_lines"], 4);
        assert!(r["content"]
            .as_str()
            .unwrap()
            .starts_with("    3| def main"));
        let l = c.dispatch("2", "fs.list", &json!({})).await.unwrap();
        let entries = l["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e == "scripts/a.py"));
        assert!(!entries
            .iter()
            .any(|e| e.as_str().unwrap().starts_with(".git")));
        let s = c
            .dispatch("3", "fs.search", &json!({"pattern":"def main"}))
            .await
            .unwrap();
        assert_eq!(s["matches"][0]["path"], "scripts/a.py");
        assert_eq!(s["matches"][0]["line"], 3);
        assert!(c
            .dispatch("4", "fs.read", &json!({"path":"../etc/passwd"}))
            .await
            .is_err());
        assert!(c
            .dispatch("5", "scripts.index", &json!({}))
            .await
            .unwrap_err()
            .message
            .contains("not connected"));

        let big = json!({"data": "x".repeat(40_000)});
        let o = c.cap_result("big", Ok(big));
        assert!(o.truncated);
        assert_eq!(o.result["truncated"], true);
        let art = o.result["artifact"].as_str().unwrap().to_string();
        assert!(Path::new(&art).is_file());
        // The artifact is readable through fs.read by absolute path.
        let page = c
            .dispatch(
                "6",
                "fs.read",
                &json!({"path": art, "start_line": 1, "end_line": 1}),
            )
            .await
            .unwrap();
        assert!(page["content"].as_str().unwrap().contains("{"));
        let small = c.cap_result("s", Ok(json!({"a":1})));
        assert!(!small.truncated);
        let err = c.cap_result("e", Err(ToolError::new("nope")));
        assert!(!err.ok);
        assert_eq!(err.result["error"], "nope");
    }

    #[tokio::test]
    async fn edit_tools_apply_and_git_tools_work() {
        let (_h, r, c) = ctx();
        c.dispatch(
            "1",
            "fs.propose_create",
            &json!({"path":"analysis/new.py","content":"x = 1\n"}),
        )
        .await
        .unwrap();
        assert!(r.path().join("analysis/new.py").is_file());
        c.dispatch(
            "2",
            "fs.propose_edit",
            &json!({"path":"analysis/new.py","edits":[{"old":"x = 1","new":"x = 2"}]}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(r.path().join("analysis/new.py")).unwrap(),
            "x = 2\n"
        );
        let st = c.dispatch("3", "git.status", &json!({})).await.unwrap();
        assert!(st["status"].as_str().unwrap().contains("analysis/"));
        let cm = c
            .dispatch("4", "git.commit", &json!({"message":"m"}))
            .await
            .unwrap();
        assert_eq!(cm["committed"], true);
        c.dispatch("5", "fs.propose_delete", &json!({"path":"analysis/new.py"}))
            .await
            .unwrap();
        assert!(!r.path().join("analysis/new.py").exists());
        let d = c.dispatch("6", "git.diff", &json!({})).await.unwrap();
        assert!(d["diff"].as_str().unwrap().contains("x = 2"));
    }
}
