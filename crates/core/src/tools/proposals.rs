//! Edit proposals: `edits` (old/new, exact-once matching) or full `content`, create, delete,
//! with conflict detection by content hash of the file the proposal was based on.

use crate::util::{normalize_rel_path, sha256_hex};
use analyst_protocol::transcript::ProposalKind;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub id: String,
    pub kind: ProposalKind,
    pub path: String,
    /// Full new content (None for delete).
    pub new_content: Option<String>,
    /// SHA-256 of the file the proposal was based on (None for create).
    pub base_hash: Option<String>,
    pub summary: String,
    pub diff: String,
}

fn read_existing(repo: &Path, rel: &str) -> Result<String> {
    let full = repo.join(rel);
    std::fs::read_to_string(&full).map_err(|e| anyhow!("cannot read `{rel}`: {e}"))
}

/// Build a proposal for `fs.propose_edit`, `fs.propose_create`, or `fs.propose_delete`.
pub fn build(repo: &Path, id: &str, tool: &str, args: &Value) -> Result<Proposal> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("`path` is required"))?;
    let path = normalize_rel_path(raw).map_err(|e| anyhow!(e))?;
    match tool {
        "fs.propose_create" => {
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("`content` is required"))?;
            if repo.join(&path).exists() {
                return Err(anyhow!("`{path}` already exists; use fs.propose_edit"));
            }
            Ok(Proposal {
                id: id.to_string(),
                kind: ProposalKind::Create,
                path: path.clone(),
                new_content: Some(content.to_string()),
                base_hash: None,
                summary: format!("create {path} ({} lines)", content.lines().count()),
                diff: unified_diff("", content, &path),
            })
        }
        "fs.propose_delete" => {
            let existing = read_existing(repo, &path)?;
            Ok(Proposal {
                id: id.to_string(),
                kind: ProposalKind::Delete,
                path: path.clone(),
                new_content: None,
                base_hash: Some(sha256_hex(existing.as_bytes())),
                summary: format!("delete {path}"),
                diff: unified_diff(&existing, "", &path),
            })
        }
        "fs.propose_edit" => {
            let existing = read_existing(repo, &path)?;
            let new_content = if let Some(edits) = args.get("edits").and_then(Value::as_array) {
                apply_edits(&existing, edits)?
            } else if let Some(c) = args.get("content").and_then(Value::as_str) {
                c.to_string()
            } else {
                return Err(anyhow!("fs.propose_edit needs `edits` or `content`"));
            };
            if new_content == existing {
                return Err(anyhow!("edit produces no change to `{path}`"));
            }
            let diff = unified_diff(&existing, &new_content, &path);
            let (added, removed) = count_changes(&diff);
            Ok(Proposal {
                id: id.to_string(),
                kind: ProposalKind::Edit,
                path: path.clone(),
                new_content: Some(new_content),
                base_hash: Some(sha256_hex(existing.as_bytes())),
                summary: format!("edit {path} (+{added} -{removed})"),
                diff,
            })
        }
        other => Err(anyhow!("`{other}` is not an edit tool")),
    }
}

/// Apply `{old, new}` edits; each `old` must occur exactly once in the current text.
pub fn apply_edits(existing: &str, edits: &[Value]) -> Result<String> {
    let mut text = existing.to_string();
    for (i, e) in edits.iter().enumerate() {
        let old = e
            .get("old")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("edit {i}: `old` is required"))?;
        let new = e
            .get("new")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("edit {i}: `new` is required"))?;
        if old.is_empty() {
            return Err(anyhow!("edit {i}: `old` must not be empty"));
        }
        let count = text.matches(old).count();
        if count == 0 {
            return Err(anyhow!(
                "edit {i}: `old` text not found in the file (re-read the file; it may have changed)"
            ));
        }
        if count > 1 {
            return Err(anyhow!(
                "edit {i}: `old` text occurs {count} times; include more context so it matches exactly once"
            ));
        }
        text = text.replacen(old, new, 1);
    }
    Ok(text)
}

/// Apply a proposal to the working tree, rejecting it when the file changed since it was
/// built (conflict).
pub fn apply(repo: &Path, p: &Proposal) -> Result<()> {
    let full = repo.join(&p.path);
    if let Some(h) = &p.base_hash {
        let current = std::fs::read(&full).map_err(|e| anyhow!("cannot read `{}`: {e}", p.path))?;
        if sha256_hex(&current) != *h {
            return Err(anyhow!(
                "conflict: `{}` changed since the proposal was made; re-read it and propose again",
                p.path
            ));
        }
    }
    match p.kind {
        ProposalKind::Delete => {
            std::fs::remove_file(&full).map_err(|e| anyhow!("deleting `{}`: {e}", p.path))?;
        }
        _ => {
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content = p.new_content.as_deref().unwrap_or("");
            std::fs::write(&full, content).map_err(|e| anyhow!("writing `{}`: {e}", p.path))?;
        }
    }
    Ok(())
}

fn count_changes(diff: &str) -> (usize, usize) {
    let mut a = 0;
    let mut r = 0;
    for l in diff.lines() {
        if l.starts_with("+++") || l.starts_with("---") {
            continue;
        }
        if l.starts_with('+') {
            a += 1;
        } else if l.starts_with('-') {
            r += 1;
        }
    }
    (a, r)
}

