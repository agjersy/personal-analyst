//! Terminals (ARCHITECTURE 5.15) on `portable-pty`. Local: `$SHELL` (fallback `/bin/sh`) in the
//! repo with the session venv on PATH and the figure-capture environment, so anything run from
//! the terminal produces figures. Remote: `ssh -t <host>` into the mirror with the same setup.
//! A reader thread forwards output as `terminal_data` (lossy UTF-8); exit is `terminal_exit`.

use super::error::{ServiceError, ServiceResult};
use crate::target::Target;
use crate::util::sh_quote;
use analyst_protocol::ui::{SessionEvent, TerminalId};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// The figure-capture environment exported into every terminal (PROTOCOL 1.4 and 1.6).
#[derive(Debug, Clone)]
pub struct CaptureEnv {
    /// Directory containing the `analyst_mpl` package (prepended to `PYTHONPATH`); `None` when
    /// it cannot be located locally.
    pub mpl_dir: Option<PathBuf>,
    /// `ANALYST_OUT_DIR` for this terminal.
    pub out_dir: PathBuf,
    /// `ANALYST_RUN_ID` for this terminal (`terminal-<stamp>`).
    pub run_id: String,
}

impl CaptureEnv {
    /// A fresh capture environment under `<repo>/<out_dir>/terminal/<stamp>`.
    pub fn new(repo: &Path, out_dir: &str, mpl_dir: Option<PathBuf>) -> CaptureEnv {
        let stamp = crate::util::stamped_id();
        CaptureEnv {
            mpl_dir,
            out_dir: repo.join(out_dir).join("terminal").join(&stamp),
            run_id: format!("terminal-{stamp}"),
        }
    }

    /// Environment variables for a local terminal.
    pub fn local_vars(&self, repo: &Path) -> Vec<(String, String)> {
        let mut vars = vec![
            ("MPLBACKEND".to_string(), "module://analyst_mpl".to_string()),
            (
                "ANALYST_OUT_DIR".to_string(),
                self.out_dir.to_string_lossy().to_string(),
            ),
            ("ANALYST_RUN_ID".to_string(), self.run_id.clone()),
            (
                "ANALYST_ROOT".to_string(),
                repo.to_string_lossy().to_string(),
            ),
            ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
        ];
        let mut pythonpath: Vec<String> = Vec::new();
        if let Some(m) = &self.mpl_dir {
            pythonpath.push(m.to_string_lossy().to_string());
        }
        if let Ok(existing) = std::env::var("PYTHONPATH") {
            if !existing.is_empty() {
                pythonpath.push(existing);
            }
        }
        pythonpath.push(repo.to_string_lossy().to_string());
        vars.push(("PYTHONPATH".to_string(), pythonpath.join(":")));
        let venv = repo.join(".venv");
        if venv.is_dir() {
            vars.push((
                "VIRTUAL_ENV".to_string(),
                venv.to_string_lossy().to_string(),
            ));
            let bin = venv.join("bin").to_string_lossy().to_string();
            let path = std::env::var("PATH").unwrap_or_default();
            vars.push(("PATH".to_string(), format!("{bin}:{path}")));
        }
        vars
    }

    /// The remote shell command for `ssh -t`: enter the mirror, export the environment, exec a
    /// login shell.
    pub fn remote_command(&self, mirror: &str) -> String {
        let m = if let Some(rest) = mirror.strip_prefix("~/") {
            format!("~/{}", sh_quote(rest))
        } else {
            sh_quote(mirror)
        };
        let out_rel = self
            .out_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        format!(
            "cd {m} && export VIRTUAL_ENV=\"$PWD/.venv\" PATH=\"$PWD/.venv/bin:$HOME/.analyst/bin:$PATH\" \
MPLBACKEND=module://analyst_mpl ANALYST_ROOT=\"$PWD\" ANALYST_OUT_DIR=\"$PWD/out/terminal/{out_rel}\" \
ANALYST_RUN_ID={run_id} PYTHONUNBUFFERED=1 \
PYTHONPATH=\"$($HOME/.analyst/tooling/.venv/bin/python -c 'import analyst_mpl,os;print(os.path.dirname(os.path.dirname(analyst_mpl.__file__)))' 2>/dev/null):$PWD\"; \
exec \"${{SHELL:-/bin/sh}}\" -l",
            run_id = sh_quote(&self.run_id),
            out_rel = sh_quote(&out_rel),
        )
    }
}

