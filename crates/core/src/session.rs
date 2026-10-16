//! Session store under `$ANALYST_HOME/sessions/<id>/` (PROTOCOL 2.3): `session.toml`,
//! transcript JSONL per conversation, `runs.jsonl`, `artifacts/`, and the "files changed
//! since last turn" tree diff.

use crate::checkpoint;
use analyst_protocol::kernel::RunMeta;
use analyst_protocol::session::{SessionRecord, SessionRun};
use analyst_protocol::transcript::Event;
use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Session {
    pub home: PathBuf,
    pub dir: PathBuf,
    pub record: SessionRecord,
}

pub fn sessions_dir(home: &Path) -> PathBuf {
    home.join("sessions")
}

impl Session {
    /// Create a new session for a repo. `model` is `"<provider>/<id>"`.
    pub fn create(home: &Path, repo: &Path, target: &str, model: &str) -> Result<Session> {
        let id = crate::util::stamped_id();
        let dir = sessions_dir(home).join(&id);
        std::fs::create_dir_all(dir.join("conversations"))
            .with_context(|| format!("creating {}", dir.display()))?;
        std::fs::create_dir_all(dir.join("artifacts"))?;
        let repo_abs = std::fs::canonicalize(repo)
            .with_context(|| format!("resolving repo {}", repo.display()))?;
        let record = SessionRecord {
            id: id.clone(),
            repo: repo_abs.to_string_lossy().to_string(),
            target: target.to_string(),
            model: model.to_string(),
            created_at: crate::util::now_rfc3339(),
            last_tree_hash: None,
        };
        let s = Session {
            home: home.to_path_buf(),
            dir,
            record,
        };
        s.save()?;
        Ok(s)
    }

    /// Open an existing session by id.
    pub fn open(home: &Path, id: &str) -> Result<Session> {
        let dir = sessions_dir(home).join(id);
        let text = std::fs::read_to_string(dir.join("session.toml"))
            .with_context(|| format!("no session `{id}` under {}", sessions_dir(home).display()))?;
        let record: SessionRecord = toml::from_str(&text).context("parsing session.toml")?;
        Ok(Session {
            home: home.to_path_buf(),
            dir,
            record,
        })
    }

    /// Find the most recent session for a repo and target, if any.
    pub fn find_latest(home: &Path, repo: &Path, target: &str) -> Result<Option<Session>> {
        let repo_abs = std::fs::canonicalize(repo)?.to_string_lossy().to_string();
        let mut found: Vec<Session> = list(home)?
            .into_iter()
            .filter(|s| s.record.repo == repo_abs && s.record.target == target)
            .collect();
        found.sort_by(|a, b| a.record.id.cmp(&b.record.id));
        Ok(found.pop())
    }

    pub fn save(&self) -> Result<()> {
        let text = toml::to_string_pretty(&self.record)?;
        std::fs::write(self.dir.join("session.toml"), text)
            .with_context(|| format!("writing session.toml in {}", self.dir.display()))
    }

    pub fn id(&self) -> &str {
        &self.record.id
    }

    pub fn repo(&self) -> PathBuf {
        PathBuf::from(&self.record.repo)
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.dir.join("artifacts")
    }

    pub fn conversations_dir(&self) -> PathBuf {
        self.dir.join("conversations")
    }

    pub fn transcript_path(&self, conv_id: &str) -> PathBuf {
        self.conversations_dir().join(format!("{conv_id}.jsonl"))
    }

    /// Append one transcript event.
    pub fn append_event(&self, conv_id: &str, event: &Event) -> Result<()> {
        append_jsonl(&self.transcript_path(conv_id), event)
    }

    /// Read a conversation's events.
    pub fn read_transcript(&self, conv_id: &str) -> Result<Vec<Event>> {
        read_jsonl(&self.transcript_path(conv_id))
    }

