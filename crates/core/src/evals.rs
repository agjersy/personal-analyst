//! Evaluation harness (PROTOCOL 8): task loading, fixture preparation, running the agent with
//! `AutoApprover`, check executors, and scorecards.

use crate::agent::{Agent, AgentConfig, AutoApprover};
use crate::kernel::{KernelArgs, KernelClient, LocalStdio};
use crate::policy::Policy;
use crate::providers::ModelClient;
use crate::session::Session;
use crate::tools::ToolContext;
use analyst_protocol::config::{EditPolicy, ProjectConfig, RunPolicy};
use analyst_protocol::evals::*;
use analyst_protocol::kernel::*;
use analyst_protocol::model::ModelCapabilities;
use analyst_protocol::transcript::{Event, ProposalKind};
use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct LoadedTask {
    pub dir: PathBuf,
    pub file: TaskFile,
}

/// Load `<tasks_dir>/*/task.toml`, optionally only the task named `filter`.
pub fn load_tasks(tasks_dir: &Path, filter: Option<&str>) -> Result<Vec<LoadedTask>> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(tasks_dir)
        .with_context(|| format!("reading tasks dir {}", tasks_dir.display()))?;
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let path = dir.join("task.toml");
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let file: TaskFile =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if let Some(f) = filter {
            if file.task.name != f && dir.file_name().map(|n| n != f).unwrap_or(true) {
                continue;
            }
        }
        out.push(LoadedTask { dir, file });
    }
    if let Some(f) = filter {
        if out.is_empty() {
            return Err(anyhow!("no task named `{f}` under {}", tasks_dir.display()));
        }
    }
    Ok(out)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    for e in walkdir::WalkDir::new(src)
        .min_depth(1)
        .into_iter()
        .flatten()
    {
        let rel = e.path().strip_prefix(src)?;
        let target = dst.join(rel);
        if e.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// Copy the fixture into `dest`, `git init` + initial commit, and make sure
/// `.analyst/project.toml` exists.
pub fn prepare_fixture(task: &LoadedTask, dest: &Path) -> Result<()> {
    let src = task.dir.join(&task.file.fixture.dir);
    std::fs::create_dir_all(dest)?;
    if src.is_dir() {
        copy_tree(&src, dest)?;
    }
    let pc = dest.join(".analyst/project.toml");
    if !pc.is_file() {
        std::fs::create_dir_all(dest.join(".analyst"))?;
        std::fs::write(&pc, toml::to_string_pretty(&ProjectConfig::default())?)?;
    }
    let gi = dest.join(".gitignore");
    if !gi.is_file() {
        std::fs::write(&gi, "out/\n.venv/\n__pycache__/\n.analyst/env.sh\n")?;
    }
    crate::checkpoint::git(dest, &["init", "-q"], None)?;
    crate::checkpoint::git(dest, &["add", "-A"], None)?;
    crate::checkpoint::git(
        dest,
        &["commit", "-q", "--allow-empty", "-m", "fixture"],
        None,
    )?;
    Ok(())
}

/// How tasks are run.
pub struct EvalEnv {
    pub home: PathBuf,
    pub model: Arc<dyn ModelClient>,
    pub model_name: String,
    pub caps: ModelCapabilities,
    /// Kernel launcher (`[tooling_python, "-m", "analyst_kernel"]`); `None` runs without a
    /// kernel (kernel-backed checks then fail with a clear detail).
    pub kernel_launcher: Option<Vec<String>>,
    /// Session interpreter passed to the kernel (`--python`); default `python3`.
    pub session_python: Option<String>,
    /// `--auto`: edits=auto, runs=auto regardless of the fixture's policy.
    pub auto: bool,
}

/// Context for check executors.
pub struct CheckContext<'a> {
    pub repo: &'a Path,
    pub kernel: Option<&'a KernelClient>,
    pub session: &'a Session,
    pub conv_id: &'a str,
    pub baseline_similar: &'a [SimilarPair],
}

