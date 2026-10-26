//! Repo filesystem operations behind `fs_*` commands: tree (respecting `.gitignore`), read and
//! write with SHA-256 hashes and conflict detection, create, delete (OS trash when available),
//! rename, and search through `rg --json`.

use super::error::{ServiceError, ServiceResult};
use crate::util::{normalize_rel_path, sha256_hex};
use analyst_protocol::ui::{FileContent, FileEntry, FileKind, SearchHit, WriteResult};
use base64::Engine;
use std::path::{Path, PathBuf};

/// Directories never shown or searched.
const ALWAYS_HIDDEN: &[&str] = &[
    ".git",
    ".venv",
    "__pycache__",
    ".ruff_cache",
    "node_modules",
];

fn rel(raw: &str) -> ServiceResult<String> {
    normalize_rel_path(raw).map_err(ServiceError::invalid)
}

/// Resolve a repo-relative path (empty or `.` means the root).
pub fn resolve(repo: &Path, raw: &str) -> ServiceResult<(String, PathBuf)> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "." || raw == "./" {
        return Ok((String::new(), repo.to_path_buf()));
    }
    let r = rel(raw)?;
    Ok((r.clone(), repo.join(&r)))
}

/// Directory tree under `path` (default root) to `depth` levels (default 1). Entries respect
/// `.gitignore`; the project `out_dir` is shown at the top level but never expanded unless it
/// is the requested path itself (`children: null` = not expanded).
pub fn tree(
    repo: &Path,
    path: Option<&str>,
    depth: Option<u32>,
    out_dir: &str,
) -> ServiceResult<Vec<FileEntry>> {
    let (base_rel, base) = resolve(repo, path.unwrap_or(""))?;
    if !base.is_dir() {
        return Err(ServiceError::not_found(format!(
            "`{}` is not a directory",
            if base_rel.is_empty() { "." } else { &base_rel }
        )));
    }
    let depth = depth.unwrap_or(1).clamp(1, 8) as usize;
    let out_rel = out_dir.trim_matches('/').to_string();
    let inside_out = !out_rel.is_empty()
        && (base_rel == out_rel || base_rel.starts_with(&format!("{out_rel}/")));
    let mut root_children = walk_dir(repo, &base, &base_rel, depth, &out_rel, inside_out)?;
    // `out/` is gitignored by the template; show it (collapsed) at the top level anyway.
    if base_rel.is_empty() && !out_rel.is_empty() && !out_rel.contains('/') {
        let out_abs = repo.join(&out_rel);
        if out_abs.is_dir() && !root_children.iter().any(|e| e.path == out_rel) {
            root_children.push(FileEntry {
                path: out_rel.clone(),
                name: out_rel.clone(),
                kind: FileKind::Dir,
                size: None,
                children: None,
            });
            sort_entries(&mut root_children);
        }
    }
    Ok(root_children)
}

fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let ka = (a.kind != FileKind::Dir, a.name.to_lowercase());
        let kb = (b.kind != FileKind::Dir, b.name.to_lowercase());
        ka.cmp(&kb)
    });
}

