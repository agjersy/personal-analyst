//! The desktop service layer (docs/IPC.md): a [`Workbench`] per app that owns the global config,
//! the open [`SessionRuntime`]s, terminals, and task ids. The Tauri commands in
//! `apps/desktop/src-tauri` are thin wrappers over these methods; the service never touches the
//! webview.

pub mod agent_runner;
pub mod error;
pub mod figures;
pub mod fs;
pub mod runtime;
pub mod tasks;
pub mod terminal;

pub use agent_runner::{ConversationRuntime, UiApprover};
pub use error::{ServiceError, ServiceResult};
pub use runtime::{RuntimeOptions, SessionRuntime};
pub use tasks::{TaskLogger, TaskRegistry};
pub use terminal::{CaptureEnv, Terminal, TerminalManager, TerminalSpec};

use crate::config;
use crate::providers::capabilities;
use crate::session::Session;
use crate::target::Target;
use analyst_protocol::config::{GlobalConfig, ProviderKind, TargetKind};
use analyst_protocol::ui::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

/// Options for the whole app.
#[derive(Debug, Clone)]
pub struct WorkbenchOptions {
    /// `$ANALYST_HOME` (default `~/.analyst`).
    pub home: PathBuf,
    /// Override of the local kernel launcher (tests inject a fake kernel).
    pub kernel_launcher: Option<Vec<String>>,
    /// `--sandbox` override for kernels.
    pub sandbox: Option<String>,
    /// Session interpreter override.
    pub session_python: Option<String>,
    /// Start the kernel in the background on `session_open`.
    pub autostart_kernel: bool,
    /// Reported in `app_info`.
    pub dev_mode: bool,
    /// How `session_create { init: true }` applies the template.
    pub init: crate::init::InitOptions,
}

impl WorkbenchOptions {
    pub fn new(home: PathBuf) -> Self {
        WorkbenchOptions {
            home,
            kernel_launcher: None,
            sandbox: None,
            session_python: None,
            autostart_kernel: true,
            dev_mode: cfg!(debug_assertions),
            init: crate::init::InitOptions::default(),
        }
    }
}

/// One per app.
pub struct Workbench {
    pub opts: WorkbenchOptions,
    pub global: RwLock<GlobalConfig>,
    pub sessions: RwLock<HashMap<SessionId, Arc<SessionRuntime>>>,
    pub terminals: TerminalManager,
    pub tasks: Arc<TaskRegistry>,
    mpl_dir: OnceLock<Option<PathBuf>>,
}

impl Workbench {
    /// Load the global config from `opts.home` (missing file = defaults).
    pub fn new(opts: WorkbenchOptions) -> ServiceResult<Workbench> {
        std::fs::create_dir_all(&opts.home)?;
        let global =
            config::load_global(&opts.home).map_err(|e| ServiceError::invalid(format!("{e:#}")))?;
        Ok(Workbench {
            opts,
            global: RwLock::new(global),
            sessions: RwLock::new(HashMap::new()),
            terminals: TerminalManager::new(),
            tasks: Arc::new(TaskRegistry::new()),
            mpl_dir: OnceLock::new(),
        })
    }

    pub fn home(&self) -> &Path {
        &self.opts.home
    }

    pub fn global(&self) -> GlobalConfig {
        self.global
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    /// Re-read `config.toml` (models or targets edited outside the app).
    pub fn reload_global(&self) -> ServiceResult<GlobalConfig> {
        let g = config::load_global(&self.opts.home)
            .map_err(|e| ServiceError::invalid(format!("{e:#}")))?;
        if let Ok(mut w) = self.global.write() {
            *w = g.clone();
        }
        Ok(g)
    }

    fn control_dir(&self) -> Option<PathBuf> {
        let d = self.opts.home.join("ssh");
        std::fs::create_dir_all(&d).ok()?;
        Some(d)
    }

    fn tooling_python(&self) -> String {
        runtime::tooling_python(&self.global())
    }

    fn mpl_dir(&self) -> Option<PathBuf> {
        self.mpl_dir
            .get_or_init(|| terminal::find_mpl_dir(&self.tooling_python()))
            .clone()
    }

    fn runtime_options(&self) -> RuntimeOptions {
        RuntimeOptions {
            tooling_python: self.tooling_python(),
            kernel_launcher: self.opts.kernel_launcher.clone(),
            sandbox: self.opts.sandbox.clone(),
            session_python: self.opts.session_python.clone(),
            autostart_kernel: self.opts.autostart_kernel,
            control_dir: self.control_dir(),
            mpl_dir: self.mpl_dir(),
        }
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            version: crate::CORE_VERSION.to_string(),
            home_abs: self.opts.home.to_string_lossy().to_string(),
            platform: Platform::current(),
            dev_mode: self.opts.dev_mode,
        }
    }

