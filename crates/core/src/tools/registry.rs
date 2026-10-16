//! The phase 0 tool set with policy classes and model-facing JSON schemas.

use crate::providers::names::to_model_name;
use analyst_protocol::evals::Mode;
use analyst_protocol::model::ToolDef;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    Read,
    Edit,
    Run,
    Package,
    Shell,
    Git,
    /// Agent bookkeeping (`declare_scope`, `pin_fact`): always allowed, never gated.
    Meta,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub class: ToolClass,
    pub description: &'static str,
    pub parameters: Value,
}

impl ToolSpec {
    pub fn def(&self) -> ToolDef {
        ToolDef {
            name: to_model_name(self.name),
            description: self.description.to_string(),
            parameters: self.parameters.clone(),
        }
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

/// Every tool in the phase 0 set.
pub fn registry() -> Vec<ToolSpec> {
    use ToolClass::*;
    vec![
        ToolSpec { name: "declare_scope", class: Meta, description: "Declare the repo-relative files you intend to create or modify this turn. Required in build mode before any edit; edits outside the declared set need user approval.", parameters: obj(json!({"files": {"type":"array","items":{"type":"string"}}, "plan": {"type":"string","description":"One short paragraph describing the intended change."}}), &["files"]) },
        ToolSpec { name: "pin_fact", class: Meta, description: "Pin a short fact (decision, file name, parameter value) that must survive context compaction.", parameters: obj(json!({"fact": {"type":"string"}}), &["fact"]) },
        ToolSpec { name: "fs.read", class: Read, description: "Read a repo file (optionally a line range). Also reads session artifact files returned by truncated tool results.", parameters: obj(json!({"path":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}}), &["path"]) },
        ToolSpec { name: "fs.list", class: Read, description: "List files and directories under a repo path (depth-limited; ignores .git, .venv, out, __pycache__).", parameters: obj(json!({"path":{"type":"string"},"depth":{"type":"integer","minimum":1,"maximum":6}}), &[]) },
        ToolSpec { name: "fs.search", class: Read, description: "Search file contents with a regex (ripgrep). Returns path, line, and text per match.", parameters: obj(json!({"pattern":{"type":"string"},"glob":{"type":"string"},"max_results":{"type":"integer","minimum":1}}), &["pattern"]) },
        ToolSpec { name: "scripts.index", class: Read, description: "Index of command-line scripts: name, description, argparse options with defaults.", parameters: obj(json!({}), &[]) },
        ToolSpec { name: "lib.index", class: Read, description: "Index of public library functions with signatures, doc summaries, and call sites. Consult before writing a new function.", parameters: obj(json!({}), &[]) },
        ToolSpec { name: "runs.list", class: Read, description: "Recent runs on the target, newest first.", parameters: obj(json!({"limit":{"type":"integer","minimum":1}}), &[]) },
        ToolSpec { name: "runs.get", class: Read, description: "Metadata of one run.", parameters: obj(json!({"run_id":{"type":"string"}}), &["run_id"]) },
        ToolSpec { name: "out.list", class: Read, description: "List output files on the target for a run or path under out/.", parameters: obj(json!({"path":{"type":"string"},"run_id":{"type":"string"}}), &[]) },
        ToolSpec { name: "out.read", class: Read, description: "Bounded preview of an output file on the target (head of text/CSV, array summary).", parameters: obj(json!({"path":{"type":"string"},"max_bytes":{"type":"integer","minimum":1}}), &["path"]) },
        ToolSpec { name: "eq.extract", class: Read, description: "Extract equations (derived or declared) from a file or function.", parameters: obj(json!({"path":{"type":"string"},"function":{"type":"string"}}), &["path"]) },
        ToolSpec { name: "eq.check", class: Read, description: "Numerically check a function against its declared Math: equation.", parameters: obj(json!({"path":{"type":"string"},"function":{"type":"string"},"trials":{"type":"integer"},"tol":{"type":"number"}}), &["path","function"]) },
        ToolSpec { name: "contract.check", class: Read, description: "Run the script contract checker (default: all scripts and the library).", parameters: obj(json!({"paths":{"type":"array","items":{"type":"string"}}}), &[]) },
        ToolSpec { name: "contract.similar", class: Read, description: "Find duplicate and near-duplicate functions across scripts and the library.", parameters: obj(json!({"threshold":{"type":"number"}}), &[]) },
        ToolSpec { name: "lint.run", class: Read, description: "Run ruff on the given paths (default: all).", parameters: obj(json!({"paths":{"type":"array","items":{"type":"string"}}}), &[]) },
        ToolSpec { name: "fig.list", class: Read, description: "List figures produced by runs (all, or one run).", parameters: obj(json!({"run_id":{"type":"string"}}), &[]) },
        ToolSpec { name: "target.info", class: Read, description: "Name, kind, sandbox level, and data directories of the execution target.", parameters: obj(json!({}), &[]) },
        ToolSpec { name: "fs.propose_edit", class: Edit, description: "Edit a file: either `edits` (each `old` must match exactly once and is replaced by `new`) or full `content`.", parameters: obj(json!({"path":{"type":"string"},"edits":{"type":"array","items":{"type":"object","properties":{"old":{"type":"string"},"new":{"type":"string"}},"required":["old","new"]}},"content":{"type":"string"}}), &["path"]) },
        ToolSpec { name: "fs.propose_create", class: Edit, description: "Create a new file with the given content.", parameters: obj(json!({"path":{"type":"string"},"content":{"type":"string"}}), &["path","content"]) },
        ToolSpec { name: "fs.propose_delete", class: Edit, description: "Delete a file.", parameters: obj(json!({"path":{"type":"string"}}), &["path"]) },
        ToolSpec { name: "run.script", class: Run, description: "Run a script on the target with arguments; returns status, stdout/stderr tails, and artifacts.", parameters: obj(json!({"script":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"timeout_s":{"type":"number"},"network":{"type":"boolean"}}), &["script"]) },
        ToolSpec { name: "exec.snippet", class: Run, description: "Run a short Python snippet in the session environment (persistent REPL, sandboxed).", parameters: obj(json!({"code":{"type":"string"}}), &["code"]) },
        ToolSpec { name: "pkg.add", class: Package, description: "Add packages to the project environment with uv.", parameters: obj(json!({"packages":{"type":"array","items":{"type":"string"}}}), &["packages"]) },
        ToolSpec { name: "pkg.remove", class: Package, description: "Remove packages from the project environment with uv.", parameters: obj(json!({"packages":{"type":"array","items":{"type":"string"}}}), &["packages"]) },
        ToolSpec { name: "pkg.sync", class: Package, description: "Sync the environment to the lockfile.", parameters: obj(json!({}), &[]) },
        ToolSpec { name: "shell.run", class: Shell, description: "Run a shell command inside the target sandbox.", parameters: obj(json!({"command":{"type":"string"},"timeout_s":{"type":"number"}}), &["command"]) },
        ToolSpec { name: "git.status", class: Read, description: "git status of the local repo.", parameters: obj(json!({}), &[]) },
        ToolSpec { name: "git.diff", class: Read, description: "git diff of the working tree (optionally one path).", parameters: obj(json!({"path":{"type":"string"}}), &[]) },
        ToolSpec { name: "git.commit", class: Git, description: "Commit all working tree changes with a message.", parameters: obj(json!({"message":{"type":"string"}}), &["message"]) },
    ]
}

pub fn find(name: &str) -> Option<ToolSpec> {
    registry().into_iter().find(|t| t.name == name)
}

pub fn tool_class(name: &str) -> Option<ToolClass> {
    find(name).map(|t| t.class)
}

/// Internal ids of every tool.
pub fn names() -> Vec<&'static str> {
    registry().into_iter().map(|t| t.name).collect()
}

