//! `KernelClient`: request ids, a pending-response map, a notification channel, typed helpers
//! for every method, and reconnect (`run.attach` after respawn).

use super::transport::{KernelTransport, SpawnedKernel};
use analyst_protocol::kernel::*;
use analyst_protocol::rpc::{Incoming, Request, RequestId, RpcError};
use analyst_protocol::PROTOCOL_VERSION;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{broadcast, oneshot, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("{0}")]
    Rpc(#[from] RpcError),
    #[error("kernel I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("kernel is not connected")]
    Disconnected,
    #[error("kernel exited while a request was pending")]
    Exited,
    #[error("invalid kernel response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

/// Byte offsets already consumed for a run, so `run.attach` resumes where streaming stopped.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOffsets {
    pub stdout: u64,
    pub stderr: u64,
}

/// Pending requests keyed by id, tagged with the connection generation that sent them so a
/// reader task that ends for an old connection never fails requests of a newer one.
type Pending = HashMap<RequestId, (u64, oneshot::Sender<Result<Value, RpcError>>)>;

struct Inner {
    transport: Box<dyn KernelTransport>,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    /// Incremented on every (re)spawn.
    generation: AtomicI64,
    notifications: broadcast::Sender<KernelNotification>,
    /// Runs started through this client that have not reported `run.done`.
    active_runs: Mutex<HashMap<String, RunOffsets>>,
    hello: Mutex<Option<HelloResult>>,
}

#[derive(Clone)]
pub struct KernelClient {
    inner: Arc<Inner>,
}

impl KernelClient {
    /// Spawn the kernel through a transport and complete `kernel.hello`.
    pub async fn connect(
        transport: Box<dyn KernelTransport>,
        core_version: &str,
    ) -> Result<KernelClient, KernelError> {
        let (tx, _) = broadcast::channel(4096);
        let inner = Arc::new(Inner {
            transport,
            stdin: Mutex::new(None),
            child: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            generation: AtomicI64::new(0),
            notifications: tx,
            active_runs: Mutex::new(HashMap::new()),
            hello: Mutex::new(None),
        });
        let client = KernelClient { inner };
        client.spawn_and_hello(core_version).await?;
        Ok(client)
    }

