/// og-* fleet services management (build & deploy via A2A orchestration)
use crate::a2a::{A2AClient, BuildRequest, DeployRequest};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
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
    pub fn load() -> Result<Self> {
        Ok(ServiceCatalog {
            services: vec![
                OgService {
                    name: "og-leader".to_string(),
                    description: "Primary orchestrator for og-* agents".to_string(),
                    repo: "aivcs://lornu-ai/og-leader".to_string(),
                    image_name: "og-leader".to_string(),
                    docker_image: "gcr.io/lornu-ai/og-leader".to_string(),
                    k8s_namespace: "og-agents".to_string(),
                    flake_ref: ".#image".to_string(),
                    cluster: "aivcs-builds".to_string(),
                },
                OgService {
                    name: "og-propel".to_string(),
                    description: "Propel-based build orchestrator".to_string(),
                    repo: "aivcs://lornu-ai/og-propel".to_string(),
                    image_name: "og-propel".to_string(),
                    docker_image: "gcr.io/lornu-ai/og-propel".to_string(),
                    k8s_namespace: "og-agents".to_string(),
                    flake_ref: ".#image".to_string(),
                    cluster: "aivcs-builds".to_string(),
                },
                OgService {
                    name: "og-builds".to_string(),
                    description: "OCI image builder (multi-arch)".to_string(),
                    repo: "aivcs://lornu-ai/og-builds".to_string(),
                    image_name: "og-builds".to_string(),
                    docker_image: "gcr.io/lornu-ai/og-builds".to_string(),
                    k8s_namespace: "og-agents".to_string(),
                    flake_ref: ".#image".to_string(),
                    cluster: "aivcs-builds".to_string(),
                },
                OgService {
                    name: "og-crab".to_string(),
                    description: "Test execution engine".to_string(),
                    repo: "aivcs://lornu-ai/og-crab".to_string(),
                    image_name: "og-crab".to_string(),
                    docker_image: "gcr.io/lornu-ai/og-crab".to_string(),
                    k8s_namespace: "og-agents".to_string(),
                    flake_ref: ".#image".to_string(),
                    cluster: "aivcs-builds".to_string(),
                },
                OgService {
                    name: "og-code-review-agent".to_string(),
                    description: "Code review agent".to_string(),
                    repo: "aivcs://lornu-ai/og-code-review-agent".to_string(),
                    image_name: "og-code-review-agent".to_string(),
                    docker_image: "gcr.io/lornu-ai/og-code-review-agent".to_string(),
                    k8s_namespace: "og-agents".to_string(),
                    flake_ref: ".#image".to_string(),
                    cluster: "aivcs-builds".to_string(),
                },
                OgService {
                    name: "ogre".to_string(),
                    description: "Agent integration wiring (ogre-svc)".to_string(),
                    repo: "aivcs://lornu-ai/ogre".to_string(),
                    image_name: "ogre-svc".to_string(),
                    docker_image: "us-central1-docker.pkg.dev/gcp-lornu-ai/lornu/ogre-svc"
                        .to_string(),
                    k8s_namespace: "ogre".to_string(),
                    flake_ref: ".#ogre-svc-image".to_string(),
                    cluster: "lornu-gke-prod".to_string(),
                },
                OgService {
                    name: "og-pr-merge-agent".to_string(),
                    description: "PR merge agent".to_string(),
                    repo: "aivcs://lornu-ai/og-pr-merge-agent".to_string(),
                    image_name: "og-pr-merge-agent".to_string(),
                    docker_image: "gcr.io/lornu-ai/og-pr-merge-agent".to_string(),
                    k8s_namespace: "og-agents".to_string(),
                    flake_ref: ".#image".to_string(),
                    cluster: "aivcs-builds".to_string(),
                },
            ],
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
        .ok_or_else(|| anyhow!("Service not found: {}", name))
}

pub struct ServiceManager;

impl ServiceManager {
    pub fn list() -> Result<()> {
        let catalog = ServiceCatalog::load()?;
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

    /// Build a service via og-leader A2A orchestration
    pub async fn build(service_name: &str) -> Result<()> {
        let service = get(service_name)?;
        info!("Building service: {} (via A2A → og-leader)", service.name);

        println!("🏗️  Building: {}", service.name);
        println!("   Repository: {}", service.repo);
        println!("   Image: {}", service.docker_image);
        println!("   Flake: {}", service.flake_ref);

        let req = BuildRequest {
            service: service.name.clone(),
            repo: service.repo.clone(),
            flake_ref: service.flake_ref.clone(),
            registry: "gcr.io".to_string(),
            push: true,
        };

        let resp = A2AClient::build_service(&req, "lornu-ai").await?;

        println!("   Status: {}", resp.status);
        if let Some(err) = resp.error {
            println!("   Error: {}", err);
            return Err(anyhow!("Build failed: {}", err));
        }

        println!("✓ Build request sent to og-leader");
        println!("  ID: {}", resp.id);

        Ok(())
    }

    /// Deploy a service via og-leader A2A orchestration
    pub async fn deploy(service_name: &str, cluster: &str) -> Result<()> {
        let service = get(service_name)?;
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

        let resp = A2AClient::deploy_service(&req, "lornu-ai").await?;

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
    fn test_catalog_loads() {
        let catalog = ServiceCatalog::load().unwrap();
        assert_eq!(catalog.services.len(), 7);
    }

    #[test]
    fn test_find_og_leader() {
        let catalog = ServiceCatalog::load().unwrap();
        let service = catalog.find("og-leader").unwrap();
        assert_eq!(service.name, "og-leader");
        assert!(service.repo.contains("og-leader"));
    }

    #[test]
    fn test_get_service() {
        let service = get("og-leader").unwrap();
        assert_eq!(service.name, "og-leader");
        assert_eq!(service.repo, "aivcs://lornu-ai/og-leader");
    }
}
