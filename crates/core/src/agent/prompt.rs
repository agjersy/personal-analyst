//! System prompt and project map assembly (ARCHITECTURE 5.7, context assembly steps 1 and 2).

use crate::tools::ToolContext;
use analyst_protocol::config::ProjectConfig;
use analyst_protocol::evals::Mode;
use serde_json::{json, Value};

pub const SYSTEM_ROLE: &str = "You are Personal Analyst, an agent that writes and runs reusable Python analysis tools inside a git repository under the user's supervision. You work in a plan / propose / run / verify cycle, keep changes small and reviewable, and never pretend a step succeeded.";

/// The script contract rules as told to the model (ARCHITECTURE 5.13).
pub fn contract_rules(project: &ProjectConfig) -> String {
    let c = &project.contract;
    let p = &project.project;
    format!(
        "SCRIPT CONTRACT (checked automatically after every edit; fix violations before finishing):\n\
1. Every script under {scripts} is a command-line tool: it builds an argparse.ArgumentParser with a description, gives help= text to every option, defines a top-level `def main(` and has an `if __name__ == \"__main__\":` guard. Every input (data path, parameter, output location, seed, plot options) is an option with a default; path defaults are repo-relative; no absolute paths or hard-coded literals flow from main into library calls.\n\
2. Shared logic lives in the library package `{lib}/`, not in scripts. A script may define only {allowed} plus small private helpers (`_name`, at most {max_helper} lines, no use of {numerical}). No two functions anywhere may implement the same logic (duplicate and near-duplicate detection at similarity {thr}); consult lib.index and fs.search before writing a new function, and extend or call an existing library function when one covers the need.\n\
3. Outputs go under `{out}/<script stem>/` (the run output directory is `$ANALYST_OUT_DIR`); figures get descriptive names; plt.show() and savefig() are captured automatically, so prefer plt.show() or save under $ANALYST_OUT_DIR. Random seeds are options with dest `seed`. Library functions have docstrings; functions in modules matching {math_req:?} carry a `Math:` docstring section with lines `<lhs> = <expr>` in SymPy syntax using the parameter names (last line is the result).\n\
4. Read before editing; edits with `edits` must match the current file text exactly once. After editing, lint and contract diagnostics are fed back to you; after runs, failures are fed back with stderr.",
        scripts = p.scripts.join(", "),
        lib = p.library,
        allowed = c.script_allowed_functions.join(", "),
        max_helper = c.max_helper_lines,
        numerical = c.numerical_modules.join("/"),
        thr = c.near_duplicate_threshold,
        out = p.out_dir,
        math_req = c.math_required,
    )
}

pub fn mode_instructions(mode: Mode) -> &'static str {
    match mode {
        Mode::Plan => "MODE plan: read-only. Investigate and propose a plan; you cannot edit or run anything.",
        Mode::Build => "MODE build: before your first edit on a task, call declare_scope with the list of files you intend to create or modify and a one-paragraph plan. Edits outside the declared scope require user approval. Verify your work by running the script and reading the result before you finish.",
        Mode::Run => "MODE run: you may run existing scripts and snippets and inspect outputs, but you cannot edit files or install packages. Do not propose code changes; report results.",
    }
}

pub const OUTPUT_CONVENTIONS: &str = "WORKING STYLE: use tools rather than guessing; keep replies short and factual; when a run fails, read stderr, fix, and rerun; when you finish, state what changed, what ran, and what to look at. Tool results larger than 16 KiB are truncated and stored as an artifact you can page with fs.read.";

/// Build the project map (directory tree, script index, library index, environment, target,
/// git status, user-changed files).
pub async fn project_map(ctx: &ToolContext, user_changed: &[String]) -> String {
    let mut s = String::from("PROJECT MAP\n");
    if let Ok(v) = ctx
        .dispatch("map-tree", "fs.list", &json!({"depth": 2}))
        .await
    {
        let entries: Vec<String> = v["entries"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let shown: Vec<&String> = entries.iter().take(150).collect();
        s.push_str(&format!(
            "Tree (depth 2): {}\n",
            shown
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        if entries.len() > 150 {
            s.push_str(&format!("... {} more entries\n", entries.len() - 150));
        }
    }
    match ctx
        .dispatch("map-scripts", "scripts.index", &json!({}))
        .await
    {
        Ok(v) => {
            s.push_str("Scripts:\n");
            for sc in v["scripts"].as_array().unwrap_or(&vec![]) {
                let opts: Vec<String> = sc["options"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|o| {
                                let flag = o["flags"]
                                    .as_array()
                                    .and_then(|f| f.last())
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                format!("{flag}={}", compact(&o["default"]))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                s.push_str(&format!(
                    "- {} : {} [{}]\n",
                    sc["path"].as_str().unwrap_or(""),
                    sc["description"].as_str().unwrap_or("(no description)"),
                    opts.join(" ")
                ));
            }
        }
        Err(e) => s.push_str(&format!("Scripts: unavailable ({})\n", e.message)),
    }
    match ctx.dispatch("map-lib", "lib.index", &json!({})).await {
        Ok(v) => {
            s.push_str("Library functions:\n");
            for f in v["functions"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .take(200)
            {
                s.push_str(&format!(
                    "- {}.{}{} : {}{}\n",
                    f["module"].as_str().unwrap_or(""),
                    f["qualname"].as_str().unwrap_or(""),
                    f["signature"].as_str().unwrap_or("()"),
                    f["doc_summary"].as_str().unwrap_or("(no docstring)"),
                    if f["has_math"].as_bool().unwrap_or(false) {
                        " [Math]"
                    } else {
                        ""
                    }
                ));
            }
        }
        Err(e) => s.push_str(&format!("Library: unavailable ({})\n", e.message)),
    }
    if let Ok(v) = ctx.dispatch("map-target", "target.info", &json!({})).await {
        s.push_str(&format!(
            "Target: {} ({}), connected={}, sandbox={}, python={}, data_dirs={}\n",
            v["name"].as_str().unwrap_or("local"),
            v["kind"].as_str().unwrap_or(""),
            v["connected"],
            v["sandbox"].as_str().unwrap_or("unknown"),
            v["python"].as_str().unwrap_or("unknown"),
            compact(&v["data_dirs"]),
        ));
    }
    if let Ok(v) = ctx.dispatch("map-git", "git.status", &json!({})).await {
        let st = v["status"].as_str().unwrap_or("").trim();
        let lines: Vec<&str> = st.lines().take(40).collect();
        s.push_str(&format!(
            "Git status:\n{}\n",
            if lines.is_empty() { "(clean)" } else { "" }
        ));
        for l in lines {
            s.push_str(l);
            s.push('\n');
        }
    }
    if !user_changed.is_empty() {
        s.push_str(&format!(
            "Files changed by the user since the last turn: {}\n",
            user_changed.join(", ")
        ));
    }
    s
}

fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}
