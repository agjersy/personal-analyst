//! Checkpoints (ARCHITECTURE 5.9): the working tree (tracked and untracked, not ignored) is
//! written to a tree object through a temporary index (`GIT_INDEX_FILE`) and stored under
//! `refs/analyst/checkpoints/<id>`. The user's index, branches, and history are untouched.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

pub const REF_PREFIX: &str = "refs/analyst/checkpoints/";

fn git_dir(repo: &Path) -> Result<PathBuf> {
    let out = git(repo, &["rev-parse", "--git-dir"], None)?;
    let p = PathBuf::from(out.trim());
    Ok(if p.is_absolute() { p } else { repo.join(p) })
}

/// Run git in the repo; optional extra environment. Returns stdout.
pub fn git(repo: &Path, args: &[&str], env: Option<&[(&str, &str)]>) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args).current_dir(repo);
    cmd.env("GIT_AUTHOR_NAME", "analyst")
        .env("GIT_AUTHOR_EMAIL", "analyst@localhost")
        .env("GIT_COMMITTER_NAME", "analyst")
        .env("GIT_COMMITTER_EMAIL", "analyst@localhost");
    if let Some(env) = env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    let out = cmd
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// A temporary index file inside `.git`, removed on drop.
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    fn new(repo: &Path) -> Result<Self> {
        let dir = git_dir(repo)?;
        let path = dir.join(format!("analyst-index-{}", crate::util::short_rand()));
        Ok(TempIndex { path })
    }
    fn env(&self) -> Vec<(&str, String)> {
        vec![("GIT_INDEX_FILE", self.path.display().to_string())]
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{}.lock", self.path.display()));
    }
}

fn with_env<'a>(env: &'a [(&'a str, String)]) -> Vec<(&'a str, &'a str)> {
    env.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// Write the current working tree (tracked + untracked, excluding ignored) to a tree object
/// without touching the user's index. Returns the tree sha.
pub fn worktree_tree(repo: &Path) -> Result<String> {
    let idx = TempIndex::new(repo)?;
    let env = idx.env();
    let env = with_env(&env);
    git(repo, &["add", "-A", "--", "."], Some(&env))?;
    let tree = git(repo, &["write-tree"], Some(&env))?;
    Ok(tree.trim().to_string())
}

/// Create a checkpoint `refs/analyst/checkpoints/<id>`; returns the ref name.
pub fn create(repo: &Path, id: &str) -> Result<String> {
    let tree = worktree_tree(repo)?;
    let head = git(repo, &["rev-parse", "--verify", "-q", "HEAD"], None)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let msg = format!("analyst checkpoint {id}");
    let mut args = vec!["commit-tree", tree.as_str(), "-m", msg.as_str()];
    if let Some(h) = head.as_deref() {
        args.push("-p");
        args.push(h);
    }
    let commit = git(repo, &args, None)?.trim().to_string();
    let r = format!("{REF_PREFIX}{id}");
    git(repo, &["update-ref", &r, &commit], None)?;
    Ok(r)
}

/// Tree sha of a checkpoint.
pub fn tree_of(repo: &Path, id: &str) -> Result<String> {
    let r = format!("{REF_PREFIX}{id}");
    Ok(git(repo, &["rev-parse", &format!("{r}^{{tree}}")], None)?
        .trim()
        .to_string())
}

/// Restore the working tree to a checkpoint: files added since are removed, changed files are
/// rewritten. Ignored files and the user's index are untouched.
pub fn restore(repo: &Path, id: &str) -> Result<()> {
    let tree = tree_of(repo, id)?;
    let idx = TempIndex::new(repo)?;
    let env = idx.env();
    let env = with_env(&env);
    // Load the current working tree into the temp index so read-tree knows what to remove.
    git(repo, &["add", "-A", "--", "."], Some(&env))?;
    git(repo, &["read-tree", "-u", "--reset", &tree], Some(&env))?;
    Ok(())
}

/// Checkpoint ids with their commits, oldest first (creation order by committer date).
pub fn list(repo: &Path) -> Result<Vec<(String, String)>> {
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--sort=committerdate",
            "--format=%(refname) %(objectname)",
            REF_PREFIX,
        ],
        None,
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let (r, sha) = l.split_once(' ')?;
            Some((r.strip_prefix(REF_PREFIX)?.to_string(), sha.to_string()))
        })
        .collect())
}

