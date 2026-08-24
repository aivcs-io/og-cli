use anyhow::Result;
use tracing::info;

/// System health check for og-cli dependencies
#[derive(Debug, Clone)]
pub struct HealthCheck {
    pub aoa_reachable: bool,
    pub kubernetes_available: bool,
    pub harbormaster_available: bool,
    pub github_reachable: bool,
}

impl HealthCheck {
    /// Run all health checks
    pub async fn run() -> Result<Self> {
        info!("Running system health checks...");

        Ok(Self {
            aoa_reachable: Self::check_aoa().await,
            kubernetes_available: Self::check_kubernetes().await,
            harbormaster_available: Self::check_harbormaster().await,
            github_reachable: Self::check_github().await,
        })
    }

    /// Check if AOA endpoint is reachable
    async fn check_aoa() -> bool {
        let Ok(aoa_url) = std::env::var("AOA_URL") else {
            info!("AOA_URL not set — skipping AOA health check");
            return false;
        };
        if aoa_url.trim().is_empty() {
            return false;
        }

        info!("Checking AOA reachability at: {}", aoa_url);

        // Try to connect to AOA endpoint
        if let Ok(client) = reqwest::Client::new()
            .get(format!("{}/health", aoa_url))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            client.status().is_success()
        } else {
            info!("AOA unreachable (expected in local dev)");
            false
        }
    }

    /// Check if Kubernetes cluster is available
    async fn check_kubernetes() -> bool {
        info!("Checking Kubernetes cluster availability...");

        // Check if KUBECONFIG is set or kubectl is available
        let kubeconfig_set = std::env::var("KUBECONFIG").is_ok();
        let kubectl_available = which::which("kubectl").is_ok();

        if kubeconfig_set || kubectl_available {
            info!("Kubernetes: potentially available");
            true
        } else {
            info!("Kubernetes: not configured");
            false
        }
    }

    /// Check if Harbormaster is available
    async fn check_harbormaster() -> bool {
        info!("Checking Harbormaster availability...");

        // Check if 'hm' binary is in PATH
        match which::which("hm") {
            Ok(_) => {
                info!("Harbormaster: available in PATH");
                true
            }
            Err(_) => {
                info!("Harbormaster: not found in PATH");
                false
            }
        }
    }

    /// Check if GitHub is reachable
    async fn check_github() -> bool {
        info!("Checking GitHub API reachability...");

        match reqwest::Client::new()
            .get("https://api.github.com/")
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("GitHub: reachable");
                    true
                } else {
                    info!("GitHub: returned status {}", resp.status());
                    true // GitHub API is up, just rate-limited maybe
                }
            }
            Err(e) => {
                info!("GitHub: unreachable ({})", e);
                false
            }
        }
    }

    /// Get a summary of the health check
    pub fn summary(&self) -> String {
        let mut status = String::new();

        status.push_str("System Health:\n");
        status.push_str(&format!(
            "  AOA: {}\n",
            if self.aoa_reachable { "✅" } else { "⚠️" }
        ));
        status.push_str(&format!(
            "  Kubernetes: {}\n",
            if self.kubernetes_available {
                "✅"
            } else {
                "⚠️"
            }
        ));
        status.push_str(&format!(
            "  Harbormaster: {}\n",
            if self.harbormaster_available {
                "✅"
            } else {
                "⚠️"
            }
        ));
        status.push_str(&format!(
            "  GitHub: {}\n",
            if self.github_reachable {
                "✅"
            } else {
                "⚠️"
            }
        ));

        status
    }

    /// Check if at least one backend is available
    pub fn has_backend(&self) -> bool {
        self.aoa_reachable || self.kubernetes_available || self.harbormaster_available
    }

    /// Recommend a fallback strategy
    pub fn recommend_backend(&self) -> &'static str {
        if self.aoa_reachable {
            "AOA (agent-orchestrated)"
        } else if self.kubernetes_available {
            "Kubernetes (distributed agents)"
        } else if self.harbormaster_available {
            "Harbormaster (tool orchestrator)"
        } else {
            "Local (git + cargo + docker)"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_check_new() {
        let hc = HealthCheck {
            aoa_reachable: false,
            kubernetes_available: false,
            harbormaster_available: false,
            github_reachable: false,
        };

        assert!(!hc.has_backend());
        assert_eq!(hc.recommend_backend(), "Local (git + cargo + docker)");
    }

    #[test]
    fn test_health_check_with_kubernetes() {
        let hc = HealthCheck {
            aoa_reachable: false,
            kubernetes_available: true,
            harbormaster_available: false,
            github_reachable: false,
        };

        assert!(hc.has_backend());
        assert_eq!(hc.recommend_backend(), "Kubernetes (distributed agents)");
    }

    #[test]
    fn test_health_check_priority() {
        let hc = HealthCheck {
            aoa_reachable: true,
            kubernetes_available: true,
            harbormaster_available: true,
            github_reachable: false,
        };

        assert!(hc.has_backend());
        assert_eq!(hc.recommend_backend(), "AOA (agent-orchestrated)");
    }

    #[test]
    fn test_health_summary() {
        let hc = HealthCheck {
            aoa_reachable: true,
            kubernetes_available: false,
            harbormaster_available: true,
            github_reachable: true,
        };

        let summary = hc.summary();
        assert!(summary.contains("AOA: ✅"));
        assert!(summary.contains("Kubernetes: ⚠️"));
        assert!(summary.contains("Harbormaster: ✅"));
        assert!(summary.contains("GitHub: ✅"));
    }
}