    async fn spawn_and_hello(&self, core_version: &str) -> Result<HelloResult, KernelError> {
        let spawned = self
            .inner
            .transport
            .spawn()
            .await
            .map_err(|e| KernelError::Other(e.to_string()))?;
        let SpawnedKernel {
            child,
            stdin,
            stdout,
        } = spawned;
        *self.inner.stdin.lock().await = Some(stdin);
        *self.inner.child.lock().await = Some(child);
        let generation = (self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1) as u64;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => inner.dispatch_line(&line).await,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("kernel stdout read error: {e}");
                        break;
                    }
                }
            }
            // EOF: fail every pending request that belongs to this connection.
            let mut pending = inner.pending.lock().await;
            let stale: Vec<RequestId> = pending
                .iter()
                .filter(|(_, (g, _))| *g == generation)
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                if let Some((_, tx)) = pending.remove(&id) {
                    let _ = tx.send(Err(RpcError {
                        code: analyst_protocol::rpc::codes::INTERNAL_ERROR,
                        message: "kernel exited".into(),
                        data: None,
                    }));
                }
            }
            drop(pending);
            if inner.generation.load(Ordering::SeqCst) as u64 == generation {
                *inner.stdin.lock().await = None;
            }
        });
        let hello: HelloResult = self
            .call(
                "kernel.hello",
                HelloParams {
                    core_version: core_version.to_string(),
                    protocol_version: PROTOCOL_VERSION,
                },
            )
            .await?;
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(KernelError::Other(format!(
                "kernel speaks protocol {} but core expects {PROTOCOL_VERSION}",
                hello.protocol_version
            )));
        }
        *self.inner.hello.lock().await = Some(hello.clone());
        Ok(hello)
    }

    /// The `kernel.hello` result of the current connection.
    pub async fn hello(&self) -> Option<HelloResult> {
        self.inner.hello.lock().await.clone()
    }

    pub fn describe(&self) -> String {
        self.inner.transport.describe()
    }

    /// Subscribe to kernel notifications. Each receiver sees every notification sent after
    /// subscribing (`run.stdout`, `run.stderr`, `run.artifact`, `run.done`).
    pub fn subscribe(&self) -> broadcast::Receiver<KernelNotification> {
        self.inner.notifications.subscribe()
    }

    /// Whether the kernel process is still attached (stdin open).
    pub async fn is_connected(&self) -> bool {
        self.inner.stdin.lock().await.is_some()
    }

    /// Send a raw JSON-RPC line (used by the `analyst kernel` passthrough).
    pub async fn send_raw(&self, line: &str) -> Result<(), KernelError> {
        let mut guard = self.inner.stdin.lock().await;
        let stdin = guard.as_mut().ok_or(KernelError::Disconnected)?;
        stdin.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            stdin.write_all(b"\n").await?;
        }
        stdin.flush().await?;
        Ok(())
    }

    /// Call a method and decode its result.
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, KernelError> {
        let v = self
            .call_value(method, serde_json::to_value(params)?)
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    /// Call a method with raw JSON params and return the raw result.
    pub async fn call_value(&self, method: &str, params: Value) -> Result<Value, KernelError> {
        let id = RequestId::Number(self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        let generation = self.inner.generation.load(Ordering::SeqCst) as u64;
        self.inner
            .pending
            .lock()
            .await
            .insert(id.clone(), (generation, tx));
        let req = Request::new(id.clone(), method, params);
        let line = serde_json::to_string(&req)?;
        if let Err(e) = self.send_raw(&line).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(e);
        }
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(KernelError::Rpc(e)),
            Err(_) => Err(KernelError::Exited),
        }
    }

    // ---- typed helpers ----

    pub async fn shutdown(&self) -> Result<(), KernelError> {
        let _: OkResult = self.call("kernel.shutdown", serde_json::json!({})).await?;
        *self.inner.stdin.lock().await = None;
        if let Some(mut child) = self.inner.child.lock().await.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
        }
        Ok(())
    }

    pub async fn env_info(&self) -> Result<EnvInfo, KernelError> {
        self.call("env.info", serde_json::json!({})).await
    }

    pub async fn env_packages(&self) -> Result<PackagesResult, KernelError> {
        self.call("env.packages", serde_json::json!({})).await
    }

    pub async fn scripts_index(
        &self,
        globs: Option<Vec<String>>,
    ) -> Result<ScriptsIndexResult, KernelError> {
        self.call("scripts.index", ScriptsIndexParams { globs })
            .await
    }

    pub async fn lib_index(&self, package: Option<String>) -> Result<LibIndexResult, KernelError> {
        self.call("lib.index", LibIndexParams { package }).await
    }

    pub async fn run_start(&self, params: RunStartParams) -> Result<RunStartResult, KernelError> {
        let r: RunStartResult = self.call("run.start", params).await?;
        self.inner
            .active_runs
            .lock()
            .await
            .insert(r.run_id.clone(), RunOffsets::default());
        Ok(r)
    }

    pub async fn run_attach(
        &self,
        params: RunAttachParams,
    ) -> Result<RunAttachResult, KernelError> {
        let run_id = params.run_id.clone();
        let r: RunAttachResult = self.call("run.attach", params).await?;
        if !r.status.is_finished() {
            self.inner
                .active_runs
                .lock()
                .await
                .entry(run_id)
                .or_default();
        }
        Ok(r)
    }

    pub async fn run_status(&self, run_id: &str) -> Result<RunMeta, KernelError> {
        self.call(
            "run.status",
            RunIdParams {
                run_id: run_id.to_string(),
            },
        )
        .await
    }

    pub async fn run_cancel(&self, run_id: &str) -> Result<OkResult, KernelError> {
        self.call(
            "run.cancel",
            RunIdParams {
                run_id: run_id.to_string(),
            },
        )
        .await
    }

    pub async fn runs_list(&self, limit: Option<u64>) -> Result<RunsListResult, KernelError> {
        self.call("runs.list", RunsListParams { limit }).await
    }

    pub async fn exec_snippet(
        &self,
        code: &str,
        timeout_s: Option<f64>,
    ) -> Result<ExecSnippetResult, KernelError> {
        self.call(
            "exec.snippet",
            ExecSnippetParams {
                code: code.to_string(),
                timeout_s,
            },
        )
        .await
    }

    pub async fn out_list(&self, params: OutListParams) -> Result<OutListResult, KernelError> {
        self.call("out.list", params).await
    }

    pub async fn out_read(&self, params: OutReadParams) -> Result<OutReadResult, KernelError> {
        self.call("out.read", params).await
    }

    pub async fn eq_extract(
        &self,
        params: EqExtractParams,
    ) -> Result<EqExtractResult, KernelError> {
        self.call("eq.extract", params).await
    }

    pub async fn eq_check(&self, params: EqCheckParams) -> Result<EqCheckResult, KernelError> {
        self.call("eq.check", params).await
    }

    pub async fn contract_check(
        &self,
        paths: Option<Vec<String>>,
    ) -> Result<DiagnosticsResult, KernelError> {
        self.call("contract.check", PathsParams { paths }).await
    }

    pub async fn contract_similar(
        &self,
        threshold: Option<f64>,
    ) -> Result<SimilarResult, KernelError> {
        self.call("contract.similar", SimilarParams { threshold })
            .await
    }

    pub async fn lint_run(
        &self,
        paths: Option<Vec<String>>,
    ) -> Result<DiagnosticsResult, KernelError> {
        self.call("lint.run", PathsParams { paths }).await
    }

    /// Run ids started through this client that have not finished.
    pub async fn active_runs(&self) -> Vec<(String, RunOffsets)> {
        self.inner
            .active_runs
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Respawn the kernel after a dropped connection or exit, then `run.attach` every active
    /// run from its recorded offsets so streaming resumes. Returns the new hello.
    pub async fn reconnect(&self, core_version: &str) -> Result<HelloResult, KernelError> {
        if let Some(mut child) = self.inner.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        *self.inner.stdin.lock().await = None;
        let pending: Vec<_> = self.inner.pending.lock().await.drain().collect();
        for (_, (_, tx)) in pending {
            let _ = tx.send(Err(RpcError {
                code: analyst_protocol::rpc::codes::INTERNAL_ERROR,
                message: "kernel reconnecting".into(),
                data: None,
            }));
        }
        let hello = self.spawn_and_hello(core_version).await?;
        let runs = self.active_runs().await;
        for (run_id, off) in runs {
            match self
                .run_attach(RunAttachParams {
                    run_id: run_id.clone(),
                    stdout_offset: Some(off.stdout),
                    stderr_offset: Some(off.stderr),
                })
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("run.attach {run_id} after reconnect failed: {e}");
                    self.inner.active_runs.lock().await.remove(&run_id);
                }
            }
        }
        Ok(hello)
    }
}

