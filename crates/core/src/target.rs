//! Execution targets (ARCHITECTURE 5.5, PROTOCOL 7): the `Target` enum, remote bootstrap,
//! and repo sync (file list, tree hash, rsync or tar fallback, artifact pull).

use crate::util::sh_quote;
use analyst_protocol::config::{TargetConfig, TargetKind};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local,
    Ssh {
        name: String,
        host: String,
        remote_root: String,
        python: String,
        sandbox: String,
        data_dirs: Vec<String>,
    },
}

impl Target {
    pub fn from_config(cfg: &TargetConfig) -> Result<Target> {
        match cfg.kind {
            TargetKind::Local => Ok(Target::Local),
            TargetKind::Ssh => Ok(Target::Ssh {
                name: cfg.name.clone(),
                host: cfg
                    .host
                    .clone()
                    .ok_or_else(|| anyhow!("target `{}` has kind ssh but no host", cfg.name))?,
                remote_root: cfg
                    .remote_root
                    .clone()
                    .unwrap_or_else(|| "~/analyst".to_string()),
                python: cfg.python.clone().unwrap_or_else(|| "3.12".to_string()),
                sandbox: cfg.sandbox.clone().unwrap_or_else(|| "auto".to_string()),
                data_dirs: cfg.data_dirs.clone(),
            }),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Target::Local => "local",
            Target::Ssh { name, .. } => name,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Target::Ssh { .. })
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            Target::Local => None,
            Target::Ssh { host, .. } => Some(host),
        }
    }

    pub fn data_dirs(&self) -> &[String] {
        match self {
            Target::Local => &[],
            Target::Ssh { data_dirs, .. } => data_dirs,
        }
    }

    pub fn sandbox(&self) -> &str {
        match self {
            Target::Local => "auto",
            Target::Ssh { sandbox, .. } => sandbox,
        }
    }

    /// Mirror directory on the host: `<remote_root>/<repo basename>`.
    pub fn mirror_dir(&self, repo: &Path) -> Option<String> {
        match self {
            Target::Local => None,
            Target::Ssh { remote_root, .. } => {
                let base = repo
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "repo".to_string());
                Some(format!("{}/{}", remote_root.trim_end_matches('/'), base))
            }
        }
    }
}

/// Run a command line, capturing output; errors on non-zero exit.
pub async fn run_command(line: &[String], cwd: Option<&Path>) -> Result<std::process::Output> {
    let (program, args) = line
        .split_first()
        .ok_or_else(|| anyhow!("empty command line"))?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).stdin(Stdio::null());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let out = cmd
        .output()
        .await
        .with_context(|| format!("running `{}`", line.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "`{}` failed ({}): {}",
            line.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out)
}

/// Pipe `producer`'s stdout into `consumer`'s stdin (both run with `cwd`). Used for the tar
/// push and pull fallbacks.
pub async fn pipe_commands(
    producer: &[String],
    consumer: &[String],
    cwd: Option<&Path>,
) -> Result<()> {
    let (p0, pargs) = producer
        .split_first()
        .ok_or_else(|| anyhow!("empty producer command"))?;
    let (c0, cargs) = consumer
        .split_first()
        .ok_or_else(|| anyhow!("empty consumer command"))?;
    let mut prod = tokio::process::Command::new(p0);
    prod.args(pargs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        prod.current_dir(d);
    }
    let mut prod = prod
        .spawn()
        .with_context(|| format!("spawning `{}`", producer.join(" ")))?;
    let stdout = prod
        .stdout
        .take()
        .ok_or_else(|| anyhow!("producer stdout not captured"))?;
    let mut cons = tokio::process::Command::new(c0);
    cons.args(cargs)
        .stdin(Stdio::from(stdout.into_owned_fd()?))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        cons.current_dir(d);
    }
    let cons = cons
        .spawn()
        .with_context(|| format!("spawning `{}`", consumer.join(" ")))?;
    let cons_out = cons.wait_with_output().await?;
    let prod_out = prod.wait_with_output().await?;
    if !prod_out.status.success() {
        return Err(anyhow!(
            "`{}` failed: {}",
            producer.join(" "),
            String::from_utf8_lossy(&prod_out.stderr).trim()
        ));
    }
    if !cons_out.status.success() {
        return Err(anyhow!(
            "`{}` failed: {}",
            consumer.join(" "),
            String::from_utf8_lossy(&cons_out.stderr).trim()
        ));
    }
    Ok(())
}

