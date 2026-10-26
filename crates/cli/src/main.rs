//! `analyst`: the phase 0 headless harness for Personal Analyst.

use analyst_core::agent::{Agent, AgentConfig, ApprovalRequest, Approver, AutoApprover};
use analyst_core::config::{self, ModelSpec};
use analyst_core::kernel::{
    resolve_tooling_python, KernelArgs, KernelClient, KernelTransport, LocalStdio, SshStdio,
};
use analyst_core::policy::Policy;
use analyst_core::providers::{self, capabilities, ModelClient};
use analyst_core::session::Session;
use analyst_core::target::{self, Target};
use analyst_core::tools::ToolContext;
use analyst_protocol::config::{
    EditPolicy, GlobalConfig, ProjectConfig, ProviderKind, RunPolicy, TargetKind,
};
use analyst_protocol::evals::Mode;
use analyst_protocol::kernel::*;
use analyst_protocol::model::ModelCapabilities;
use analyst_protocol::transcript::Event;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use clap::{Args, Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "analyst",
    version,
    about = "Personal Analyst: agent-driven, reusable Python analysis tools"
)]
struct Cli {
    /// Analyst home (default: $ANALYST_HOME or ~/.analyst).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// Repository root (default: current directory).
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
    /// Verbose logging (-v info, -vv debug).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Sandbox kind for runs on the chosen target: auto, bwrap, sandbox-exec, limits, none.
    /// Overrides the target's setting; use `limits` to diagnose a denial from the OS sandbox.
    #[arg(long, global = true, value_name = "KIND")]
    sandbox: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a template repo (.analyst/project.toml, scripts/, analysis/, data/, out/, pyproject.toml, uv venv, git init).
    Init { path: Option<PathBuf> },
    /// Manage the project environment with uv.
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
        #[arg(long)]
        target: Option<String>,
    },
    /// Execution targets.
    Target {
        #[command(subcommand)]
        cmd: TargetCmd,
    },
    /// Run a script on the target and stream its output.
    Run {
        script: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        timeout_s: Option<f64>,
        #[arg(long)]
        network: bool,
        /// Script arguments (after `--`).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Contract check + lint; exits non-zero on errors.
    Check {
        paths: Vec<String>,
        #[arg(long)]
        target: Option<String>,
    },
    /// Extract equations from a file; `--check` verifies declared equations numerically.
    Eq {
        path: String,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        function: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
    /// Print the script and library indexes.
    Index {
        #[arg(long)]
        target: Option<String>,
    },
    /// Chat with the agent (interactive REPL when no message is given).
    Chat(ChatArgs),
    /// Evaluation suite.
    Eval {
        #[command(subcommand)]
        cmd: EvalCmd,
    },
    /// Raw JSON-RPC passthrough to the kernel (stdin lines -> kernel, kernel stdout -> stdout).
    Kernel {
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
enum EnvCmd {
    List,
    Add { packages: Vec<String> },
    Remove { packages: Vec<String> },
    Sync,
}

#[derive(Subcommand)]
enum TargetCmd {
    List,
    Bootstrap { name: String },
    Sync { name: String },
    Info { name: Option<String> },
}

#[derive(Args)]
struct ChatArgs {
    /// `provider/model` (default from config).
    #[arg(short, long)]
    model: Option<String>,
    #[arg(long, default_value = "build")]
    mode: String,
    /// edits=auto, runs=auto and approve every remaining question.
    #[arg(long)]
    auto: bool,
    #[arg(long)]
    target: Option<String>,
    #[arg(long, default_value_t = 40)]
    max_steps: u32,
    /// Re-run the capability probe.
    #[arg(long)]
    reprobe: bool,
    message: Option<String>,
}

#[derive(Subcommand)]
enum EvalCmd {
    Run {
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "evals/tasks")]
        tasks: PathBuf,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        auto: bool,
        #[arg(long, default_value = "evals/results")]
        results: PathBuf,
    },
}

struct App {
    home: PathBuf,
    repo: PathBuf,
    global: GlobalConfig,
    project: ProjectConfig,
    sandbox: Option<String>,
}

impl App {
    fn new(cli: &Cli) -> Result<App> {
        let home = cli.home.clone().unwrap_or_else(config::analyst_home);
        std::fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
        let repo = cli.repo.clone().unwrap_or(std::env::current_dir()?);
        let repo = std::fs::canonicalize(&repo).unwrap_or(repo);
        let global = config::load_global(&home)?;
        let project = config::load_project(&repo)?;
        Ok(App {
            home,
            repo,
            global,
            project,
            sandbox: cli.sandbox.clone(),
        })
    }

    fn target(&self, name: Option<&str>) -> Result<Target> {
        let name = name.unwrap_or(&self.global.defaults.target);
        Target::from_config(&config::find_target(&self.global, name)?)
    }

    fn control_dir(&self) -> Option<PathBuf> {
        let d = self.home.join("ssh");
        std::fs::create_dir_all(&d).ok()?;
        Some(d)
    }

    fn session_python(&self) -> String {
        let venv = self.repo.join(".venv/bin/python");
        if venv.is_file() {
            venv.to_string_lossy().to_string()
        } else {
            "python3".to_string()
        }
    }

    fn transport(&self, target: &Target) -> Box<dyn KernelTransport> {
        let eff = config::effective_project(&self.project, target.name());
        match target {
            Target::Local => {
                let runs = self.home.join("runs");
                let _ = std::fs::create_dir_all(&runs);
                Box::new(LocalStdio::new(
                    resolve_tooling_python(&self.global),
                    KernelArgs {
                        python: self.session_python(),
                        root: self.repo.to_string_lossy().to_string(),
                        runs_dir: runs.to_string_lossy().to_string(),
                        out_dir: Some(eff.out_dir),
                        sandbox: Some(self.sandbox.clone().unwrap_or_else(|| "auto".into())),
                    },
                ))
            }
            Target::Ssh { host, sandbox, .. } => {
                let mirror = target.mirror_dir(&self.repo).unwrap_or_default();
                Box::new(SshStdio {
                    host: host.clone(),
                    control_dir: self.control_dir(),
                    remote_launcher: vec![
                        "~/.analyst/tooling/.venv/bin/python".into(),
                        "-m".into(),
                        "analyst_kernel".into(),
                    ],
                    args: KernelArgs {
                        python: format!("{mirror}/.venv/bin/python"),
                        root: mirror.clone(),
                        runs_dir: "~/.analyst/runs".into(),
                        out_dir: Some(eff.out_dir),
                        sandbox: Some(self.sandbox.clone().unwrap_or_else(|| sandbox.clone())),
                    },
                })
            }
        }
    }

    async fn connect(&self, target: &Target) -> Result<KernelClient> {
        let t = self.transport(target);
        tracing::info!("kernel: {}", t.describe());
        KernelClient::connect(t, analyst_core::CORE_VERSION)
            .await
            .map_err(|e| anyhow!("starting the kernel on target `{}`: {e}\n  (set ANALYST_TOOLING_PYTHON or `tooling_python` in config.toml to the interpreter that has analyst_kernel installed)", target.name()))
    }

    fn session(&self, target: &Target, model: &str) -> Result<Session> {
        match Session::find_latest(&self.home, &self.repo, target.name())? {
            Some(s) => Ok(s),
            None => Session::create(&self.home, &self.repo, target.name(), model),
        }
    }

    async fn model(
        &self,
        spec: Option<&str>,
        reprobe: bool,
    ) -> Result<(Arc<dyn ModelClient>, ModelCapabilities, String)> {
        let spec = match spec {
            Some(s) => ModelSpec::parse(s)?,
            None => config::default_model(&self.global)?,
        };
        let resolved = config::resolve_model(&self.global, &spec)?;
        let client = providers::client_for(&resolved)?;
        let key = spec.to_string();
        let info_window = if reprobe || capabilities::cached(&self.home, &key).is_none() {
            model_info_window(&resolved, &spec).await
        } else {
            None
        };
        let caps = capabilities::ensure(
            &self.home,
            &key,
            client.as_ref(),
            resolved.overrides.as_ref(),
            info_window,
            reprobe,
        )
        .await?;
        Ok((client, caps, key))
    }
}

async fn model_info_window(resolved: &config::ResolvedModel, spec: &ModelSpec) -> Option<u32> {
    if resolved.provider.kind != ProviderKind::OpenAiCompatible {
        return None;
    }
    let client = providers::openai_compat::OpenAiCompatClient::new(
        resolved.provider.effective_base_url(),
        config::api_key(&resolved.provider).ok().flatten(),
        spec.id.clone(),
    )
    .ok()?;
    let info = tokio::time::timeout(Duration::from_secs(10), client.list_models())
        .await
        .ok()?
        .ok()?;
    capabilities::context_window_from_info(&info, &spec.id)
}

struct InteractiveApprover;

#[async_trait]
impl Approver for InteractiveApprover {
    async fn approve(&self, req: &ApprovalRequest) -> bool {
        println!();
        println!("── approval needed: {} ({})", req.tool, req.reason);
        if let Some(p) = &req.proposal {
            println!("   {}", p.summary);
            for (i, line) in p.diff.lines().enumerate() {
                if i >= 120 {
                    println!("   ... (diff truncated)");
                    break;
                }
                println!("   {line}");
            }
        } else {
            println!(
                "   {}",
                serde_json::to_string(&req.args).unwrap_or_default()
            );
        }
        print!("   accept? [y/N] ");
        let _ = std::io::stdout().flush();
        let line = tokio::task::spawn_blocking(|| {
            let mut s = String::new();
            let _ = std::io::stdin().read_line(&mut s);
            s
        })
        .await
        .unwrap_or_default();
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

fn print_diagnostics(diags: &[Diagnostic]) -> usize {
    let mut errors = 0;
    for d in diags {
        let sev = match d.severity {
            Severity::Error => {
                errors += 1;
                "error"
            }
            Severity::Warn => "warn ",
        };
        println!(
            "{sev} {}:{}:{} {} {}",
            d.path, d.line, d.col, d.rule, d.message
        );
        if let Some(h) = &d.hint {
            println!("      hint: {h}");
        }
    }
    errors
}

const TEMPLATE_PROJECT_TOML: &str = r#"[project]
scripts = ["scripts/**/*.py"]
library = "analysis"
out_dir = "out"
data_dirs = ["data"]
timeout_s = 600

[contract]
max_helper_lines = 15
near_duplicate_threshold = 0.80
min_tokens_for_similarity = 30
math_required = []

[policy]
edits = "review"
runs = "auto"
packages = "ask"
shell = "ask"
network = "deny"
git = "ask"
protected = []
max_steps = 40
"#;

fn cmd_init(path: Option<PathBuf>) -> Result<()> {
    let root = path.unwrap_or(std::env::current_dir()?);
    std::fs::create_dir_all(&root)?;
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "analysis-repo".into());
    let files: Vec<(&str, String)> = vec![
        (".analyst/project.toml", TEMPLATE_PROJECT_TOML.into()),
        ("analysis/__init__.py", "\"\"\"Shared analysis library: documented, reusable functions.\"\"\"\n".into()),
        ("scripts/.gitkeep", String::new()),
        ("data/.gitkeep", String::new()),
        ("out/.gitkeep", String::new()),
        ("notes.md", format!("# {name}\n\nNotes and decisions.\n")),
        (".gitignore", "out/\n!out/.gitkeep\n.venv/\n__pycache__/\n*.pyc\n.analyst/env.sh\n.ruff_cache/\n".into()),
        (
            "pyproject.toml",
            format!(
                "[project]\nname = \"{}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\ndependencies = [\"numpy\", \"scipy\", \"matplotlib\", \"pandas\"]\n\n[tool.ruff]\nline-length = 100\n",
                name.replace(' ', "-").to_lowercase()
            ),
        ),
    ];
    for (rel, content) in files {
        let p = root.join(rel);
        if p.exists() {
            println!("keep   {rel}");
            continue;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, content)?;
        println!("create {rel}");
    }
    let has_uv = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("uv").is_file()))
        .unwrap_or(false);
    if has_uv {
        for args in [vec!["venv", "-q"], vec!["sync", "-q"]] {
            let st = std::process::Command::new("uv")
                .args(&args)
                .current_dir(&root)
                .status()?;
            println!(
                "uv {} -> {}",
                args.join(" "),
                if st.success() { "ok" } else { "failed" }
            );
        }
    } else {
        println!("uv not found on PATH; skipped `uv venv` and `uv sync`");
    }
    if !root.join(".git").exists() {
        analyst_core::checkpoint::git(&root, &["init", "-q"], None)?;
        analyst_core::checkpoint::git(&root, &["add", "-A"], None)?;
        analyst_core::checkpoint::git(&root, &["commit", "-q", "-m", "analyst init"], None)?;
        println!("git init + initial commit");
    }
    println!("ready: {}", root.display());
    Ok(())
}