/// A minimal unified diff (LCS on lines, 3 lines of context).
pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = diff_ops(&a, &b);
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    // Group ops into hunks with context.
    let ctx = 3usize;
    let mut i = 0;
    while i < ops.len() {
        if ops[i].0 == ' ' {
            i += 1;
            continue;
        }
        let start = i.saturating_sub(ctx);
        let mut end = i;
        let mut last_change = i;
        while end < ops.len() && end <= last_change + ctx {
            if ops[end].0 != ' ' {
                last_change = end;
            }
            end += 1;
        }
        let end = end.min(ops.len());
        let (mut a_start, mut b_start, mut a_len, mut b_len) = (0usize, 0usize, 0usize, 0usize);
        let mut body = String::new();
        for (k, (tag, ai, bi)) in ops.iter().enumerate() {
            if k < start {
                continue;
            }
            if k >= end {
                break;
            }
            if a_len == 0 && b_len == 0 {
                a_start = ai.map(|x| x + 1).unwrap_or(0);
                b_start = bi.map(|x| x + 1).unwrap_or(0);
            }
            match tag {
                ' ' => {
                    a_len += 1;
                    b_len += 1;
                    body.push_str(&format!(" {}\n", a[ai.unwrap_or(0)]));
                }
                '-' => {
                    a_len += 1;
                    body.push_str(&format!("-{}\n", a[ai.unwrap_or(0)]));
                }
                _ => {
                    b_len += 1;
                    body.push_str(&format!("+{}\n", b[bi.unwrap_or(0)]));
                }
            }
        }
        if a_start == 0 && a_len > 0 {
            a_start = 1;
        }
        if b_start == 0 && b_len > 0 {
            b_start = 1;
        }
        out.push_str(&format!("@@ -{a_start},{a_len} +{b_start},{b_len} @@\n"));
        out.push_str(&body);
        i = end;
    }
    out
}

/// Line-level diff ops: (' ' | '-' | '+', index in a, index in b).
fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, Option<usize>, Option<usize>)> {
    let n = a.len();
    let m = b.len();
    // Guard against quadratic blowup on huge files: fall back to whole replacement.
    if n * m > 4_000_000 {
        let mut ops = Vec::new();
        for i in 0..n {
            ops.push(('-', Some(i), None));
        }
        for j in 0..m {
            ops.push(('+', None, Some(j)));
        }
        return ops;
    }
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((' ', Some(i), Some(j)));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(('-', Some(i), None));
            i += 1;
        } else {
            ops.push(('+', None, Some(j)));
            j += 1;
        }
    }
    while i < n {
        ops.push(('-', Some(i), None));
        i += 1;
    }
    while j < m {
        ops.push(('+', None, Some(j)));
        j += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edits_require_exact_once_match() {
        let text = "a\nb\na\n";
        assert!(apply_edits(text, &[json!({"old":"a","new":"c"})]).is_err());
        assert!(apply_edits(text, &[json!({"old":"zz","new":"c"})]).is_err());
        assert_eq!(
            apply_edits(text, &[json!({"old":"b","new":"c"})]).unwrap(),
            "a\nc\na\n"
        );
    }

    #[test]
    fn build_apply_and_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        std::fs::write(repo.join("f.py"), "x = 1\ny = 2\n").unwrap();
        let p = build(
            repo,
            "p1",
            "fs.propose_edit",
            &json!({"path":"f.py","edits":[{"old":"y = 2","new":"y = 3"}]}),
        )
        .unwrap();
        assert_eq!(p.kind, ProposalKind::Edit);
        assert!(p.diff.contains("-y = 2"));
        assert!(p.diff.contains("+y = 3"));
        assert_eq!(p.summary, "edit f.py (+1 -1)");
        // Concurrent change -> conflict.
        std::fs::write(repo.join("f.py"), "x = 9\ny = 2\n").unwrap();
        let err = apply(repo, &p).unwrap_err().to_string();
        assert!(err.contains("conflict"));
        std::fs::write(repo.join("f.py"), "x = 1\ny = 2\n").unwrap();
        apply(repo, &p).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("f.py")).unwrap(),
            "x = 1\ny = 3\n"
        );

        let c = build(
            repo,
            "p2",
            "fs.propose_create",
            &json!({"path":"new/g.py","content":"g\n"}),
        )
        .unwrap();
        apply(repo, &c).unwrap();
        assert!(repo.join("new/g.py").is_file());
        assert!(build(
            repo,
            "p3",
            "fs.propose_create",
            &json!({"path":"new/g.py","content":"g\n"})
        )
        .is_err());
        let d = build(repo, "p4", "fs.propose_delete", &json!({"path":"new/g.py"})).unwrap();
        apply(repo, &d).unwrap();
        assert!(!repo.join("new/g.py").exists());
        assert!(build(
            repo,
            "p5",
            "fs.propose_edit",
            &json!({"path":"../x","content":""})
        )
        .is_err());
    }

    #[test]
    fn unified_diff_shape() {
        let d = unified_diff("a\nb\nc\nd\ne\nf\ng\n", "a\nb\nc\nX\ne\nf\ng\n", "p");
        assert!(d.starts_with("--- a/p\n+++ b/p\n@@ -1,7 +1,7 @@\n"));
        assert!(d.contains("-d\n+X\n"));
    }
}