/// `ssh <opts> <host> <remote command>` as a command line.
pub fn ssh_command(host: &str, control_dir: Option<&Path>, remote: &str) -> Vec<String> {
    let mut v = crate::kernel::transport::ssh_prefix(host, control_dir);
    v.push(remote.to_string());
    v
}

/// Run a remote shell command over ssh, returning stdout.
pub async fn run_ssh(host: &str, control_dir: Option<&Path>, remote: &str) -> Result<String> {
    let out = run_command(&ssh_command(host, control_dir, remote), None).await?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub mod sync {
    //! Repo sync: local canonical, remote mirror (PROTOCOL 7).

    use super::*;
    use sha2::{Digest, Sha256};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FileEntry {
        pub path: String,
        pub blob: String,
    }

    /// `git ls-files --cached --others --exclude-standard -z` plus blob shas from
    /// `git hash-object --stdin-paths`.
    pub fn file_list(repo: &Path) -> Result<Vec<FileEntry>> {
        let out = std::process::Command::new("git")
            .args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ])
            .current_dir(repo)
            .output()
            .context("running git ls-files")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git ls-files failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let mut paths: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).to_string())
            .filter(|p| repo.join(p).is_file())
            .collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Ok(vec![]);
        }
        use std::io::Write;
        let mut child = std::process::Command::new("git")
            .args(["hash-object", "--stdin-paths"])
            .current_dir(repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("running git hash-object")?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("git hash-object stdin not captured"))?;
            for p in &paths {
                writeln!(stdin, "{p}")?;
            }
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(anyhow!(
                "git hash-object failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let shas: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .collect();
        if shas.len() != paths.len() {
            return Err(anyhow!(
                "git hash-object returned {} hashes for {} paths",
                shas.len(),
                paths.len()
            ));
        }
        Ok(paths
            .into_iter()
            .zip(shas)
            .map(|(path, blob)| FileEntry { path, blob })
            .collect())
    }

    /// SHA-256 over sorted `(path, blob sha)` pairs, as `"<path>\0<sha>\n"` records.
    pub fn tree_hash(entries: &[FileEntry]) -> String {
        let mut sorted: Vec<&FileEntry> = entries.iter().collect();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        let mut h = Sha256::new();
        for e in sorted {
            h.update(e.path.as_bytes());
            h.update([0u8]);
            h.update(e.blob.as_bytes());
            h.update(b"\n");
        }
        hex::encode(h.finalize())
    }

    /// Write a NUL-separated file list (consumed by `rsync --from0` and `tar --null -T`).
    pub fn write_list_file(entries: &[FileEntry], path: &Path) -> Result<()> {
        let mut buf = Vec::new();
        for e in entries {
            buf.extend_from_slice(e.path.as_bytes());
            buf.push(0);
        }
        std::fs::write(path, buf).with_context(|| format!("writing {}", path.display()))
    }

    /// `rsync -az --delete --from0 --files-from=<list> ./ <host>:<mirror>/`
    pub fn rsync_command(
        host: &str,
        mirror: &str,
        list_file: &Path,
        control_dir: Option<&Path>,
    ) -> Vec<String> {
        let mut v = vec![
            "rsync".to_string(),
            "-az".to_string(),
            "--delete".to_string(),
            "--from0".to_string(),
            format!("--files-from={}", list_file.display()),
        ];
        if let Some(dir) = control_dir {
            v.push("-e".into());
            v.push(format!(
                "ssh -o ControlMaster=auto -o ControlPath={}/%C -o ControlPersist=10m -o BatchMode=yes",
                dir.display()
            ));
        }
        v.push("./".into());
        v.push(format!("{host}:{}/", mirror.trim_end_matches('/')));
        v
    }

    /// Local half of the tar fallback: `tar -cf - --null -T <list>` (run in the repo).
    pub fn tar_create_command(list_file: &Path) -> Vec<String> {
        vec![
            "tar".into(),
            "-cf".into(),
            "-".into(),
            "--null".into(),
            "-T".into(),
            list_file.display().to_string(),
        ]
    }

    /// Remote half of the tar fallback: `mkdir -p <mirror> && tar -xf - -C <mirror>`.
    pub fn tar_extract_remote(mirror: &str) -> String {
        let m = quote_remote_path(mirror);
        format!("mkdir -p {m} && tar -xf - -C {m}")
    }

    /// `ssh <host> '<tar_extract_remote>'` for the tar fallback consumer.
    pub fn tar_push_ssh_command(
        host: &str,
        mirror: &str,
        control_dir: Option<&Path>,
    ) -> Vec<String> {
        ssh_command(host, control_dir, &tar_extract_remote(mirror))
    }

    /// `scp -q <host>:<mirror>/<path> <local>`
    pub fn scp_pull_command(
        host: &str,
        mirror: &str,
        path: &str,
        local: &Path,
        control_dir: Option<&Path>,
    ) -> Vec<String> {
        let mut v = vec!["scp".to_string(), "-q".to_string()];
        if let Some(dir) = control_dir {
            v.push("-o".into());
            v.push(format!("ControlPath={}/%C", dir.display()));
        }
        v.push(format!("{host}:{}/{path}", mirror.trim_end_matches('/')));
        v.push(local.display().to_string());
        v
    }

    /// Producer for a whole-run pull: `ssh <host> 'tar -cf - -C <mirror> <run out dir>'`.
    pub fn tar_pull_ssh_command(
        host: &str,
        mirror: &str,
        run_out_dir: &str,
        control_dir: Option<&Path>,
    ) -> Vec<String> {
        let remote = format!(
            "tar -cf - -C {} {}",
            quote_remote_path(mirror),
            sh_quote(run_out_dir)
        );
        ssh_command(host, control_dir, &remote)
    }

    /// Consumer for a whole-run pull: `tar -xf - -C <local repo>`.
    pub fn tar_extract_local_command(local_root: &Path) -> Vec<String> {
        vec![
            "tar".into(),
            "-xf".into(),
            "-".into(),
            "-C".into(),
            local_root.display().to_string(),
        ]
    }

    fn quote_remote_path(p: &str) -> String {
        if let Some(rest) = p.strip_prefix("~/") {
            format!("~/{}", sh_quote(rest))
        } else {
            sh_quote(p)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Method {
        Rsync,
        Tar,
        Skipped,
    }

    #[derive(Debug, Clone)]
    pub struct SyncReport {
        pub tree_hash: String,
        pub files: usize,
        pub method: Method,
        pub warning: Option<String>,
    }

    /// Whether `rsync` is on the local PATH.
    pub fn local_has_rsync() -> bool {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join("rsync").is_file()))
            .unwrap_or(false)
    }

    /// Push the local repo to the mirror. Skips when `last_hash` equals the current tree hash.
    pub async fn push(
        repo: &Path,
        target: &Target,
        control_dir: Option<&Path>,
        last_hash: Option<&str>,
        force: bool,
    ) -> Result<SyncReport> {
        let (host, mirror) = match target {
            Target::Ssh { host, .. } => (
                host.as_str(),
                target
                    .mirror_dir(repo)
                    .ok_or_else(|| anyhow!("no mirror dir"))?,
            ),
            Target::Local => return Err(anyhow!("the local target has no mirror to sync")),
        };
        let entries = file_list(repo)?;
        let hash = tree_hash(&entries);
        if !force && last_hash == Some(hash.as_str()) {
            return Ok(SyncReport {
                tree_hash: hash,
                files: entries.len(),
                method: Method::Skipped,
                warning: None,
            });
        }
        let list = tempfile_path(repo, "sync-list")?;
        write_list_file(&entries, &list)?;
        let remote_has_rsync = run_ssh(
            host,
            control_dir,
            "command -v rsync >/dev/null 2>&1 && echo yes || echo no",
        )
        .await
        .map(|s| s.trim() == "yes")
        .unwrap_or(false);
        let result = if local_has_rsync() && remote_has_rsync {
            run_ssh(
                host,
                control_dir,
                &format!("mkdir -p {}", quote_remote_path(&mirror)),
            )
            .await?;
            run_command(
                &rsync_command(host, &mirror, &list, control_dir),
                Some(repo),
            )
            .await
            .map(|_| SyncReport {
                tree_hash: hash.clone(),
                files: entries.len(),
                method: Method::Rsync,
                warning: None,
            })
        } else {
            pipe_commands(
                &tar_create_command(&list),
                &tar_push_ssh_command(host, &mirror, control_dir),
                Some(repo),
            )
            .await
            .map(|_| SyncReport {
                tree_hash: hash.clone(),
                files: entries.len(),
                method: Method::Tar,
                warning: Some(
                    "rsync is unavailable; used a tar pipe (files deleted locally are not removed from the mirror)"
                        .to_string(),
                ),
            })
        };
        let _ = std::fs::remove_file(&list);
        result
    }

    /// Pull one rendered artifact from the mirror into `local` (parent dirs created).
    pub async fn pull_artifact(
        host: &str,
        mirror: &str,
        path: &str,
        local: &Path,
        control_dir: Option<&Path>,
    ) -> Result<()> {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)?;
        }
        run_command(
            &scp_pull_command(host, mirror, path, local, control_dir),
            None,
        )
        .await?;
        Ok(())
    }

    /// Pull a whole run output directory with a single tar pipe into the local repo.
    pub async fn pull_run(
        host: &str,
        mirror: &str,
        run_out_dir: &str,
        local_root: &Path,
        control_dir: Option<&Path>,
    ) -> Result<()> {
        std::fs::create_dir_all(local_root)?;
        pipe_commands(
            &tar_pull_ssh_command(host, mirror, run_out_dir, control_dir),
            &tar_extract_local_command(local_root),
            None,
        )
        .await
    }

    fn tempfile_path(repo: &Path, stem: &str) -> Result<PathBuf> {
        let dir = repo.join(".analyst");
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join(format!(".{stem}-{}", crate::util::short_rand())))
    }
}

