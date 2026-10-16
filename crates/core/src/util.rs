//! Small shared helpers: timestamps, hashing, ids, path normalization.

use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// Current time as RFC 3339 UTC with second precision (`2026-10-16T18:15:02Z`).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Compact UTC stamp used in ids and file names (`20261016-181502`).
pub fn now_stamp() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Six lowercase hex characters of randomness.
pub fn short_rand() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

/// A fresh id of the form `<stamp>-<6 hex>`.
pub fn stamped_id() -> String {
    format!("{}-{}", now_stamp(), short_rand())
}

/// Hex SHA-256 of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Normalize a repo-relative path: forward slashes, no `.` components, and reject any path
/// that escapes the root (`..`) or is absolute.
pub fn normalize_rel_path(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if p.is_absolute() || path.starts_with('~') {
        return Err(format!(
            "path `{path}` is absolute; paths must be relative to the repo root"
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
            Component::ParentDir => {
                return Err(format!("path `{path}` escapes the repo root"));
            }
            _ => return Err(format!("path `{path}` is not a relative path")),
        }
    }
    if parts.is_empty() {
        return Err("empty path".to_string());
    }
    Ok(parts.join("/"))
}

/// Join a normalized relative path onto the repo root.
pub fn repo_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    Ok(root.join(normalize_rel_path(rel)?))
}

/// Rough token estimate (4 characters per token) used for context budgeting.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Truncate a string at a byte limit on a char boundary.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Expand a leading `~` with `$HOME`.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    } else if p == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(p)
}

/// Shell-quote a string for use inside a remote `ssh` command string (POSIX sh).
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,~".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize_rel_path("./a/b.py").unwrap(), "a/b.py");
        assert!(normalize_rel_path("../x").is_err());
        assert!(normalize_rel_path("/etc/passwd").is_err());
        assert!(normalize_rel_path("a/../../b").is_err());
        assert_eq!(normalize_rel_path("a/./b").unwrap(), "a/b");
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("abc"), "abc");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let s = "héllo";
        assert_eq!(truncate_bytes(s, 2), "h");
        assert_eq!(truncate_bytes(s, 3), "hé");
        assert_eq!(truncate_bytes(s, 100), s);
    }
}