fn pair_key(p: &SimilarPair) -> (String, String, String, String) {
    (
        p.a.path.clone(),
        p.a.qualname.clone(),
        p.b.path.clone(),
        p.b.qualname.clone(),
    )
}

/// Wait for a run to finish, collecting its notifications. `rx` must have been subscribed
/// before the run started.
pub async fn wait_for_run(
    rx: &mut tokio::sync::broadcast::Receiver<KernelNotification>,
    run_id: &str,
    timeout: Duration,
) -> (Option<RunDone>, String, String, Vec<Artifact>) {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut artifacts = Vec::new();
    loop {
        let n = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(n)) => n,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            _ => return (None, stdout, stderr, artifacts),
        };
        if n.run_id() != Some(run_id) {
            continue;
        }
        match n {
            KernelNotification::Stdout(o) => stdout.push_str(&o.chunk),
            KernelNotification::Stderr(o) => stderr.push_str(&o.chunk),
            KernelNotification::Artifact(a) => artifacts.push(a.artifact),
            KernelNotification::Done(d) => return (Some(d), stdout, stderr, artifacts),
            KernelNotification::Other { .. } => {}
        }
    }
}

fn result(kind: &str, passed: bool, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        kind: kind.to_string(),
        passed,
        detail: detail.into(),
    }
}

/// Declared scope and accepted file edits from a transcript.
pub fn scope_and_edits(events: &[Event]) -> (Option<BTreeSet<String>>, Vec<String>) {
    let mut scope: Option<BTreeSet<String>> = None;
    let mut call_paths: BTreeMap<String, String> = BTreeMap::new();
    let mut proposals: BTreeMap<String, ProposalKind> = BTreeMap::new();
    let mut accepted = Vec::new();
    for e in events {
        match e {
            Event::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                if name == "declare_scope" {
                    let files = arguments
                        .get("files")
                        .and_then(|f| f.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .filter_map(|s| crate::util::normalize_rel_path(s).ok())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    scope.get_or_insert_with(BTreeSet::new).extend(files);
                }
                if let Some(p) = arguments.get("path").and_then(|p| p.as_str()) {
                    if let Ok(n) = crate::util::normalize_rel_path(p) {
                        call_paths.insert(id.clone(), n);
                    }
                }
            }
            Event::Proposal { id, kind, .. } => {
                proposals.insert(id.clone(), *kind);
            }
            Event::Decision {
                proposal_id,
                decision,
                ..
            } if decision.is_accepted() => {
                let is_file_change = proposals
                    .get(proposal_id)
                    .map(|k| k.is_file_change())
                    .unwrap_or(false);
                if is_file_change {
                    if let Some(p) = call_paths.get(proposal_id) {
                        accepted.push(p.clone());
                    }
                }
            }
            _ => {}
        }
    }
    (scope, accepted)
}

