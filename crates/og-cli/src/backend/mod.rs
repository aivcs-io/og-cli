use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use tracing::info;

pub mod fallback;
pub mod gateway;
#[cfg(feature = "harbormaster-fallback")]
pub mod harbormaster;
pub mod local;
pub mod og_leader;
pub mod types;

pub use fallback::FallbackBackend;
#[cfg(feature = "harbormaster-fallback")]
pub use harbormaster::HarbormasterBackend;
pub use types::{
    BuildContext, BuildOutput, CreateRepoOutput, CreateRepoSpec, DesignOutput, FixConflictsOutput,
    ForwardOutput, MergeOutput, ReviewComment, ReviewOutput, TestOutput,
};

/// Execution mode determines which backend to use for workflow commands.
///
/// og-cli connects to exactly one sovereign seam — **og-leader** (over A2A);
/// `Local` is the honest, non-sovereign dev mode that runs real local tools
/// (git/cargo/docker) on this machine. The former AOA/Kubernetes/Harbormaster
/// modes were removed: they dialed the wrong seams and returned fabricated
/// results (og-bug-report#31, code-governance#630).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Local execution: real git + cargo + docker on this machine (dev only).
    Local,
    /// og-leader over A2A: the sole sovereign backend. og-leader owns all
    /// downstream execution (AOA → og-* agents) and durable state.
    OgLeader,
    /// og-leader **edge gateway** over HTTPS — the off-cluster transport
    /// (og-cli#65). Same sovereign semantics as `OgLeader`; the gateway enqueues
    /// into the data-mesh in-cluster on the caller's behalf.
    EdgeGateway,
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ExecutionMode::Local => write!(f, "local"),
            ExecutionMode::OgLeader => write!(f, "og-leader"),
            ExecutionMode::EdgeGateway => write!(f, "og-leader-gateway"),
        }
    }
}

/// Core backend abstraction trait for workflow execution
#[async_trait]
pub trait Backend: Send + Sync {
    /// Returns the execution mode for this backend
    fn mode(&self) -> ExecutionMode;

    /// Design: Generate architecture/design from prompt
    async fn design(&self, prompt: Option<&str>) -> Result<DesignOutput>;

    /// Build: Compile/package project or submit og-propel build for a fleet service.
    async fn build(&self, ctx: &BuildContext) -> Result<BuildOutput>;

    /// Test: Run unit and integration tests
    async fn test(&self, unit: bool, integration: bool) -> Result<TestOutput>;

    /// Review: Analyze code and review PR
    async fn review(&self, pr_url: &str) -> Result<ReviewOutput>;

    /// Merge: Merge approved PR
    async fn merge(&self, pr_url: &str, if_approved: bool) -> Result<MergeOutput>;

    /// FixConflicts: resolve a PR's merge conflicts by dispatching to
    /// **og-fix-merge-conflicts-agent** (og-cli → og-leader → AOA → the agent).
    /// og-cli only *dispatches* — it never resolves conflicts locally (that would
    /// be a thin-agent/WB1 violation).
    ///
    /// SEAM — the other og-* agent verbs plug in exactly here, one method each,
    /// dispatching their own A2A action to og-leader (see the matching PlanStep
    /// variants in `intent.rs`):
    ///   rebase            → og-pull-request-rebase
    ///   fix-failing-checks → og-fix-failing-checks-agent
    ///   diagnose          → og-diagnostic
    ///   cleanup           → og-cleanup
    ///   red-team          → og-red-teaming
    async fn fix_conflicts(&self, pr_url: &str) -> Result<FixConflictsOutput>;

    /// ForwardToLeader: hand a free-form / **fleet-scoped** objective to og-leader
    /// over A2A. og-leader + AOA plan and decompose it into the required og-* agent
    /// runs (e.g. a per-PR og-crab fan-out for "test all open PRs across the org").
    ///
    /// og-cli NEVER decomposes or executes a fleet objective locally — there is no
    /// local "test every PR in the org", so a single local tool run would only
    /// touch the current checkout and fabricate a verdict for work that never
    /// happened. Any **non-og-leader** backend (harbormaster/local) MUST therefore
    /// **fail closed** here, exactly like `merge`/`review`/`fix_conflicts` (#49) —
    /// this holds even when the harbormaster fallback is enabled (og-cli#48).
    async fn forward_to_leader(&self, objective: &str) -> Result<ForwardOutput>;

