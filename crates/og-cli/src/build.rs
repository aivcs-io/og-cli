// This module's OCI build client (BuildConfig / OgBuildsClient / PropelBuild*)
// is reserved infrastructure. Several items are currently unused pending the
// og-cli → og-leader architecture: og-cli connects only to og-leader, and
// og-propel / og-builds are driven server-side rather than from the CLI. Allow
// dead_code module-wide until that refactor lands, rather than deleting wiring
// that is under active development. Tracked separately from clippy/fmt debt.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Build configuration from project config
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildConfig {
    pub dockerfile: Option<String>,
    pub registry: Option<String>,
    pub platforms: Vec<String>, // amd64, arm64, etc.
    pub cache: bool,
}

impl BuildConfig {
    pub fn default_registry() -> String {
        std::env::var("OG_BUILD_REGISTRY")
            .unwrap_or_else(|_| "registry.aivcs.io/aivcs".to_string())
    }

    pub fn dockerfile_path(&self) -> &str {
        self.dockerfile.as_deref().unwrap_or("Dockerfile")
    }

    pub fn registry(&self) -> String {
        self.registry.clone().unwrap_or_else(Self::default_registry)
    }

    pub fn platforms(&self) -> Vec<String> {
        if self.platforms.is_empty() {
            vec!["linux/amd64".to_string()]
        } else {
            self.platforms.clone()
        }
    }
}

/// Request to og-propel for OCI build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropelBuildRequest {
    pub repo_url: String,
    pub dockerfile: String,
    pub image_name: String,
    pub platforms: Vec<String>,
    pub registry: String,
    pub push: bool,
    /// Fleet service id (og-leader, og-crab, …) when building from catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    /// Nix flake attribute (og-propel sovereign rail).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flake_ref: Option<String>,
}

/// Response from og-propel after build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropelBuildResponse {
    pub status: String, // success | failure
    pub image_digest: String,
    pub artifacts: Vec<String>,
    pub logs: String,
    pub duration_secs: u32,
}

/// og-builds client for OCI image compilation
pub struct OgBuildsClient {
    endpoint: String,
}

impl OgBuildsClient {
    pub fn new() -> Self {
        let endpoint = std::env::var("OG_BUILDS_ENDPOINT")
            .unwrap_or_else(|_| "http://og-builds.og-builds:5000".to_string());

        Self { endpoint }
    }

    pub fn with_endpoint(endpoint: String) -> Self {
        Self { endpoint }
    }

    /// Build OCI image via og-builds
    pub async fn build(&self, req: &PropelBuildRequest) -> Result<PropelBuildResponse> {
        info!(
            "Building {} via og-builds: {:?}",
            req.image_name, req.platforms
        );

        // In real implementation, would call:
        // POST {endpoint}/build with PropelBuildRequest
        // Returns PropelBuildResponse with image digest

        // Placeholder response
        Ok(PropelBuildResponse {
            status: "success".to_string(),
            image_digest: format!("sha256:og-builds-{}", uuid()),
            artifacts: vec![
                format!("{}:latest", req.image_name),
                format!("{}:latest-amd64", req.image_name),
            ],
            logs: format!(
                "Building {} on platforms {:?}",
                req.image_name, req.platforms
            ),
            duration_secs: 180,
        })
    }

    /// Push image to registry
    pub async fn push(&self, image_name: &str, registry: &str) -> Result<String> {
        info!("Pushing {} to {}", image_name, registry);

        // In real implementation:
        // - Get credentials from secrets plane
        // - Push to registry (GAR/ACR/ECR)
        // - Return digest

        Ok(format!("sha256:pushed-{}", uuid()))
    }

    /// Check if og-builds is reachable
    pub async fn is_reachable(&self) -> bool {
        info!("Checking og-builds connectivity at {}", self.endpoint);

        match reqwest::Client::new()
            .get(format!("{}/health", self.endpoint))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                info!("og-builds unreachable: {}", e);
                false
            }
        }
    }
}

impl Default for OgBuildsClient {
    fn default() -> Self {
        Self::new()
    }
}

fn uuid() -> String {
    // Simple placeholder UUID-like string
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_config_defaults() {
        let cfg = BuildConfig::default();
        assert_eq!(cfg.dockerfile_path(), "Dockerfile");
        assert!(!cfg.cache);
    }

    #[test]
    fn test_build_config_registry() {
        let cfg = BuildConfig {
            registry: Some("registry.example/my-org".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.registry(), "registry.example/my-org");
    }

    #[test]
    fn test_build_config_platforms_default() {
        let cfg = BuildConfig::default();
        let platforms = cfg.platforms();
        assert_eq!(platforms.len(), 1);
        assert_eq!(platforms[0], "linux/amd64");
    }

    #[test]
    fn test_build_config_platforms_custom() {
        let cfg = BuildConfig {
            platforms: vec!["linux/amd64".to_string(), "linux/arm64".to_string()],
            ..Default::default()
        };
        assert_eq!(cfg.platforms().len(), 2);
    }

    #[test]
    fn test_og_builds_client_new() {
        let client = OgBuildsClient::new();
        assert!(!client.endpoint.is_empty());
    }

    #[test]
    fn test_propel_build_request_structure() {
        let req = PropelBuildRequest {
            repo_url: "https://github.com/org/repo".to_string(),
            dockerfile: "Dockerfile".to_string(),
            image_name: "registry.example/org/service:v1.0".to_string(),
            platforms: vec!["linux/amd64".to_string()],
            registry: "registry.example".to_string(),
            push: true,
            service_id: None,
            flake_ref: None,
        };

        assert_eq!(req.image_name, "registry.example/org/service:v1.0");
        assert_eq!(req.platforms.len(), 1);
    }

    #[tokio::test]
    async fn test_og_builds_client_build() {
        let client = OgBuildsClient::new();
        let req = PropelBuildRequest {
            repo_url: "https://github.com/org/repo".to_string(),
            dockerfile: "Dockerfile".to_string(),
            image_name: "registry.example/org/service:v1.0".to_string(),
            platforms: vec!["linux/amd64".to_string()],
            registry: "registry.example".to_string(),
            push: true,
            service_id: None,
            flake_ref: None,
        };

        let response = client.build(&req).await.unwrap();
        assert_eq!(response.status, "success");
        assert!(!response.image_digest.is_empty());
    }
}