/// Execute one check.
pub async fn run_check(cx: &CheckContext<'_>, check: &Check) -> CheckResult {
    let kind = check.kind();
    let need_kernel = || result(kind, false, "kernel unavailable");
    match check {
        Check::ContractClean => {
            let Some(k) = cx.kernel else {
                return need_kernel();
            };
            match k.contract_check(None).await {
                Ok(r) => {
                    let errors: Vec<String> = r
                        .diagnostics
                        .iter()
                        .filter(|d| d.severity == Severity::Error)
                        .map(|d| format!("{} {}:{} {}", d.rule, d.path, d.line, d.message))
                        .collect();
                    result(
                        kind,
                        errors.is_empty(),
                        if errors.is_empty() {
                            format!("0 errors, {} warnings", r.diagnostics.len())
                        } else {
                            errors.join("; ")
                        },
                    )
                }
                Err(e) => result(kind, false, format!("contract.check failed: {e}")),
            }
        }
        Check::ScriptExists { path } => {
            let ok = cx.repo.join(path).is_file() && path.ends_with(".py");
            result(
                kind,
                ok,
                if ok {
                    format!("{path} exists")
                } else {
                    format!("{path} missing")
                },
            )
        }
        Check::RunSucceeds { script, args } => {
            let Some(k) = cx.kernel else {
                return need_kernel();
            };
            let mut rx = k.subscribe();
            let started = match k
                .run_start(RunStartParams {
                    script: script.clone(),
                    args: Some(args.clone()),
                    timeout_s: Some(300.0),
                    ..Default::default()
                })
                .await
            {
                Ok(s) => s,
                Err(e) => return result(kind, false, format!("run.start failed: {e}")),
            };
            let (done, _out, err, _) =
                wait_for_run(&mut rx, &started.run_id, Duration::from_secs(330)).await;
            if let Ok(meta) = k.run_status(&started.run_id).await {
                let _ = cx.session.append_run(&meta);
            }
            match done {
                Some(d) if d.status == RunStatus::Done && d.exit_code == Some(0) => result(
                    kind,
                    true,
                    format!("{script} {} exit 0 in {:.1}s", args.join(" "), d.duration_s),
                ),
                Some(d) => result(
                    kind,
                    false,
                    format!(
                        "{script} {}: {} exit {:?}; stderr tail: {}",
                        args.join(" "),
                        d.status,
                        d.exit_code,
                        tail(&err, 600)
                    ),
                ),
                None => result(kind, false, format!("{script}: no run.done received")),
            }
        }
        Check::ArtifactExists { glob, min_count } => {
            let pattern = cx.repo.join(glob).to_string_lossy().to_string();
            let count = glob::glob(&pattern)
                .map(|g| g.flatten().filter(|p| p.is_file()).count())
                .unwrap_or(0);
            result(
                kind,
                count as u64 >= *min_count,
                format!("{count} file(s) match {glob} (need {min_count})"),
            )
        }
        Check::EqVerified { path, function } => {
            let Some(k) = cx.kernel else {
                return need_kernel();
            };
            match k
                .eq_check(EqCheckParams {
                    path: path.clone(),
                    function: function.clone(),
                    trials: None,
                    tol: None,
                    ranges: None,
                })
                .await
            {
                Ok(r) => result(
                    kind,
                    r.status == EqCheckStatus::Agree,
                    format!(
                        "{function}: {:?} (trials {}, max_rel_err {:?}) {}",
                        r.status,
                        r.trials,
                        r.max_rel_err,
                        r.reason.unwrap_or_default()
                    ),
                ),
                Err(e) => result(kind, false, format!("eq.check failed: {e}")),
            }
        }
        Check::NoNewDuplicates => {
            let Some(k) = cx.kernel else {
                return need_kernel();
            };
            match k.contract_similar(None).await {
                Ok(r) => {
                    let base: BTreeSet<_> = cx
                        .baseline_similar
                        .iter()
                        .filter(|p| p.kind == SimilarKind::Duplicate)
                        .map(pair_key)
                        .collect();
                    let new: Vec<String> = r
                        .pairs
                        .iter()
                        .filter(|p| {
                            p.kind == SimilarKind::Duplicate && !base.contains(&pair_key(p))
                        })
                        .map(|p| {
                            format!(
                                "{}:{} ~ {}:{}",
                                p.a.path, p.a.qualname, p.b.path, p.b.qualname
                            )
                        })
                        .collect();
                    result(
                        kind,
                        new.is_empty(),
                        if new.is_empty() {
                            "no new duplicate pairs".to_string()
                        } else {
                            new.join("; ")
                        },
                    )
                }
                Err(e) => result(kind, false, format!("contract.similar failed: {e}")),
            }
        }
        Check::EditsWithinScope => {
            let events = cx.session.read_transcript(cx.conv_id).unwrap_or_default();
            let (scope, accepted) = scope_and_edits(&events);
            if accepted.is_empty() {
                return result(kind, true, "no accepted edits");
            }
            match scope {
                None => result(
                    kind,
                    false,
                    format!("{} accepted edit(s) but no scope declared", accepted.len()),
                ),
                Some(s) => {
                    let outside: Vec<&String> =
                        accepted.iter().filter(|p| !s.contains(*p)).collect();
                    result(
                        kind,
                        outside.is_empty(),
                        if outside.is_empty() {
                            format!("{} edit(s) all in scope", accepted.len())
                        } else {
                            format!(
                                "outside scope: {}",
                                outside
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        },
                    )
                }
            }
        }
        Check::FileMatches { path, regex } => {
            let re = match regex::Regex::new(regex) {
                Ok(r) => r,
                Err(e) => return result(kind, false, format!("bad regex: {e}")),
            };
            match std::fs::read_to_string(cx.repo.join(path)) {
                Ok(t) => {
                    let ok = re.is_match(&t);
                    result(
                        kind,
                        ok,
                        if ok {
                            format!("{path} matches /{regex}/")
                        } else {
                            format!("{path} does not match /{regex}/")
                        },
                    )
                }
                Err(e) => result(kind, false, format!("{path}: {e}")),
            }
        }
        Check::NoEdits => {
            let events = cx.session.read_transcript(cx.conv_id).unwrap_or_default();
            let (_, accepted) = scope_and_edits(&events);
            result(
                kind,
                accepted.is_empty(),
                if accepted.is_empty() {
                    "no accepted edits".to_string()
                } else {
                    format!("accepted edits: {}", accepted.join(", "))
                },
            )
        }
        Check::RunsAtLeast { script, count } => {
            let runs = cx.session.read_runs().unwrap_or_default();
            let ids: BTreeSet<&str> = runs
                .iter()
                .filter(|r| &r.meta.script == script)
                .map(|r| r.meta.run_id.as_str())
                .collect();
            result(
                kind,
                ids.len() as u64 >= *count,
                format!("{} run(s) of {script} (need {count})", ids.len()),
            )
        }
    }
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut start = s.len() - n;
        while !s.is_char_boundary(start) {
            start += 1;
        }
        format!("...{}", &s[start..])
    }
}