async fn cmd_env(app: &App, cmd: EnvCmd, target: Option<String>) -> Result<()> {
    let target = app.target(target.as_deref())?;
    let (sub, pkgs): (Vec<&str>, Vec<String>) = match cmd {
        EnvCmd::List => (vec!["pip", "list"], vec![]),
        EnvCmd::Add { packages } => (vec!["add"], packages),
        EnvCmd::Remove { packages } => (vec!["remove"], packages),
        EnvCmd::Sync => (vec!["sync"], vec![]),
    };
    match &target {
        Target::Local => {
            let mut line = vec!["uv".to_string()];
            line.extend(sub.iter().map(|s| s.to_string()));
            if sub == ["pip", "list"] {
                line.push("--python".into());
                line.push(app.session_python());
            }
            line.extend(pkgs);
            let out = target::run_command(&line, Some(&app.repo)).await?;
            print!("{}", String::from_utf8_lossy(&out.stdout));
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
        }
        Target::Ssh { host, .. } => {
            let mirror = target.mirror_dir(&app.repo).unwrap_or_default();
            let mut parts = vec!["~/.analyst/bin/uv".to_string()];
            parts.extend(sub.iter().map(|s| s.to_string()));
            parts.extend(pkgs.iter().map(|p| analyst_core::util::sh_quote(p)));
            let remote = format!(
                "cd {} && {}",
                analyst_core::util::sh_quote(&mirror),
                parts.join(" ")
            );
            print!(
                "{}",
                target::run_ssh(host, app.control_dir().as_deref(), &remote).await?
            );
        }
    }
    Ok(())
}

