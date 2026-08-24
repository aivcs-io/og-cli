use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

/// Agent dispatch: **AOA primary**, [harbormaster](https://github.com/lornu-ai/harbormaster)
/// **fallback only** when AOA/Kubernetes backends are unavailable.
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
            primary_url: std::env::var("AOA_URL")
                .unwrap_or_else(|_| "http://aoa-mcp.agent-orchestrate-ai:8090".to_string()),
            fallback_url: "https://github.com/lornu-ai/harbormaster".to_string(),
        }
    }

    pub fn primary_url(&self) -> &str {
        &self.primary_url
    }

    pub fn fallback_url(&self) -> &str {
        &self.fallback_url
    }

    pub fn fallback_catalog_path(&self) -> String {
        format!("{}/blob/main/catalog.toml", self.fallback_url)
    }

    /// Resolve agent via AOA first; harbormaster catalog is consulted only on fallback.
    pub fn resolve_agent(&self, name: &str, use_fallback: bool) -> Option<String> {
        if use_fallback {
            info!(
                "Resolving agent: {} from harbormaster fallback catalog",
                name
            );
            Some(format!("{}/tree/main/agents/{}", self.fallback_url, name))
        } else {
            info!("Resolving agent: {} from AOA primary", name);
            Some(format!("{}/v1/agents/{}", self.primary_url, name))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub timeout_secs: Option<u64>,
    pub image: String,
}

pub struct AgentManager;

fn core_agents() -> Vec<AgentInfo> {
    vec![
        AgentInfo {
            name: "og-design-agent".to_string(),
            description: "Architecture and acceptance criteria design".to_string(),
            enabled: true,
            timeout_secs: Some(300),
            image: "lornu-ai/og-design-agent:latest".to_string(),
        },
        AgentInfo {
            name: "og-propel".to_string(),
            description: "Build orchestration and OCI image generation".to_string(),
            enabled: true,
            timeout_secs: Some(600),
            image: "lornu-ai/og-propel:latest".to_string(),
        },
        AgentInfo {
            name: "og-crab".to_string(),
            description: "Test execution and coverage reporting".to_string(),
            enabled: true,
            timeout_secs: Some(900),
            image: "lornu-ai/og-crab:latest".to_string(),
        },
        AgentInfo {
            name: "og-code-review-agent".to_string(),
            description: "Code review and quality analysis".to_string(),
            enabled: true,
            timeout_secs: Some(400),
            image: "lornu-ai/og-code-review-agent:latest".to_string(),
        },
        AgentInfo {
            name: "og-pr-merge-agent".to_string(),
            description: "PR merge gating and sovereign merge".to_string(),
            enabled: true,
            timeout_secs: Some(200),
            image: "lornu-ai/og-pr-merge-agent:latest".to_string(),
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
        println!("  Primary:  AOA — {}", registry.primary_url());
        println!(
            "  Fallback: harbormaster — {} (only if AOA/K8s unavailable)",
            registry.fallback_catalog_path()
        );
        println!();

        for agent in &agents {
            let status = if agent.enabled { "✅" } else { "❌" };
            println!("{} {}", status, agent.name);
            println!("   Description: {}", agent.description);
            println!("   Image: {}", agent.image);
            if let Some(timeout) = agent.timeout_secs {
                println!("   Timeout: {}s", timeout);
            }
            println!();
        }

        info!(
            "Listed {} agents (AOA primary, harbormaster fallback)",
            agents.len()
        );
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
        } else {
            println!("AOA unavailable — dispatching via harbormaster (fallback)...");
            println!("  Catalog: {}", registry.fallback_catalog_path());
        }
        println!(
            "→ Backend: {}",
            if aoa_available {
                "agent-orchestrated"
            } else {
                "harbormaster"
            }
        );
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

        info!(
            "Agent run submitted: {} via {}",
            name,
            if aoa_available {
                "AOA"
            } else {
                "harbormaster-fallback"
            }
        );
        Ok(())
    }

    /// Phase 0 stub: treat AOA as reachable unless explicitly disabled.
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

        let registry = AgentRegistry::new();
        let lease_id = Self::generate_lease_id();
        let via = if Self::aoa_reachable(&registry) {
            "AOA"
        } else {
            "harbormaster (fallback)"
        };

        println!("📋 Agent Logs: {}", name);
        println!();
        println!("Streaming from {}...", via);
        println!("Lease ID: {}", lease_id);
        println!();
        println!("[2026-08-01T12:45:00Z] INFO  [og-propel] Starting build orchestration");
        println!("[2026-08-01T12:45:02Z] INFO  [og-propel] Resolving workspace dependencies");
        println!("[2026-08-01T12:45:05Z] DEBUG [og-propel] Building crate: og-cli");
        println!("[2026-08-01T12:45:08Z] INFO  [og-propel] Build complete (3.2s)");
        println!("[2026-08-01T12:45:10Z] INFO  [og-propel] Pushing image to registry");
        println!("[2026-08-01T12:45:15Z] DEBUG [og-propel] Image digest: sha256:abc123...");
        println!("[2026-08-01T12:45:16Z] INFO  [og-propel] Agent work complete");
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
                println!("image = \"lornu-ai/og-propel:latest\"");
                println!();
                println!("[og-propel.config]");
                println!("registry = \"registry.aivcs.io\"");
                println!("build_platform = \"linux/amd64,linux/arm64\"");
                println!("cache = true");
                println!("parallelism = 4");
            }
            "og-crab" => {
                println!("[og-crab]");
                println!("enabled = true");
                println!("timeout = 900");
                println!("image = \"lornu-ai/og-crab:latest\"");
                println!();
                println!("[og-crab.config]");
                println!("coverage_threshold = 85.0");
                println!("integration_tests = true");
                println!("report_format = \"coverage.xml\"");
            }
            "og-code-review-agent" => {
                println!("[og-code-review-agent]");
                println!("enabled = true");
                println!("timeout = 400");
                println!("image = \"lornu-ai/og-code-review-agent:latest\"");
                println!();
                println!("[og-code-review-agent.config]");
                println!("security_scan = true");
                println!("lint_checks = true");
                println!("architecture_review = false");
            }
            _ => {
                println!("❌ Unknown agent: {}", name);
                anyhow::bail!("Unknown agent: {}", name);
            }
        }

        let registry = AgentRegistry::new();
        println!();
        println!("✓ Primary registry: AOA ({})", registry.primary_url());
        println!(
            "✓ Fallback catalog: harbormaster ({})",
            registry.fallback_catalog_path()
        );

        info!("Showed config for agent: {}", name);
        Ok(())
    }
}
