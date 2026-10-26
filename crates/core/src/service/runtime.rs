//! `SessionRuntime`: one open session. Owns the repo, target, project config with session policy
//! overrides, the lazily started kernel (with `kernel_state` events and reconnect), the file
//! watcher, the per-session event broadcast, conversations, runs, figures, equations, package
//! tasks, output previews, filesystem operations, and diagnostics.

use super::agent_runner::ConversationRuntime;
use super::error::{ServiceError, ServiceResult};
use super::figures::{self, ScannedRun};
use super::fs;
use super::tasks::{TaskLogger, TaskRegistry};
use crate::config;
use crate::kernel::{
    resolve_tooling_python, KernelArgs, KernelClient, KernelTransport, LocalStdio, SshStdio,
};
use crate::session::Session;
use crate::target::{self, Target};
use crate::util::{normalize_rel_path, now_rfc3339};
use analyst_protocol::config::{GlobalConfig, PolicyConfig, ProjectConfig, TargetKind};
use analyst_protocol::evals::Mode;
use analyst_protocol::kernel::*;
use analyst_protocol::ui::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Last N bytes of stdout/stderr carried in a `RunView`.
pub const TAIL_BYTES: usize = 4096;
/// Watcher debounce.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Per-session options the workbench passes down.
#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    /// Interpreter hosting `analyst_kernel` (local target).
    pub tooling_python: String,
    /// Override of the local kernel launcher (tests inject a fake kernel).
    pub kernel_launcher: Option<Vec<String>>,
    /// `--sandbox` override for the kernel.
    pub sandbox: Option<String>,
    /// Session interpreter override (default `<repo>/.venv/bin/python`, else `python3`).
    pub session_python: Option<String>,
    /// Start the kernel in the background when the session opens.
    pub autostart_kernel: bool,
    /// `$ANALYST_HOME/ssh` for multiplexed SSH connections.
    pub control_dir: Option<PathBuf>,
    /// Directory containing `analyst_mpl` (terminal figure capture).
    pub mpl_dir: Option<PathBuf>,
}

/// `meta.json` in the session directory: workbench-level session state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMeta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub last_opened_at: String,
    #[serde(default)]
    pub active_conversation: Option<String>,
    #[serde(default)]
    pub conversations: Vec<ConversationMeta>,
    /// Session policy override (not written to project.toml).
    #[serde(default)]
    pub policy_override: Option<PolicyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Mode,
    pub created_at: String,
}

pub fn meta_path(session_dir: &Path) -> PathBuf {
    session_dir.join("meta.json")
}