fn walk_dir(
    repo: &Path,
    dir: &Path,
    dir_rel: &str,
    depth: usize,
    out_rel: &str,
    inside_out: bool,
) -> ServiceResult<Vec<FileEntry>> {
    let mut walker = ignore::WalkBuilder::new(dir);
    walker
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .git_ignore(!inside_out)
        .git_global(false)
        .git_exclude(!inside_out)
        .sort_by_file_name(|a, b| a.cmp(b));
    // `.gitignore` files above `dir` must apply too.
    if dir != repo {
        let mut parents: Vec<PathBuf> = vec![];
        let mut cur = dir.parent();
        while let Some(p) = cur {
            parents.push(p.to_path_buf());
            if p == repo {
                break;
            }
            cur = p.parent();
        }
        for p in parents.iter().rev() {
            let gi = p.join(".gitignore");
            if gi.is_file() && !inside_out {
                walker.add_ignore(gi);
            }
        }
    }
    let mut entries = Vec::new();
    for item in walker.build().flatten() {
        if item.depth() == 0 {
            continue;
        }
        let name = item.file_name().to_string_lossy().to_string();
        if ALWAYS_HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let path = if dir_rel.is_empty() {
            name.clone()
        } else {
            format!("{dir_rel}/{name}")
        };
        let is_dir = item.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            let is_out = !out_rel.is_empty() && path == out_rel;
            let children = if depth > 1 && !is_out {
                Some(walk_dir(
                    repo,
                    item.path(),
                    &path,
                    depth - 1,
                    out_rel,
                    inside_out,
                )?)
            } else {
                None
            };
            entries.push(FileEntry {
                path,
                name,
                kind: FileKind::Dir,
                size: None,
                children,
            });
        } else {
            let size = item.metadata().ok().map(|m| m.len());
            entries.push(FileEntry {
                path,
                name,
                kind: FileKind::File,
                size,
                children: None,
            });
        }
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// Read a text file; binary content is `invalid`.
pub fn read(repo: &Path, path: &str) -> ServiceResult<FileContent> {
    let r = rel(path)?;
    let full = repo.join(&r);
    let bytes = std::fs::read(&full).map_err(|e| map_io(&r, e))?;
    let hash = sha256_hex(&bytes);
    let content = String::from_utf8(bytes)
        .map_err(|_| ServiceError::invalid(format!("`{r}` is not a UTF-8 text file")))?;
    Ok(FileContent {
        path: r,
        content,
        hash,
    })
}

/// Any file under the repo, base64.
pub fn read_binary(repo: &Path, path: &str) -> ServiceResult<(String, Vec<u8>)> {
    let r = rel(path)?;
    let full = repo.join(&r);
    let bytes = std::fs::read(&full).map_err(|e| map_io(&r, e))?;
    Ok((r, bytes))
}

pub fn to_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Write a file. With `expected_hash`, the current content must hash to it (`conflict`
/// otherwise; a missing file matches only an empty `expected_hash`).
pub fn write(
    repo: &Path,
    path: &str,
    content: &str,
    expected_hash: Option<&str>,
) -> ServiceResult<WriteResult> {
    let r = rel(path)?;
    let full = repo.join(&r);
    if let Some(expected) = expected_hash {
        let current = match std::fs::read(&full) {
            Ok(b) => Some(sha256_hex(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(map_io(&r, e)),
        };
        let matches = match &current {
            Some(h) => h == expected,
            None => expected.is_empty(),
        };
        if !matches {
            return Err(
                ServiceError::conflict(format!("`{r}` changed on disk since it was read"))
                    .with_detail(serde_json::json!({
                        "path": r,
                        "current_hash": current,
                        "expected_hash": expected,
                    })),
            );
        }
    }
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| map_io(&r, e))?;
    }
    std::fs::write(&full, content.as_bytes()).map_err(|e| map_io(&r, e))?;
    Ok(WriteResult {
        hash: sha256_hex(content.as_bytes()),
    })
}

pub fn create(repo: &Path, path: &str, is_dir: bool) -> ServiceResult<()> {
    let r = rel(path)?;
    let full = repo.join(&r);
    if full.exists() {
        return Err(ServiceError::conflict(format!("`{r}` already exists")));
    }
    if is_dir {
        std::fs::create_dir_all(&full).map_err(|e| map_io(&r, e))?;
    } else {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| map_io(&r, e))?;
        }
        std::fs::write(&full, b"").map_err(|e| map_io(&r, e))?;
    }
    Ok(())
}

/// Move to the OS trash when available, else delete.
pub fn delete(repo: &Path, path: &str) -> ServiceResult<()> {
    let r = rel(path)?;
    let full = repo.join(&r);
    if !full.exists() {
        return Err(ServiceError::not_found(format!("`{r}` does not exist")));
    }
    if trash::delete(&full).is_ok() {
        return Ok(());
    }
    if full.is_dir() {
        std::fs::remove_dir_all(&full).map_err(|e| map_io(&r, e))?;
    } else {
        std::fs::remove_file(&full).map_err(|e| map_io(&r, e))?;
    }
    Ok(())
}

