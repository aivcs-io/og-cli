/// og-* fleet services management (build & deploy via A2A orchestration)
use crate::a2a::{A2AClient, BuildRequest, DeployRequest};
use crate::org::OrgManager;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OgService {
    pub name: String,
    pub description: String,
    pub repo: String,
    pub image_name: String,
    pub docker_image: String,
    pub k8s_namespace: String,
    pub flake_ref: String,
    pub cluster: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCatalog {
    pub services: Vec<OgService>,
}

impl ServiceCatalog {
    /// Load the fleet catalog from `OG_SERVICES_CATALOG` (JSON file path).
    /// Public builds ship with an empty catalog — fleet operators provide their own.
    pub fn load() -> Result<Self> {
        let path = std::env::var("OG_SERVICES_CATALOG").ok();
        let Some(path) = path.filter(|p| !p.trim().is_empty()) else {
            return Ok(ServiceCatalog {
                services: vec![],
            });
        };
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("read OG_SERVICES_CATALOG at {path}"))?;
        serde_json::from_str(&content).with_context(|| {
            format!(
                "parse OG_SERVICES_CATALOG JSON at {}",
                Path::new(&path).display()
            )
        })
    }

    pub fn find(&self, name: &str) -> Option<&OgService> {
        self.services.iter().find(|s| s.name == name)
    }
}

pub fn get(name: &str) -> Result<OgService> {
    let catalog = ServiceCatalog::load()?;
    catalog
        .find(name)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Service not found: {name}. Set OG_SERVICES_CATALOG to a JSON catalog file."
            )
        })
}

pub struct ServiceManager;

impl ServiceManager {
    pub fn list() -> Result<()> {
        let catalog = ServiceCatalog::load()?;
        if catalog.services.is_empty() {
            println!("No fleet services configured.");
            println!("Set OG_SERVICES_CATALOG to a JSON catalog file to list build/deploy targets.");
            return Ok(());
        }
        println!("og-* Fleet Services:");
        println!();
        for service in &catalog.services {
            println!("  {} — {}", service.name, service.description);
            println!("    repo: {}", service.repo);
            println!("    image: {}", service.docker_image);
            println!("    namespace: {}", service.k8s_namespace);
            println!();
        }
        Ok(())
    }

    pub fn info(service_name: &str) -> Result<()> {
        let service = get(service_name)?;
        println!("Service: {}", service.name);
        println!("Description: {}", service.description);
        println!("Repository: {}", service.repo);
        println!("Docker Image: {}", service.docker_image);
        println!("Kubernetes Namespace: {}", service.k8s_namespace);
        println!("Flake Ref: {}", service.flake_ref);
        println!("Target Cluster: {}", service.cluster);
        Ok(())
    }

    pub async fn build(service_name: &str) -> Result<()> {
        let service = get(service_name)?;
        let org = OrgManager::resolve_org(None);
        info!("Building service: {} (via A2A → og-leader)", service.name);

        println!("🏗️  Building: {}", service.name);
        println!("   Repository: {}", service.repo);
        println!("   Image: {}", service.docker_image);
        println!("   Flake: {}", service.flake_ref);

        let registry = std::env::var("OG_OCI_REGISTRY").unwrap_or_else(|_| "registry.example".to_string());
        let req = BuildRequest {
            service: service.name.clone(),
            repo: service.repo.clone(),
            flake_ref: service.flake_ref.clone(),
            registry,
            push: true,
        };

        let resp = A2AClient::build_service(&req, &org).await?;

        println!("   Status: {}", resp.status);
        if let Some(err) = resp.error {
            println!("   Error: {}", err);
            return Err(anyhow!("Build failed: {}", err));
        }

        println!("✓ Build request sent to og-leader");
        println!("  ID: {}", resp.id);

        Ok(())
    }

    pub async fn deploy(service_name: &str, cluster: &str) -> Result<()> {
        let service = get(service_name)?;
        let org = OrgManager::resolve_org(None);
        info!(
            "Deploying service: {} to cluster: {} (via A2A)",
            service.name, cluster
        );

        println!("🚀 Deploying: {}", service.name);
        println!("   Target Cluster: {}", cluster);
        println!("   Namespace: {}", service.k8s_namespace);

        let req = DeployRequest {
            service: service.name.clone(),
            cluster: cluster.to_string(),
            namespace: service.k8s_namespace.clone(),
        };

        let resp = A2AClient::deploy_service(&req, &org).await?;

        println!("   Status: {}", resp.status);
        if let Some(err) = resp.error {
            println!("   Error: {}", err);
            return Err(anyhow!("Deploy failed: {}", err));
        }

        println!("✓ Deploy request sent to og-leader");
        println!("  ID: {}", resp.id);

        Ok(())
    }

    pub async fn ship(service_name: &str, cluster: &str) -> Result<()> {
        println!("🎯 Shipping: {}", service_name);
        Self::build(service_name).await?;
        println!();
        Self::deploy(service_name, cluster).await?;
        Ok(())
    }
}

pub fn print_list() -> Result<()> {
    ServiceManager::list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_empty_without_env() {
        std::env::remove_var("OG_SERVICES_CATALOG");
        let catalog = ServiceCatalog::load().unwrap();
        assert!(catalog.services.is_empty());
    }
}