/// Locate the directory containing `analyst_mpl` for the local target: ask the tooling
/// interpreter, else look for `python/kernel/analyst_mpl` in a development checkout.
pub fn find_mpl_dir(tooling_python: &str) -> Option<PathBuf> {
    let out = std::process::Command::new(tooling_python)
        .args([
            "-c",
            "import analyst_mpl, os; print(os.path.dirname(os.path.dirname(analyst_mpl.__file__)))",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() && Path::new(&s).join("analyst_mpl").is_dir() {
            return Some(PathBuf::from(s));
        }
    }
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            starts.push(p.to_path_buf());
        }
    }
    for s in starts {
        for dir in s.ancestors() {
            let c = dir.join("python/kernel");
            if c.join("analyst_mpl/__init__.py").is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// How to open a terminal.
#[derive(Debug, Clone)]
pub struct TerminalSpec {
    pub repo: PathBuf,
    pub target: Target,
    /// SSH control directory for multiplexing (remote targets).
    pub control_dir: Option<PathBuf>,
    pub capture: CaptureEnv,
    /// Shell for local terminals; `None` = `$SHELL` or `/bin/sh`.
    pub shell: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

/// One open terminal.
pub struct Terminal {
    pub id: TerminalId,
    pub session: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pub out_dir: PathBuf,
    pub run_id: String,
}

impl Terminal {
    /// Spawn the pty and its reader thread. Output and exit go to `events`.
    pub fn open(
        id: TerminalId,
        session: &str,
        spec: &TerminalSpec,
        events: broadcast::Sender<SessionEvent>,
    ) -> ServiceResult<Terminal> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: spec.rows.max(1),
                cols: spec.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| ServiceError::internal(format!("opening pty: {e}")))?;
        let mut cmd = match &spec.target {
            Target::Local => {
                let shell = spec
                    .shell
                    .clone()
                    .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
                    .unwrap_or_else(|| "/bin/sh".to_string());
                let mut c = CommandBuilder::new(&shell);
                if shell.ends_with("bash") || shell.ends_with("zsh") || shell.ends_with("fish") {
                    c.arg("-l");
                }
                c.cwd(&spec.repo);
                for (k, v) in spec.capture.local_vars(&spec.repo) {
                    c.env(k, v);
                }
                c
            }
            Target::Ssh { host, .. } => {
                let mirror = spec.target.mirror_dir(&spec.repo).unwrap_or_default();
                let mut line =
                    crate::kernel::transport::ssh_prefix(host, spec.control_dir.as_deref());
                // `-t` forces a tty; BatchMode stays so a missing key fails fast.
                line.insert(1, "-t".to_string());
                line.push(spec.capture.remote_command(&mirror));
                let mut c = CommandBuilder::new(&line[0]);
                c.args(&line[1..]);
                c.cwd(&spec.repo);
                c
            }
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| ServiceError::internal(format!("spawning shell: {e}")))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| ServiceError::internal(format!("pty reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| ServiceError::internal(format!("pty writer: {e}")))?;
        let killer = child.clone_killer();
        let tid = id.clone();
        std::thread::Builder::new()
            .name(format!("pty-{id}"))
            .spawn(move || reader_loop(tid, reader, child, events))
            .map_err(|e| ServiceError::internal(format!("pty thread: {e}")))?;
        Ok(Terminal {
            id,
            session: session.to_string(),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            out_dir: spec.capture.out_dir.clone(),
            run_id: spec.capture.run_id.clone(),
        })
    }

    pub fn write(&self, data: &str) -> ServiceResult<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| ServiceError::internal("terminal writer poisoned"))?;
        w.write_all(data.as_bytes())
            .and_then(|_| w.flush())
            .map_err(|e| ServiceError::io(format!("terminal write: {e}")))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> ServiceResult<()> {
        let m = self
            .master
            .lock()
            .map_err(|_| ServiceError::internal("terminal master poisoned"))?;
        m.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| ServiceError::io(format!("terminal resize: {e}")))
    }

    /// Kill the shell; the reader thread then reports `terminal_exit`.
    pub fn close(&self) {
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.close();
    }
}

fn reader_loop(
    id: TerminalId,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    events: broadcast::Sender<SessionEvent>,
) {
    let mut buf = [0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                carry.extend_from_slice(&buf[..n]);
                let (text, rest) = decode_prefix(&carry);
                if !text.is_empty() {
                    let _ = events.send(SessionEvent::TerminalData {
                        terminal_id: id.clone(),
                        data: text,
                    });
                }
                carry = rest;
            }
        }
    }
    if !carry.is_empty() {
        let _ = events.send(SessionEvent::TerminalData {
            terminal_id: id.clone(),
            data: String::from_utf8_lossy(&carry).to_string(),
        });
    }
    let code = child.wait().ok().map(|st| {
        if st.success() {
            0
        } else {
            st.exit_code() as i64
        }
    });
    let _ = events.send(SessionEvent::TerminalExit {
        terminal_id: id,
        code,
    });
}

