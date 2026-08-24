use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub repo: RepoConfig,
    #[serde(default)]
    pub vcs: VcsConfig,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub test: TestConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VcsConfig {
    /// VCS backend: "git" | "aivcs" | "auto" (default)
    #[serde(default)]
    pub backend: String,
    /// Git repository (for git backend)
    pub git_url: Option<String>,
    /// AIVCS repository (for aivcs backend)
    pub aivcs_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoConfig {
    pub name: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    #[serde(default)]
    pub og_design_agent: AgentConfig,
    #[serde(default)]
    pub og_propel: AgentConfig,
    #[serde(default)]
    pub og_crab: AgentConfig,
    #[serde(default)]
    pub og_code_review_agent: AgentConfig,
    #[serde(default)]
    pub og_pr_merge_agent: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub timeout: Option<String>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildConfig {
    pub dockerfile: Option<String>,
    pub image_repo: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestConfig {
    pub coverage_threshold: Option<f32>,
}

pub struct ConfigManager;

impl ConfigManager {
    fn config_path() -> PathBuf {
        PathBuf::from("og-cli.toml")
    }

    pub async fn load_project() -> Result<ProjectConfig> {
        Self::load()
    }

    pub fn load() -> Result<ProjectConfig> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(ProjectConfig::default());
        }

        let content = fs::read_to_string(&path).context("Failed to read og-cli.toml")?;
        toml::from_str(&content).context("Failed to parse og-cli.toml")
    }

    pub fn init(name: String) -> Result<()> {
        info!("Initializing project config for: {}", name);

        let config = ProjectConfig {
            repo: RepoConfig {
                name: Some(name.clone()),
                description: None,
                language: Some("rust".to_string()),
                owner: Some("acme".to_string()),
            },
            vcs: VcsConfig {
                backend: "auto".to_string(),
                git_url: None,
                aivcs_url: None,
            },
            agents: AgentsConfig::default(),
            build: BuildConfig::default(),
            test: TestConfig::default(),
        };

        let path = Self::config_path();
        let toml_str = toml::to_string_pretty(&config).context("Failed to serialize config")?;
        fs::write(&path, toml_str).context("Failed to write og-cli.toml")?;

        println!("Created og-cli.toml with defaults:");
        println!("[repo]");
        println!("name = \"{}\"", name);
        println!("language = \"rust\"");
        println!("owner = \"acme\"");

        info!("Config init complete");
        Ok(())
    }

    pub fn get(key: String) -> Result<()> {
        info!("Getting config key: {}", key);

        let config = Self::load()?;
        let value = get_nested_value(&config, &key).unwrap_or_else(|| "key not found".to_string());

        println!("{}", value);
        Ok(())
    }

    pub fn set(key: String, value: String) -> Result<()> {
        info!("Setting config: {} = {}", key, value);

        let mut config = Self::load()?;
        set_nested_value(&mut config, &key, &value)?;

        let path = Self::config_path();
        let toml_str = toml::to_string_pretty(&config).context("Failed to serialize config")?;
        fs::write(&path, toml_str).context("Failed to write og-cli.toml")?;

        println!("Set: {} = {}", key, value);
        info!("Config set complete");
        Ok(())
    }
}

fn get_nested_value(config: &ProjectConfig, key: &str) -> Option<String> {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.is_empty() {
        return None;
    }

    match parts[0] {
        "repo" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "name" => config.repo.name.clone(),
                    "description" => config.repo.description.clone(),
                    "language" => config.repo.language.clone(),
                    "owner" => config.repo.owner.clone(),
                    _ => None,
                }
            } else {
                None
            }
        }
        "vcs" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "backend" => Some(config.vcs.backend.clone()),
                    "git_url" => config.vcs.git_url.clone(),
                    "aivcs_url" => config.vcs.aivcs_url.clone(),
                    _ => None,
                }
            } else {
                None
            }
        }
        "build" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "dockerfile" => config.build.dockerfile.clone(),
                    "image_repo" => config.build.image_repo.clone(),
                    _ => None,
                }
            } else {
                None
            }
        }
        "test" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "coverage_threshold" => config.test.coverage_threshold.map(|v| v.to_string()),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn set_nested_value(config: &mut ProjectConfig, key: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.is_empty() {
        anyhow::bail!("Empty config key provided");
    }

    match parts[0] {
        "repo" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "name" => config.repo.name = Some(value.to_string()),
                    "description" => config.repo.description = Some(value.to_string()),
                    "language" => config.repo.language = Some(value.to_string()),
                    "owner" => config.repo.owner = Some(value.to_string()),
                    _ => anyhow::bail!("Unknown field: repo.{}", field),
                }
            }
        }
        "vcs" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "backend" => config.vcs.backend = value.to_string(),
                    "git_url" => config.vcs.git_url = Some(value.to_string()),
                    "aivcs_url" => config.vcs.aivcs_url = Some(value.to_string()),
                    _ => anyhow::bail!("Unknown field: vcs.{}", field),
                }
            }
        }
        "build" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "dockerfile" => config.build.dockerfile = Some(value.to_string()),
                    "image_repo" => config.build.image_repo = Some(value.to_string()),
                    _ => anyhow::bail!("Unknown field: build.{}", field),
                }
            }
        }
        "test" => {
            if let Some(field) = parts.get(1) {
                match *field {
                    "coverage_threshold" => {
                        let threshold: f32 = value.parse()?;
                        config.test.coverage_threshold = Some(threshold);
                    }
                    _ => anyhow::bail!("Unknown field: test.{}", field),
                }
            }
        }
        _ => anyhow::bail!("Unknown section: {}", parts[0]),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_nested_value_empty_key() {
        let config = ProjectConfig::default();
        let value = get_nested_value(&config, "");
        assert!(value.is_none(), "Empty key should return None");
    }

    #[test]
    fn test_get_nested_value_valid_key() {
        let config = ProjectConfig {
            repo: RepoConfig {
                name: Some("test-repo".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let value = get_nested_value(&config, "repo.name");
        assert_eq!(value, Some("test-repo".to_string()));
    }

    #[test]
    fn test_get_nested_value_invalid_key() {
        let config = ProjectConfig::default();
        let value = get_nested_value(&config, "invalid.key");
        assert!(value.is_none());
    }

    #[test]
    fn test_set_nested_value_empty_key() {
        let mut config = ProjectConfig::default();
        let result = set_nested_value(&mut config, "", "value");
        assert!(result.is_err(), "Empty key should return error");
    }

    #[test]
    fn test_set_nested_value_valid_key() -> Result<()> {
        let mut config = ProjectConfig::default();
        set_nested_value(&mut config, "repo.name", "new-repo")?;
        assert_eq!(config.repo.name, Some("new-repo".to_string()));
        Ok(())
    }

    #[test]
    fn test_set_nested_value_coverage_threshold() -> Result<()> {
        let mut config = ProjectConfig::default();
        set_nested_value(&mut config, "test.coverage_threshold", "85.5")?;
        assert_eq!(config.test.coverage_threshold, Some(85.5));
        Ok(())
    }
}