pub fn load_meta(session_dir: &Path) -> SessionMeta {
    std::fs::read_to_string(meta_path(session_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_meta(session_dir: &Path, meta: &SessionMeta) -> ServiceResult<()> {
    let text = serde_json::to_string_pretty(meta)?;
    std::fs::write(meta_path(session_dir), text)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSource {
    Kernel,
    Terminal,
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub meta: RunMeta,
    pub target: String,
    pub stdout: String,
    pub stderr: String,
    pub artifacts: Vec<Artifact>,
    pub source: RunSource,
    pub finished: bool,
}

impl RunRecord {
    pub fn view(&self) -> RunView {
        RunView {
            meta: self.meta.clone(),
            target: self.target.clone(),
            artifacts: self.artifacts.clone(),
            stdout_tail: tail(&self.stdout, TAIL_BYTES),
            stderr_tail: tail(&self.stderr, TAIL_BYTES),
        }
    }
}

pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

struct KernelSlot {
    client: tokio::sync::RwLock<Option<KernelClient>>,
    state: Mutex<KernelState>,
    starting: tokio::sync::Mutex<()>,
}

/// A proposal waiting for `proposal_respond`.
pub struct PendingProposal {
    pub conversation: String,
    pub tx: tokio::sync::oneshot::Sender<ProposalDecision>,
}

pub struct SessionRuntime {
    pub id: String,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub dir: PathBuf,
    pub opts: RuntimeOptions,
    pub global: RwLock<GlobalConfig>,
    session: Mutex<Session>,
    meta: Mutex<SessionMeta>,
    target: RwLock<Target>,
    project: RwLock<ProjectConfig>,
    kernel: KernelSlot,
    events: broadcast::Sender<SessionEvent>,
    runs: Mutex<HashMap<String, RunRecord>>,
    run_registration: tokio::sync::Mutex<()>,
    index_cache: tokio::sync::RwLock<Option<IndexResult>>,
    conversations: Mutex<HashMap<String, Arc<ConversationRuntime>>>,
    pub(super) pending: Mutex<HashMap<ProposalId, PendingProposal>>,
    pub tasks: Arc<TaskRegistry>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    background: Mutex<Vec<JoinHandle<()>>>,
    closed: AtomicBool,
    last_sync_hash: tokio::sync::Mutex<Option<String>>,
    /// Test hook: a model client used for every conversation instead of the configured one.
    model_override: Mutex<Option<Arc<dyn crate::providers::ModelClient>>>,
    /// `Weak` self reference so background tasks and approvers can reach the runtime.
    this: std::sync::Weak<SessionRuntime>,
}

impl SessionRuntime {
    /// Open a stored session. Starts the watcher; the kernel starts in the background when
    /// `opts.autostart_kernel` is set.
    pub fn open(
        session: Session,
        global: GlobalConfig,
        opts: RuntimeOptions,
        tasks: Arc<TaskRegistry>,
    ) -> ServiceResult<Arc<SessionRuntime>> {
        let repo = session.repo();
        if !repo.is_dir() {
            return Err(ServiceError::not_found(format!(
                "repo `{}` does not exist",
                repo.display()
            )));
        }
        let project = config::load_project(&repo)?;
        let target_cfg = config::find_target(&global, &session.record.target)?;
        let target = Target::from_config(&target_cfg)?;
        let dir = session.dir.clone();
        let mut meta = load_meta(&dir);
        meta.last_opened_at = now_rfc3339();
        if meta.conversations.is_empty() {
            let existing = session.conversations().unwrap_or_default();
            for id in existing {
                meta.conversations.push(ConversationMeta {
                    id: id.clone(),
                    title: id.clone(),
                    model: None,
                    mode: Mode::Build,
                    created_at: now_rfc3339(),
                });
            }
        }
        if meta.conversations.is_empty() {
            meta.conversations.push(ConversationMeta {
                id: crate::util::stamped_id(),
                title: "Conversation 1".into(),
                model: None,
                mode: Mode::Build,
                created_at: now_rfc3339(),
            });
        }
        let active_ok = meta
            .active_conversation
            .as_ref()
            .map(|a| meta.conversations.iter().any(|c| &c.id == a))
            .unwrap_or(false);
        if !active_ok {
            meta.active_conversation = meta.conversations.last().map(|c| c.id.clone());
        }
        save_meta(&dir, &meta)?;
        let (events, _) = broadcast::channel(8192);
        let rt = Arc::new_cyclic(|weak| SessionRuntime {
            id: session.id().to_string(),
            repo: repo.clone(),
            home: session.home.clone(),
            dir,
            opts,
            global: RwLock::new(global),
            session: Mutex::new(session),
            meta: Mutex::new(meta),
            target: RwLock::new(target),
            project: RwLock::new(project),
            kernel: KernelSlot {
                client: tokio::sync::RwLock::new(None),
                state: Mutex::new(KernelState::Stopped),
                starting: tokio::sync::Mutex::new(()),
            },
            events,
            runs: Mutex::new(HashMap::new()),
            run_registration: tokio::sync::Mutex::new(()),
            index_cache: tokio::sync::RwLock::new(None),
            conversations: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            tasks,
            watcher: Mutex::new(None),
            background: Mutex::new(vec![]),
            closed: AtomicBool::new(false),
            last_sync_hash: tokio::sync::Mutex::new(None),
            model_override: Mutex::new(None),
            this: weak.clone(),
        });
        rt.start_watcher();
        if rt.opts.autostart_kernel {
            let r = rt.clone();
            rt.spawn(async move {
                let _ = r.ensure_kernel().await;
            });
        }
        Ok(rt)
    }

    /// Use `client` for every conversation (tests with `ScriptedModelClient`).
    pub fn set_model_override(&self, client: Option<Arc<dyn crate::providers::ModelClient>>) {
        if let Ok(mut m) = self.model_override.lock() {
            *m = client;
        }
    }

    pub fn model_override(&self) -> Option<Arc<dyn crate::providers::ModelClient>> {
        self.model_override.lock().ok().and_then(|m| m.clone())
    }

    pub(super) fn arc(&self) -> Option<Arc<SessionRuntime>> {
        self.this.upgrade()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let h = tokio::spawn(fut);
        if let Ok(mut b) = self.background.lock() {
            b.retain(|h| !h.is_finished());
            b.push(h);
        }
    }

    // ---- events ----

    pub fn events(&self) -> broadcast::Sender<SessionEvent> {
        self.events.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    pub fn emit(&self, e: SessionEvent) {
        let _ = self.events.send(e);
    }

    // ---- state accessors ----

    pub fn session_snapshot(&self) -> Session {
        self.session
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn target(&self) -> Target {
        self.target
            .read()
            .map(|t| t.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn project(&self) -> ProjectConfig {
        self.project
            .read()
            .map(|p| p.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn global(&self) -> GlobalConfig {
        self.global
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn meta(&self) -> SessionMeta {
        self.meta
            .lock()
            .map(|m| m.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    fn update_meta<R>(&self, f: impl FnOnce(&mut SessionMeta) -> R) -> ServiceResult<R> {
        let mut m = self
            .meta
            .lock()
            .map_err(|_| ServiceError::internal("session meta poisoned"))?;
        let r = f(&mut m);
        save_meta(&self.dir, &m)?;
        Ok(r)
    }

    /// Project policy with the session override applied.
    pub fn effective_policy(&self) -> PolicyConfig {
        match self.meta().policy_override {
            Some(p) => p,
            None => self.project().policy,
        }
    }

    /// Project config with the effective policy.
    pub fn effective_project(&self) -> ProjectConfig {
        let mut p = self.project();
        p.policy = self.effective_policy();
        p
    }

    pub fn out_dir(&self) -> String {
        config::effective_project(&self.project(), self.target().name()).out_dir
    }

    pub fn session_python(&self) -> String {
        if let Some(p) = &self.opts.session_python {
            return p.clone();
        }
        let venv = self.repo.join(".venv/bin/python");
        if venv.is_file() {
            venv.to_string_lossy().to_string()
        } else {
            "python3".to_string()
        }
    }

    pub fn kernel_state(&self) -> KernelState {
        self.kernel
            .state
            .lock()
            .map(|s| *s)
            .unwrap_or(KernelState::Error)
    }

    fn set_kernel_state(&self, state: KernelState, message: Option<String>) {
        if let Ok(mut s) = self.kernel.state.lock() {
            *s = state;
        }
        self.emit(SessionEvent::KernelState { state, message });
    }

    pub fn name(&self) -> String {
        self.meta().name.unwrap_or_else(|| {
            self.repo
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| self.id.clone())
        })
    }

    pub fn summary(&self) -> SessionSummary {
        let s = self.session_snapshot();
        let meta = self.meta();
        SessionSummary {
            id: self.id.clone(),
            name: self.name(),
            repo_abs: self.repo.to_string_lossy().to_string(),
            target: s.record.target.clone(),
            model: Some(s.record.model.clone()).filter(|m| !m.is_empty() && m != "-"),
            created_at: s.record.created_at.clone(),
            last_opened_at: meta.last_opened_at.clone(),
            open: !self.is_closed(),
        }
    }

    pub async fn info(&self) -> SessionInfo {
        let summary = self.summary();
        let meta = self.meta();
        let hello = match self.kernel_client().await {
            Some(k) => k.hello().await,
            None => None,
        };
        let session = self.session_snapshot();
        let conversations = meta
            .conversations
            .iter()
            .map(|c| {
                let turns = session
                    .read_transcript(&c.id)
                    .map(|evs| {
                        evs.iter()
                            .filter(|e| {
                                matches!(e, analyst_protocol::transcript::Event::User { .. })
                            })
                            .count() as u32
                    })
                    .unwrap_or(0);
                ConversationSummary {
                    id: c.id.clone(),
                    title: c.title.clone(),
                    model: c.model.clone().or_else(|| summary.model.clone()),
                    mode: c.mode,
                    created_at: c.created_at.clone(),
                    turns,
                    busy: self.conversation(&c.id).map(|r| r.busy()).unwrap_or(false),
                }
            })
            .collect();
        let target = self.target();
        SessionInfo {
            policy: PolicyView::from(&self.effective_policy()),
            kernel: self.kernel_state(),
            sandbox: hello.as_ref().map(|h| h.sandbox.clone()),
            python_abs: Some(match &hello {
                Some(h) => h.python.clone(),
                None => self.session_python(),
            }),
            conversations,
            active_conversation: meta.active_conversation.clone().unwrap_or_default(),
            target_kind: if target.is_remote() {
                TargetKind::Ssh
            } else {
                TargetKind::Local
            },
            target_host: target.host().map(|h| h.to_string()),
            summary,
        }
    }

    pub async fn emit_session_changed(&self) {
        let info = self.info().await;
        self.emit(SessionEvent::SessionChanged { info });
    }

    pub fn rename(&self, name: &str) -> ServiceResult<SessionSummary> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ServiceError::invalid("name must not be empty"));
        }
        self.update_meta(|m| m.name = Some(name.to_string()))?;
        Ok(self.summary())
    }

    pub fn set_policy(&self, patch: &PolicyPatch) -> ServiceResult<PolicyView> {
        let mut cfg = self.effective_policy();
        patch.apply(&mut cfg);
        // Validate protected globs early.
        crate::policy::Policy::new(&self.repo, cfg.clone(), Mode::Build)
            .map_err(|e| ServiceError::invalid(e.to_string()))?;
        self.update_meta(|m| m.policy_override = Some(cfg.clone()))?;
        Ok(PolicyView::from(&cfg))
    }

    pub fn layout_get(&self) -> ServiceResult<Option<String>> {
        match std::fs::read_to_string(self.dir.join("layout.json")) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn layout_save(&self, layout: &str) -> ServiceResult<()> {
        std::fs::write(self.dir.join("layout.json"), layout)?;
        Ok(())
    }

    // ---- kernel ----

    fn transport(&self) -> Box<dyn KernelTransport> {
        let project = self.project();
        let target = self.target();
        let eff = config::effective_project(&project, target.name());
        match &target {
            Target::Local => {
                let runs = self.home.join("runs");
                let _ = std::fs::create_dir_all(&runs);
                let args = KernelArgs {
                    python: self.session_python(),
                    root: self.repo.to_string_lossy().to_string(),
                    runs_dir: runs.to_string_lossy().to_string(),
                    out_dir: Some(eff.out_dir),
                    sandbox: Some(self.opts.sandbox.clone().unwrap_or_else(|| "auto".into())),
                };
                match &self.opts.kernel_launcher {
                    Some(l) => Box::new(LocalStdio::with_launcher(l.clone(), args)),
                    None => Box::new(LocalStdio::new(self.opts.tooling_python.clone(), args)),
                }
            }
            Target::Ssh { host, sandbox, .. } => {
                let mirror = target.mirror_dir(&self.repo).unwrap_or_default();
                Box::new(SshStdio {
                    host: host.clone(),
                    control_dir: self.opts.control_dir.clone(),
                    remote_launcher: vec![
                        "~/.analyst/tooling/.venv/bin/python".into(),
                        "-m".into(),
                        "analyst_kernel".into(),
                    ],
                    args: KernelArgs {
                        python: format!("{mirror}/.venv/bin/python"),
                        root: mirror,
                        runs_dir: "~/.analyst/runs".into(),
                        out_dir: Some(eff.out_dir),
                        sandbox: Some(self.opts.sandbox.clone().unwrap_or_else(|| sandbox.clone())),
                    },
                })
            }
        }
    }

    /// The connected kernel client, if any.
    pub async fn kernel_client(&self) -> Option<KernelClient> {
        let guard = self.kernel.client.read().await;
        match guard.as_ref() {
            Some(k) if k.is_connected().await => Some(k.clone()),
            _ => None,
        }
    }

    fn offline_error(&self, msg: String) -> ServiceError {
        if self.target().is_remote() {
            ServiceError::target_offline(msg)
        } else {
            ServiceError::kernel_offline(msg)
        }
    }

    /// The kernel, starting (or reconnecting) it on demand.
    pub async fn ensure_kernel(&self) -> ServiceResult<KernelClient> {
        if let Some(k) = self.kernel_client().await {
            return Ok(k);
        }
        if self.is_closed() {
            return Err(ServiceError::invalid("session is closed"));
        }
        let _starting = self.kernel.starting.lock().await;
        if let Some(k) = self.kernel_client().await {
            return Ok(k);
        }
        self.set_kernel_state(KernelState::Starting, None);
        let existing = self.kernel.client.read().await.clone();
        let result = match existing {
            Some(k) => {
                tokio::time::timeout(Duration::from_secs(90), k.reconnect(crate::CORE_VERSION))
                    .await
                    .map_err(|_| ServiceError::internal("kernel start timed out"))
                    .and_then(|r| r.map(|_| k).map_err(ServiceError::from))
            }
            None => {
                let t = self.transport();
                tracing::info!("kernel: {}", t.describe());
                tokio::time::timeout(
                    Duration::from_secs(90),
                    KernelClient::connect(t, crate::CORE_VERSION),
                )
                .await
                .map_err(|_| ServiceError::internal("kernel start timed out"))
                .and_then(|r| r.map_err(ServiceError::from))
            }
        };
        match result {
            Ok(k) => {
                let first = self.kernel.client.read().await.is_none();
                *self.kernel.client.write().await = Some(k.clone());
                self.set_kernel_state(KernelState::Ready, None);
                if first {
                    self.start_kernel_forwarder(k.clone());
                }
                // Warm the index in the background.
                if let Some(rt) = self.arc() {
                    self.spawn(async move {
                        let _ = rt.refresh_index().await;
                    });
                }
                Ok(k)
            }
            Err(e) => {
                let remote = self.target().is_remote();
                let msg = format!("{e}");
                self.set_kernel_state(
                    if remote {
                        KernelState::Offline
                    } else {
                        KernelState::Error
                    },
                    Some(msg.clone()),
                );
                Err(self.offline_error(format!("kernel unavailable: {msg}")))
            }
        }
    }

    /// Kernel if connected, else the offline error (no start attempt).
    pub async fn kernel(&self) -> ServiceResult<KernelClient> {
        match self.kernel_client().await {
            Some(k) => Ok(k),
            None => self.ensure_kernel().await,
        }
    }

    pub async fn shutdown_kernel(&self) {
        let k = self.kernel.client.write().await.take();
        if let Some(k) = k {
            let _ = tokio::time::timeout(Duration::from_secs(5), k.shutdown()).await;
        }
        if let Ok(mut s) = self.kernel.state.lock() {
            *s = KernelState::Stopped;
        }
        self.emit(SessionEvent::KernelState {
            state: KernelState::Stopped,
            message: None,
        });
    }

    /// Forward kernel notifications as run events and watch the connection.
    fn start_kernel_forwarder(&self, k: KernelClient) {
        let Some(rt) = self.arc() else { return };
        let mut rx = k.subscribe();
        self.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut was_ready = true;
            loop {
                if rt.is_closed() {
                    break;
                }
                tokio::select! {
                    n = rx.recv() => match n {
                        Ok(n) => rt.handle_notification(n).await,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    _ = tick.tick() => {
                        let connected = k.is_connected().await;
                        if was_ready && !connected && rt.kernel_state() == KernelState::Ready {
                            let remote = rt.target().is_remote();
                            rt.set_kernel_state(
                                if remote { KernelState::Offline } else { KernelState::Error },
                                Some("kernel connection lost".into()),
                            );
                            was_ready = false;
                            // One automatic reconnect attempt; later commands retry on demand.
                            let r = rt.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                let _ = r.ensure_kernel().await;
                            });
                        } else if connected {
                            was_ready = true;
                        }
                    }
                }
            }
        });
    }

    // ---- runs ----

    fn run_record(&self, run_id: &str) -> Option<RunRecord> {
        self.runs.lock().ok().and_then(|r| r.get(run_id).cloned())
    }

    fn with_run<R>(&self, run_id: &str, f: impl FnOnce(&mut RunRecord) -> R) -> Option<R> {
        self.runs
            .lock()
            .ok()
            .and_then(|mut r| r.get_mut(run_id).map(f))
    }

    fn insert_run(&self, rec: RunRecord) {
        if let Ok(mut r) = self.runs.lock() {
            r.insert(rec.meta.run_id.clone(), rec);
        }
    }

    fn record_from_meta(&self, meta: RunMeta, source: RunSource) -> RunRecord {
        let finished = meta.status.is_finished();
        RunRecord {
            meta,
            target: self.target().name().to_string(),
            stdout: String::new(),
            stderr: String::new(),
            artifacts: vec![],
            source,
            finished,
        }
    }

    /// Register a kernel run that was started elsewhere (agent tool, terminal through the
    /// kernel) when its first notification or `run.json` shows up.
    async fn register_kernel_run(&self, run_id: &str) -> Option<RunRecord> {
        let _g = self.run_registration.lock().await;
        if let Some(r) = self.run_record(run_id) {
            return Some(r);
        }
        let k = self.kernel_client().await?;
        let meta = k.run_status(run_id).await.ok()?;
        let rec = self.record_from_meta(meta, RunSource::Kernel);
        self.insert_run(rec.clone());
        if let Ok(s) = self.session.lock() {
            let _ = s.append_run(&rec.meta);
        }
        self.emit(SessionEvent::RunStarted { run: rec.view() });
        if rec.finished {
            self.emit(SessionEvent::RunDone { run: rec.view() });
        }
        Some(rec)
    }

    async fn handle_notification(&self, n: KernelNotification) {
        let Some(run_id) = n.run_id().map(|s| s.to_string()) else {
            return;
        };
        if self.run_record(&run_id).is_none() && self.register_kernel_run(&run_id).await.is_none() {
            return;
        }
        match n {
            KernelNotification::Stdout(o) => {
                self.with_run(&run_id, |r| r.stdout.push_str(&o.chunk));
                self.emit(SessionEvent::RunOutput {
                    run_id,
                    stream: OutputStream::Stdout,
                    chunk: o.chunk,
                });
            }
            KernelNotification::Stderr(o) => {
                self.with_run(&run_id, |r| r.stderr.push_str(&o.chunk));
                self.emit(SessionEvent::RunOutput {
                    run_id,
                    stream: OutputStream::Stderr,
                    chunk: o.chunk,
                });
            }
            KernelNotification::Artifact(a) => {
                let script = self
                    .with_run(&run_id, |r| {
                        r.artifacts.push(a.artifact.clone());
                        r.meta.script.clone()
                    })
                    .unwrap_or_default();
                let figure = if a.artifact.kind == ArtifactKind::Figure {
                    figures::figure_view(
                        &self.repo,
                        &self.repo.join(&a.artifact.path),
                        &run_id,
                        &script,
                    )
                } else {
                    None
                };
                self.emit(SessionEvent::RunArtifact {
                    run_id,
                    artifact: a.artifact,
                    figure,
                });
            }
            KernelNotification::Done(d) => {
                let meta = match self.kernel_client().await {
                    Some(k) => k.run_status(&run_id).await.ok(),
                    None => None,
                };
                let view = self.with_run(&run_id, |r| {
                    match meta {
                        Some(m) => r.meta = m,
                        None => {
                            r.meta.status = d.status;
                            r.meta.exit_code = d.exit_code;
                            r.meta.finished_at = Some(now_rfc3339());
                        }
                    }
                    r.finished = true;
                    r.view()
                });
                if let Some(v) = view {
                    if let Ok(s) = self.session.lock() {
                        let _ = s.append_run(&v.meta);
                    }
                    self.pull_run_artifacts(&v).await;
                    self.emit(SessionEvent::RunDone { run: v });
                }
            }
            KernelNotification::Other { .. } => {}
        }
    }

    /// On a remote target, pull the run's rendered artifacts into the repo and the session.
    async fn pull_run_artifacts(&self, v: &RunView) {
        let target = self.target();
        let (Target::Ssh { host, .. }, Some(mirror)) = (&target, target.mirror_dir(&self.repo))
        else {
            return;
        };
        if let Err(e) = target::sync::pull_run(
            host,
            &mirror,
            &v.meta.out_dir,
            &self.repo,
            self.opts.control_dir.as_deref(),
        )
        .await
        {
            tracing::warn!("pulling run {} failed: {e}", v.meta.run_id);
            return;
        }
        let session = self.session_snapshot();
        for a in v.artifacts.iter().filter(|a| a.rendered) {
            let src = self.repo.join(&a.path);
            let dst = session.pulled_artifact_path(&v.meta.run_id, &a.path);
            if let Some(p) = dst.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::copy(&src, &dst);
        }
    }

    /// Sync the mirror when the target is remote. Returns the tree hash.
    pub async fn sync_if_needed(&self) -> ServiceResult<Option<String>> {
        let target = self.target();
        if !target.is_remote() {
            return Ok(None);
        }
        let last = self.last_sync_hash.lock().await.clone();
        let report = target::sync::push(
            &self.repo,
            &target,
            self.opts.control_dir.as_deref(),
            last.as_deref(),
            false,
        )
        .await
        .map_err(|e| ServiceError::target_offline(format!("sync failed: {e:#}")))?;
        if let Some(w) = &report.warning {
            tracing::warn!("{w}");
        }
        *self.last_sync_hash.lock().await = Some(report.tree_hash.clone());
        if let Ok(mut s) = self.session.lock() {
            let _ = s.set_last_tree_hash(Some(report.tree_hash.clone()));
        }
        Ok(Some(report.tree_hash))
    }

    pub async fn run_start(
        &self,
        script: &str,
        args: Vec<String>,
        network: Option<bool>,
    ) -> ServiceResult<RunView> {
        let script = normalize_rel_path(script).map_err(ServiceError::invalid)?;
        if !self.repo.join(&script).is_file() {
            return Err(ServiceError::not_found(format!(
                "script `{script}` not found"
            )));
        }
        if network.unwrap_or(false)
            && self.effective_policy().network == analyst_protocol::config::NetworkPolicy::Deny
        {
            return Err(ServiceError::policy_denied(
                "network access for runs is denied by policy",
            ));
        }
        let k = self.ensure_kernel().await?;
        let tree_hash = match self.sync_if_needed().await? {
            Some(h) => Some(h),
            None => target::sync::file_list(&self.repo)
                .ok()
                .map(|l| target::sync::tree_hash(&l)),
        };
        let eff = config::effective_project(&self.project(), self.target().name());
        let _g = self.run_registration.lock().await;
        let started = k
            .run_start(RunStartParams {
                script: script.clone(),
                args: Some(args.clone()),
                env: None,
                timeout_s: Some(eff.timeout_s as f64),
                network,
                tree_hash,
            })
            .await?;
        let meta = match k.run_status(&started.run_id).await {
            Ok(m) => m,
            Err(_) => RunMeta {
                run_id: started.run_id.clone(),
                script: script.clone(),
                args: args.clone(),
                status: RunStatus::Running,
                exit_code: None,
                started_at: now_rfc3339(),
                finished_at: None,
                out_dir: started.out_dir.clone(),
                sandbox: "unknown".into(),
                tree_hash: None,
                pid: None,
                host: "local".into(),
            },
        };
        let rec = self.record_from_meta(meta, RunSource::Kernel);
        self.insert_run(rec.clone());
        if let Ok(s) = self.session.lock() {
            let _ = s.append_run(&rec.meta);
        }
        let view = rec.view();
        self.emit(SessionEvent::RunStarted { run: view.clone() });
        Ok(view)
    }

    pub async fn run_cancel(&self, run_id: &str) -> ServiceResult<()> {
        let k = self.kernel().await?;
        let r = k.run_cancel(run_id).await?;
        if !r.ok {
            return Err(ServiceError::not_found(format!(
                "run `{run_id}` is not running"
            )));
        }
        Ok(())
    }

    /// Runs known to this process plus the session's run history, newest first.
    pub fn runs_list(&self, limit: Option<u32>) -> Vec<RunView> {
        let mut views: Vec<RunView> = self
            .runs
            .lock()
            .map(|r| r.values().map(|v| v.view()).collect())
            .unwrap_or_default();
        let mut seen: BTreeSet<String> = views.iter().map(|v| v.meta.run_id.clone()).collect();
        if let Ok(runs) = self.session_snapshot().read_runs() {
            for r in runs.into_iter().rev() {
                if seen.insert(r.meta.run_id.clone()) {
                    views.push(RunView {
                        meta: r.meta,
                        target: r.target,
                        artifacts: vec![],
                        stdout_tail: String::new(),
                        stderr_tail: String::new(),
                    });
                }
            }
        }
        views.sort_by(|a, b| b.meta.started_at.cmp(&a.meta.started_at));
        views.truncate(limit.unwrap_or(50).max(1) as usize);
        views
    }

    pub fn run_get(&self, run_id: &str) -> ServiceResult<RunDetail> {
        if let Some(r) = self.run_record(run_id) {
            return Ok(RunDetail {
                view: r.view(),
                stdout: r.stdout,
                stderr: r.stderr,
            });
        }
        let run = self
            .session_snapshot()
            .find_run(run_id)?
            .ok_or_else(|| ServiceError::not_found(format!("run `{run_id}` not found")))?;
        let runs_dir = self.home.join("runs").join(run_id);
        let stdout = std::fs::read_to_string(runs_dir.join("stdout.log")).unwrap_or_default();
        let stderr = std::fs::read_to_string(runs_dir.join("stderr.log")).unwrap_or_default();
        let artifacts = crate::session::read_jsonl::<Artifact>(&runs_dir.join("artifacts.jsonl"))
            .unwrap_or_default();
        Ok(RunDetail {
            view: RunView {
                meta: run.meta,
                target: run.target,
                artifacts,
                stdout_tail: tail(&stdout, TAIL_BYTES),
                stderr_tail: tail(&stderr, TAIL_BYTES),
            },
            stdout,
            stderr,
        })
    }

    // ---- index and checks ----

    pub async fn index_get(&self) -> ServiceResult<IndexResult> {
        if let Some(i) = self.index_cache.read().await.clone() {
            return Ok(i);
        }
        self.kernel().await?;
        self.refresh_index().await
    }

    /// Re-index with the connected kernel (does not start it).
    pub async fn refresh_index(&self) -> ServiceResult<IndexResult> {
        let k = self
            .kernel_client()
            .await
            .ok_or_else(|| self.offline_error("kernel is not connected".into()))?;
        let project = self.project();
        let scripts = k
            .scripts_index(Some(project.project.scripts.clone()))
            .await?;
        let lib = k.lib_index(Some(project.project.library.clone())).await?;
        let result = IndexResult {
            scripts: scripts.scripts,
            library: lib.functions,
        };
        *self.index_cache.write().await = Some(result.clone());
        self.emit(SessionEvent::IndexChanged);
        Ok(result)
    }

    /// Every `.py` under the script globs and the library (for clearing stale diagnostics).
    fn all_py_paths(&self) -> Vec<String> {
        let project = self.project();
        let mut out = BTreeSet::new();
        let mut roots: Vec<String> = project
            .project
            .scripts
            .iter()
            .map(|g| {
                g.split(['*', '?', '['])
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/')
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect();
        roots.push(project.project.library.replace('.', "/"));
        for root in roots {
            let base = self.repo.join(&root);
            for e in walkdir::WalkDir::new(&base)
                .into_iter()
                .filter_entry(|e| {
                    let n = e.file_name().to_string_lossy();
                    n != "__pycache__" && n != ".venv"
                })
                .flatten()
            {
                if e.file_type().is_file()
                    && e.path().extension().map(|x| x == "py").unwrap_or(false)
                {
                    if let Ok(rel) = e.path().strip_prefix(&self.repo) {
                        out.insert(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// Contract check + lint; also emits `diagnostics` for the affected paths.
    pub async fn check_run(&self, paths: Option<Vec<String>>) -> ServiceResult<Vec<Diagnostic>> {
        let k = self.kernel().await?;
        let paths = paths.map(|ps| {
            ps.into_iter()
                .filter_map(|p| normalize_rel_path(&p).ok())
                .collect::<Vec<_>>()
        });
        let mut diags = k.contract_check(paths.clone()).await?.diagnostics;
        match k.lint_run(paths.clone()).await {
            Ok(r) => diags.extend(r.diagnostics),
            Err(e) => tracing::warn!("lint.run failed: {e}"),
        }
        diags.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        let mut affected: BTreeSet<String> = match &paths {
            Some(p) => p.iter().cloned().collect(),
            None => self.all_py_paths().into_iter().collect(),
        };
        for d in &diags {
            affected.insert(d.path.clone());
        }
        self.emit(SessionEvent::Diagnostics {
            paths: affected.into_iter().collect(),
            diagnostics: diags.clone(),
        });
        Ok(diags)
    }

    // ---- outputs, equations, packages ----

    pub async fn out_list(
        &self,
        path: Option<String>,
        run_id: Option<String>,
    ) -> ServiceResult<OutListResult> {
        let k = self.kernel().await?;
        Ok(k.out_list(OutListParams { path, run_id }).await?)
    }

    pub async fn out_read(
        &self,
        path: String,
        max_bytes: Option<u64>,
    ) -> ServiceResult<OutReadResult> {
        let k = self.kernel().await?;
        Ok(k.out_read(OutReadParams { path, max_bytes }).await?)
    }

    pub async fn eq_extract(&self, path: &str) -> ServiceResult<Vec<Equation>> {
        let path = normalize_rel_path(path).map_err(ServiceError::invalid)?;
        let k = self.kernel().await?;
        Ok(k.eq_extract(EqExtractParams {
            path,
            function: None,
        })
        .await?
        .equations)
    }

    pub async fn eq_check(&self, path: &str, function: &str) -> ServiceResult<EqCheckResult> {
        let path = normalize_rel_path(path).map_err(ServiceError::invalid)?;
        let k = self.kernel().await?;
        Ok(k.eq_check(EqCheckParams {
            path,
            function: function.to_string(),
            trials: None,
            tol: None,
            ranges: None,
        })
        .await?)
    }

    pub async fn env_packages(&self) -> ServiceResult<Vec<PackageInfo>> {
        let k = self.kernel().await?;
        Ok(k.env_packages().await?.packages)
    }

    /// `uv add|remove|sync` as a task; `packages_changed` on success.
    pub fn env_task(&self, op: &str, packages: Vec<String>) -> ServiceResult<TaskId> {
        if op != "sync" && packages.is_empty() {
            return Err(ServiceError::invalid(
                "`packages` must list at least one package",
            ));
        }
        let policy = self.effective_policy();
        if policy.packages == analyst_protocol::config::AskPolicy::Deny {
            return Err(ServiceError::policy_denied(
                "package changes are denied by policy",
            ));
        }
        let has_pyproject = self.repo.join("pyproject.toml").is_file();
        let sub: Vec<String> = match (op, has_pyproject) {
            ("add", true) => vec!["add".into()],
            ("add", false) => vec!["pip".into(), "install".into()],
            ("remove", true) => vec!["remove".into()],
            ("remove", false) => vec!["pip".into(), "uninstall".into()],
            ("sync", true) => vec!["sync".into()],
            ("sync", false) => {
                return Err(ServiceError::invalid(
                    "env_sync needs a pyproject.toml / uv.lock",
                ))
            }
            _ => {
                return Err(ServiceError::invalid(format!(
                    "unknown package operation `{op}`"
                )))
            }
        };
        let id = self.tasks.alloc(&format!("env-{op}"));
        let logger = TaskLogger::new(id.clone(), self.events());
        let Some(rt) = self.arc() else {
            return Err(ServiceError::internal("session runtime gone"));
        };
        let target = self.target();
        self.spawn(async move {
            let r: anyhow::Result<()> = async {
                match &target {
                    Target::Local => {
                        let mut line = vec!["uv".to_string()];
                        line.extend(sub);
                        if !has_pyproject {
                            line.push("--python".into());
                            line.push(rt.session_python());
                        }
                        line.extend(packages);
                        logger.stream_command(&line, Some(&rt.repo)).await
                    }
                    Target::Ssh { host, .. } => {
                        rt.sync_if_needed().await?;
                        let mirror = target.mirror_dir(&rt.repo).unwrap_or_default();
                        let mut parts = vec!["~/.analyst/bin/uv".to_string()];
                        parts.extend(sub);
                        parts.extend(packages.iter().map(|p| crate::util::sh_quote(p)));
                        let remote = format!(
                            "cd {} && {}",
                            crate::util::sh_quote(&mirror),
                            parts.join(" ")
                        );
                        let line =
                            target::ssh_command(host, rt.opts.control_dir.as_deref(), &remote);
                        logger.stream_command(&line, None).await
                    }
                }
            }
            .await;
            let ok = r.is_ok();
            logger.finish(r);
            if ok {
                rt.emit(SessionEvent::PackagesChanged);
            }
        });
        Ok(id)
    }

    /// Re-bind the session to another target (sync + session env on remote hosts), restart the
    /// kernel, and report `session_changed` when done.
    pub fn set_target_task(&self, target_name: &str) -> ServiceResult<TaskId> {
        let cfg = config::find_target(&self.global(), target_name)
            .map_err(|e| ServiceError::not_found(e.to_string()))?;
        let new_target =
            Target::from_config(&cfg).map_err(|e| ServiceError::invalid(e.to_string()))?;
        let id = self.tasks.alloc("set-target");
        let logger = TaskLogger::new(id.clone(), self.events());
        let Some(rt) = self.arc() else {
            return Err(ServiceError::internal("session runtime gone"));
        };
        self.spawn(async move {
            let r: anyhow::Result<()> = async {
                logger.log(format!("switching target to {}", new_target.name()));
                rt.shutdown_kernel().await;
                if let Ok(mut t) = rt.target.write() {
                    *t = new_target.clone();
                }
                *rt.last_sync_hash.lock().await = None;
                if let Ok(mut s) = rt.session.lock() {
                    s.record.target = new_target.name().to_string();
                    s.save()?;
                }
                if let Target::Ssh { host, .. } = &new_target {
                    let wheel = target::bootstrap::find_dev_wheel();
                    logger.log("checking remote tooling");
                    match target::bootstrap::ensure(
                        &new_target,
                        rt.opts.control_dir.as_deref(),
                        wheel.as_deref(),
                    )
                    .await?
                    {
                        target::bootstrap::Outcome::UpToDate(v) => {
                            logger.log(format!("tooling {v} already installed"))
                        }
                        target::bootstrap::Outcome::Installed(v) => {
                            logger.log(format!("installed tooling {v}"))
                        }
                    }
                    logger.log("syncing repository mirror");
                    rt.sync_if_needed().await?;
                    let mirror = new_target.mirror_dir(&rt.repo).unwrap_or_default();
                    let cmd = target::bootstrap::session_env_command(&mirror);
                    let line = target::ssh_command(host, rt.opts.control_dir.as_deref(), &cmd);
                    logger.stream_command(&line, None).await?;
                } else if rt.repo.join("pyproject.toml").is_file() && crate::init::has_uv() {
                    let line = vec!["uv".to_string(), "sync".to_string()];
                    logger.stream_command(&line, Some(&rt.repo)).await?;
                }
                *rt.index_cache.write().await = None;
                let _ = rt.ensure_kernel().await;
                Ok(())
            }
            .await;
            logger.finish(r);
            rt.emit_session_changed().await;
        });
        Ok(id)
    }

    // ---- figures ----

    fn synthesized_view(&self, s: &ScannedRun) -> RunView {
        RunView {
            meta: RunMeta {
                run_id: s.run_id.clone(),
                script: s.script.clone(),
                args: s.args.clone(),
                status: RunStatus::Done,
                exit_code: None,
                started_at: s.started_at.clone(),
                finished_at: None,
                out_dir: s.dir_rel.clone(),
                sandbox: "none".into(),
                tree_hash: None,
                pid: None,
                host: s.host.clone().unwrap_or_else(|| "local".into()),
            },
            target: self.target().name().to_string(),
            artifacts: vec![],
            stdout_tail: String::new(),
            stderr_tail: String::new(),
        }
    }

    /// Figures grouped by run, newest run first, including terminal runs found under `out/`
    /// and artifacts pulled from remote targets.
    pub fn figures_list(&self, limit: Option<u32>) -> Vec<FigureGroup> {
        let out_dir = self.out_dir();
        let session = self.session_snapshot();
        let mut groups: Vec<FigureGroup> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for scanned in figures::scan_out(&self.repo, &out_dir) {
            let run = match self.run_record(&scanned.run_id) {
                Some(r) => r.view(),
                None => match session.find_run(&scanned.run_id).ok().flatten() {
                    Some(r) => RunView {
                        meta: r.meta,
                        target: r.target,
                        artifacts: vec![],
                        stdout_tail: String::new(),
                        stderr_tail: String::new(),
                    },
                    None => self.synthesized_view(&scanned),
                },
            };
            seen.insert(scanned.run_id.clone());
            groups.push(FigureGroup {
                run,
                figures: scanned.figures,
            });
        }
        // Remote runs whose figures were pulled into the session artifacts.
        if let Ok(runs) = session.read_runs() {
            for r in runs.into_iter().rev() {
                if seen.contains(&r.meta.run_id) {
                    continue;
                }
                let figs =
                    figures::scan_pulled(&session.artifacts_dir(), &r.meta.run_id, &r.meta.script);
                if figs.is_empty() {
                    continue;
                }
                seen.insert(r.meta.run_id.clone());
                let run = self
                    .run_record(&r.meta.run_id)
                    .map(|rec| rec.view())
                    .unwrap_or(RunView {
                        meta: r.meta,
                        target: r.target,
                        artifacts: vec![],
                        stdout_tail: String::new(),
                        stderr_tail: String::new(),
                    });
                groups.push(FigureGroup { run, figures: figs });
            }
        }
        groups.sort_by(|a, b| b.run.meta.started_at.cmp(&a.run.meta.started_at));
        groups.truncate(limit.unwrap_or(100).max(1) as usize);
        groups
    }

    /// Bytes of a figure (or any file) under the repo; pulled from a remote target on first
    /// access and cached under the session's `artifacts/`.
    pub async fn figure_data(&self, path: &str) -> ServiceResult<FigureData> {
        let rel = normalize_rel_path(path).map_err(ServiceError::invalid)?;
        let local = self.repo.join(&rel);
        let bytes = match std::fs::read(&local) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let target = self.target();
                let (Target::Ssh { host, .. }, Some(mirror)) =
                    (&target, target.mirror_dir(&self.repo))
                else {
                    return Err(ServiceError::not_found(format!("`{rel}` not found")));
                };
                let session = self.session_snapshot();
                let run_id = figures::run_dir_of(&self.out_dir(), &rel)
                    .and_then(|d| figures::read_run_json(&self.repo.join(d).join("run.json")))
                    .map(|j| j.run_id)
                    .unwrap_or_else(|| "pulled".into());
                let dst = session.pulled_artifact_path(&run_id, &rel);
                if !dst.is_file() {
                    target::sync::pull_artifact(
                        host,
                        &mirror,
                        &rel,
                        &dst,
                        self.opts.control_dir.as_deref(),
                    )
                    .await
                    .map_err(|e| ServiceError::target_offline(format!("pulling `{rel}`: {e:#}")))?;
                }
                std::fs::read(&dst)?
            }
            Err(e) => return Err(e.into()),
        };
        let mime = FigureFormat::from_path(&rel)
            .map(|f| f.mime().to_string())
            .unwrap_or_else(|| guess_mime(&rel).to_string());
        Ok(FigureData {
            mime,
            base64: fs::to_base64(&bytes),
        })
    }

    // ---- filesystem ----

    pub fn fs_tree(&self, path: Option<&str>, depth: Option<u32>) -> ServiceResult<Vec<FileEntry>> {
        fs::tree(&self.repo, path, depth, &self.out_dir())
    }

    pub fn fs_read(&self, path: &str) -> ServiceResult<FileContent> {
        fs::read(&self.repo, path)
    }

    pub fn fs_read_binary(&self, path: &str) -> ServiceResult<FigureData> {
        let (rel, bytes) = fs::read_binary(&self.repo, path)?;
        Ok(FigureData {
            mime: guess_mime(&rel).to_string(),
            base64: fs::to_base64(&bytes),
        })
    }

    /// Editor save: writes immediately; `.py` files are checked afterwards (`diagnostics`).
    pub fn fs_write(
        &self,
        path: &str,
        content: &str,
        expected_hash: Option<&str>,
    ) -> ServiceResult<WriteResult> {
        let r = fs::write(&self.repo, path, content, expected_hash)?;
        if path.ends_with(".py") {
            if let (Some(rt), Ok(rel)) = (self.arc(), normalize_rel_path(path)) {
                self.spawn(async move {
                    if rt.kernel_client().await.is_some() {
                        let _ = rt.check_run(Some(vec![rel])).await;
                    }
                });
            }
        }
        Ok(r)
    }

    pub fn fs_create(&self, path: &str, is_dir: bool) -> ServiceResult<()> {
        fs::create(&self.repo, path, is_dir)
    }

    pub fn fs_delete(&self, path: &str) -> ServiceResult<()> {
        fs::delete(&self.repo, path)
    }

    pub fn fs_rename(&self, from: &str, to: &str) -> ServiceResult<()> {
        fs::rename(&self.repo, from, to)
    }

    pub async fn fs_search(
        &self,
        pattern: &str,
        glob: Option<&str>,
        max_results: Option<u32>,
    ) -> ServiceResult<Vec<SearchHit>> {
        fs::search(&self.repo, pattern, glob, max_results).await
    }

    // ---- conversations ----

    pub fn conversation(&self, id: &str) -> Option<Arc<ConversationRuntime>> {
        self.conversations
            .lock()
            .ok()
            .and_then(|c| c.get(id).cloned())
    }

    /// The conversation runtime, created on first use for a known conversation id.
    pub fn conversation_runtime(&self, id: &str) -> ServiceResult<Arc<ConversationRuntime>> {
        if let Some(c) = self.conversation(id) {
            return Ok(c);
        }
        let meta = self.meta();
        if !meta.conversations.iter().any(|c| c.id == id) {
            return Err(ServiceError::not_found(format!(
                "conversation `{id}` not found"
            )));
        }
        let c = Arc::new(ConversationRuntime::new(id));
        if let Ok(mut m) = self.conversations.lock() {
            m.insert(id.to_string(), c.clone());
        }
        Ok(c)
    }

    pub fn conversation_meta(&self, id: &str) -> ServiceResult<ConversationMeta> {
        self.meta()
            .conversations
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| ServiceError::not_found(format!("conversation `{id}` not found")))
    }

    pub fn conversation_new(&self, title: Option<&str>) -> ServiceResult<ConversationSummary> {
        let id = crate::util::stamped_id();
        let model = self.summary().model;
        let created_at = now_rfc3339();
        let title = title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Conversation {}", self.meta().conversations.len() + 1));
        let meta = ConversationMeta {
            id: id.clone(),
            title: title.clone(),
            model: None,
            mode: Mode::Build,
            created_at: created_at.clone(),
        };
        self.update_meta(|m| {
            m.conversations.push(meta);
            m.active_conversation = Some(id.clone());
        })?;
        Ok(ConversationSummary {
            id,
            title,
            model,
            mode: Mode::Build,
            created_at,
            turns: 0,
            busy: false,
        })
    }

    pub fn conversation_select(&self, id: &str) -> ServiceResult<()> {
        self.conversation_meta(id)?;
        self.update_meta(|m| m.active_conversation = Some(id.to_string()))
    }

    pub fn conversation_transcript(
        &self,
        id: &str,
    ) -> ServiceResult<Vec<analyst_protocol::transcript::Event>> {
        self.conversation_meta(id)?;
        Ok(self.session_snapshot().read_transcript(id)?)
    }

    pub async fn chat_set_mode(&self, id: &str, mode: Mode) -> ServiceResult<()> {
        self.conversation_meta(id)?;
        self.update_meta(|m| {
            if let Some(c) = m.conversations.iter_mut().find(|c| c.id == id) {
                c.mode = mode;
            }
        })?;
        if let Some(c) = self.conversation(id) {
            c.set_mode(mode).await;
        }
        self.emit_session_changed().await;
        Ok(())
    }

    pub async fn chat_set_model(&self, id: &str, model: &str) -> ServiceResult<()> {
        self.conversation_meta(id)?;
        let spec =
            config::ModelSpec::parse(model).map_err(|e| ServiceError::invalid(e.to_string()))?;
        config::resolve_model(&self.global(), &spec)
            .map_err(|e| ServiceError::not_found(e.to_string()))?;
        self.update_meta(|m| {
            if let Some(c) = m.conversations.iter_mut().find(|c| c.id == id) {
                c.model = Some(model.to_string());
            }
        })?;
        if let Some(c) = self.conversation(id) {
            c.invalidate_model().await;
        }
        self.emit_session_changed().await;
        Ok(())
    }

    /// Resolve the pending proposal; `not_found` when it was already resolved.
    pub fn proposal_respond(
        &self,
        proposal_id: &str,
        decision: ProposalDecision,
    ) -> ServiceResult<()> {
        let pending = self
            .pending
            .lock()
            .ok()
            .and_then(|mut p| p.remove(proposal_id))
            .ok_or_else(|| {
                ServiceError::not_found(format!("proposal `{proposal_id}` is not pending"))
            })?;
        pending.tx.send(decision).map_err(|_| {
            ServiceError::not_found(format!("proposal `{proposal_id}` is no longer awaited"))
        })
    }

    /// Reject every pending proposal of a conversation (`chat_cancel`, close).
    pub fn reject_pending(&self, conversation: Option<&str>) {
        let Ok(mut p) = self.pending.lock() else {
            return;
        };
        let ids: Vec<String> = p
            .iter()
            .filter(|(_, v)| conversation.map(|c| v.conversation == c).unwrap_or(true))
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(pp) = p.remove(&id) {
                let _ = pp.tx.send(ProposalDecision::Reject {
                    reason: Some("cancelled".into()),
                });
                self.emit(SessionEvent::ProposalResolved {
                    proposal_id: id,
                    decision: analyst_protocol::transcript::Decision::Reject,
                    by: analyst_protocol::transcript::DecidedBy::User,
                });
            }
        }
    }

    // ---- watcher ----

    fn start_watcher(&self) {
        use notify::Watcher;
        let Some(rt) = self.arc() else { return };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let _ = tx.send(ev.paths);
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("file watcher unavailable: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&self.repo, notify::RecursiveMode::Recursive) {
            tracing::warn!("watching {}: {e}", self.repo.display());
            return;
        }
        if let Ok(mut w) = self.watcher.lock() {
            *w = Some(watcher);
        }
        self.spawn(async move {
            while let Some(first) = rx.recv().await {
                let mut batch: Vec<PathBuf> = first;
                let deadline = tokio::time::Instant::now() + DEBOUNCE;
                loop {
                    match tokio::time::timeout_at(deadline, rx.recv()).await {
                        Ok(Some(more)) => batch.extend(more),
                        Ok(None) => return,
                        Err(_) => break,
                    }
                }
                if rt.is_closed() {
                    return;
                }
                rt.handle_fs_batch(batch).await;
            }
        });
    }

    fn rel_of(&self, p: &Path) -> Option<String> {
        let rel = p.strip_prefix(&self.repo).ok()?;
        let s = rel.to_string_lossy().replace('\\', "/");
        if s.is_empty() {
            return None;
        }
        let first = s.split('/').next().unwrap_or("");
        if matches!(first, ".git" | ".venv" | ".ruff_cache" | "node_modules")
            || s.split('/').any(|c| c == "__pycache__")
            || s.ends_with(".tmp")
            || s.ends_with('~')
        {
            return None;
        }
        Some(s)
    }

    async fn handle_fs_batch(&self, batch: Vec<PathBuf>) {
        let mut paths: BTreeSet<String> = BTreeSet::new();
        for p in &batch {
            if let Some(r) = self.rel_of(p) {
                paths.insert(r);
            }
        }
        if paths.is_empty() {
            return;
        }
        let out_dir = self.out_dir();
        let out_prefix = format!("{}/", out_dir.trim_matches('/'));
        let paths: Vec<String> = paths.into_iter().collect();
        self.emit(SessionEvent::FilesChanged {
            paths: paths.clone(),
        });
        let py_changed = paths
            .iter()
            .any(|p| p.ends_with(".py") && !p.starts_with(&out_prefix));
        if py_changed && self.kernel_client().await.is_some() {
            let _ = self.refresh_index().await;
        }
        let mut run_dirs: BTreeSet<String> = BTreeSet::new();
        for p in &paths {
            if let Some(d) = figures::run_dir_of(&out_dir, p) {
                run_dirs.insert(d);
            } else if let Some(rest) = p.strip_prefix(&out_prefix) {
                // `out/<script>` itself appeared (new directory tree): inotify may have missed
                // what was created inside it before the watch was added, so look now.
                if !rest.contains('/') && !rest.starts_with('_') {
                    if let Ok(rd) = std::fs::read_dir(self.repo.join(p)) {
                        for e in rd.flatten() {
                            if e.path().is_dir() {
                                let name = e.file_name().to_string_lossy().to_string();
                                if !name.starts_with('_') {
                                    run_dirs.insert(format!("{p}/{name}"));
                                }
                            }
                        }
                    }
                }
            }
        }
        for d in run_dirs {
            self.discover_run_dir(&d).await;
        }
    }

    /// A run directory under `out/` changed: register terminal runs (run_started + run_done)
    /// and report their figures as artifacts.
    async fn discover_run_dir(&self, dir_rel: &str) {
        let abs = self.repo.join(dir_rel);
        if !abs.is_dir() {
            return;
        }
        // Terminals opened by the app get a run.json once they produce something.
        let run_json = abs.join("run.json");
        if !run_json.is_file() {
            let parent = Path::new(dir_rel)
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if parent == "terminal" {
                let stamp = Path::new(dir_rel)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let j = serde_json::json!({
                    "run_id": format!("terminal-{stamp}"),
                    "script": "terminal",
                    "args": [],
                    "started_at": now_rfc3339(),
                    "host": "local",
                    "tree_hash": null,
                });
                let _ = std::fs::write(&run_json, j.to_string());
            }
        }
        let Some(scanned) = figures::scan_run_dir(&self.repo, &abs) else {
            return;
        };
        if !scanned.has_run_json && scanned.figures.is_empty() {
            return;
        }
        let existing = self.run_record(&scanned.run_id);
        let record = match existing {
            Some(r) => r,
            None => {
                // A kernel run started elsewhere (agent tool) shows up here too.
                if let Some(r) = self.register_kernel_run(&scanned.run_id).await {
                    r
                } else {
                    let view = self.synthesized_view(&scanned);
                    let rec = RunRecord {
                        meta: view.meta.clone(),
                        target: view.target.clone(),
                        stdout: String::new(),
                        stderr: String::new(),
                        artifacts: vec![],
                        source: RunSource::Terminal,
                        finished: true,
                    };
                    self.insert_run(rec.clone());
                    if let Ok(s) = self.session.lock() {
                        let _ = s.append_run(&rec.meta);
                    }
                    self.emit(SessionEvent::RunStarted { run: rec.view() });
                    self.emit(SessionEvent::RunDone { run: rec.view() });
                    rec
                }
            }
        };
        if record.source != RunSource::Terminal {
            return;
        }
        for f in scanned.figures {
            let known = record.artifacts.iter().any(|a| a.path == f.path);
            if known {
                continue;
            }
            let artifact = Artifact {
                kind: ArtifactKind::Figure,
                path: f.path.clone(),
                size: f.size,
                created_at: f.created_at.clone(),
                sidecar: f.sidecar.clone(),
                rendered: true,
            };
            self.with_run(&scanned.run_id, |r| r.artifacts.push(artifact.clone()));
            self.emit(SessionEvent::RunArtifact {
                run_id: scanned.run_id.clone(),
                artifact,
                figure: Some(f),
            });
        }
    }

    // ---- lifecycle ----

    /// Suspend the session: cancel turns, reject proposals, stop the watcher and the kernel.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let convs: Vec<Arc<ConversationRuntime>> = self
            .conversations
            .lock()
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default();
        for c in convs {
            c.abort();
        }
        self.reject_pending(None);
        if let Ok(mut w) = self.watcher.lock() {
            *w = None;
        }
        self.shutdown_kernel().await;
        let handles: Vec<JoinHandle<()>> = self
            .background
            .lock()
            .map(|mut b| b.drain(..).collect())
            .unwrap_or_default();
        for h in handles {
            h.abort();
        }
    }
}

pub fn guess_mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "json" => "application/json",
        "csv" => "text/csv",
        "md" | "txt" | "log" | "py" | "toml" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// The tooling interpreter for a global config (dev checkout aware).
pub fn tooling_python(global: &GlobalConfig) -> String {
    resolve_tooling_python(global)
}