impl Inner {
    async fn dispatch_line(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let incoming: Incoming = match serde_json::from_str(line) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(
                    "unparseable kernel line ({e}): {}",
                    crate::util::truncate_bytes(line, 200)
                );
                return;
            }
        };
        match incoming {
            Incoming::Response(resp) => {
                let tx = self.pending.lock().await.remove(&resp.id);
                match tx {
                    Some((_, tx)) => {
                        let r = match (resp.result, resp.error) {
                            (_, Some(e)) => Err(e),
                            (Some(v), None) => Ok(v),
                            (None, None) => Ok(Value::Null),
                        };
                        let _ = tx.send(r);
                    }
                    None => tracing::warn!("kernel response for unknown id {}", resp.id),
                }
            }
            Incoming::Notification(n) => {
                let params = n.params.unwrap_or(Value::Null);
                match KernelNotification::parse(&n.method, params) {
                    Ok(kn) => {
                        self.track(&kn).await;
                        let _ = self.notifications.send(kn);
                    }
                    Err(e) => tracing::warn!("bad notification {}: {e}", n.method),
                }
            }
        }
    }

    async fn track(&self, n: &KernelNotification) {
        let mut runs = self.active_runs.lock().await;
        match n {
            KernelNotification::Stdout(o) => {
                if let Some(r) = runs.get_mut(&o.run_id) {
                    r.stdout = r.stdout.max(o.offset + o.chunk.len() as u64);
                }
            }
            KernelNotification::Stderr(o) => {
                if let Some(r) = runs.get_mut(&o.run_id) {
                    r.stderr = r.stderr.max(o.offset + o.chunk.len() as u64);
                }
            }
            KernelNotification::Done(d) => {
                runs.remove(&d.run_id);
            }
            _ => {}
        }
    }
}
