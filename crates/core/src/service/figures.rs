//! Figure discovery: `out/<script>/<run>/**` (figures and their sidecars), including terminal
//! runs found only through their `run.json`, plus artifacts pulled from remote targets into the
//! session's `artifacts/<run_id>/` directory.

use analyst_protocol::kernel::{ArtifactKind, FigureSidecar};
use analyst_protocol::ui::{FigureFormat, FigureView};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// `run.json` as written by the kernel (PROTOCOL 1.4 step 3) or by the terminal watcher.
#[derive(Debug, Clone, Deserialize)]
pub struct RunJson {
    pub run_id: String,
    pub script: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub tree_hash: Option<String>,
}

/// One run directory under `out/`.
#[derive(Debug, Clone)]
pub struct ScannedRun {
    pub run_id: String,
    pub script: String,
    pub args: Vec<String>,
    pub started_at: String,
    pub host: Option<String>,
    /// Repo-relative run output directory, e.g. `out/fit_decay/<run>`.
    pub dir_rel: String,
    pub has_run_json: bool,
    pub figures: Vec<FigureView>,
}

pub fn read_run_json(path: &Path) -> Option<RunJson> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn mtime_rfc3339(p: &Path) -> String {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| {
            chrono::DateTime::<chrono::Utc>::from(t)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_default()
}

fn to_rel(repo: &Path, p: &Path) -> String {
    p.strip_prefix(repo)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Build a `FigureView` for a figure file (sidecar read from `<stem>.json` next to it).
pub fn figure_view(repo: &Path, file: &Path, run_id: &str, script: &str) -> Option<FigureView> {
    let rel = to_rel(repo, file);
    let format = FigureFormat::from_path(&rel)?;
    let sidecar_path = file.with_extension("json");
    let sidecar: Option<FigureSidecar> = std::fs::read_to_string(&sidecar_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let meta = std::fs::metadata(file).ok()?;
    let name = file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let created_at = sidecar
        .as_ref()
        .map(|s| s.created_at.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| mtime_rfc3339(file));
    Some(FigureView {
        run_id: run_id.to_string(),
        script: script.to_string(),
        path: rel,
        name,
        format,
        sidecar,
        created_at,
        size: meta.len(),
    })
}

/// Figure files under a directory, sorted by path (skips `_`-prefixed directories).
pub fn figure_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && e.depth() > 0
                && e.file_name().to_string_lossy().starts_with('_'))
        })
        .flatten()
    {
        if e.file_type().is_file()
            && ArtifactKind::from_path(&e.path().to_string_lossy()) == ArtifactKind::Figure
        {
            out.push(e.path().to_path_buf());
        }
    }
    out.sort();
    out
}

/// Scan a single run directory `out/<script>/<run>/`.
pub fn scan_run_dir(repo: &Path, dir: &Path) -> Option<ScannedRun> {
    if !dir.is_dir() {
        return None;
    }
    let dir_rel = to_rel(repo, dir);
    let run_json = read_run_json(&dir.join("run.json"));
    let dir_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent_name = dir
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (run_id, script, args, started_at, host) = match &run_json {
        Some(r) => (
            r.run_id.clone(),
            r.script.clone(),
            r.args.clone(),
            r.started_at.clone().unwrap_or_else(|| mtime_rfc3339(dir)),
            r.host.clone(),
        ),
        None => (
            format!("{parent_name}-{dir_name}"),
            parent_name.clone(),
            vec![],
            mtime_rfc3339(dir),
            None,
        ),
    };
    let figures = figure_files(dir)
        .iter()
        .filter_map(|f| figure_view(repo, f, &run_id, &script))
        .collect();
    Some(ScannedRun {
        run_id,
        script,
        args,
        started_at,
        host,
        dir_rel,
        has_run_json: run_json.is_some(),
        figures,
    })
}

/// Every run directory under `out/` (two levels: `<script>/<run>`), newest first by
/// `started_at`, including terminal runs and directories that only hold figures.
pub fn scan_out(repo: &Path, out_dir: &str) -> Vec<ScannedRun> {
    let out = repo.join(out_dir);
    let mut runs = Vec::new();
    let Ok(scripts) = std::fs::read_dir(&out) else {
        return runs;
    };
    for s in scripts.flatten() {
        let sp = s.path();
        let sname = s.file_name().to_string_lossy().to_string();
        if !sp.is_dir() || sname.starts_with('_') || sname.starts_with('.') {
            continue;
        }
        let Ok(run_dirs) = std::fs::read_dir(&sp) else {
            continue;
        };
        for r in run_dirs.flatten() {
            let rp = r.path();
            let rname = r.file_name().to_string_lossy().to_string();
            if !rp.is_dir() || rname.starts_with('_') || rname.starts_with('.') {
                continue;
            }
            if let Some(scanned) = scan_run_dir(repo, &rp) {
                if scanned.has_run_json || !scanned.figures.is_empty() {
                    runs.push(scanned);
                }
            }
        }
    }
    runs.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then(b.run_id.cmp(&a.run_id))
    });
    runs
}

