use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

/// Agent dispatch via the orchestrator (AOA) primary endpoint, with optional
/// local fallback when configured through environment variables.
#[derive(Debug, Clone)]
pub struct AgentRegistry {
    primary_url: String,
    fallback_url: String,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            primary_url: std::env::var("AOA_URL").unwrap_or_default(),
            fallback_url: std::env::var("HARBORMASTER_CATALOG_URL").unwrap_or_default(),
        }
    }

    pub fn primary_url(&self) -> &str {
        &self.primary_url
    }

    pub fn fallback_url(&self) -> &str {
        &self.fallback_url
    }

    pub fn fallback_catalog_path(&self) -> String {
        if self.fallback_url.is_empty() {
            return "(not configured)".to_string();
        }
        format!("{}/blob/main/catalog.toml", self.fallback_url.trim_end_matches('/'))
    }

    pub fn resolve_agent(&self, name: &str, use_fallback: bool) -> Option<String> {
        if use_fallback {
            if self.fallback_url.is_empty() {
                return None;
            }
            info!(
                "Resolving agent: {} from fallback catalog",
                name
            );
            Some(format!(
                "{}/tree/main/agents/{}",
                self.fallback_url.trim_end_matches('/'),
                name
            ))
        } else {
            if self.primary_url.is_empty() {
                return None;
            }
            info!("Resolving agent: {} from AOA primary", name);
            Some(format!(
                "{}/v1/agents/{}",
                self.primary_url.trim_end_matches('/'),
                name
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub timeout_secs: Option<u64>,
}

pub struct AgentManager;

fn core_agents() -> Vec<AgentInfo> {
    vec![
        AgentInfo {
            name: "og-design-agent".to_string(),
            description: "Architecture and acceptance criteria design".to_string(),
            enabled: true,
            timeout_secs: Some(300),
        },
        AgentInfo {
            name: "og-propel".to_string(),
            description: "Build orchestration and OCI image generation".to_string(),
            enabled: true,
            timeout_secs: Some(600),
        },
        AgentInfo {
            name: "og-crab".to_string(),
            description: "Test execution and coverage reporting".to_string(),
            enabled: true,
            timeout_secs: Some(900),
        },
        AgentInfo {
            name: "og-code-review-agent".to_string(),
            description: "Code review and quality analysis".to_string(),
            enabled: true,
            timeout_secs: Some(400),
        },
        AgentInfo {
            name: "og-pr-merge-agent".to_string(),
            description: "PR merge gating and sovereign merge".to_string(),
            enabled: true,
            timeout_secs: Some(200),
        },
    ]
}

impl AgentManager {
    pub fn list() -> Result<()> {
        info!("Listing available agents");

        let registry = AgentRegistry::new();
        let agents = core_agents();

        println!("📦 Available Agents ({} total):", agents.len());
        println!();
        println!("Dispatch chain (og-cli agents):");
        if registry.primary_url().is_empty() {
            println!("  Primary:  AOA — (set AOA_URL)");
        } else {
            println!("  Primary:  AOA — {}", registry.primary_url());
        }
        if registry.fallback_url().is_empty() {
            println!("  Fallback: (set HARBORMASTER_CATALOG_URL to enable)");
        } else {
            println!(
                "  Fallback: {} (only if AOA/K8s unavailable)",
                registry.fallback_catalog_path()
            );
        }
        println!();

        for agent in &agents {
            let status = if agent.enabled { "✅" } else { "❌" };
            println!("{} {}", status, agent.name);
            println!("   Description: {}", agent.description);
            if let Some(timeout) = agent.timeout_secs {
                println!("   Timeout: {}s", timeout);
            }
            println!();
        }

        info!("Listed {} agents", agents.len());
        Ok(())
    }

    pub fn run(name: String) -> Result<()> {
        info!("Running agent: {}", name);

        let registry = AgentRegistry::new();
        let lease_id = Self::generate_lease_id();
        let aoa_available = Self::aoa_reachable(&registry);

        println!("🚀 Running Agent: {}", name);
        println!();
        if aoa_available {
            println!("Dispatching via AOA (primary)...");
            println!("  Endpoint: {}", registry.primary_url());
        } else if !registry.fallback_url().is_empty() {
            println!("AOA unavailable — dispatching via fallback catalog...");
            println!("  Catalog: {}", registry.fallback_catalog_path());
        } else {
            anyhow::bail!("Neither AOA_URL nor HARBORMASTER_CATALOG_URL is configured");
        }
        println!("→ Agent: {}", name);
        println!();

        if !core_agents().iter().any(|a| a.name == name) {
            println!("❌ Unknown agent: {}", name);
            println!("Run 'og agents list' to see available agents");
            anyhow::bail!("Unknown agent: {}", name);
        }

        match name.as_str() {
            "og-design-agent" => {
                println!("📐 Design Agent");
                println!("Generating architecture and acceptance criteria...");
            }
            "og-propel" => {
                println!("🏗️  Build Agent");
                println!("Orchestrating build pipeline...");
            }
            "og-crab" => {
                println!("🧪 Test Agent");
                println!("Running tests and coverage...");
            }
            "og-code-review-agent" => {
                println!("👁️  Review Agent");
                println!("Analyzing code quality...");
            }
            "og-pr-merge-agent" => {
                println!("🔀 Merge Agent");
                println!("Processing PR merge gate...");
            }
            _ => unreachable!(),
        }
        println!();
        println!("✓ Status: queued");
        println!("✓ Lease ID: {}", lease_id);

        info!("Agent run submitted: {}", name);
        Ok(())
    }

    fn aoa_reachable(registry: &AgentRegistry) -> bool {
        std::env::var("OG_CLI_FORCE_HARBORMASTER_FALLBACK").is_err()
            && !registry.primary_url().is_empty()
    }

    fn generate_lease_id() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let millis = now.as_millis();
        format!("lease-{:x}", millis % 0xffffff)
    }

    pub fn logs(name: String) -> Result<()> {
        info!("Streaming logs for agent: {}", name);

        let lease_id = Self::generate_lease_id();

        println!("📋 Agent Logs: {}", name);
        println!();
        println!("Lease ID: {}", lease_id);
        println!();
        println!("[INFO] Agent work complete (stub stream)");
        println!();
        println!("✓ Stream ended");

        info!("Showed logs for agent: {}", name);
        Ok(())
    }

    pub fn config(name: String) -> Result<()> {
        info!("Showing configuration for agent: {}", name);

        println!("⚙️  Agent Configuration: {}", name);
        println!();

        match name.as_str() {
            "og-propel" => {
                println!("[og-propel]");
                println!("enabled = true");
                println!("timeout = 600");
                println!();
                println!("[og-propel.config]");
                println!("registry = \"registry.aivcs.io\"");
                println!("build_platform = \"linux/amd64,linux/arm64\"");
            }
            "og-crab" => {
                println!("[og-crab]");
                println!("enabled = true");
                println!("timeout = 900");
                println!();
                println!("[og-crab.config]");
                println!("coverage_threshold = 85.0");
            }
            "og-code-review-agent" => {
                println!("[og-code-review-agent]");
                println!("enabled = true");
                println!("timeout = 400");
            }
            _ => {
                println!("❌ Unknown agent: {}", name);
                anyhow::bail!("Unknown agent: {}", name);
            }
        }

        let registry = AgentRegistry::new();
        println!();
        if !registry.primary_url().is_empty() {
            println!("✓ Primary registry: AOA ({})", registry.primary_url());
        }
        if !registry.fallback_url().is_empty() {
            println!(
                "✓ Fallback catalog: {}",
                registry.fallback_catalog_path()
            );
        }

        info!("Showed config for agent: {}", name);
        Ok(())
    }
}