async fn cmd_target(app: &App, cmd: TargetCmd) -> Result<()> {
    match cmd {
        TargetCmd::List => {
            println!(
                "local  (default: {})",
                if app.global.defaults.target == "local" {
                    "yes"
                } else {
                    "no"
                }
            );
            for t in &app.global.targets {
                let kind = match t.kind {
                    TargetKind::Local => "local",
                    TargetKind::Ssh => "ssh",
                };
                println!(
                    "{}  kind={kind} host={} remote_root={} python={} sandbox={} data_dirs={:?}{}",
                    t.name,
                    t.host.as_deref().unwrap_or("-"),
                    t.remote_root.as_deref().unwrap_or("~/analyst"),
                    t.python.as_deref().unwrap_or("3.12"),
                    t.sandbox.as_deref().unwrap_or("auto"),
                    t.data_dirs,
                    if app.global.defaults.target == t.name {
                        " (default)"
                    } else {
                        ""
                    }
                );
            }
        }
        TargetCmd::Bootstrap { name } => {
            let target = app.target(Some(&name))?;
            let wheel = target::bootstrap::find_dev_wheel();
            match target::bootstrap::ensure(&target, app.control_dir().as_deref(), wheel.as_deref())
                .await?
            {
                target::bootstrap::Outcome::UpToDate(v) => {
                    println!("{name}: tooling {v} already installed")
                }
                target::bootstrap::Outcome::Installed(v) => {
                    println!("{name}: installed tooling {v}")
                }
            }
            if let Target::Ssh { host, .. } = &target {
                let mirror = target.mirror_dir(&app.repo).unwrap_or_default();
                println!(
                    "session env: {}",
                    target::bootstrap::session_env_command(&mirror)
                );
                target::run_ssh(
                    host,
                    app.control_dir().as_deref(),
                    &target::bootstrap::session_env_command(&mirror),
                )
                .await?;
                println!("{name}: session environment ready in {mirror}");
            }
        }
        TargetCmd::Sync { name } => {
            let target = app.target(Some(&name))?;
            let mut session =
                app.session(&target, app.global.defaults.model.as_deref().unwrap_or("-"))?;
            let report = target::sync::push(
                &app.repo,
                &target,
                app.control_dir().as_deref(),
                session.record.last_tree_hash.as_deref(),
                false,
            )
            .await?;
            session.set_last_tree_hash(Some(report.tree_hash.clone()))?;
            println!(
                "{name}: {:?} {} files, tree {}",
                report.method,
                report.files,
                &report.tree_hash[..12]
            );
            if let Some(w) = report.warning {
                println!("warning: {w}");
            }
        }
        TargetCmd::Info { name } => {
            let target = app.target(name.as_deref())?;
            println!(
                "target {} ({})",
                target.name(),
                if target.is_remote() { "ssh" } else { "local" }
            );
            if let Some(m) = target.mirror_dir(&app.repo) {
                println!("mirror {m}");
            }
            let k = app.connect(&target).await?;
            if let Some(h) = k.hello().await {
                println!(
                    "kernel {} protocol {} host {} os {} sandbox {}",
                    h.kernel_version, h.protocol_version, h.host, h.os, h.sandbox
                );
                println!("python {}  root {}  runs {}", h.python, h.root, h.runs_dir);
            }
            let info = k.env_info().await?;
            println!(
                "version {}  venv {}  uv {}",
                info.version,
                info.venv.as_deref().unwrap_or("-"),
                info.uv.as_deref().unwrap_or("-")
            );
            k.shutdown().await?;
        }
    }
    Ok(())
}