pub fn rename(repo: &Path, from: &str, to: &str) -> ServiceResult<()> {
    let f = rel(from)?;
    let t = rel(to)?;
    let src = repo.join(&f);
    let dst = repo.join(&t);
    if !src.exists() {
        return Err(ServiceError::not_found(format!("`{f}` does not exist")));
    }
    if dst.exists() {
        return Err(ServiceError::conflict(format!("`{t}` already exists")));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| map_io(&t, e))?;
    }
    std::fs::rename(&src, &dst).map_err(|e| map_io(&f, e))?;
    Ok(())
}

/// Search file contents with ripgrep.
pub async fn search(
    repo: &Path,
    pattern: &str,
    glob: Option<&str>,
    max_results: Option<u32>,
) -> ServiceResult<Vec<SearchHit>> {
    if pattern.is_empty() {
        return Err(ServiceError::invalid("`pattern` must not be empty"));
    }
    let max = max_results.unwrap_or(200).max(1) as usize;
    let mut cmd = tokio::process::Command::new("rg");
    cmd.args(["--json", "--no-messages", "-e", pattern])
        .current_dir(repo)
        .stdin(std::process::Stdio::null());
    for h in ALWAYS_HIDDEN {
        cmd.arg("--glob").arg(format!("!{h}"));
    }
    if let Some(g) = glob {
        cmd.arg("--glob").arg(g);
    }
    cmd.arg(".");
    let out = cmd
        .output()
        .await
        .map_err(|e| ServiceError::internal(format!("running rg: {e} (is ripgrep installed?)")))?;
    let mut hits = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["type"] != "match" {
            continue;
        }
        let d = &v["data"];
        let path = d["path"]["text"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("./")
            .to_string();
        let line_no = d["line_number"].as_u64().unwrap_or(0);
        let text = d["lines"]["text"].as_str().unwrap_or("").trim_end();
        let text = crate::util::truncate_bytes(text, 400).to_string();
        let subs = d["submatches"].as_array();
        if let Some(subs) = subs.filter(|s| !s.is_empty()) {
            for sm in subs {
                let col = sm["start"].as_u64().unwrap_or(0) + 1;
                hits.push(SearchHit {
                    path: path.clone(),
                    line: line_no,
                    col,
                    text: text.clone(),
                });
                if hits.len() >= max {
                    return Ok(hits);
                }
            }
        } else {
            hits.push(SearchHit {
                path,
                line: line_no,
                col: 1,
                text,
            });
            if hits.len() >= max {
                break;
            }
        }
    }
    Ok(hits)
}

