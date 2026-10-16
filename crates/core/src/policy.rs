//! Policy engine (ARCHITECTURE 5.8): `(tool, args) -> Allow | Ask | Deny` from the project
//! policy table, the conversation mode, protected globs, the declared edit scope, and
//! `# analyst: lock` / `# analyst: unlock` protected regions.

use crate::tools::registry::{tool_class, ToolClass};
use analyst_protocol::config::{AskPolicy, EditPolicy, NetworkPolicy, PolicyConfig, RunPolicy};
use analyst_protocol::evals::Mode;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub decision: Decision,
    pub reason: String,
}

impl Evaluation {
    fn allow(reason: impl Into<String>) -> Self {
        Evaluation {
            decision: Decision::Allow,
            reason: reason.into(),
        }
    }
    fn ask(reason: impl Into<String>) -> Self {
        Evaluation {
            decision: Decision::Ask,
            reason: reason.into(),
        }
    }
    fn deny(reason: impl Into<String>) -> Self {
        Evaluation {
            decision: Decision::Deny,
            reason: reason.into(),
        }
    }
}

pub const LOCK_MARKER: &str = "analyst: lock";
pub const UNLOCK_MARKER: &str = "analyst: unlock";

/// Policy state for one conversation.
#[derive(Debug, Clone)]
pub struct Policy {
    pub config: PolicyConfig,
    pub mode: Mode,
    repo: PathBuf,
    protected: GlobSet,
    /// Files the agent declared it intends to touch (`declare_scope`). `None` = not declared.
    pub scope: Option<Vec<String>>,
}

impl Policy {
    pub fn new(repo: &Path, config: PolicyConfig, mode: Mode) -> anyhow::Result<Policy> {
        let mut b = GlobSetBuilder::new();
        for g in &config.protected {
            b.add(Glob::new(g).map_err(|e| anyhow::anyhow!("protected glob `{g}`: {e}"))?);
        }
        let protected = b.build()?;
        Ok(Policy {
            config,
            mode,
            repo: repo.to_path_buf(),
            protected,
            scope: None,
        })
    }

    pub fn declare_scope(&mut self, files: Vec<String>) {
        self.scope = Some(
            files
                .into_iter()
                .filter_map(|f| crate::util::normalize_rel_path(&f).ok())
                .collect(),
        );
    }

    pub fn is_protected(&self, path: &str) -> bool {
        self.protected.is_match(path)
    }

    fn in_scope(&self, path: &str) -> bool {
        match &self.scope {
            None => false,
            Some(s) => s.iter().any(|f| f == path),
        }
    }

    /// Whether the mode makes a tool available at all.
    pub fn tool_available(&self, tool: &str) -> bool {
        let Some(class) = tool_class(tool) else {
            return false;
        };
        match self.mode {
            Mode::Build => true,
            Mode::Plan => matches!(class, ToolClass::Read | ToolClass::Meta),
            Mode::Run => {
                matches!(class, ToolClass::Read | ToolClass::Meta)
                    || matches!(tool, "run.script" | "exec.snippet")
            }
        }
    }