/// Internal id for a model-facing name.
pub fn internal_name(model_name: &str) -> Option<&'static str> {
    registry()
        .into_iter()
        .map(|t| t.name)
        .find(|n| to_model_name(n) == model_name)
}

/// Tool definitions available in a mode (ARCHITECTURE 5.7 table).
pub fn defs_for_mode(mode: Mode) -> Vec<ToolDef> {
    registry()
        .into_iter()
        .filter(|t| match mode {
            Mode::Build => true,
            Mode::Plan => matches!(t.class, ToolClass::Read | ToolClass::Meta),
            Mode::Run => {
                matches!(t.class, ToolClass::Read | ToolClass::Meta)
                    || matches!(t.name, "run.script" | "exec.snippet")
            }
        })
        .map(|t| t.def())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::names::is_valid_model_name;

    #[test]
    fn registry_is_well_formed() {
        let r = registry();
        for t in &r {
            assert!(is_valid_model_name(&t.def().name), "{}", t.name);
            assert_eq!(t.parameters["type"], "object");
        }
        assert_eq!(internal_name("fs_propose_edit"), Some("fs.propose_edit"));
        assert_eq!(tool_class("run.script"), Some(ToolClass::Run));
        let plan: Vec<String> = defs_for_mode(Mode::Plan)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(plan.contains(&"fs_read".to_string()));
        assert!(!plan.contains(&"run_script".to_string()));
        let run: Vec<String> = defs_for_mode(Mode::Run)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(run.contains(&"run_script".to_string()));
        assert!(!run.contains(&"fs_propose_edit".to_string()));
        assert_eq!(defs_for_mode(Mode::Build).len(), r.len());
    }
}