async fn cmd_run(
    app: &App,
    script: String,
    args: Vec<String>,
    target: Option<String>,
    timeout_s: Option<f64>,
    network: bool,
) -> Result<i32> {
    let target = app.target(target.as_deref())?;
    let session = app.session(&target, app.global.defaults.model.as_deref().unwrap_or("-"))?;
    let kernel = app.connect(&target).await?;
    let mut ctx = ToolContext::new(&app.repo, &app.home, session, "cli", app.project.clone());
    ctx.kernel = Some(kernel.clone());
    ctx.target = target.clone();
    ctx.control_dir = app.control_dir();
    ctx.output_sink = Some(Arc::new(|stream, chunk| {
        if stream == "stdout" {
            print!("{chunk}");
            let _ = std::io::stdout().flush();
        } else {
            eprint!("{chunk}");
        }
    }));
    let mut a = serde_json::json!({"script": script, "args": args});
    if let Some(t) = timeout_s {
        a["timeout_s"] = serde_json::json!(t);
    }
    if network {
        a["network"] = serde_json::json!(true);
    }
    let result = ctx.dispatch("cli-run", "run.script", &a).await;
    let _ = kernel.shutdown().await;
    let v = result.map_err(|e| anyhow!("{}", e.message))?;
    println!();
    println!(
        "run {} {} exit={} ({:.1}s) out={}",
        v["run_id"].as_str().unwrap_or(""),
        v["status"].as_str().unwrap_or(""),
        v["exit_code"],
        v["duration_s"].as_f64().unwrap_or(0.0),
        v["out_dir"].as_str().unwrap_or("")
    );
    for art in v["artifacts"].as_array().unwrap_or(&vec![]) {
        println!(
            "  {:<7} {} ({} bytes)",
            art["kind"].as_str().unwrap_or(""),
            art["path"].as_str().unwrap_or(""),
            art["size"]
        );
    }
    let code = v["exit_code"].as_i64().unwrap_or(1) as i32;
    Ok(if v["status"] == "done" { code } else { 1 })
}