fn map_io(path: &str, e: std::io::Error) -> ServiceError {
    match e.kind() {
        std::io::ErrorKind::NotFound => ServiceError::not_found(format!("`{path}`: {e}")),
        _ => ServiceError::io(format!("`{path}`: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyst_protocol::ui::IpcErrorCode;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("scripts")).unwrap();
        std::fs::create_dir_all(r.join("analysis/__pycache__")).unwrap();
        std::fs::create_dir_all(r.join(".venv/bin")).unwrap();
        std::fs::create_dir_all(r.join("out/fit/r1/figures")).unwrap();
        std::fs::create_dir_all(r.join(".git")).unwrap();
        std::fs::write(r.join(".gitignore"), "out/\n.venv/\n__pycache__/\n*.pyc\n").unwrap();
        std::fs::write(r.join("scripts/a.py"), "print('hi')\n").unwrap();
        std::fs::write(r.join("scripts/b.pyc"), "x").unwrap();
        std::fs::write(r.join("analysis/__init__.py"), "").unwrap();
        std::fs::write(r.join("analysis/__pycache__/x.pyc"), "x").unwrap();
        std::fs::write(r.join(".venv/bin/python"), "").unwrap();
        std::fs::write(r.join("out/fit/r1/figures/f.png"), "png").unwrap();
        d
    }

    #[test]
    fn tree_respects_gitignore_and_shows_out_collapsed() {
        let d = repo();
        let t = tree(d.path(), None, Some(3), "out").unwrap();
        let names: Vec<&str> = t.iter().map(|e| e.path.as_str()).collect();
        assert!(names.contains(&"scripts"));
        assert!(names.contains(&"analysis"));
        assert!(names.contains(&".gitignore"));
        assert!(!names.contains(&".venv"), "{names:?}");
        assert!(!names.contains(&".git"));
        let out = t.iter().find(|e| e.path == "out").expect("out shown");
        assert_eq!(out.kind, FileKind::Dir);
        assert!(out.children.is_none(), "out is collapsed");
        let analysis = t.iter().find(|e| e.path == "analysis").unwrap();
        let kids: Vec<&str> = analysis
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(kids, vec!["analysis/__init__.py"]);
        let scripts = t.iter().find(|e| e.path == "scripts").unwrap();
        let kids: Vec<&str> = scripts
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(kids, vec!["scripts/a.py"], "pyc ignored");
        // Explicitly listing inside out/ works.
        let inside = tree(d.path(), Some("out/fit/r1/figures"), None, "out").unwrap();
        assert_eq!(inside[0].path, "out/fit/r1/figures/f.png");
        assert_eq!(inside[0].size, Some(3));
        assert_eq!(
            tree(d.path(), Some("nope"), None, "out").unwrap_err().code,
            IpcErrorCode::NotFound
        );
    }

    #[test]
    fn write_detects_conflicts() {
        let d = repo();
        let c = read(d.path(), "scripts/a.py").unwrap();
        assert_eq!(c.content, "print('hi')\n");
        let w = write(d.path(), "scripts/a.py", "x = 1\n", Some(&c.hash)).unwrap();
        assert_eq!(w.hash, sha256_hex(b"x = 1\n"));
        let err = write(d.path(), "scripts/a.py", "y\n", Some(&c.hash)).unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Conflict);
        assert_eq!(err.detail.unwrap()["path"], "scripts/a.py");
        write(d.path(), "scripts/a.py", "z\n", Some(&w.hash)).unwrap();
        assert_eq!(read(d.path(), "scripts/a.py").unwrap().content, "z\n");
        // New file: no expected hash, or an empty one.
        write(d.path(), "new/dir/f.py", "n\n", None).unwrap();
        let err = write(d.path(), "new/dir/g.py", "n\n", Some("abc")).unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Conflict);
        write(d.path(), "new/dir/g.py", "n\n", Some("")).unwrap();
        assert_eq!(
            read(d.path(), "../etc/passwd").unwrap_err().code,
            IpcErrorCode::Invalid
        );
        std::fs::write(d.path().join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            read(d.path(), "bin.dat").unwrap_err().code,
            IpcErrorCode::Invalid
        );
        assert_eq!(read_binary(d.path(), "bin.dat").unwrap().1.len(), 3);
    }

    #[test]
    fn create_rename_delete() {
        let d = repo();
        create(d.path(), "docs", true).unwrap();
        assert!(d.path().join("docs").is_dir());
        create(d.path(), "docs/x.md", false).unwrap();
        assert_eq!(
            create(d.path(), "docs/x.md", false).unwrap_err().code,
            IpcErrorCode::Conflict
        );
        rename(d.path(), "docs/x.md", "docs/y.md").unwrap();
        assert!(d.path().join("docs/y.md").is_file());
        delete(d.path(), "docs/y.md").unwrap();
        assert!(!d.path().join("docs/y.md").exists());
        assert_eq!(
            delete(d.path(), "docs/y.md").unwrap_err().code,
            IpcErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn search_reports_columns() {
        let d = repo();
        let hits = search(d.path(), "hi", None, None).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "scripts/a.py");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[0].col, 8);
        let none = search(d.path(), "hi", Some("*.md"), None).await.unwrap();
        assert!(none.is_empty());
    }
}
