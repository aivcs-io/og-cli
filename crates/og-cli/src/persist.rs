use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

/// Configuration file locations for og-cli
pub struct ConfigPaths;

impl ConfigPaths {
    /// Get the config directory (e.g., ~/.og or ~/.config/og)
    pub fn config_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("Cannot determine home directory"))?;

        // Try ~/.og first, fallback to ~/.config/og
        let og_dir = home.join(".og");
        if og_dir.exists() {
            return Ok(og_dir);
        }

        let config_dir = home.join(".config").join("og");
        Ok(config_dir)
    }

    /// Get the main config file path (~/.og/config.toml)
    pub fn config_file() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    /// Get the credentials file path (~/.og/credentials.json)
    pub fn credentials_file() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("credentials.json"))
    }

    /// Get the project config path (~/.og/projects/PROJECT.toml)
    pub fn project_config(project: &str) -> Result<PathBuf> {
        let projects_dir = Self::config_dir()?.join("projects");
        Ok(projects_dir.join(format!("{}.toml", project)))
    }
}

/// og-cli global configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub user: Option<String>,
    pub email: Option<String>,
    pub github_token: Option<String>,
    pub aoa_url: Option<String>,
    pub default_backend: Option<String>,
    pub prefer_streaming: bool,
    /// LLM model used by agents (claude, codex, …). `/config` option 1.
    #[serde(default)]
    pub model: Option<String>,
    /// Enable the harbormaster fallback when the mesh is unreachable.
    /// `/config` option 2. Off by default to preserve the strict fail-closed
    /// contract (og-bug-report#31); opt in explicitly.
    #[serde(default)]
    pub fallback_enabled: bool,
    /// Override `DATA_MESH_URL` (the A2A work-plane endpoint). `/config` option 4.
    #[serde(default)]
    pub data_mesh_url: Option<String>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            user: None,
            email: None,
            github_token: None,
            aoa_url: None,
            default_backend: Some("auto".to_string()),
            prefer_streaming: true,
            model: None,
            fallback_enabled: false,
            data_mesh_url: None,
        }
    }
}

impl GlobalConfig {
    /// Load global config from disk
    pub fn load() -> Result<Self> {
        let config_file = ConfigPaths::config_file()?;

        if !config_file.exists() {
            info!("Config file not found, using defaults");
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&config_file)?;
        let config: GlobalConfig = toml::from_str(&content)?;

        info!("Loaded config from: {:?}", config_file);
        Ok(config)
    }

    /// Save global config to disk
    pub fn save(&self) -> Result<()> {
        let config_file = ConfigPaths::config_file()?;
        let config_dir = config_file
            .parent()
            .ok_or_else(|| anyhow!("Invalid config path"))?;

        // Create config directory if it doesn't exist
        fs::create_dir_all(config_dir)?;

        let content = toml::to_string_pretty(self)?;
        fs::write(&config_file, content)?;

        info!("Saved config to: {:?}", config_file);
        Ok(())
    }

    /// Update a config value
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "user" => self.user = Some(value.to_string()),
            "email" => self.email = Some(value.to_string()),
            "github-token" => self.github_token = Some(value.to_string()),
            "aoa-url" => self.aoa_url = Some(value.to_string()),
            "default-backend" => self.default_backend = Some(value.to_string()),
            "prefer-streaming" => self.prefer_streaming = value.to_lowercase() == "true",
            "model" => self.model = Some(value.to_string()),
            "fallback-enabled" => {
                self.fallback_enabled = matches!(
                    value.trim().to_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            }
            "data-mesh-url" => self.data_mesh_url = Some(value.to_string()),
            _ => return Err(anyhow!("Unknown config key: {}", key)),
        }

        info!("Set config: {} = {}", key, value);
        Ok(())
    }

    /// Get a config value
    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "user" => self.user.clone(),
            "email" => self.email.clone(),
            "github-token" => self.github_token.clone(),
            "aoa-url" => self.aoa_url.clone(),
            "default-backend" => self.default_backend.clone(),
            "prefer-streaming" => Some(self.prefer_streaming.to_string()),
            "model" => self.model.clone(),
            "fallback-enabled" => Some(self.fallback_enabled.to_string()),
            "data-mesh-url" => self.data_mesh_url.clone(),
            _ => None,
        }
    }

    /// List all config values
    pub fn list(&self) -> Vec<(String, String)> {
        vec![
            ("user".to_string(), self.user.clone().unwrap_or_default()),
            ("email".to_string(), self.email.clone().unwrap_or_default()),
            (
                "github-token".to_string(),
                if self.github_token.is_some() {
                    "***hidden***".to_string()
                } else {
                    String::new()
                },
            ),
            (
                "aoa-url".to_string(),
                self.aoa_url.clone().unwrap_or_default(),
            ),
            (
                "default-backend".to_string(),
                self.default_backend.clone().unwrap_or_default(),
            ),
            (
                "prefer-streaming".to_string(),
                self.prefer_streaming.to_string(),
            ),
            ("model".to_string(), self.model.clone().unwrap_or_default()),
            (
                "fallback-enabled".to_string(),
                self.fallback_enabled.to_string(),
            ),
            (
                "data-mesh-url".to_string(),
                self.data_mesh_url.clone().unwrap_or_default(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GlobalConfig::default();
        assert_eq!(config.default_backend, Some("auto".to_string()));
        assert!(config.prefer_streaming);
    }

    #[test]
    fn test_config_set_get() {
        let mut config = GlobalConfig::default();
        config.set("user", "testuser").unwrap();
        assert_eq!(config.get("user"), Some("testuser".to_string()));
    }

    #[test]
    fn test_config_list() {
        let config = GlobalConfig::default();
        let list = config.list();
        assert!(!list.is_empty());
        assert_eq!(list.len(), 9);
    }

    #[test]
    fn test_config_paths() {
        let config_file = ConfigPaths::config_file();
        assert!(config_file.is_ok());

        let cred_file = ConfigPaths::credentials_file();
        assert!(cred_file.is_ok());

        let project_file = ConfigPaths::project_config("test");
        assert!(project_file.is_ok());
    }

    #[test]
    fn test_config_serialization() {
        let config = GlobalConfig {
            user: Some("testuser".to_string()),
            email: Some("test@example.com".to_string()),
            ..Default::default()
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("testuser"));
        assert!(toml_str.contains("test@example.com"));
    }
}
