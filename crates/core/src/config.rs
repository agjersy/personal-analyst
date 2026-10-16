//! Configuration loading: global `$ANALYST_HOME/config.toml` (default `~/.analyst`), project
//! `<repo>/.analyst/project.toml`, API keys from environment variables, model spec parsing,
//! and per-target overrides (PROTOCOL 2.1, 2.2).

use crate::util::expand_tilde;
use analyst_protocol::config::{
    GlobalConfig, ModelConfig, ProjectConfig, ProjectSection, ProviderConfig, TargetConfig,
};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// `$ANALYST_HOME`, else `~/.analyst`.
pub fn analyst_home() -> PathBuf {
    if let Ok(h) = std::env::var("ANALYST_HOME") {
        if !h.is_empty() {
            return expand_tilde(&h);
        }
    }
    match std::env::var("HOME") {
        Ok(home) => Path::new(&home).join(".analyst"),
        Err(_) => PathBuf::from(".analyst"),
    }
}

/// Path of the global config file under a home directory.
pub fn global_config_path(home: &Path) -> PathBuf {
    home.join("config.toml")
}

/// Load the global config; a missing file means all defaults.
pub fn load_global(home: &Path) -> Result<GlobalConfig> {
    let path = global_config_path(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("parsing global config {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GlobalConfig::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write the global config back (creates the home directory).
pub fn save_global(home: &Path, config: &GlobalConfig) -> Result<()> {
    std::fs::create_dir_all(home).with_context(|| format!("creating {}", home.display()))?;
    let text = toml::to_string_pretty(config).context("serializing global config")?;
    let path = global_config_path(home);
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

/// Path of the project config inside a repo.
pub fn project_config_path(repo: &Path) -> PathBuf {
    repo.join(".analyst").join("project.toml")
}

/// Load the project config; a missing file means all defaults.
pub fn load_project(repo: &Path) -> Result<ProjectConfig> {
    let path = project_config_path(repo);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("parsing project config {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProjectConfig::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// `"<provider name>/<model id>"`. The model id may itself contain slashes; the split is at
/// the first one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub provider: String,
    pub id: String,
}

impl ModelSpec {
    pub fn parse(spec: &str) -> Result<ModelSpec> {
        let (provider, id) = spec
            .split_once('/')
            .ok_or_else(|| anyhow!("model spec `{spec}` must be `<provider>/<model id>`"))?;
        if provider.is_empty() || id.is_empty() {
            return Err(anyhow!(
                "model spec `{spec}` must be `<provider>/<model id>`"
            ));
        }
        Ok(ModelSpec {
            provider: provider.to_string(),
            id: id.to_string(),
        })
    }
}

impl std::fmt::Display for ModelSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.id)
    }
}

/// A model spec resolved against the global config.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub spec: ModelSpec,
    pub provider: ProviderConfig,
    pub overrides: Option<ModelConfig>,
}

/// Find the provider (and any `[[models]]` override) for a spec.
pub fn resolve_model(global: &GlobalConfig, spec: &ModelSpec) -> Result<ResolvedModel> {
    let provider = global
        .providers
        .iter()
        .find(|p| p.name == spec.provider)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "provider `{}` is not configured (known: {})",
                spec.provider,
                global
                    .providers
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let overrides = global
        .models
        .iter()
        .find(|m| m.provider == spec.provider && m.id == spec.id)
        .cloned();
    Ok(ResolvedModel {
        spec: spec.clone(),
        provider,
        overrides,
    })
}

/// The default model spec from config, or an error naming the file to edit.
pub fn default_model(global: &GlobalConfig) -> Result<ModelSpec> {
    let s = global.defaults.model.as_deref().ok_or_else(|| {
        anyhow!("no model given and `[defaults].model` is not set in config.toml")
    })?;
    ModelSpec::parse(s)
}

/// API key for a provider: read from the named environment variable (phase 0).
/// `Ok(None)` when the provider declares no key variable; an error when the variable is named
/// but unset.
pub fn api_key(provider: &ProviderConfig) -> Result<Option<String>> {
    match &provider.api_key_env {
        None => Ok(None),
        Some(var) => match std::env::var(var) {
            Ok(v) if !v.is_empty() => Ok(Some(v)),
            _ => Err(anyhow!(
                "provider `{}` needs an API key in environment variable {var}, which is unset",
                provider.name
            )),
        },
    }
}

/// Look up a target by name. `local` always exists.
pub fn find_target(global: &GlobalConfig, name: &str) -> Result<TargetConfig> {
    if name == "local" {
        return Ok(TargetConfig {
            name: "local".into(),
            kind: analyst_protocol::config::TargetKind::Local,
            host: None,
            remote_root: None,
            python: None,
            sandbox: None,
            data_dirs: vec![],
        });
    }
    global
        .targets
        .iter()
        .find(|t| t.name == name)
        .cloned()
        .ok_or_else(|| anyhow!("target `{name}` is not configured in config.toml"))
}

/// The project section with per-target overrides applied.
pub fn effective_project(project: &ProjectConfig, target: &str) -> ProjectSection {
    let mut section = project.project.clone();
    if let Some(o) = project.targets.get(target) {
        if let Some(d) = &o.data_dirs {
            section.data_dirs = d.clone();
        }
        if let Some(t) = o.timeout_s {
            section.timeout_s = t;
        }
        if let Some(out) = &o.out_dir {
            section.out_dir = out.clone();
        }
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;
    use analyst_protocol::config::{ProviderKind, TargetOverride};

    #[test]
    fn model_spec_parsing() {
        let s = ModelSpec::parse("ollama/meta/llama-3").unwrap();
        assert_eq!(s.provider, "ollama");
        assert_eq!(s.id, "meta/llama-3");
        assert!(ModelSpec::parse("nope").is_err());
        assert!(ModelSpec::parse("/x").is_err());
    }

    #[test]
    fn resolve_and_overrides() {
        let mut g = GlobalConfig::default();
        g.providers.push(ProviderConfig {
            name: "p".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Some("http://x/v1".into()),
            api_key_env: None,
        });
        g.models.push(ModelConfig {
            provider: "p".into(),
            id: "m".into(),
            context_window: Some(1000),
            tools: None,
            vision: None,
        });
        let r = resolve_model(&g, &ModelSpec::parse("p/m").unwrap()).unwrap();
        assert_eq!(r.overrides.unwrap().context_window, Some(1000));
        assert!(resolve_model(&g, &ModelSpec::parse("q/m").unwrap()).is_err());
        assert!(api_key(&r.provider).unwrap().is_none());
    }

    #[test]
    fn loads_defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_global(dir.path()).unwrap(), GlobalConfig::default());
        assert_eq!(load_project(dir.path()).unwrap(), ProjectConfig::default());
    }

    #[test]
    fn per_target_overrides() {
        let mut p = ProjectConfig::default();
        p.targets.insert(
            "lab".into(),
            TargetOverride {
                data_dirs: Some(vec!["/data".into()]),
                timeout_s: Some(5),
                out_dir: None,
            },
        );
        let e = effective_project(&p, "lab");
        assert_eq!(e.data_dirs, vec!["/data"]);
        assert_eq!(e.timeout_s, 5);
        assert_eq!(e.out_dir, "out");
        assert_eq!(effective_project(&p, "local").timeout_s, 600);
    }
}
