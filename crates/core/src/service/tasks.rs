//! Long operations (bootstrap, target switch, package changes) return a `TaskId` immediately
//! and report through `task_log` / `task_done` events on the session channel.

use analyst_protocol::ui::{SessionEvent, TaskId};
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast;

/// Allocates task ids.
#[derive(Debug, Default)]
pub struct TaskRegistry {
    next: AtomicU64,
}

impl TaskRegistry {
    pub fn new() -> Self {
        TaskRegistry::default()
    }

    pub fn alloc(&self, kind: &str) -> TaskId {
        let n = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        format!("task-{kind}-{n}-{}", crate::util::short_rand())
    }
}

/// A handle that writes `task_log` lines and the final `task_done` for one task.
#[derive(Clone)]
pub struct TaskLogger {
    pub id: TaskId,
    events: broadcast::Sender<SessionEvent>,
}

impl TaskLogger {
    pub fn new(id: TaskId, events: broadcast::Sender<SessionEvent>) -> Self {
        TaskLogger { id, events }
    }

    pub fn log(&self, line: impl Into<String>) {
        let _ = self.events.send(SessionEvent::TaskLog {
            task_id: self.id.clone(),
            line: line.into(),
        });
    }

    pub fn done(&self, ok: bool, message: Option<String>) {
        let _ = self.events.send(SessionEvent::TaskDone {
            task_id: self.id.clone(),
            ok,
            message,
        });
    }

    /// Finish from a result: `Ok` -> `task_done{ok:true}`, `Err` -> logged and `ok:false`.
    pub fn finish<T>(&self, r: Result<T>) -> Option<T> {
        match r {
            Ok(v) => {
                self.done(true, None);
                Some(v)
            }
            Err(e) => {
                let msg = format!("{e:#}");
                self.log(format!("error: {msg}"));
                self.done(false, Some(msg));
                None
            }
        }
    }

    /// Run a command line, streaming stdout and stderr lines as `task_log`. Errors on a
    /// non-zero exit status.
    pub async fn stream_command(&self, line: &[String], cwd: Option<&Path>) -> Result<()> {
        let (program, args) = line
            .split_first()
            .ok_or_else(|| anyhow!("empty command line"))?;
        self.log(format!("$ {}", line.join(" ")));
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("running `{}`", line.join(" ")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("stdout not captured"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("stderr not captured"))?;
        let out_logger = self.clone();
        let err_logger = self.clone();
        let out_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                out_logger.log(l);
            }
        });
        let err_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                err_logger.log(l);
            }
        });
        let status = child.wait().await?;
        let _ = out_task.await;
        let _ = err_task.await;
        if !status.success() {
            return Err(anyhow!("`{}` failed ({status})", line.join(" ")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_command_logs_lines_and_reports_failure() {
        let (tx, mut rx) = broadcast::channel(64);
        let reg = TaskRegistry::new();
        let id = reg.alloc("test");
        assert!(id.starts_with("task-test-1-"));
        let t = TaskLogger::new(id.clone(), tx);
        let r = t
            .stream_command(
                &[
                    "sh".into(),
                    "-c".into(),
                    "echo one; echo two >&2; exit 3".into(),
                ],
                None,
            )
            .await;
        assert!(r.is_err());
        t.finish(r);
        let mut lines = vec![];
        let mut done = None;
        while let Ok(e) = rx.try_recv() {
            match e {
                SessionEvent::TaskLog { task_id, line } => {
                    assert_eq!(task_id, id);
                    lines.push(line);
                }
                SessionEvent::TaskDone { ok, message, .. } => done = Some((ok, message)),
                _ => {}
            }
        }
        assert!(lines.iter().any(|l| l == "one"));
        assert!(lines.iter().any(|l| l == "two"));
        let (ok, msg) = done.unwrap();
        assert!(!ok);
        assert!(msg.unwrap().contains("failed"));
    }
}