    /// One-paragraph summary for the system prompt.
    pub fn summary(&self) -> String {
        let c = &self.config;
        let edits = match c.edits {
            EditPolicy::Review => "every edit pauses for review",
            EditPolicy::AutoNewFiles => {
                "new files are written immediately, edits to existing files pause for review"
            }
            EditPolicy::Auto => "in-scope edits apply immediately; out-of-scope edits pause",
        };
        let runs = match c.runs {
            RunPolicy::Auto => "runs start without asking",
            RunPolicy::Ask => "runs ask first",
        };
        let ask = |p: AskPolicy| match p {
            AskPolicy::Ask => "ask",
            AskPolicy::Auto => "auto",
            AskPolicy::Deny => "denied",
        };
        let net = match c.network {
            NetworkPolicy::Deny => "denied",
            NetworkPolicy::Ask => "ask",
            NetworkPolicy::Allow => "allowed",
        };
        let mut s = format!(
            "Mode: {}. Edits: {edits}. Runs: {runs}. Packages: {}. Shell: {}. Git commits: {}. Network for runs: {net}. Max steps per turn: {}.",
            self.mode,
            ask(c.packages),
            ask(c.shell),
            ask(c.git),
            c.max_steps
        );
        if !c.protected.is_empty() {
            s.push_str(&format!(
                " Protected (never editable): {}.",
                c.protected.join(", ")
            ));
        }
        match &self.scope {
            Some(files) if !files.is_empty() => {
                s.push_str(&format!(" Declared scope: {}.", files.join(", ")))
            }
            _ if self.mode == Mode::Build => s.push_str(
                " No scope declared yet: call declare_scope before editing, or every edit will need approval.",
            ),
            _ => {}
        }
        s
    }

    /// Evaluate a tool call.
    pub fn evaluate(&self, tool: &str, args: &Value) -> Evaluation {
        let Some(class) = tool_class(tool) else {
            return Evaluation::deny(format!("unknown tool `{tool}`"));
        };
        if !self.tool_available(tool) {
            return Evaluation::deny(format!(
                "tool `{tool}` is not available in {} mode",
                self.mode
            ));
        }
        match class {
            ToolClass::Read | ToolClass::Meta => Evaluation::allow("read-only"),
            ToolClass::Edit => self.evaluate_edit(tool, args),
            ToolClass::Run => self.evaluate_run(args),
            ToolClass::Package => Self::from_ask(self.config.packages, "packages"),
            ToolClass::Shell => Self::from_ask(self.config.shell, "shell"),
            ToolClass::Git => Self::from_ask(self.config.git, "git"),
        }
    }

    fn from_ask(p: AskPolicy, what: &str) -> Evaluation {
        match p {
            AskPolicy::Auto => Evaluation::allow(format!("{what} policy is auto")),
            AskPolicy::Ask => Evaluation::ask(format!("{what} policy is ask")),
            AskPolicy::Deny => Evaluation::deny(format!("{what} policy is deny")),
        }
    }

    fn evaluate_run(&self, args: &Value) -> Evaluation {
        let wants_network = args
            .get("network")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if wants_network {
            match self.config.network {
                NetworkPolicy::Deny => {
                    return Evaluation::deny("network access is denied by policy")
                }
                NetworkPolicy::Ask => return Evaluation::ask("network access requires approval"),
                NetworkPolicy::Allow => {}
            }
        }
        match self.config.runs {
            RunPolicy::Auto => Evaluation::allow("runs policy is auto"),
            RunPolicy::Ask => Evaluation::ask("runs policy is ask"),
        }
    }

    fn evaluate_edit(&self, tool: &str, args: &Value) -> Evaluation {
        let raw = args.get("path").and_then(Value::as_str).unwrap_or("");
        let path = match crate::util::normalize_rel_path(raw) {
            Ok(p) => p,
            Err(e) => return Evaluation::deny(e),
        };
        if self.is_protected(&path) {
            return Evaluation::deny(format!("`{path}` matches a protected pattern"));
        }
        // Protected regions inside the existing file.
        if tool != "fs.propose_create" {
            if let Ok(existing) = std::fs::read_to_string(self.repo.join(&path)) {
                if let Some(reason) = edit_touches_locked_region(tool, &existing, args) {
                    return Evaluation::deny(reason);
                }
            }
        }
        let is_new = tool == "fs.propose_create";
        match self.config.edits {
            EditPolicy::Review => Evaluation::ask("edits policy is review"),
            EditPolicy::AutoNewFiles if is_new => {
                Evaluation::allow("new file under auto-new-files")
            }
            EditPolicy::AutoNewFiles => Evaluation::ask("existing file under auto-new-files"),
            EditPolicy::Auto => {
                if self.in_scope(&path) {
                    Evaluation::allow("in declared scope under auto")
                } else if self.scope.is_none() {
                    Evaluation::ask("no scope declared; edit requires approval")
                } else {
                    Evaluation::ask(format!("`{path}` is outside the declared scope"))
                }
            }
        }
    }
}