    /// Conversation ids with transcripts on disk.
    pub fn conversations(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.conversations_dir()) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(stem) = name.strip_suffix(".jsonl") {
                    ids.push(stem.to_string());
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Mirror a RunMeta into `runs.jsonl`.
    pub fn append_run(&self, meta: &RunMeta) -> Result<()> {
        let run = SessionRun {
            meta: meta.clone(),
            target: self.record.target.clone(),
        };
        append_jsonl(&self.dir.join("runs.jsonl"), &run)
    }

    pub fn read_runs(&self) -> Result<Vec<SessionRun>> {
        read_jsonl(&self.dir.join("runs.jsonl"))
    }

    /// Latest record for a run id (runs are appended again when they finish).
    pub fn find_run(&self, run_id: &str) -> Result<Option<SessionRun>> {
        Ok(self
            .read_runs()?
            .into_iter()
            .rfind(|r| r.meta.run_id == run_id))
    }

    /// Store bytes under `artifacts/<name>`; returns the absolute path.
    pub fn spill_artifact(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.artifacts_dir().join(name);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Local path for a pulled artifact, keyed `<run_id>/<relative path>`.
    pub fn pulled_artifact_path(&self, run_id: &str, rel: &str) -> PathBuf {
        self.artifacts_dir().join(run_id).join(rel)
    }

    pub fn set_last_tree_hash(&mut self, hash: Option<String>) -> Result<()> {
        self.record.last_tree_hash = hash;
        self.save()
    }

    fn worktree_marker(&self) -> PathBuf {
        self.dir.join("last_worktree_tree")
    }

    /// Files changed in the working tree since the last call (tree diff against the recorded
    /// worktree tree). The first call records a baseline and returns an empty list.
    pub fn files_changed_since_last_turn(&self) -> Result<Vec<String>> {
        let repo = self.repo();
        let now = checkpoint::worktree_tree(&repo)?;
        let marker = self.worktree_marker();
        let prev = std::fs::read_to_string(&marker)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        std::fs::write(&marker, &now)?;
        match prev {
            Some(p) if p != now => checkpoint::changed_files(&repo, &p, &now),
            _ => Ok(vec![]),
        }
    }

    /// Record the current worktree as the baseline (after the agent's own edits).
    pub fn mark_worktree(&self) -> Result<()> {
        let now = checkpoint::worktree_tree(&self.repo())?;
        std::fs::write(self.worktree_marker(), now)?;
        Ok(())
    }
}

/// All sessions under a home.
pub fn list(home: &Path) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    let dir = sessions_dir(home);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if let Ok(s) = Session::open(home, &id) {
                out.push(s);
            }
        }
    }
    out.sort_by(|a, b| a.record.id.cmp(&b.record.id));
    Ok(out)
}

pub fn append_jsonl<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let line = serde_json::to_string(value)?;
    if line.contains('\n') {
        return Err(anyhow!("JSONL line contains a newline"));
    }
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

pub fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    let mut out = Vec::new();
    for (i, line) in std::io::BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: T = serde_json::from_str(&line)
            .with_context(|| format!("{}:{}: bad JSONL line", path.display(), i + 1))?;
        out.push(v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyst_protocol::kernel::RunStatus;

    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        checkpoint::git(dir.path(), &["init", "-q"], None).unwrap();
        std::fs::write(dir.path().join("a.py"), "a = 1\n").unwrap();
        checkpoint::git(dir.path(), &["add", "-A"], None).unwrap();
        checkpoint::git(dir.path(), &["commit", "-q", "-m", "init"], None).unwrap();
        dir
    }

    #[test]
    fn create_open_transcript_runs_and_changes() {
        let home = tempfile::tempdir().unwrap();
        let repo = temp_repo();
        let s = Session::create(home.path(), repo.path(), "local", "p/m").unwrap();
        let again = Session::open(home.path(), s.id()).unwrap();
        assert_eq!(again.record, s.record);
        assert_eq!(list(home.path()).unwrap().len(), 1);
        assert_eq!(
            Session::find_latest(home.path(), repo.path(), "local")
                .unwrap()
                .unwrap()
                .id(),
            s.id()
        );

        s.append_event(
            "c1",
            &Event::User {
                ts: "t".into(),
                text: "hi".into(),
                attachments: None,
            },
        )
        .unwrap();
        s.append_event(
            "c1",
            &Event::Assistant {
                ts: "t".into(),
                text: "line1\nline2".into(),
            },
        )
        .unwrap();
        let events = s.read_transcript("c1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(s.conversations().unwrap(), vec!["c1"]);

        let meta = RunMeta {
            run_id: "r1".into(),
            script: "scripts/a.py".into(),
            args: vec![],
            status: RunStatus::Running,
            exit_code: None,
            started_at: "t".into(),
            finished_at: None,
            out_dir: "out/a/r1".into(),
            sandbox: "none".into(),
            tree_hash: None,
            pid: None,
            host: "h".into(),
        };
        s.append_run(&meta).unwrap();
        let mut done = meta.clone();
        done.status = RunStatus::Done;
        s.append_run(&done).unwrap();
        assert_eq!(s.read_runs().unwrap().len(), 2);
        assert_eq!(
            s.find_run("r1").unwrap().unwrap().meta.status,
            RunStatus::Done
        );
        assert_eq!(s.read_runs().unwrap()[0].target, "local");

        let p = s.spill_artifact("c1/result.json", b"{}").unwrap();
        assert!(p.starts_with(s.artifacts_dir()));

        assert!(
            s.files_changed_since_last_turn().unwrap().is_empty(),
            "baseline"
        );
        std::fs::write(repo.path().join("b.py"), "b\n").unwrap();
        assert_eq!(s.files_changed_since_last_turn().unwrap(), vec!["b.py"]);
        assert!(s.files_changed_since_last_turn().unwrap().is_empty());
    }
}