async fn cmd_check(app: &App, paths: Vec<String>, target: Option<String>) -> Result<i32> {
    let target = app.target(target.as_deref())?;
    let k = app.connect(&target).await?;
    let paths = if paths.is_empty() { None } else { Some(paths) };
    let mut diags = k.contract_check(paths.clone()).await?.diagnostics;
    match k.lint_run(paths).await {
        Ok(r) => diags.extend(r.diagnostics),
        Err(e) => eprintln!("lint unavailable: {e}"),
    }
    let _ = k.shutdown().await;
    diags.sort_by_key(|a| (a.path.clone(), a.line));
    let errors = print_diagnostics(&diags);
    println!("{} diagnostic(s), {errors} error(s)", diags.len());
    Ok(if errors > 0 { 1 } else { 0 })
}

async fn cmd_eq(
    app: &App,
    path: String,
    check: bool,
    function: Option<String>,
    target: Option<String>,
) -> Result<i32> {
    let target = app.target(target.as_deref())?;
    let k = app.connect(&target).await?;
    let r = k
        .eq_extract(EqExtractParams {
            path: path.clone(),
            function: function.clone(),
        })
        .await?;
    let mut mismatches = 0;
    for e in &r.equations {
        println!(
            "{}:{} {} [{:?}] {:?}",
            path, e.line, e.qualname, e.tier, e.status
        );
        if let Some(l) = &e.latex {
            println!("    latex: {l}");
        }
        if let Some(s) = &e.source_expr {
            println!("    expr:  {s}");
        }
        if let Some(reason) = &e.reason {
            println!("    note:  {reason}");
        }
        if check && e.latex.is_some() {
            let c = k
                .eq_check(EqCheckParams {
                    path: path.clone(),
                    function: e.function.clone(),
                    trials: None,
                    tol: None,
                    ranges: None,
                })
                .await?;
            println!(
                "    check: {:?} trials={} max_rel_err={:?} {}",
                c.status,
                c.trials,
                c.max_rel_err,
                c.reason.unwrap_or_default()
            );
            if let Some(ce) = c.counterexample {
                println!(
                    "    counterexample: inputs={:?} expected={} actual={}",
                    ce.inputs, ce.expected, ce.actual
                );
            }
            if c.status == EqCheckStatus::Mismatch {
                mismatches += 1;
            }
        }
    }
    let _ = k.shutdown().await;
    Ok(if mismatches > 0 { 1 } else { 0 })
}