/// Locked line ranges (1-based, inclusive, markers included). An unterminated lock runs to EOF.
pub fn locked_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    let mut last = 0;
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        last = n;
        let t = line.trim();
        let is_comment = t.starts_with('#') || t.starts_with("//") || t.starts_with("--");
        if !is_comment {
            continue;
        }
        if t.contains(UNLOCK_MARKER) {
            if let Some(s) = start.take() {
                ranges.push((s, n));
            }
        } else if t.contains(LOCK_MARKER) && start.is_none() {
            start = Some(n);
        }
    }
    if let Some(s) = start {
        ranges.push((s, last.max(s)));
    }
    ranges
}

/// Text of each locked block, verbatim.
pub fn locked_blocks(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    locked_ranges(text)
        .into_iter()
        .map(|(s, e)| lines[s - 1..e.min(lines.len())].join("\n"))
        .collect()
}

fn line_of_offset(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// Returns a denial reason when an edit proposal would modify a locked region.
pub fn edit_touches_locked_region(tool: &str, existing: &str, args: &Value) -> Option<String> {
    let ranges = locked_ranges(existing);
    if ranges.is_empty() {
        return None;
    }
    if tool == "fs.propose_delete" {
        return Some(
            "file contains a locked region (`# analyst: lock`) and cannot be deleted".into(),
        );
    }
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        for e in edits {
            let old = e.get("old").and_then(Value::as_str).unwrap_or("");
            if old.is_empty() {
                continue;
            }
            if let Some(pos) = existing.find(old) {
                let first = line_of_offset(existing, pos);
                let last = line_of_offset(existing, pos + old.len().saturating_sub(1));
                for (s, en) in &ranges {
                    if first <= *en && last >= *s {
                        return Some(format!(
                            "edit touches locked region lines {s}-{en} (`# analyst: lock`)"
                        ));
                    }
                }
            }
        }
        return None;
    }
    if let Some(content) = args.get("content").and_then(Value::as_str) {
        for block in locked_blocks(existing) {
            if !content.contains(&block) {
                return Some(
                    "replacement content changes a locked region (`# analyst: lock`)".to_string(),
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(edits: EditPolicy, mode: Mode, repo: &Path) -> Policy {
        let cfg = PolicyConfig {
            edits,
            protected: vec!["data/**".into()],
            ..Default::default()
        };
        Policy::new(repo, cfg, mode).unwrap()
    }

    #[test]
    fn read_tools_allowed_everywhere_edits_follow_table() {
        let dir = tempfile::tempdir().unwrap();
        let p = policy(EditPolicy::Review, Mode::Build, dir.path());
        assert_eq!(
            p.evaluate("fs.read", &json!({"path":"a"})).decision,
            Decision::Allow
        );
        assert_eq!(
            p.evaluate(
                "fs.propose_edit",
                &json!({"path":"scripts/a.py","content":"x"})
            )
            .decision,
            Decision::Ask
        );
        assert_eq!(
            p.evaluate(
                "fs.propose_create",
                &json!({"path":"data/x.csv","content":"x"})
            )
            .decision,
            Decision::Deny
        );
        assert_eq!(p.evaluate("pkg.add", &json!({})).decision, Decision::Ask);
        assert_eq!(
            p.evaluate("run.script", &json!({"script":"s"})).decision,
            Decision::Allow
        );
        assert_eq!(
            p.evaluate("run.script", &json!({"script":"s","network":true}))
                .decision,
            Decision::Deny
        );
        assert_eq!(p.evaluate("nope", &json!({})).decision, Decision::Deny);
    }

    #[test]
    fn modes_restrict_tool_sets() {
        let dir = tempfile::tempdir().unwrap();
        let plan = policy(EditPolicy::Auto, Mode::Plan, dir.path());
        assert_eq!(
            plan.evaluate("fs.propose_create", &json!({"path":"a.py","content":""}))
                .decision,
            Decision::Deny
        );
        assert_eq!(
            plan.evaluate("run.script", &json!({})).decision,
            Decision::Deny
        );
        let run = policy(EditPolicy::Auto, Mode::Run, dir.path());
        assert_eq!(
            run.evaluate("run.script", &json!({})).decision,
            Decision::Allow
        );
        assert_eq!(
            run.evaluate("exec.snippet", &json!({})).decision,
            Decision::Allow
        );
        assert_eq!(run.evaluate("pkg.add", &json!({})).decision, Decision::Deny);
        assert_eq!(
            run.evaluate("fs.propose_edit", &json!({"path":"a.py"}))
                .decision,
            Decision::Deny
        );
    }

    #[test]
    fn auto_mode_respects_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = policy(EditPolicy::Auto, Mode::Build, dir.path());
        let args = json!({"path":"scripts/a.py","content":"x"});
        assert_eq!(p.evaluate("fs.propose_edit", &args).decision, Decision::Ask);
        p.declare_scope(vec!["scripts/a.py".into()]);
        assert_eq!(
            p.evaluate("fs.propose_edit", &args).decision,
            Decision::Allow
        );
        let other = json!({"path":"scripts/b.py","content":"x"});
        let ev = p.evaluate("fs.propose_edit", &other);
        assert_eq!(ev.decision, Decision::Ask);
        assert!(ev.reason.contains("outside the declared scope"));
        let anf = policy(EditPolicy::AutoNewFiles, Mode::Build, dir.path());
        assert_eq!(
            anf.evaluate("fs.propose_create", &other).decision,
            Decision::Allow
        );
        assert_eq!(
            anf.evaluate("fs.propose_edit", &other).decision,
            Decision::Ask
        );
    }

    #[test]
    fn locked_regions_deny() {
        let dir = tempfile::tempdir().unwrap();
        let file = "import x\n# analyst: lock\nSECRET = 1\n# analyst: unlock\ny = 2\n";
        std::fs::create_dir_all(dir.path().join("analysis")).unwrap();
        std::fs::write(dir.path().join("analysis/core.py"), file).unwrap();
        assert_eq!(locked_ranges(file), vec![(2, 4)]);
        let mut p = policy(EditPolicy::Auto, Mode::Build, dir.path());
        p.declare_scope(vec!["analysis/core.py".into()]);
        let touching =
            json!({"path":"analysis/core.py","edits":[{"old":"SECRET = 1","new":"SECRET = 2"}]});
        assert_eq!(
            p.evaluate("fs.propose_edit", &touching).decision,
            Decision::Deny
        );
        let outside = json!({"path":"analysis/core.py","edits":[{"old":"y = 2","new":"y = 3"}]});
        assert_eq!(
            p.evaluate("fs.propose_edit", &outside).decision,
            Decision::Allow
        );
        let full_keep = json!({"path":"analysis/core.py","content":"import z\n# analyst: lock\nSECRET = 1\n# analyst: unlock\n"});
        assert_eq!(
            p.evaluate("fs.propose_edit", &full_keep).decision,
            Decision::Allow
        );
        let full_break = json!({"path":"analysis/core.py","content":"import z\n"});
        assert_eq!(
            p.evaluate("fs.propose_edit", &full_break).decision,
            Decision::Deny
        );
        assert_eq!(
            p.evaluate("fs.propose_delete", &json!({"path":"analysis/core.py"}))
                .decision,
            Decision::Deny
        );
        assert_eq!(locked_ranges("# analyst: lock\na\nb\n"), vec![(1, 3)]);
    }
}