/// Run one task end to end.
pub async fn run_task(env: &EvalEnv, task: &LoadedTask) -> Result<TaskResult> {
    let name = task.file.task.name.clone();
    let work = tempfile::Builder::new()
        .prefix(&format!("analyst-eval-{name}-"))
        .tempdir()?;
    let repo = work.path().join("repo");
    prepare_fixture(task, &repo)?;
    let repo = std::fs::canonicalize(&repo)?;
    let session_python = match &env.session_python {
        Some(p) => p.clone(),
        None => fixture_python(&repo).await,
    };
    let mut project = crate::config::load_project(&repo)?;
    if env.auto {
        project.policy.edits = EditPolicy::Auto;
        project.policy.runs = RunPolicy::Auto;
    }
    project.policy.max_steps = task.file.task.max_steps;

    let kernel = match &env.kernel_launcher {
        Some(launcher) => {
            let runs_dir = env.home.join("runs");
            std::fs::create_dir_all(&runs_dir)?;
            let args = KernelArgs {
                python: session_python.clone(),
                root: repo.to_string_lossy().to_string(),
                runs_dir: runs_dir.to_string_lossy().to_string(),
                out_dir: Some(project.project.out_dir.clone()),
                sandbox: Some("auto".into()),
            };
            match KernelClient::connect(
                Box::new(LocalStdio::with_launcher(launcher.clone(), args)),
                crate::CORE_VERSION,
            )
            .await
            {
                Ok(k) => Some(k),
                Err(e) => {
                    tracing::warn!("kernel connect failed for task {name}: {e}");
                    None
                }
            }
        }
        None => None,
    };
    let baseline = match &kernel {
        Some(k) => k
            .contract_similar(None)
            .await
            .map(|r| r.pairs)
            .unwrap_or_default(),
        None => vec![],
    };

    let session = Session::create(&env.home, &repo, "local", &env.model_name)?;
    let conv_id = "eval".to_string();
    let mut ctx = ToolContext::new(&repo, &env.home, session, &conv_id, project.clone());
    ctx.kernel = kernel.clone();
    let policy = Policy::new(&repo, project.policy.clone(), task.file.task.mode)?;
    let config = AgentConfig::new(
        task.file.task.mode,
        &env.model_name,
        env.caps.clone(),
        task.file.task.max_steps,
    );
    let mut agent = Agent::new(
        env.model.clone(),
        ctx,
        policy,
        Arc::new(AutoApprover),
        config,
    );

    let start = std::time::Instant::now();
    let turn = tokio::time::timeout(
        Duration::from_secs(task.file.task.timeout_s),
        agent.run_turn(&task.file.task.prompt),
    )
    .await;
    let wall_s = start.elapsed().as_secs_f64();
    let (summary, turn_error) = match turn {
        Ok(Ok(s)) => (s, None),
        Ok(Err(e)) => (Default::default(), Some(format!("turn failed: {e}"))),
        Err(_) => (
            Default::default(),
            Some(format!(
                "task timed out after {}s",
                task.file.task.timeout_s
            )),
        ),
    };

    let cx = CheckContext {
        repo: &repo,
        kernel: kernel.as_ref(),
        session: &agent.ctx.session,
        conv_id: &conv_id,
        baseline_similar: &baseline,
    };
    let mut checks = Vec::new();
    for c in &task.file.checks {
        checks.push(run_check(&cx, c).await);
    }
    if let Some(e) = turn_error {
        checks.push(CheckResult {
            kind: "agent_turn".into(),
            passed: false,
            detail: e,
        });
    }
    if let Some(k) = &kernel {
        let _ = k.shutdown().await;
    }
    let passed = checks.iter().all(|c| c.passed);
    Ok(TaskResult {
        name,
        passed,
        checks,
        steps: summary.steps,
        input_tokens: summary.input_tokens,
        output_tokens: summary.output_tokens,
        wall_s,
        unwanted_edits: summary.unwanted_edits,
        tool_errors: summary.tool_errors,
        tool_calls: summary.tool_calls,
    })
}