async fn cmd_index(app: &App, target: Option<String>) -> Result<()> {
    let target = app.target(target.as_deref())?;
    let k = app.connect(&target).await?;
    let s = k
        .scripts_index(Some(app.project.project.scripts.clone()))
        .await?;
    println!("scripts ({})", s.scripts.len());
    for sc in &s.scripts {
        println!(
            "  {}  {}",
            sc.path,
            sc.description.as_deref().unwrap_or("(no description)")
        );
        for o in &sc.options {
            println!(
                "      {:<22} {:<6} default={} {}",
                o.flags.join("/"),
                o.r#type.as_deref().unwrap_or("-"),
                o.default,
                o.help.as_deref().unwrap_or("")
            );
        }
    }
    let l = k
        .lib_index(Some(app.project.project.library.clone()))
        .await?;
    println!("library functions ({})", l.functions.len());
    for f in &l.functions {
        println!(
            "  {}.{}{}  {}{}  callers={}",
            f.module,
            f.qualname,
            f.signature,
            f.doc_summary.as_deref().unwrap_or("(no docstring)"),
            if f.has_math { " [Math]" } else { "" },
            f.callers.len()
        );
    }
    let _ = k.shutdown().await;
    Ok(())
}

fn event_printer() -> analyst_core::agent::EventSink {
    Arc::new(|e: &Event| match e {
        Event::ToolCall {
            name, arguments, ..
        } => {
            let a = serde_json::to_string(arguments).unwrap_or_default();
            println!("\n» {name} {}", analyst_core::util::truncate_bytes(&a, 160));
        }
        Event::ToolResult {
            ok,
            result,
            truncated,
            ..
        } => {
            let r = serde_json::to_string(result).unwrap_or_default();
            println!(
                "  {} {}{}",
                if *ok { "ok" } else { "ERR" },
                analyst_core::util::truncate_bytes(&r, 160),
                if truncated.unwrap_or(false) {
                    " (truncated)"
                } else {
                    ""
                }
            );
        }
        Event::Run {
            run_id,
            script,
            args,
            target,
            ..
        } => println!("  run {run_id}: {script} {} on {target}", args.join(" ")),
        Event::Decision {
            proposal_id,
            decision,
            by,
            ..
        } => println!("  decision {proposal_id}: {decision:?} by {by:?}"),
        _ => {}
    })
}