/// Delete a checkpoint ref.
pub fn delete(repo: &Path, id: &str) -> Result<()> {
    git(
        repo,
        &["update-ref", "-d", &format!("{REF_PREFIX}{id}")],
        None,
    )?;
    Ok(())
}

/// Keep only the newest `keep` checkpoints. Returns the ids removed.
pub fn prune(repo: &Path, keep: usize) -> Result<Vec<String>> {
    let all = list(repo)?;
    let mut removed = Vec::new();
    if all.len() > keep {
        for (id, _) in &all[..all.len() - keep] {
            delete(repo, id)?;
            removed.push(id.clone());
        }
    }
    Ok(removed)
}

/// Paths that differ between two tree objects.
pub fn changed_files(repo: &Path, from_tree: &str, to_tree: &str) -> Result<Vec<String>> {
    let out = git(
        repo,
        &["diff-tree", "-r", "--name-only", from_tree, to_tree],
        None,
    )?;
    Ok(out
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "out/\n").unwrap();
        std::fs::write(dir.path().join("a.py"), "a = 1\n").unwrap();
        git(dir.path(), &["add", "-A"], None).unwrap();
        git(dir.path(), &["commit", "-q", "-m", "init"], None).unwrap();
        dir
    }

    #[test]
    fn checkpoint_restore_and_prune() {
        let repo = temp_repo();
        let r = repo.path();
        std::fs::write(r.join("untracked.py"), "u = 1\n").unwrap();
        std::fs::create_dir_all(r.join("out")).unwrap();
        std::fs::write(r.join("out/ignored.txt"), "ignored\n").unwrap();
        let index_before = std::fs::read(r.join(".git/index")).unwrap();

        let cp = create(r, "t1").unwrap();
        assert_eq!(cp, "refs/analyst/checkpoints/t1");
        assert_eq!(
            std::fs::read(r.join(".git/index")).unwrap(),
            index_before,
            "user index untouched"
        );
        // `git status` must still show untracked.py as untracked (not staged).
        let st = git(r, &["status", "--porcelain"], None).unwrap();
        assert!(st.contains("?? untracked.py"), "{st}");
        assert!(
            !r.join(".git").read_dir().unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("analyst-index-")
            }),
            "temp index removed"
        );

        // Mutate: change a.py, delete untracked.py, add new.py.
        std::fs::write(r.join("a.py"), "a = 2\n").unwrap();
        std::fs::remove_file(r.join("untracked.py")).unwrap();
        std::fs::write(r.join("new.py"), "n = 1\n").unwrap();
        let t2 = worktree_tree(r).unwrap();
        let t1 = tree_of(r, "t1").unwrap();
        let mut changed = changed_files(r, &t1, &t2).unwrap();
        changed.sort();
        assert_eq!(changed, vec!["a.py", "new.py", "untracked.py"]);

        restore(r, "t1").unwrap();
        assert_eq!(std::fs::read_to_string(r.join("a.py")).unwrap(), "a = 1\n");
        assert_eq!(
            std::fs::read_to_string(r.join("untracked.py")).unwrap(),
            "u = 1\n"
        );
        assert!(
            !r.join("new.py").exists(),
            "file added after checkpoint removed"
        );
        assert_eq!(
            std::fs::read_to_string(r.join("out/ignored.txt")).unwrap(),
            "ignored\n"
        );

        create(r, "t2").unwrap();
        create(r, "t3").unwrap();
        assert_eq!(list(r).unwrap().len(), 3);
        let removed = prune(r, 2).unwrap();
        assert_eq!(removed, vec!["t1"]);
        let ids: Vec<String> = list(r).unwrap().into_iter().map(|(i, _)| i).collect();
        assert_eq!(ids, vec!["t2", "t3"]);
        assert!(
            git(r, &["log", "--oneline"], None).unwrap().lines().count() == 1,
            "history untouched"
        );
    }
}
