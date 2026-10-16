//! Global `~/.analyst/config.toml` and project `<repo>/.analyst/project.toml` (PROTOCOL 2.1, 2.2).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- global config ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct GlobalConfig {
    pub defaults: Defaults,
    pub providers: Vec<ProviderConfig>,
    pub models: Vec<ModelConfig>,
    pub targets: Vec<TargetConfig>,
    /// Interpreter of the app tooling environment (hosts `analyst_kernel`); optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooling_python: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Defaults {
    /// `"<provider name>/<model id>"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub target: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            model: None,
            target: "local".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderKind {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    #[serde(rename = "anthropic")]
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

impl ProviderConfig {
    /// The effective base URL: the configured one or the provider kind's default.
    pub fn effective_base_url(&self) -> String {
        match (&self.base_url, self.kind) {
            (Some(u), _) => u.trim_end_matches('/').to_string(),
            (None, ProviderKind::Anthropic) => "https://api.anthropic.com".to_string(),
            (None, ProviderKind::OpenAiCompatible) => "http://localhost:11434/v1".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ToolSupport {
    Native,
    TextProtocol,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    pub provider: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    #[default]
    Local,
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TargetConfig {
    pub name: String,
    #[serde(default)]
    pub kind: TargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default)]
    pub data_dirs: Vec<String>,
}

// ---- project config ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub contract: ContractConfig,
    pub policy: PolicyConfig,
    /// Per-target overrides keyed by target name.
    pub targets: BTreeMap<String, TargetOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectSection {
    pub scripts: Vec<String>,
    pub library: String,
    pub out_dir: String,
    pub data_dirs: Vec<String>,
    pub timeout_s: u64,
}

impl Default for ProjectSection {
    fn default() -> Self {
        ProjectSection {
            scripts: vec!["scripts/**/*.py".to_string()],
            library: "analysis".to_string(),
            out_dir: "out".to_string(),
            data_dirs: vec!["data".to_string()],
            timeout_s: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ContractConfig {
    pub max_helper_lines: u32,
    pub near_duplicate_threshold: f64,
    pub min_tokens_for_similarity: u32,
    pub math_required: Vec<String>,
    pub numerical_modules: Vec<String>,
    pub script_allowed_functions: Vec<String>,
    /// Rule id -> `"error"` | `"warn"`.
    pub severity: BTreeMap<String, String>,
}

impl Default for ContractConfig {
    fn default() -> Self {
        ContractConfig {
            max_helper_lines: 15,
            near_duplicate_threshold: 0.80,
            min_tokens_for_similarity: 30,
            math_required: vec![],
            numerical_modules: [
                "numpy",
                "scipy",
                "pandas",
                "sklearn",
                "jax",
                "torch",
                "xarray",
                "statsmodels",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            script_allowed_functions: ["main", "build_parser", "parse_args"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            severity: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EditPolicy {
    #[default]
    Review,
    AutoNewFiles,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RunPolicy {
    #[default]
    Auto,
    Ask,
}

/// `ask | auto | deny` for packages, shell, git.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AskPolicy {
    #[default]
    Ask,
    Auto,
    Deny,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicy {
    #[default]
    Deny,
    Ask,
    Allow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PolicyConfig {
    pub edits: EditPolicy,
    pub runs: RunPolicy,
    pub packages: AskPolicy,
    pub shell: AskPolicy,
    pub network: NetworkPolicy,
    pub git: AskPolicy,
    pub protected: Vec<String>,
    pub max_steps: u32,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        PolicyConfig {
            edits: EditPolicy::Review,
            runs: RunPolicy::Auto,
            packages: AskPolicy::Ask,
            shell: AskPolicy::Ask,
            network: NetworkPolicy::Deny,
            git: AskPolicy::Ask,
            protected: vec![],
            max_steps: 40,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TargetOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dirs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_dir: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_config_parses_documented_example() {
        let text = r#"
[defaults]
model = "anthropic/claude-x"
target = "local"

[[providers]]
name = "ollama-lab"
kind = "openai-compatible"
base_url = "http://lab-gpu:11434/v1"
api_key_env = "OLLAMA_LAB_API_KEY"

[[providers]]
name = "anthropic"
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"

[[models]]
provider = "ollama-lab"
id = "some-model"
context_window = 65536
tools = "native"
vision = false

[[targets]]
name = "lab-gpu"
kind = "ssh"
host = "lab-gpu"
remote_root = "~/analyst"
python = "3.12"
sandbox = "auto"
data_dirs = ["/data/shared"]
"#;
        let c: GlobalConfig = toml::from_str(text).unwrap();
        assert_eq!(c.defaults.model.as_deref(), Some("anthropic/claude-x"));
        assert_eq!(c.providers.len(), 2);
        assert_eq!(c.providers[1].kind, ProviderKind::Anthropic);
        assert_eq!(
            c.providers[1].effective_base_url(),
            "https://api.anthropic.com"
        );
        assert_eq!(c.models[0].tools, Some(ToolSupport::Native));
        assert_eq!(c.targets[0].kind, TargetKind::Ssh);
        let s = toml::to_string(&c).unwrap();
        let back: GlobalConfig = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn project_config_defaults_match_doc() {
        let c: ProjectConfig = toml::from_str("").unwrap();
        assert_eq!(c, ProjectConfig::default());
        assert_eq!(c.project.scripts, vec!["scripts/**/*.py"]);
        assert_eq!(c.project.library, "analysis");
        assert_eq!(c.project.out_dir, "out");
        assert_eq!(c.project.timeout_s, 600);
        assert_eq!(c.contract.max_helper_lines, 15);
        assert!((c.contract.near_duplicate_threshold - 0.8).abs() < 1e-9);
        assert_eq!(c.policy.edits, EditPolicy::Review);
        assert_eq!(c.policy.runs, RunPolicy::Auto);
        assert_eq!(c.policy.network, NetworkPolicy::Deny);
        assert_eq!(c.policy.max_steps, 40);
    }

    #[test]
    fn project_config_parses_documented_example() {
        let text = r#"
[project]
scripts = ["scripts/**/*.py"]
library = "analysis"
out_dir = "out"
data_dirs = ["data"]
timeout_s = 600

[contract]
max_helper_lines = 15
near_duplicate_threshold = 0.80
math_required = []

[contract.severity]
"lib.near-duplicate" = "warn"

[policy]
edits = "auto-new-files"
runs = "ask"
packages = "deny"
protected = ["analysis/core/**"]
max_steps = 12

[targets.lab-gpu]
data_dirs = ["/data/shared/run3"]
"#;
        let c: ProjectConfig = toml::from_str(text).unwrap();
        assert_eq!(c.policy.edits, EditPolicy::AutoNewFiles);
        assert_eq!(c.policy.runs, RunPolicy::Ask);
        assert_eq!(c.policy.packages, AskPolicy::Deny);
        assert_eq!(c.policy.max_steps, 12);
        assert_eq!(c.contract.severity["lib.near-duplicate"], "warn");
        assert_eq!(
            c.targets["lab-gpu"].data_dirs.as_deref(),
            Some(&["/data/shared/run3".to_string()][..])
        );
        let s = toml::to_string(&c).unwrap();
        let back: ProjectConfig = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}