async fn cmd_chat(app: &App, args: ChatArgs) -> Result<()> {
    let mode: Mode = args.mode.parse().map_err(|e: String| anyhow!(e))?;
    let target = app.target(args.target.as_deref())?;
    let (model, caps, model_name) = app.model(args.model.as_deref(), args.reprobe).await?;
    println!(
        "model {model_name}  tools={:?} context={} target={} mode={mode}",
        caps.tools,
        caps.context_window,
        target.name()
    );
    let kernel = match app.connect(&target).await {
        Ok(k) => Some(k),
        Err(e) => {
            eprintln!("warning: {e}\n(continuing without a kernel: index, run, and check tools are unavailable)");
            None
        }
    };
    let session = app.session(&target, &model_name)?;
    let conv_id = analyst_core::util::stamped_id();
    println!("session {} conversation {}", session.id(), conv_id);
    let mut project = app.project.clone();
    if args.auto {
        project.policy.edits = EditPolicy::Auto;
        project.policy.runs = RunPolicy::Auto;
    }
    project.policy.max_steps = args.max_steps;
    let mut ctx = ToolContext::new(&app.repo, &app.home, session, &conv_id, project.clone());
    ctx.kernel = kernel.clone();
    ctx.target = target.clone();
    ctx.control_dir = app.control_dir();
    ctx.output_sink = Some(Arc::new(|stream, chunk| {
        if stream == "stdout" {
            print!("{chunk}");
        } else {
            eprint!("{chunk}");
        }
    }));
    let policy = Policy::new(&app.repo, project.policy.clone(), mode)?;
    let approver: Arc<dyn Approver> = if args.auto {
        Arc::new(AutoApprover)
    } else {
        Arc::new(InteractiveApprover)
    };
    let config = AgentConfig::new(mode, &model_name, caps, args.max_steps);
    let mut agent = Agent::new(model, ctx, policy, approver, config);
    agent.on_text = Some(Arc::new(|t| {
        print!("{t}");
        let _ = std::io::stdout().flush();
    }));
    agent.on_event = Some(event_printer());

    if let Some(m) = args.message {
        run_one(&mut agent, &m).await;
    } else {
        println!("interactive; /mode plan|build|run, /quit");
        loop {
            print!("\n{}> ", agent.config.mode);
            let _ = std::io::stdout().flush();
            let line = tokio::task::spawn_blocking(|| {
                let mut s = String::new();
                match std::io::stdin().read_line(&mut s) {
                    Ok(0) => None,
                    Ok(_) => Some(s),
                    Err(_) => None,
                }
            })
            .await?;
            let Some(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line == "/quit" || line == "/exit" {
                break;
            }
            if let Some(m) = line.strip_prefix("/mode ") {
                match m.trim().parse::<Mode>() {
                    Ok(m) => {
                        agent.set_mode(m);
                        println!("mode = {m}");
                    }
                    Err(e) => println!("{e}"),
                }
                continue;
            }
            run_one(&mut agent, line).await;
        }
    }
    if let Some(k) = kernel {
        let _ = k.shutdown().await;
    }
    Ok(())
}

async fn run_one(agent: &mut Agent, text: &str) {
    match agent.run_turn(text).await {
        Ok(s) => {
            println!();
            println!(
                "── turn: {} step(s), {} tool call(s) ({} error(s)), tokens {}/{}",
                s.steps, s.tool_calls, s.tool_errors, s.input_tokens, s.output_tokens
            );
            if !s.files_changed.is_empty() {
                println!("   files: {}", s.files_changed.join(", "));
            }
            if !s.runs.is_empty() {
                println!("   runs: {}", s.runs.join(", "));
            }
            if !s.figures.is_empty() {
                println!("   figures: {}", s.figures.join(", "));
            }
            for i in &s.open_issues {
                println!("   open: {i}");
            }
        }
        Err(e) => eprintln!("\nturn failed: {e:#}"),
    }
}

async fn cmd_eval(
    app: &App,
    model: String,
    tasks: PathBuf,
    task: Option<String>,
    auto: bool,
    results: PathBuf,
) -> Result<i32> {
    let loaded = analyst_core::evals::load_tasks(&tasks, task.as_deref())?;
    let (client, caps, model_name) = app.model(Some(&model), false).await?;
    let launcher = vec![
        resolve_tooling_python(&app.global),
        "-m".into(),
        "analyst_kernel".into(),
    ];
    let env = analyst_core::evals::EvalEnv {
        home: app.home.clone(),
        model: client,
        model_name: model_name.clone(),
        caps,
        kernel_launcher: Some(launcher),
        session_python: None,
        auto,
    };
    let mut out = Vec::new();
    for t in &loaded {
        println!("task {} ...", t.file.task.name);
        let r = analyst_core::evals::run_task(&env, t).await?;
        println!(
            "  {} ({}/{} checks, {} steps, {:.0}s)",
            if r.passed { "PASS" } else { "FAIL" },
            r.checks.iter().filter(|c| c.passed).count(),
            r.checks.len(),
            r.steps,
            r.wall_s
        );
        out.push(r);
    }
    let sc = analyst_core::evals::scorecard(&model_name, out);
    let path = analyst_core::evals::write_scorecard(&results, &sc)?;
    println!();
    print!("{}", analyst_core::evals::markdown_table(&sc));
    println!("scorecard: {}", path.display());
    Ok(if sc.tasks.iter().all(|t| t.passed) {
        0
    } else {
        1
    })
}

async fn cmd_kernel(app: &App, target: Option<String>) -> Result<i32> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let target = app.target(target.as_deref())?;
    let transport = app.transport(&target);
    eprintln!("spawning: {}", transport.describe());
    let spawned = transport.spawn().await?;
    let mut child = spawned.child;
    let mut stdin = spawned.stdin;
    let mut lines = BufReader::new(spawned.stdout).lines();
    let printer = tokio::spawn(async move {
        while let Ok(Some(l)) = lines.next_line().await {
            println!("{l}");
        }
    });
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(l)) = input.next_line().await {
        stdin.write_all(l.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
    }
    drop(stdin);
    let status = child.wait().await?;
    let _ = printer.await;
    Ok(status.code().unwrap_or(0))
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    let code = match run(cli).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Result<i32> {
    if let Command::Init { path } = &cli.command {
        cmd_init(path.clone())?;
        return Ok(0);
    }
    let app = App::new(&cli)?;
    match cli.command {
        Command::Init { .. } => unreachable!(),
        Command::Env { cmd, target } => cmd_env(&app, cmd, target).await.map(|_| 0),
        Command::Target { cmd } => cmd_target(&app, cmd).await.map(|_| 0),
        Command::Run {
            script,
            args,
            target,
            timeout_s,
            network,
        } => cmd_run(&app, script, args, target, timeout_s, network).await,
        Command::Check { paths, target } => cmd_check(&app, paths, target).await,
        Command::Eq {
            path,
            check,
            function,
            target,
        } => cmd_eq(&app, path, check, function, target).await,
        Command::Index { target } => cmd_index(&app, target).await.map(|_| 0),
        Command::Chat(args) => cmd_chat(&app, args).await.map(|_| 0),
        Command::Eval {
            cmd:
                EvalCmd::Run {
                    model,
                    tasks,
                    task,
                    auto,
                    results,
                },
        } => cmd_eval(&app, model, tasks, task, auto, results).await,
        Command::Kernel { target } => cmd_kernel(&app, target).await,
    }
}