/// Figures pulled from a remote target into `artifacts/<run_id>/<path>`.
pub fn scan_pulled(artifacts_dir: &Path, run_id: &str, script: &str) -> Vec<FigureView> {
    let base = artifacts_dir.join(run_id);
    if !base.is_dir() {
        return vec![];
    }
    figure_files(&base)
        .iter()
        .filter_map(|f| {
            let mut v = figure_view(&base, f, run_id, script)?;
            // Path stays repo-relative (the layout under artifacts/<run_id>/ mirrors the repo).
            v.path = to_rel(&base, f);
            Some(v)
        })
        .collect()
}

/// Which run directory (`out/<script>/<run>`) a repo-relative path belongs to, if any.
pub fn run_dir_of(out_dir: &str, rel: &str) -> Option<String> {
    let prefix = format!("{}/", out_dir.trim_matches('/'));
    let rest = rel.strip_prefix(&prefix)?;
    let mut parts = rest.split('/');
    let script = parts.next()?;
    let run = parts.next()?;
    if script.starts_with('_') || run.starts_with('_') {
        return None;
    }
    // Need at least one more component (a file inside the run dir) or the run dir itself.
    Some(format!("{}{script}/{run}", prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_runs_with_and_without_run_json() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path();
        let r1 = repo.join("out/fit/20260101-000000-aaaaaa/figures");
        std::fs::create_dir_all(&r1).unwrap();
        std::fs::write(r1.join("residuals.png"), b"png").unwrap();
        std::fs::write(
            r1.join("residuals.json"),
            serde_json::json!({
                "run_id": "20260101-000000-aaaaaa", "script": "scripts/fit.py", "args": [],
                "title": "Residuals", "axes": [], "created_at": "2026-01-01T00:00:01Z",
                "width_in": 6.4, "height_in": 4.8, "dpi": 100.0
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            repo.join("out/fit/20260101-000000-aaaaaa/run.json"),
            serde_json::json!({"run_id":"20260101-000000-aaaaaa","script":"scripts/fit.py","args":["--n","3"],"started_at":"2026-01-01T00:00:00Z","host":"h","tree_hash":null}).to_string(),
        )
        .unwrap();
        let t = repo.join("out/terminal/20260102-000000/figures");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("fig-1.svg"), b"<svg/>").unwrap();
        std::fs::create_dir_all(repo.join("out/_repl/x")).unwrap();
        std::fs::write(repo.join("out/_repl/x/f.png"), b"x").unwrap();
        std::fs::create_dir_all(repo.join("out/empty/r")).unwrap();

        let runs = scan_out(repo, "out");
        assert_eq!(runs.len(), 2, "{runs:?}");
        let fit = runs.iter().find(|r| r.script == "scripts/fit.py").unwrap();
        assert_eq!(fit.args, vec!["--n", "3"]);
        assert!(fit.has_run_json);
        assert_eq!(fit.figures.len(), 1);
        let f = &fit.figures[0];
        assert_eq!(
            f.path,
            "out/fit/20260101-000000-aaaaaa/figures/residuals.png"
        );
        assert_eq!(f.format, FigureFormat::Png);
        assert_eq!(
            f.sidecar.as_ref().unwrap().title.as_deref(),
            Some("Residuals")
        );
        assert_eq!(f.created_at, "2026-01-01T00:00:01Z");
        let term = runs.iter().find(|r| r.script == "terminal").unwrap();
        assert!(!term.has_run_json);
        assert_eq!(term.run_id, "terminal-20260102-000000");
        assert_eq!(term.figures[0].format, FigureFormat::Svg);
        assert_eq!(
            run_dir_of("out", "out/fit/r1/figures/a.png").as_deref(),
            Some("out/fit/r1")
        );
        assert_eq!(run_dir_of("out", "out/_repl/r1/a.png"), None);
        assert_eq!(run_dir_of("out", "scripts/a.py"), None);
    }
}
