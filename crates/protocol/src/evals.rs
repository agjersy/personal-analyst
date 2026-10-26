//! Evaluation task definitions and scorecards (PROTOCOL 8).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Plan,
    #[default]
    Build,
    Run,
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "plan" => Ok(Mode::Plan),
            "build" => Ok(Mode::Build),
            "run" => Ok(Mode::Run),
            other => Err(format!("unknown mode `{other}` (expected plan|build|run)")),
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mode::Plan => "plan",
            Mode::Build => "build",
            Mode::Run => "run",
        })
    }
}

/// `task.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskFile {
    pub task: TaskSection,
    #[serde(default)]
    pub fixture: FixtureSection,
    #[serde(default)]
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSection {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
}

fn default_timeout() -> u64 {
    900
}
fn default_max_steps() -> u32 {
    40
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FixtureSection {
    pub dir: String,
}

impl Default for FixtureSection {
    fn default() -> Self {
        FixtureSection {
            dir: "fixture".to_string(),
        }
    }
}

fn default_min_count() -> u64 {
    1
}

/// Every check kind listed in PROTOCOL 8.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    ContractClean,
    ScriptExists {
        path: String,
    },
    RunSucceeds {
        script: String,
        #[serde(default)]
        args: Vec<String>,
    },
    ArtifactExists {
        glob: String,
        #[serde(default = "default_min_count")]
        min_count: u64,
    },
    EqVerified {
        path: String,
        function: String,
    },
    NoNewDuplicates,
    EditsWithinScope,
    FileMatches {
        path: String,
        regex: String,
    },
    NoEdits,
    RunsAtLeast {
        script: String,
        count: u64,
    },
}

impl Check {
    pub fn kind(&self) -> &'static str {
        match self {
            Check::ContractClean => "contract_clean",
            Check::ScriptExists { .. } => "script_exists",
            Check::RunSucceeds { .. } => "run_succeeds",
            Check::ArtifactExists { .. } => "artifact_exists",
            Check::EqVerified { .. } => "eq_verified",
            Check::NoNewDuplicates => "no_new_duplicates",
            Check::EditsWithinScope => "edits_within_scope",
            Check::FileMatches { .. } => "file_matches",
            Check::NoEdits => "no_edits",
            Check::RunsAtLeast { .. } => "runs_at_least",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckResult {
    pub kind: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskResult {
    pub name: String,
    pub passed: bool,
    pub checks: Vec<CheckResult>,
    pub steps: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_s: f64,
    pub unwanted_edits: u32,
    /// Tool calls that returned an error (feeds `tool_error_rate`).
    #[serde(default)]
    pub tool_errors: u32,
    #[serde(default)]
    pub tool_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ScorecardSummary {
    pub pass_rate: f64,
    pub contract_rate: f64,
    pub eq_rate: f64,
    pub unwanted_edit_rate: f64,
    pub tool_error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scorecard {
    pub model: String,
    pub tasks: Vec<TaskResult>,
    pub summary: ScorecardSummary,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_file_parses_documented_example() {
        let text = r#"
[task]
name = "fit-double-exponential"
prompt = "..."
mode = "build"
timeout_s = 900
max_steps = 40

[fixture]
dir = "fixture"

[[checks]]
kind = "contract_clean"
[[checks]]
kind = "script_exists"
path = "scripts/fit_decay.py"
[[checks]]
kind = "run_succeeds"
script = "scripts/fit_decay.py"
args = ["--help"]
[[checks]]
kind = "artifact_exists"
glob = "out/fit_decay/*/figures/*.png"
min_count = 2
[[checks]]
kind = "eq_verified"
path = "analysis/fitting.py"
function = "double_exp"
[[checks]]
kind = "no_new_duplicates"
[[checks]]
kind = "edits_within_scope"
[[checks]]
kind = "file_matches"
path = "analysis/fitting.py"
regex = "def double_exp\\("
[[checks]]
kind = "no_edits"
[[checks]]
kind = "runs_at_least"
script = "scripts/decay_sim.py"
count = 3
[[checks]]
kind = "artifact_exists"
glob = "out/*.png"
"#;
        let t: TaskFile = toml::from_str(text).unwrap();
        assert_eq!(t.task.mode, Mode::Build);
        assert_eq!(t.checks.len(), 11);
        assert_eq!(t.checks[0], Check::ContractClean);
        assert_eq!(
            t.checks[3],
            Check::ArtifactExists {
                glob: "out/fit_decay/*/figures/*.png".into(),
                min_count: 2
            }
        );
        assert_eq!(
            t.checks[9],
            Check::RunsAtLeast {
                script: "scripts/decay_sim.py".into(),
                count: 3
            }
        );
        assert!(matches!(
            t.checks[10],
            Check::ArtifactExists { min_count: 1, .. }
        ));
        assert_eq!(t.checks[7].kind(), "file_matches");
    }

    #[test]
    fn scorecard_round_trip() {
        let sc = Scorecard {
            model: "p/m".into(),
            tasks: vec![TaskResult {
                name: "t".into(),
                passed: true,
                checks: vec![CheckResult {
                    kind: "contract_clean".into(),
                    passed: true,
                    detail: "0 errors".into(),
                }],
                steps: 3,
                input_tokens: 10,
                output_tokens: 5,
                wall_s: 1.5,
                unwanted_edits: 0,
                tool_errors: 0,
                tool_calls: 2,
            }],
            summary: ScorecardSummary {
                pass_rate: 1.0,
                ..Default::default()
            },
        };
        let s = serde_json::to_string(&sc).unwrap();
        let back: Scorecard = serde_json::from_str(&s).unwrap();
        assert_eq!(back, sc);
    }
}