    // ---- sessions ----

    pub fn session(&self, id: &str) -> ServiceResult<Arc<SessionRuntime>> {
        self.sessions
            .read()
            .ok()
            .and_then(|s| s.get(id).cloned())
            .ok_or_else(|| ServiceError::not_found(format!("session `{id}` is not open")))
    }

    fn open_ids(&self) -> Vec<String> {
        self.sessions
            .read()
            .map(|s| s.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Every stored session, newest first.
    pub fn sessions_list(&self) -> ServiceResult<Vec<SessionSummary>> {
        let open = self.open_ids();
        let mut out = Vec::new();
        for s in crate::session::list(&self.opts.home)? {
            if let Some(rt) = open
                .contains(&s.record.id.to_string())
                .then(|| self.session(s.id()).ok())
                .flatten()
            {
                out.push(rt.summary());
                continue;
            }
            let meta = runtime::load_meta(&s.dir);
            let repo = s.repo();
            out.push(SessionSummary {
                id: s.id().to_string(),
                name: meta.name.unwrap_or_else(|| {
                    repo.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| s.id().to_string())
                }),
                repo_abs: s.record.repo.clone(),
                target: s.record.target.clone(),
                model: Some(s.record.model.clone()).filter(|m| !m.is_empty() && m != "-"),
                created_at: s.record.created_at.clone(),
                last_opened_at: if meta.last_opened_at.is_empty() {
                    s.record.created_at.clone()
                } else {
                    meta.last_opened_at
                },
                open: false,
            });
        }
        out.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(out)
    }

    /// Create (optionally from the template) and open a session.
    pub async fn create_session(
        &self,
        repo_abs: &str,
        target: Option<&str>,
        model: Option<&str>,
        init: bool,
    ) -> ServiceResult<SessionInfo> {
        let repo = PathBuf::from(repo_abs);
        if !repo.is_absolute() {
            return Err(ServiceError::invalid("`repo_abs` must be an absolute path"));
        }
        let global = self.global();
        let target_name = target.unwrap_or(&global.defaults.target).to_string();
        config::find_target(&global, &target_name)
            .map_err(|e| ServiceError::not_found(e.to_string()))?;
        let model = match model {
            Some(m) => {
                let spec = config::ModelSpec::parse(m)
                    .map_err(|e| ServiceError::invalid(e.to_string()))?;
                spec.to_string()
            }
            None => global.defaults.model.clone().unwrap_or_default(),
        };
        if init {
            let repo2 = repo.clone();
            let opts = self.opts.init.clone();
            tokio::task::spawn_blocking(move || crate::init::init_repo(&repo2, &opts))
                .await
                .map_err(|e| ServiceError::internal(e.to_string()))?
                .map_err(|e| ServiceError::io(format!("{e:#}")))?;
        }
        if !repo.is_dir() {
            return Err(ServiceError::not_found(format!(
                "`{}` is not a directory (pass init: true to create it)",
                repo.display()
            )));
        }
        if !repo.join(".git").exists() {
            // Checkpoints and the working-tree diff need a git repo.
            crate::checkpoint::git(&repo, &["init", "-q"], None)
                .map_err(|e| ServiceError::io(format!("git init: {e:#}")))?;
        }
        let session = Session::create(&self.opts.home, &repo, &target_name, &model)
            .map_err(|e| ServiceError::io(format!("{e:#}")))?;
        let id = session.id().to_string();
        self.open_session(&id).await
    }

    /// Open a stored session (idempotent).
    pub async fn open_session(&self, id: &str) -> ServiceResult<SessionInfo> {
        if let Ok(rt) = self.session(id) {
            return Ok(rt.info().await);
        }
        let session = Session::open(&self.opts.home, id)
            .map_err(|e| ServiceError::not_found(format!("{e:#}")))?;
        let rt = SessionRuntime::open(
            session,
            self.global(),
            self.runtime_options(),
            self.tasks.clone(),
        )?;
        if let Ok(mut s) = self.sessions.write() {
            s.insert(id.to_string(), rt.clone());
        }
        Ok(rt.info().await)
    }

    /// Suspend a session: kernel shut down, terminals closed.
    pub async fn close_session(&self, id: &str) -> ServiceResult<()> {
        let rt = self
            .sessions
            .write()
            .ok()
            .and_then(|mut s| s.remove(id))
            .ok_or_else(|| ServiceError::not_found(format!("session `{id}` is not open")))?;
        self.terminals.close_session(id);
        rt.shutdown().await;
        Ok(())
    }

    pub async fn session_info(&self, id: &str) -> ServiceResult<SessionInfo> {
        Ok(self.session(id)?.info().await)
    }

    pub fn rename_session(&self, id: &str, name: &str) -> ServiceResult<SessionSummary> {
        if let Ok(rt) = self.session(id) {
            return rt.rename(name);
        }
        let s = Session::open(&self.opts.home, id)
            .map_err(|e| ServiceError::not_found(format!("{e:#}")))?;
        let mut meta = runtime::load_meta(&s.dir);
        let name = name.trim();
        if name.is_empty() {
            return Err(ServiceError::invalid("name must not be empty"));
        }
        meta.name = Some(name.to_string());
        runtime::save_meta(&s.dir, &meta)?;
        let list = self.sessions_list()?;
        list.into_iter()
            .find(|x| x.id == id)
            .ok_or_else(|| ServiceError::not_found(format!("session `{id}` not found")))
    }

    pub fn layout_get(&self, id: &str) -> ServiceResult<Option<String>> {
        self.session(id)?.layout_get()
    }

    pub fn layout_save(&self, id: &str, layout: &str) -> ServiceResult<()> {
        self.session(id)?.layout_save(layout)
    }

    // ---- models and targets ----

    fn is_local_provider(p: &analyst_protocol::config::ProviderConfig) -> bool {
        let url = p.effective_base_url();
        url.contains("localhost")
            || url.contains("127.0.0.1")
            || url.contains("0.0.0.0")
            || url.contains("[::1]")
    }

    fn model_entry(&self, global: &GlobalConfig, provider: &str, id: &str) -> Option<ModelEntry> {
        let p = global.providers.iter().find(|p| p.name == provider)?;
        let key = format!("{provider}/{id}");
        let caps = capabilities::cached(&self.opts.home, &key);
        let overrides = global
            .models
            .iter()
            .find(|m| m.provider == provider && m.id == id);
        let capabilities = match (caps, overrides) {
            (Some(c), _) => Some(ModelCapabilitiesView {
                context_window: c.context_window,
                tools: c.tools,
                vision: c.vision,
            }),
            (None, Some(o)) if o.context_window.is_some() && o.tools.is_some() => {
                Some(ModelCapabilitiesView {
                    context_window: o
                        .context_window
                        .unwrap_or(capabilities::DEFAULT_CONTEXT_WINDOW),
                    tools: o
                        .tools
                        .unwrap_or(analyst_protocol::config::ToolSupport::Native),
                    vision: o.vision.unwrap_or(false),
                })
            }
            _ => None,
        };
        Some(ModelEntry {
            name: key,
            provider: provider.to_string(),
            id: id.to_string(),
            kind: p.kind,
            local: p.kind == ProviderKind::OpenAiCompatible && Self::is_local_provider(p),
            capabilities,
        })
    }

    /// Models from the global config plus cached capabilities.
    pub fn models_list(&self) -> Vec<ModelEntry> {
        let global = self.global();
        let mut out: Vec<ModelEntry> = Vec::new();
        if let Some(d) = &global.defaults.model {
            if let Ok(spec) = config::ModelSpec::parse(d) {
                if let Some(e) = self.model_entry(&global, &spec.provider, &spec.id) {
                    out.push(e);
                }
            }
        }
        for m in &global.models {
            if out.iter().any(|e| e.provider == m.provider && e.id == m.id) {
                continue;
            }
            if let Some(e) = self.model_entry(&global, &m.provider, &m.id) {
                out.push(e);
            }
        }
        if let Ok(cache) = capabilities::load_cache(&self.opts.home) {
            for key in cache.keys() {
                if let Ok(spec) = config::ModelSpec::parse(key) {
                    if out.iter().any(|e| e.name == *key) {
                        continue;
                    }
                    if let Some(e) = self.model_entry(&global, &spec.provider, &spec.id) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }

    /// Run the capability probe for a model and cache the result.
    pub async fn model_probe(&self, model: &str) -> ServiceResult<ModelEntry> {
        let global = self.global();
        let spec =
            config::ModelSpec::parse(model).map_err(|e| ServiceError::invalid(e.to_string()))?;
        let resolved = config::resolve_model(&global, &spec)
            .map_err(|e| ServiceError::not_found(e.to_string()))?;
        let client = crate::providers::client_for(&resolved)
            .map_err(|e| ServiceError::invalid(format!("{e:#}")))?;
        let key = spec.to_string();
        let info_window = if resolved.provider.kind == ProviderKind::OpenAiCompatible {
            match crate::providers::openai_compat::OpenAiCompatClient::new(
                resolved.provider.effective_base_url(),
                config::api_key(&resolved.provider).ok().flatten(),
                spec.id.clone(),
            ) {
                Ok(c) => tokio::time::timeout(std::time::Duration::from_secs(10), c.list_models())
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .and_then(|info| capabilities::context_window_from_info(&info, &spec.id)),
                Err(_) => None,
            }
        } else {
            None
        };
        capabilities::ensure(
            &self.opts.home,
            &key,
            client.as_ref(),
            resolved.overrides.as_ref(),
            info_window,
            true,
        )
        .await
        .map_err(|e| ServiceError::internal(format!("probe failed: {e:#}")))?;
        self.model_entry(&global, &spec.provider, &spec.id)
            .ok_or_else(|| ServiceError::not_found(format!("model `{model}` not found")))
    }

    pub fn targets_list(&self) -> Vec<TargetInfo> {
        let global = self.global();
        let mut out = vec![TargetInfo {
            name: "local".into(),
            kind: TargetKind::Local,
            host: None,
            remote_root: None,
            sandbox: self.opts.sandbox.clone().unwrap_or_else(|| "auto".into()),
            data_dirs: vec![],
            bootstrapped: Some(true),
        }];
        for t in &global.targets {
            if t.name == "local" {
                continue;
            }
            out.push(TargetInfo {
                name: t.name.clone(),
                kind: t.kind,
                host: t.host.clone(),
                remote_root: Some(t.remote_root.clone().unwrap_or_else(|| "~/analyst".into())),
                sandbox: t.sandbox.clone().unwrap_or_else(|| "auto".into()),
                data_dirs: t.data_dirs.clone(),
                bootstrapped: None,
            });
        }
        out
    }

    /// Bootstrap a remote target as a task. Task events go to every open session's channel
    /// (the command has no session argument).
    pub fn target_bootstrap(&self, name: &str) -> ServiceResult<TaskId> {
        let global = self.global();
        let cfg = config::find_target(&global, name)
            .map_err(|e| ServiceError::not_found(e.to_string()))?;
        let target = Target::from_config(&cfg).map_err(|e| ServiceError::invalid(e.to_string()))?;
        if !target.is_remote() {
            return Err(ServiceError::invalid("the local target needs no bootstrap"));
        }
        let id = self.tasks.alloc("bootstrap");
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        let logger = TaskLogger::new(id.clone(), tx.clone());
        // Fan out to every open session.
        let sinks: Vec<tokio::sync::broadcast::Sender<SessionEvent>> = self
            .sessions
            .read()
            .map(|s| s.values().map(|r| r.events()).collect())
            .unwrap_or_default();
        let mut rx = tx.subscribe();
        tokio::spawn(async move {
            while let Ok(e) = rx.recv().await {
                for s in &sinks {
                    let _ = s.send(e.clone());
                }
                if matches!(e, SessionEvent::TaskDone { .. }) {
                    break;
                }
            }
        });
        let control_dir = self.control_dir();
        tokio::spawn(async move {
            let r: anyhow::Result<()> = async {
                let wheel = crate::target::bootstrap::find_dev_wheel();
                logger.log(format!("bootstrapping {}", target.name()));
                match crate::target::bootstrap::ensure(
                    &target,
                    control_dir.as_deref(),
                    wheel.as_deref(),
                )
                .await?
                {
                    crate::target::bootstrap::Outcome::UpToDate(v) => {
                        logger.log(format!("tooling {v} already installed"))
                    }
                    crate::target::bootstrap::Outcome::Installed(v) => {
                        logger.log(format!("installed tooling {v}"))
                    }
                }
                Ok(())
            }
            .await;
            logger.finish(r);
        });
        Ok(id)
    }

    // ---- terminals ----

    pub fn terminal_open(&self, session: &str, cols: u16, rows: u16) -> ServiceResult<TerminalId> {
        let rt = self.session(session)?;
        let id = format!("term-{}", crate::util::stamped_id());
        let spec = TerminalSpec {
            repo: rt.repo.clone(),
            target: rt.target(),
            control_dir: self.control_dir(),
            capture: CaptureEnv::new(&rt.repo, &rt.out_dir(), self.mpl_dir()),
            shell: None,
            cols,
            rows,
        };
        let t = Terminal::open(id.clone(), session, &spec, rt.events())?;
        self.terminals.insert(t);
        Ok(id)
    }

    pub fn terminal_write(&self, terminal_id: &str, data: &str) -> ServiceResult<()> {
        self.terminals.get(terminal_id)?.write(data)
    }

    pub fn terminal_resize(&self, terminal_id: &str, cols: u16, rows: u16) -> ServiceResult<()> {
        self.terminals.get(terminal_id)?.resize(cols, rows)
    }

    pub fn terminal_close(&self, terminal_id: &str) -> ServiceResult<()> {
        let t = self.terminals.remove(terminal_id).ok_or_else(|| {
            ServiceError::not_found(format!("terminal `{terminal_id}` is not open"))
        })?;
        t.close();
        Ok(())
    }

    /// Close every session (app exit).
    pub async fn shutdown(&self) {
        let ids = self.open_ids();
        for id in ids {
            let _ = self.close_session(&id).await;
        }
    }
}

#[cfg(test)]
mod tests;