/// Session interpreter for a fixture: `uv sync` when a `pyproject.toml` exists and `uv` is on
/// PATH, then `.venv/bin/python`; else `python3`.
async fn fixture_python(repo: &Path) -> String {
    if repo.join("pyproject.toml").is_file() && which("uv") {
        match crate::target::run_command(
            &["uv".to_string(), "sync".to_string(), "-q".to_string()],
            Some(repo),
        )
        .await
        {
            Ok(_) => {}
            Err(e) => tracing::warn!("uv sync in fixture failed: {e}"),
        }
    }
    let venv = repo.join(".venv/bin/python");
    if venv.is_file() {
        venv.to_string_lossy().to_string()
    } else {
        "python3".to_string()
    }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn rate(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

/// Build the scorecard summary.
pub fn scorecard(model: &str, tasks: Vec<TaskResult>) -> Scorecard {
    let n = tasks.len();
    let passed = tasks.iter().filter(|t| t.passed).count();
    let with = |kind: &str| -> (usize, usize) {
        let have: Vec<&TaskResult> = tasks
            .iter()
            .filter(|t| t.checks.iter().any(|c| c.kind == kind))
            .collect();
        let ok = have
            .iter()
            .filter(|t| t.checks.iter().filter(|c| c.kind == kind).all(|c| c.passed))
            .count();
        (ok, have.len())
    };
    let (c_ok, c_n) = with("contract_clean");
    let (e_ok, e_n) = with("eq_verified");
    let unwanted = tasks.iter().filter(|t| t.unwanted_edits > 0).count();
    let calls: u32 = tasks.iter().map(|t| t.tool_calls).sum();
    let errors: u32 = tasks.iter().map(|t| t.tool_errors).sum();
    Scorecard {
        model: model.to_string(),
        tasks,
        summary: ScorecardSummary {
            pass_rate: rate(passed, n),
            contract_rate: rate(c_ok, c_n),
            eq_rate: rate(e_ok, e_n),
            unwanted_edit_rate: rate(unwanted, n),
            tool_error_rate: rate(errors as usize, calls as usize),
        },
    }
}

/// Markdown table for stdout.
pub fn markdown_table(sc: &Scorecard) -> String {
    let mut s = String::new();
    s.push_str("| task | passed | checks | steps | tokens in/out | wall s | unwanted edits | tool errors |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for t in &sc.tasks {
        let ok = t.checks.iter().filter(|c| c.passed).count();
        s.push_str(&format!(
            "| {} | {} | {}/{} | {} | {}/{} | {:.1} | {} | {}/{} |\n",
            t.name,
            if t.passed { "yes" } else { "no" },
            ok,
            t.checks.len(),
            t.steps,
            t.input_tokens,
            t.output_tokens,
            t.wall_s,
            t.unwanted_edits,
            t.tool_errors,
            t.tool_calls
        ));
    }
    let m = &sc.summary;
    s.push_str(&format!(
        "\npass {:.0}% | contract {:.0}% | eq {:.0}% | unwanted-edit {:.0}% | tool-error {:.1}%\n",
        m.pass_rate * 100.0,
        m.contract_rate * 100.0,
        m.eq_rate * 100.0,
        m.unwanted_edit_rate * 100.0,
        m.tool_error_rate * 100.0
    ));
    for t in &sc.tasks {
        for c in t.checks.iter().filter(|c| !c.passed) {
            s.push_str(&format!("- {} / {}: {}\n", t.name, c.kind, c.detail));
        }
    }
    s
}

/// Write `evals/results/<model>-<timestamp>.json`; returns the path.
pub fn write_scorecard(results_dir: &Path, sc: &Scorecard) -> Result<PathBuf> {
    std::fs::create_dir_all(results_dir)?;
    let safe: String = sc
        .model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = results_dir.join(format!("{safe}-{}.json", crate::util::now_stamp()));
    std::fs::write(&path, serde_json::to_string_pretty(sc)?)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ScriptedModelClient;

    fn fake_kernel_launcher() -> Vec<String> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_kernel.py");
        vec!["python3".into(), p.to_string_lossy().to_string()]
    }

    fn write_task(dir: &Path) {
        let fixture = dir.join("fixture");
        std::fs::create_dir_all(fixture.join("scripts")).unwrap();
        std::fs::create_dir_all(fixture.join("analysis")).unwrap();
        std::fs::write(fixture.join("analysis/__init__.py"), "").unwrap();
        std::fs::write(fixture.join("analysis/fitting.py"), "def double_exp(t, A, tau1, B, tau2):\n    \"\"\"Math:\n        y = A*exp(-t/tau1) + B*exp(-t/tau2)\n    \"\"\"\n    return A\n").unwrap();
        std::fs::write(
            fixture.join("scripts/fit_decay.py"),
            "import argparse, os, time\n\ndef main():\n    p = argparse.ArgumentParser(description='fit')\n    p.add_argument('--n', type=int, default=2, help='count')\n    a = p.parse_args()\n    out = os.environ.get('ANALYST_OUT_DIR', 'out')\n    os.makedirs(os.path.join(out, 'figures'), exist_ok=True)\n    for i in range(a.n):\n        with open(os.path.join(out, 'figures', f'fig-{i+1}.png'), 'wb') as f:\n            f.write(b'\\x89PNG fake')\n        print('line', i)\n\nif __name__ == '__main__':\n    main()\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("task.toml"),
            r#"
[task]
name = "smoke"
prompt = "Do nothing."
mode = "build"
timeout_s = 120
max_steps = 5

[fixture]
dir = "fixture"

[[checks]]
kind = "contract_clean"
[[checks]]
kind = "script_exists"
path = "scripts/fit_decay.py"
[[checks]]
kind = "run_succeeds"
script = "scripts/fit_decay.py"
args = ["--n", "2"]
[[checks]]
kind = "artifact_exists"
glob = "out/fit_decay/*/figures/*.png"
min_count = 2
[[checks]]
kind = "eq_verified"
path = "analysis/fitting.py"
function = "double_exp"
[[checks]]
kind = "no_new_duplicates"
[[checks]]
kind = "edits_within_scope"
[[checks]]
kind = "file_matches"
path = "analysis/fitting.py"
regex = "def double_exp\\("
[[checks]]
kind = "no_edits"
[[checks]]
kind = "runs_at_least"
script = "scripts/fit_decay.py"
count = 1
"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn checks_pass_on_a_fixture_without_agent_action() {
        let tasks = tempfile::tempdir().unwrap();
        let tdir = tasks.path().join("smoke");
        std::fs::create_dir_all(&tdir).unwrap();
        write_task(&tdir);
        let loaded = load_tasks(tasks.path(), Some("smoke")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].file.checks.len(), 10);
        let home = tempfile::tempdir().unwrap();
        let env = EvalEnv {
            home: home.path().to_path_buf(),
            model: Arc::new(ScriptedModelClient::new(vec![vec![
                analyst_protocol::model::ModelEvent::Text {
                    delta: "Nothing to do.".into(),
                },
            ]])),
            model_name: "scripted/model".into(),
            caps: ModelCapabilities::default(),
            kernel_launcher: Some(fake_kernel_launcher()),
            session_python: Some("python3".into()),
            auto: true,
        };
        let r = run_task(&env, &loaded[0]).await.unwrap();
        for c in &r.checks {
            assert!(c.passed, "{} failed: {}", c.kind, c.detail);
        }
        assert!(r.passed);
        assert_eq!(r.steps, 1);
        assert_eq!(r.unwanted_edits, 0);
        let sc = scorecard("scripted/model", vec![r]);
        assert_eq!(sc.summary.pass_rate, 1.0);
        assert_eq!(sc.summary.contract_rate, 1.0);
        assert_eq!(sc.summary.eq_rate, 1.0);
        assert_eq!(sc.summary.unwanted_edit_rate, 0.0);
        assert_eq!(sc.summary.tool_error_rate, 0.0);
        let md = markdown_table(&sc);
        assert!(md.contains("| smoke | yes | 10/10 |"));
        let results = tempfile::tempdir().unwrap();
        let p = write_scorecard(results.path(), &sc).unwrap();
        let back: Scorecard = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(back, sc);
    }

    #[test]
    fn scope_and_edits_from_transcript() {
        use analyst_protocol::transcript::{DecidedBy, Decision};
        let ev = vec![
            Event::ToolCall {
                ts: "t".into(),
                id: "s".into(),
                name: "declare_scope".into(),
                arguments: serde_json::json!({"files": ["a.py"]}),
            },
            Event::ToolCall {
                ts: "t".into(),
                id: "e1".into(),
                name: "fs.propose_edit".into(),
                arguments: serde_json::json!({"path": "a.py"}),
            },
            Event::Proposal {
                ts: "t".into(),
                id: "e1".into(),
                kind: ProposalKind::Edit,
                summary: "".into(),
                diff: None,
            },
            Event::Decision {
                ts: "t".into(),
                proposal_id: "e1".into(),
                decision: Decision::Auto,
                by: DecidedBy::Policy,
            },
            Event::ToolCall {
                ts: "t".into(),
                id: "e2".into(),
                name: "fs.propose_create".into(),
                arguments: serde_json::json!({"path": "b.py"}),
            },
            Event::Proposal {
                ts: "t".into(),
                id: "e2".into(),
                kind: ProposalKind::Create,
                summary: "".into(),
                diff: None,
            },
            Event::Decision {
                ts: "t".into(),
                proposal_id: "e2".into(),
                decision: Decision::Reject,
                by: DecidedBy::User,
            },
        ];
        let (scope, accepted) = scope_and_edits(&ev);
        assert_eq!(scope.unwrap().into_iter().collect::<Vec<_>>(), vec!["a.py"]);
        assert_eq!(accepted, vec!["a.py"]);
    }
}