pub mod bootstrap {
    //! Remote bootstrap sequence (PROTOCOL 7), idempotent and version-checked.

    use super::*;

    pub const UV_INSTALLER: &str = "https://astral.sh/uv/install.sh";

    /// Version written to `~/.analyst/tooling/VERSION`: the core version.
    pub fn tooling_version() -> &'static str {
        crate::CORE_VERSION
    }

    /// Remote command that prints the installed tooling version (empty when absent).
    pub fn version_check_command() -> String {
        "cat ~/.analyst/tooling/VERSION 2>/dev/null || true".to_string()
    }

    /// The remote command sequence, run after the kernel wheel has been copied to
    /// `wheel_remote` (a path on the host).
    pub fn commands(python_version: &str, wheel_remote: &str, version: &str) -> Vec<String> {
        vec![
            "mkdir -p ~/.analyst/bin ~/.analyst/tooling".to_string(),
            format!(
                "test -x ~/.analyst/bin/uv || (curl -LsSf {UV_INSTALLER} | UV_INSTALL_DIR=~/.analyst/bin UV_NO_MODIFY_PATH=1 sh)"
            ),
            format!("~/.analyst/bin/uv python install {}", sh_quote(python_version)),
            format!(
                "test -x ~/.analyst/tooling/.venv/bin/python || ~/.analyst/bin/uv venv --python {} ~/.analyst/tooling/.venv",
                sh_quote(python_version)
            ),
            format!(
                "~/.analyst/bin/uv pip install --python ~/.analyst/tooling/.venv/bin/python {}",
                sh_quote(wheel_remote)
            ),
            format!("printf '%s\\n' {} > ~/.analyst/tooling/VERSION", sh_quote(version)),
        ]
    }

    /// The whole sequence as one `sh -e` script line (so one ssh round trip).
    pub fn script(python_version: &str, wheel_remote: &str, version: &str) -> String {
        format!(
            "set -e; {}",
            commands(python_version, wheel_remote, version).join("; ")
        )
    }

    /// Session venv setup in the mirror: `uv sync` when a pyproject exists, else venv + requirements.
    pub fn session_env_command(mirror: &str) -> String {
        let m = if let Some(rest) = mirror.strip_prefix("~/") {
            format!("~/{}", sh_quote(rest))
        } else {
            sh_quote(mirror)
        };
        format!(
            "cd {m} && if [ -f pyproject.toml ]; then ~/.analyst/bin/uv sync; else ~/.analyst/bin/uv venv && (test -f requirements.txt && ~/.analyst/bin/uv pip install -r requirements.txt || true); fi"
        )
    }

    /// `scp -q <wheel> <host>:~/.analyst/tooling/<name>`
    pub fn scp_wheel_command(host: &str, wheel: &Path, control_dir: Option<&Path>) -> Vec<String> {
        let name = wheel
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "analyst_kernel.whl".into());
        let mut v = vec!["scp".to_string(), "-q".to_string()];
        if let Some(dir) = control_dir {
            v.push("-o".into());
            v.push(format!("ControlPath={}/%C", dir.display()));
        }
        v.push(wheel.display().to_string());
        v.push(format!("{host}:~/.analyst/tooling/{name}"));
        v
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Outcome {
        UpToDate(String),
        Installed(String),
    }

    /// Check the version file and run the sequence when missing or different.
    pub async fn ensure(
        target: &Target,
        control_dir: Option<&Path>,
        wheel: Option<&Path>,
    ) -> Result<Outcome> {
        let (host, python) = match target {
            Target::Ssh { host, python, .. } => (host.as_str(), python.as_str()),
            Target::Local => return Err(anyhow!("the local target needs no bootstrap")),
        };
        let want = tooling_version();
        let have = run_ssh(host, control_dir, &version_check_command()).await?;
        if have.trim() == want {
            return Ok(Outcome::UpToDate(want.to_string()));
        }
        let wheel = wheel.ok_or_else(|| {
            anyhow!(
                "no kernel wheel found; build one with `uv build` in python/kernel (dist/*.whl)"
            )
        })?;
        run_ssh(
            host,
            control_dir,
            "mkdir -p ~/.analyst/bin ~/.analyst/tooling",
        )
        .await?;
        run_command(&scp_wheel_command(host, wheel, control_dir), None).await?;
        let name = wheel
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let remote_wheel = format!("~/.analyst/tooling/{name}");
        run_ssh(host, control_dir, &script(python, &remote_wheel, want)).await?;
        Ok(Outcome::Installed(want.to_string()))
    }

    /// Locate a kernel wheel in dev mode: `<workspace>/python/kernel/dist/*.whl`.
    pub fn find_dev_wheel() -> Option<PathBuf> {
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
                let dist = dir.join("python/kernel/dist");
                if let Ok(rd) = std::fs::read_dir(&dist) {
                    let mut wheels: Vec<PathBuf> = rd
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().map(|e| e == "whl").unwrap_or(false))
                        .collect();
                    wheels.sort();
                    if let Some(w) = wheels.pop() {
                        return Some(w);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::sync::*;
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join(".gitignore"), "out/\n.venv/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("scripts/a.py"), "print(1)\n").unwrap();
        std::fs::write(dir.path().join("out/ignored.txt"), "x").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("untracked.md"), "hello\n").unwrap();
        dir
    }

    #[test]
    fn file_list_and_tree_hash() {
        let repo = temp_repo();
        let list = file_list(repo.path()).unwrap();
        let paths: Vec<&str> = list.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec![".gitignore", "scripts/a.py", "untracked.md"]);
        assert!(list.iter().all(|e| e.blob.len() >= 40));
        let h1 = tree_hash(&list);
        assert_eq!(h1.len(), 64);
        std::fs::write(repo.path().join("scripts/a.py"), "print(2)\n").unwrap();
        let h2 = tree_hash(&file_list(repo.path()).unwrap());
        assert_ne!(h1, h2);
        // Order independence.
        let mut rev = list.clone();
        rev.reverse();
        assert_eq!(tree_hash(&rev), h1);
    }

    #[test]
    fn command_lines() {
        let list = PathBuf::from("/tmp/list");
        let ctl = PathBuf::from("/home/u/.analyst/ssh");
        assert_eq!(
            rsync_command("lab", "~/analyst/repo", &list, None),
            vec![
                "rsync",
                "-az",
                "--delete",
                "--from0",
                "--files-from=/tmp/list",
                "./",
                "lab:~/analyst/repo/"
            ]
        );
        let r = rsync_command("lab", "~/analyst/repo", &list, Some(&ctl));
        assert_eq!(r[5], "-e");
        assert!(r[6].contains("ControlPath=/home/u/.analyst/ssh/%C"));
        assert_eq!(
            tar_create_command(&list),
            vec!["tar", "-cf", "-", "--null", "-T", "/tmp/list"]
        );
        let push = tar_push_ssh_command("lab", "~/analyst/my repo", None);
        assert_eq!(push[0], "ssh");
        assert_eq!(
            push.last().unwrap(),
            "mkdir -p ~/'analyst/my repo' && tar -xf - -C ~/'analyst/my repo'"
        );
        assert_eq!(
            scp_pull_command(
                "lab",
                "~/analyst/repo",
                "out/f/r1/run.json",
                Path::new("/l/run.json"),
                None
            ),
            vec![
                "scp",
                "-q",
                "lab:~/analyst/repo/out/f/r1/run.json",
                "/l/run.json"
            ]
        );
        let pull = tar_pull_ssh_command("lab", "~/analyst/repo", "out/fit/r1", Some(&ctl));
        assert_eq!(
            pull.last().unwrap(),
            "tar -cf - -C ~/analyst/repo out/fit/r1"
        );
        assert!(pull
            .iter()
            .any(|p| p == "ControlPath=/home/u/.analyst/ssh/%C"));
        assert_eq!(
            tar_extract_local_command(Path::new("/repo")),
            vec!["tar", "-xf", "-", "-C", "/repo"]
        );
    }

    #[test]
    fn bootstrap_commands() {
        let cmds = bootstrap::commands(
            "3.12",
            "~/.analyst/tooling/analyst_kernel-0.0.1-py3-none-any.whl",
            "0.0.1",
        );
        assert_eq!(cmds[0], "mkdir -p ~/.analyst/bin ~/.analyst/tooling");
        assert!(cmds[1].contains("UV_INSTALL_DIR=~/.analyst/bin"));
        assert_eq!(cmds[2], "~/.analyst/bin/uv python install 3.12");
        assert!(cmds[3].contains("uv venv --python 3.12 ~/.analyst/tooling/.venv"));
        assert!(cmds[4].starts_with(
            "~/.analyst/bin/uv pip install --python ~/.analyst/tooling/.venv/bin/python "
        ));
        assert_eq!(cmds[5], "printf '%s\\n' 0.0.1 > ~/.analyst/tooling/VERSION");
        assert!(bootstrap::script("3.12", "w", "v").starts_with("set -e; mkdir -p"));
        assert!(
            bootstrap::session_env_command("~/analyst/repo").starts_with("cd ~/analyst/repo && ")
        );
        let scp = bootstrap::scp_wheel_command("lab", Path::new("/w/k.whl"), None);
        assert_eq!(
            scp,
            vec!["scp", "-q", "/w/k.whl", "lab:~/.analyst/tooling/k.whl"]
        );
        assert_eq!(
            bootstrap::version_check_command(),
            "cat ~/.analyst/tooling/VERSION 2>/dev/null || true"
        );
    }

    #[tokio::test]
    async fn tar_fallback_pipes_into_a_local_directory() {
        let repo = temp_repo();
        let entries = file_list(repo.path()).unwrap();
        let list = repo.path().join(".list");
        write_list_file(&entries, &list).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let mirror = dest.path().join("mirror");
        // Same remote half as the ssh fallback, but run by a local shell.
        let consumer = vec![
            "sh".to_string(),
            "-c".to_string(),
            tar_extract_remote(&mirror.display().to_string()),
        ];
        pipe_commands(&tar_create_command(&list), &consumer, Some(repo.path()))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.join("scripts/a.py")).unwrap(),
            "print(1)\n"
        );
        assert!(mirror.join("untracked.md").is_file());
        assert!(!mirror.join("out").exists(), "ignored files are not synced");
    }

    #[test]
    fn target_from_config_and_mirror() {
        let cfg = TargetConfig {
            name: "lab".into(),
            kind: TargetKind::Ssh,
            host: Some("lab-gpu".into()),
            remote_root: Some("~/analyst".into()),
            python: None,
            sandbox: None,
            data_dirs: vec!["/data".into()],
        };
        let t = Target::from_config(&cfg).unwrap();
        assert_eq!(
            t.mirror_dir(Path::new("/home/u/proj")).unwrap(),
            "~/analyst/proj"
        );
        assert_eq!(t.host(), Some("lab-gpu"));
        assert!(t.is_remote());
        assert_eq!(Target::Local.mirror_dir(Path::new("/x")), None);
    }
}