/// Decode as much of `bytes` as is valid UTF-8, keeping an incomplete trailing sequence (at
/// most 3 bytes) for the next read; invalid sequences are replaced.
fn decode_prefix(bytes: &[u8]) -> (String, Vec<u8>) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), vec![]),
        Err(e) => {
            let valid = e.valid_up_to();
            match e.error_len() {
                // Incomplete sequence at the end: keep it for the next chunk.
                None => (
                    String::from_utf8_lossy(&bytes[..valid]).to_string(),
                    bytes[valid..].to_vec(),
                ),
                Some(_) => (String::from_utf8_lossy(bytes).to_string(), vec![]),
            }
        }
    }
}

/// All terminals of the app, keyed by id.
#[derive(Default)]
pub struct TerminalManager {
    terminals: Mutex<HashMap<TerminalId, Arc<Terminal>>>,
}

impl TerminalManager {
    pub fn new() -> Self {
        TerminalManager::default()
    }

    pub fn insert(&self, t: Terminal) -> Arc<Terminal> {
        let t = Arc::new(t);
        if let Ok(mut m) = self.terminals.lock() {
            m.insert(t.id.clone(), t.clone());
        }
        t
    }

    pub fn get(&self, id: &str) -> ServiceResult<Arc<Terminal>> {
        self.terminals
            .lock()
            .ok()
            .and_then(|m| m.get(id).cloned())
            .ok_or_else(|| ServiceError::not_found(format!("terminal `{id}` is not open")))
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Terminal>> {
        self.terminals.lock().ok().and_then(|mut m| m.remove(id))
    }

    /// Close every terminal of a session.
    pub fn close_session(&self, session: &str) {
        let Ok(mut m) = self.terminals.lock() else {
            return;
        };
        let ids: Vec<String> = m
            .values()
            .filter(|t| t.session == session)
            .map(|t| t.id.clone())
            .collect();
        for id in ids {
            if let Some(t) = m.remove(&id) {
                t.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn utf8_carry_over() {
        let s = "héllo";
        let b = s.as_bytes();
        let (t, rest) = decode_prefix(&b[..2]);
        assert_eq!(t, "h");
        assert_eq!(rest, vec![b[1]]);
        let (t, rest) = decode_prefix(&[0xff, b'a']);
        assert_eq!(t, "\u{fffd}a");
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn local_terminal_echoes_a_command() {
        let repo = tempfile::tempdir().unwrap();
        let (tx, mut rx) = broadcast::channel(256);
        let spec = TerminalSpec {
            repo: repo.path().to_path_buf(),
            target: Target::Local,
            control_dir: None,
            capture: CaptureEnv::new(repo.path(), "out", None),
            shell: Some("/bin/sh".into()),
            cols: 80,
            rows: 24,
        };
        let t = Terminal::open("t1".into(), "s1", &spec, tx).unwrap();
        t.write("echo hello; echo $ANALYST_RUN_ID; echo $MPLBACKEND\n")
            .unwrap();
        t.resize(100, 30).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        while Instant::now() < deadline && !seen.contains("module://analyst_mpl") {
            match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
                Ok(Ok(SessionEvent::TerminalData { terminal_id, data })) => {
                    assert_eq!(terminal_id, "t1");
                    seen.push_str(&data);
                }
                Ok(Ok(_)) => {}
                _ => {}
            }
        }
        assert!(seen.contains("hello"), "{seen:?}");
        assert!(seen.contains("terminal-"), "{seen:?}");
        t.write("exit 3\n").unwrap();
        let mut code = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && code.is_none() {
            if let Ok(Ok(SessionEvent::TerminalExit { code: c, .. })) =
                tokio::time::timeout(Duration::from_secs(1), rx.recv()).await
            {
                code = Some(c);
            }
        }
        assert_eq!(code, Some(Some(3)));
    }
}