    /// CreateRepo: provision a repository through the sovereign path — og-cli →
    /// og-leader → AOA → **og-agent-forge**. og-cli only *dispatches* (a queued
    /// ack, not a synchronous "created" verdict); it surfaces og-leader's status
    /// verbatim and never fabricates a repo URL from a bare ack.
    ///
    /// Repo creation is remote/sovereign — there is NO local/GitHub side channel
    /// (`gh repo create` would bypass org policy, identity, and the audit trail
    /// og-agent-forge enforces). So every **non-og-leader** backend
    /// (harbormaster/local) MUST **fail closed** here, exactly like
    /// `merge`/`review`/`fix_conflicts`/`forward_to_leader` — this holds even
    /// when the harbormaster fallback is enabled (code-governance#795).
    async fn create_repo(&self, spec: &CreateRepoSpec) -> Result<CreateRepoOutput>;
}

/// Backend factory for runtime selection.
pub struct BackendFactory;

impl BackendFactory {
    /// Select the backend.
    ///
    /// og-cli routes only through **og-leader** (over A2A) — the single sovereign
    /// *network* seam. `OG_BACKEND=local` opts into honest local-tool execution
    /// for dev; it is non-sovereign and labeled as such.
    ///
    /// When the **harbormaster fallback** is enabled (via `/config`, persisted in
    /// `~/.og/config.toml`, or `OG_FALLBACK=1`) the og-leader primary is wrapped
    /// in a [`FallbackBackend`]: it still tries og-leader first, and **only** when
    /// og-leader reports the mesh work-plane unreachable does it fall back to the
    /// local `hm` orchestrator. Every other failure still **fails closed** — the
    /// og-bug-report#31 contract is preserved (see [`FallbackBackend`]).
    pub async fn select(config: &crate::config::ProjectConfig) -> Result<Box<dyn Backend>> {
        Self::select_for_org(config, None).await
    }

    /// Like [`select`], but routes the A2A tenant to an explicit org when one is
    /// known (e.g. detected from a PR URL — code-governance#785 multi-org). When
    /// `org` is `None` the tenant resolves from `OG_ORG` / the active context.
    pub async fn select_for_org(
        config: &crate::config::ProjectConfig,
        org: Option<String>,
    ) -> Result<Box<dyn Backend>> {
        if std::env::var("OG_BACKEND").unwrap_or_default() == "local" {
            info!("Backend selected: Local (real local tools; non-sovereign, dev only)");
            return Ok(Box::new(local::LocalBackend::new()));
        }

        // Off-cluster (og-cli#65): reach og-leader through the authenticated edge
        // gateway (submit/status over HTTPS) instead of the in-cluster data-mesh.
        // Opt in with OG_OFF_CLUSTER; the fast in-cluster-vs-edge auto-detect is a
        // follow-up (P4), tunable once the gateway is live. Fails closed — no
        // gateway or no session ⇒ error, never a local fabrication.
        if off_cluster_requested().await {
            let gateway_url = crate::discovery::resolve_og_leader_gateway_url()
                .await
                .context(
                    "OG_OFF_CLUSTER set but no og-leader edge gateway is discoverable \
                     (service-discovery capability `og-leader`) — nothing to route to",
                )?;
            let token = crate::auth::AuthManager::access_token().context(
                "off-cluster gateway requires a logged-in session — use `/login` inside `og` first",
            )?;
            let tenant = org
                .clone()
                .unwrap_or_else(|| crate::org::OrgManager::resolve_org(None));
            info!("Backend selected: og-leader edge gateway (off-cluster) at {gateway_url}");
            return Ok(Box::new(gateway::EdgeGatewayBackend::new(
                gateway_url,
                token,
                tenant,
            )));
        }

        let primary = og_leader::OgLeaderBackend::new_with_org(config, org);

        if Self::fallback_enabled() {
            #[cfg(feature = "harbormaster-fallback")]
            {
                info!(
                    "Backend selected: og-leader (A2A) primary + harbormaster fallback \
                     (fallback taken only on mesh-unreachable)"
                );
                return Ok(Box::new(FallbackBackend::new(
                    Box::new(primary),
                    Box::new(HarbormasterBackend::new()),
                )));
            }
            #[cfg(not(feature = "harbormaster-fallback"))]
            {
                anyhow::bail!(
                    "harbormaster fallback is enabled (OG_FALLBACK or ~/.og/config.toml) \
                     but this build of og-cli was compiled without the `harbormaster-fallback` feature"
                );
            }
        }

        info!("Backend selected: og-leader (A2A) — the sole sovereign seam");
        Ok(Box::new(primary))
    }

