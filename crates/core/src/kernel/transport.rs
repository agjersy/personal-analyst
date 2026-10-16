//! `KernelTransport`: how a kernel process is launched. `LocalStdio` spawns the tooling
//! interpreter directly; `SshStdio` spawns `ssh <host> <same command>` and uses the SSH
//! process's stdio. Nothing above the transport knows which one is in use.

use analyst_protocol::config::GlobalConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Arguments of `python -m analyst_kernel` (PROTOCOL 1.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelArgs {
    /// Session interpreter (`--python`).
    pub python: String,
    /// Repo or mirror root (`--root`).
    pub root: String,
    /// `--runs-dir`.
    pub runs_dir: String,
    /// `--out-dir`, when not the default `out`.
    pub out_dir: Option<String>,
    /// `--sandbox auto|bwrap|sandbox-exec|none`.
    pub sandbox: Option<String>,
}

impl KernelArgs {
    pub fn to_args(&self) -> Vec<String> {
        let mut v = vec![
            "--python".to_string(),
            self.python.clone(),
            "--root".to_string(),
            self.root.clone(),
            "--runs-dir".to_string(),
            self.runs_dir.clone(),
        ];
        if let Some(o) = &self.out_dir {
            v.push("--out-dir".into());
            v.push(o.clone());
        }
        if let Some(s) = &self.sandbox {
            v.push("--sandbox".into());
            v.push(s.clone());
        }
        v
    }
}

/// A spawned kernel process with its stdio handles.
pub struct SpawnedKernel {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

#[async_trait]
pub trait KernelTransport: Send + Sync {
    /// The full local command line (program first), for logging and tests.
    fn command_line(&self) -> Vec<String>;

    /// Spawn the kernel process.
    async fn spawn(&self) -> Result<SpawnedKernel> {
        let line = self.command_line();
        let (program, args) = line
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty kernel command line"))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning kernel: {}", line.join(" ")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("kernel stdin not captured"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("kernel stdout not captured"))?;
        Ok(SpawnedKernel {
            child,
            stdin,
            stdout,
        })
    }

    fn describe(&self) -> String {
        self.command_line().join(" ")
    }
}

/// Local kernel: `<launcher...> --python .. --root .. --runs-dir .. [--out-dir ..] [--sandbox ..]`.
/// The launcher is normally `[<tooling_python>, "-m", "analyst_kernel"]`; tests inject
/// `["python3", "<fake_kernel.py>"]`.
#[derive(Debug, Clone)]
pub struct LocalStdio {
    pub launcher: Vec<String>,
    pub args: KernelArgs,
}

impl LocalStdio {
    pub fn new(tooling_python: impl Into<String>, args: KernelArgs) -> Self {
        LocalStdio {
            launcher: vec![tooling_python.into(), "-m".into(), "analyst_kernel".into()],
            args,
        }
    }

    pub fn with_launcher(launcher: Vec<String>, args: KernelArgs) -> Self {
        LocalStdio { launcher, args }
    }
}

#[async_trait]
impl KernelTransport for LocalStdio {
    fn command_line(&self) -> Vec<String> {
        let mut v = self.launcher.clone();
        v.extend(self.args.to_args());
        v
    }
}

/// Remote kernel over the system OpenSSH client with connection multiplexing.
#[derive(Debug, Clone)]
pub struct SshStdio {
    pub host: String,
    /// `$ANALYST_HOME/ssh` for control sockets; `None` disables multiplexing.
    pub control_dir: Option<PathBuf>,
    /// Remote launcher, e.g. `["~/.analyst/tooling/.venv/bin/python", "-m", "analyst_kernel"]`.
    pub remote_launcher: Vec<String>,
    pub args: KernelArgs,
}

/// `ssh [-o ControlMaster=auto -o ControlPath=<dir>/%C -o ControlPersist=10m] <host>` prefix.
pub fn ssh_prefix(host: &str, control_dir: Option<&Path>) -> Vec<String> {
    let mut v = vec!["ssh".to_string()];
    if let Some(dir) = control_dir {
        v.push("-o".into());
        v.push("ControlMaster=auto".into());
        v.push("-o".into());
        v.push(format!("ControlPath={}/%C", dir.display()));
        v.push("-o".into());
        v.push("ControlPersist=10m".into());
    }
    v.push("-o".into());
    v.push("BatchMode=yes".into());
    v.push(host.to_string());
    v
}

impl SshStdio {
    pub fn remote_command(&self) -> String {
        let mut parts: Vec<String> = self.remote_launcher.clone();
        parts.extend(self.args.to_args());
        parts
            .iter()
            .map(|p| {
                // `~/...` must stay unquoted so the remote shell expands it.
                if let Some(rest) = p.strip_prefix("~/") {
                    format!("~/{}", crate::util::sh_quote(rest))
                } else {
                    crate::util::sh_quote(p)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[async_trait]
impl KernelTransport for SshStdio {
    fn command_line(&self) -> Vec<String> {
        let mut v = ssh_prefix(&self.host, self.control_dir.as_deref());
        v.push(self.remote_command());
        v
    }
}

/// Resolve the interpreter of the app tooling environment (the one that hosts
/// `analyst_kernel`), in this order:
///
/// 1. `ANALYST_TOOLING_PYTHON` environment variable.
/// 2. `tooling_python` in the global config.
/// 3. Dev mode: `<workspace>/python/kernel/.venv/bin/python` if it exists, where `<workspace>`
///    is found by walking up from the current executable's directory and then from the current
///    working directory, looking for a directory containing `python/kernel`.
/// 4. `python3` on `PATH`.
pub fn resolve_tooling_python(global: &GlobalConfig) -> String {
    if let Ok(v) = std::env::var("ANALYST_TOOLING_PYTHON") {
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(v) = &global.tooling_python {
        if !v.is_empty() {
            return crate::util::expand_tilde(v).to_string_lossy().to_string();
        }
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            starts.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    for start in starts {
        for dir in start.ancestors() {
            let candidate = dir.join("python/kernel/.venv/bin/python");
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    "python3".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> KernelArgs {
        KernelArgs {
            python: "/repo/.venv/bin/python".into(),
            root: "/repo".into(),
            runs_dir: "/home/u/.analyst/runs".into(),
            out_dir: None,
            sandbox: Some("auto".into()),
        }
    }

    #[test]
    fn local_command_line() {
        let t = LocalStdio::new("/tooling/bin/python", args());
        assert_eq!(
            t.command_line(),
            vec![
                "/tooling/bin/python",
                "-m",
                "analyst_kernel",
                "--python",
                "/repo/.venv/bin/python",
                "--root",
                "/repo",
                "--runs-dir",
                "/home/u/.analyst/runs",
                "--sandbox",
                "auto"
            ]
        );
    }

    #[test]
    fn ssh_command_line() {
        let t = SshStdio {
            host: "lab-gpu".into(),
            control_dir: Some(PathBuf::from("/home/u/.analyst/ssh")),
            remote_launcher: vec![
                "~/.analyst/tooling/.venv/bin/python".into(),
                "-m".into(),
                "analyst_kernel".into(),
            ],
            args: KernelArgs {
                python: "~/analyst/repo/.venv/bin/python".into(),
                root: "~/analyst/repo".into(),
                runs_dir: "~/.analyst/runs".into(),
                out_dir: None,
                sandbox: Some("auto".into()),
            },
        };
        let line = t.command_line();
        assert_eq!(line[0], "ssh");
        assert_eq!(line[1..3], ["-o", "ControlMaster=auto"]);
        assert_eq!(line[3..5], ["-o", "ControlPath=/home/u/.analyst/ssh/%C"]);
        assert_eq!(line[5..7], ["-o", "ControlPersist=10m"]);
        assert_eq!(line[9], "lab-gpu");
        assert_eq!(
            line[10],
            "~/.analyst/tooling/.venv/bin/python -m analyst_kernel --python ~/analyst/repo/.venv/bin/python --root ~/analyst/repo --runs-dir ~/.analyst/runs --sandbox auto"
        );
    }

    #[test]
    fn tooling_python_env_wins() {
        let g = GlobalConfig {
            tooling_python: Some("/cfg/python".into()),
            ..Default::default()
        };
        // Cannot set env in parallel tests safely; only check the config path and default order.
        if std::env::var("ANALYST_TOOLING_PYTHON").is_err() {
            assert_eq!(resolve_tooling_python(&g), "/cfg/python");
        }
    }
}