    /// Whether the harbormaster fallback is enabled.
    ///
    /// **Policy (settles code-governance#630 vs #758): opt-in, default OFF.**
    /// Dev/test **fail closed** when og-leader is unreachable — a mesh miss must
    /// never degrade to a fabricated local verdict (satisfies code-governance#630 /
    /// og-bug-report#31). Production may opt into the harbormaster fallback via
    /// `fallback_enabled = true` in `~/.og/config.toml` (or `/config`), or the
    /// `OG_FALLBACK=1` env override (#758) — which also keeps the fallback path
    /// testable. The `.unwrap_or(false)` below is that default-off gate: when no
    /// knob is set, selection returns the strict og-leader-only backend.
    ///
    /// Note: enabling the fallback does **not** weaken the fleet/forward path — a
    /// fleet objective still fails closed even here, because harbormaster's
    /// `forward_to_leader` has no local fallback and bails (og-cli#48).
    fn fallback_enabled() -> bool {
        // Env override wins when set (OG_FALLBACK=0 disables even if config enables);
        // otherwise fall back to the persisted `~/.og/config.toml` setting.
        if std::env::var("OG_FALLBACK").is_ok() {
            return Self::env_flag("OG_FALLBACK");
        }
        crate::persist::GlobalConfig::load()
            .map(|c| c.fallback_enabled)
            .unwrap_or(false)
    }

    /// Env-flag reader (accepts `1`/`true`/`on`/`yes`), shared by the fallback and
    /// off-cluster gates.
    fn env_flag(name: &str) -> bool {
        std::env::var(name)
            .map(|v| {
                matches!(
                    v.trim().to_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            })
            .unwrap_or(false)
    }
}

/// Whether the caller is running off-cluster.
///
/// If `OG_OFF_CLUSTER` is explicitly configured in the environment, we respect
/// its flag value. Otherwise, we auto-detect by attempting to resolve the in-cluster
/// data-mesh DNS endpoint using `tokio::net::lookup_host`. If resolution fails, we
/// assume the CLI is running off-cluster.
async fn off_cluster_requested() -> bool {
    if std::env::var("OG_OFF_CLUSTER").is_ok() {
        return BackendFactory::env_flag("OG_OFF_CLUSTER");
    }

    // Skip auto-detection in unit and integration tests to prevent mock-free network calls
    if cfg!(test) || std::env::var("CARGO_MANIFEST_DIR").is_ok() {
        return false;
    }

    let mut test_host = "data-fabric.data-fabric.svc.cluster.local:80".to_string();
    if let Ok(url_str) = std::env::var("DATA_MESH_URL") {
        if let Ok(url) = reqwest::Url::parse(&url_str) {
            if let Some(host) = url.host_str() {
                let port = url.port().unwrap_or(80);
                test_host = format!("{}:{}", host, port);
            }
        }
    }

    // Use tokio's non-blocking async lookup_host
    let in_cluster = tokio::net::lookup_host(&test_host).await.is_ok();
    !in_cluster
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both assertions live in ONE test: `OG_BACKEND` is process-global, so
    // splitting them into parallel tests races. Sequential here = deterministic.
    #[tokio::test]
    async fn test_backend_selection_modes() {
        let config = crate::config::ProjectConfig::default();

        // Default is the sovereign og-leader backend — never a fabricating one.
        // Selection stays infallible; the fail-closed happens at dispatch time
        // (see og_leader::dispatch, and og-bug-report#50 for the laptop hint added
        // to that dispatch error).
        std::env::remove_var("OG_BACKEND");
        assert_eq!(
            BackendFactory::select(&config).await.unwrap().mode(),
            ExecutionMode::OgLeader
        );

        // `OG_BACKEND=local` opts into honest local-tool execution.
        std::env::set_var("OG_BACKEND", "local");
        assert_eq!(
            BackendFactory::select(&config).await.unwrap().mode(),
            ExecutionMode::Local
        );
        std::env::remove_var("OG_BACKEND");
    }

    #[test]
    fn test_execution_mode_display() {
        assert_eq!(ExecutionMode::Local.to_string(), "local");
        assert_eq!(ExecutionMode::OgLeader.to_string(), "og-leader");
    }
}
