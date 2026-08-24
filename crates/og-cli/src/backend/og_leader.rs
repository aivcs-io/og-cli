//! The **only** sovereign backend: og-cli → og-leader over **A2A on the
//! data-mesh standard offering** (`og-agent-data-mesh`), per code-governance#641.
//!
//! Retires the fabricating AOA/Kubernetes/Harbormaster stub backends
//! (og-bug-report#31, code-governance#630). Every workflow action is published as
//! an A2A work item onto og-leader's data-mesh inbox; the og-leader **agent**
//! (not a server) consumes it and owns all downstream execution (AOA → og-*
//! agents) and durable state — both in-cluster, which og-cli never dials directly.
//!
//! **No fabrication, ever.** Publishing an item is an async *dispatch* ack, not a
//! synchronous verdict — so `test`/`review` map to a non-passing `"dispatched"`
//! status, and the PR gate correctly **declines** rather than inventing a pass.
//! A real gate verdict arrives only when og-leader records one; until then the
//! honest answer is "not gated". If the data-mesh work-plane is unreachable,
//! every call **fails closed** (propagates the error) — never a stub.

use crate::a2a::A2AClient;
use crate::backend::types::{
    BuildContext, BuildOutput, CreateRepoOutput, CreateRepoSpec, DesignOutput, FixConflictsOutput,
    ForwardOutput, MergeOutput, ReviewOutput, TestOutput,
};
use crate::backend::{Backend, ExecutionMode};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;

/// og-cli's sole backend: an A2A client of og-leader.
pub struct OgLeaderBackend {
    tenant: String,
}

impl OgLeaderBackend {
    pub fn new(config: &crate::config::ProjectConfig) -> Self {
        Self::new_with_org(config, None)
    }

    /// Construct with an explicit routing org when one is known (e.g. detected
    /// from a PR URL — multi-org support, code-governance#785). When `org` is
    /// `None` the tenant resolves from `OG_ORG` / the active context / default.
    /// Never fabricated — just the routing tenant header.
    pub fn new_with_org(_config: &crate::config::ProjectConfig, org: Option<String>) -> Self {
        let tenant = org.unwrap_or_else(|| crate::org::OrgManager::resolve_org(None));
        Self { tenant }
    }

    /// Dispatch an action to og-leader over the A2A data-mesh offering, failing
    /// closed (no stub) when the work-plane is unreachable.
    async fn dispatch(
        &self,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<crate::a2a::A2AResponse> {
        A2AClient::dispatch(action, payload, &self.tenant)
            .await
            .with_context(|| {
                // Laptop case (og-bug-report#50): not off-cluster and no in-cluster
                // data-mesh config ⇒ append the actionable remedy (the edge gateway
                // + login) instead of leaving the user with a bare mesh error. The
                // deployment injects DATA_MESH_* in-cluster, so this hint only shows
                // off-cluster.
                let laptop_hint = if std::env::var("OG_OFF_CLUSTER").is_err()
                    && std::env::var("DATA_MESH_URL").is_err()
                    && std::env::var("DATA_MESH_TENANT_ID").is_err()
                {
                    let login_hint = if crate::auth::AuthManager::access_token().is_some() {
                        ""
                    } else {
                        " and use `/login` inside `og` (no valid session — your token is missing/expired)"
                    };
                    format!(
                        " From a developer laptop, set OG_OFF_CLUSTER=true to reach og-leader \
                         over HTTPS via the edge gateway{login_hint}. In-cluster, DATA_MESH_* \
                         are injected by the deployment. (og-bug-report#50)"
                    )
                } else {
                    String::new()
                };
                format!(
                    "cannot {action}: og-cli routes only through og-leader over the A2A \
                     data-mesh offering (og-agent-data-mesh) — no fabricated fallback. \
                     Configure DATA_MESH_URL / DATA_MESH_TENANT_ID for a reachable mesh.{laptop_hint}"
                )
            })
    }
}

#[async_trait]
impl Backend for OgLeaderBackend {
    fn mode(&self) -> ExecutionMode {
        ExecutionMode::OgLeader
    }

    async fn design(&self, prompt: Option<&str>) -> Result<DesignOutput> {
        let resp = self.dispatch("design", json!({ "prompt": prompt })).await?;
        Ok(DesignOutput {
            design: resp
                .payload
                .get("design")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            // A dispatch-ack is not a completed design; surface og-leader's status
            // verbatim ("dispatched"/"ok") rather than claiming "approved".
            status: resp.status,
            ..Default::default()
        })
    }

    async fn build(&self, ctx: &BuildContext) -> Result<BuildOutput> {
        let resp = self
            .dispatch(
                "build",
                json!({
                    "service": ctx.service,
                    "image_name": ctx.image_name,
                    "no_push": ctx.no_push,
                }),
            )
            .await?;
        Ok(BuildOutput {
            status: resp.status,
            log: resp.payload.to_string(),
            ..Default::default()
        })
    }

    async fn test(&self, unit: bool, integration: bool) -> Result<TestOutput> {
        let resp = self
            .dispatch("test", json!({ "unit": unit, "integration": integration }))
            .await?;
        // og-leader acks the dispatch; it is NOT a synchronous "passed" verdict.
        // Surface its status verbatim so the gate declines until a real verdict
        // exists — never hardcode "passed".
        Ok(TestOutput {
            status: resp.status,
            summary: resp.payload.to_string(),
            ..Default::default()
        })
    }

    async fn review(&self, pr_url: &str) -> Result<ReviewOutput> {
        let resp = self.dispatch("review", json!({ "pr_url": pr_url })).await?;
        // Only trust a score if og-leader actually returned one; otherwise 0
        // (default) — the gate's min-score check then declines honestly.
        let score = resp
            .payload
            .get("score")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0);
        Ok(ReviewOutput {
            status: resp.status,
            score,
            ..Default::default()
        })
    }

    async fn merge(&self, pr_url: &str, if_approved: bool) -> Result<MergeOutput> {
        let resp = self
            .dispatch(
                "merge",
                json!({ "pr_url": pr_url, "if_approved": if_approved }),
            )
            .await?;
        Ok(MergeOutput {
            status: resp.status,
            message: resp.payload.to_string(),
            ..Default::default()
        })
    }

    async fn fix_conflicts(&self, pr_url: &str) -> Result<FixConflictsOutput> {
        // The `fix-conflicts` A2A action is routed by og-leader to
        // og-fix-merge-conflicts-agent (via AOA). This is a dispatch-ack — NOT a
        // synchronous "resolved" verdict; surface og-leader's status verbatim.
        let resp = self
            .dispatch("fix-conflicts", json!({ "pr_url": pr_url }))
            .await?;
        Ok(FixConflictsOutput {
            status: resp.status,
            message: resp.payload.to_string(),
        })
    }

    async fn forward_to_leader(&self, objective: &str) -> Result<ForwardOutput> {
        // The `objective` A2A action carries a free-form / fleet-scoped goal to
        // og-leader, which (with AOA) plans and dispatches the required og-* agents
        // — og-cli never chooses the agent (code-governance#769). This is a
        // dispatch-ack, NOT a synchronous verdict; surface og-leader's status
        // verbatim. Fails closed (no fabricated fallback) when the mesh is
        // unreachable — a fleet objective has no local execution.
        let resp =
            A2AClient::dispatch("objective", json!({ "objective": objective }), &self.tenant)
                .await
                .context(
                    "cannot forward objective to og-leader: og-cli routes only through og-leader \
                 over the A2A data-mesh offering (og-agent-data-mesh) — no fabricated local \
                 fallback for a fleet objective. Configure DATA_MESH_URL / DATA_MESH_TENANT_ID \
                 for a reachable mesh.",
                )?;
        Ok(ForwardOutput {
            status: resp.status,
            message: resp.payload.to_string(),
        })
    }

    async fn create_repo(&self, spec: &CreateRepoSpec) -> Result<CreateRepoOutput> {
        // The `create-repo` A2A action is routed by og-leader to og-agent-forge
        // (via AOA). This is a dispatch-ack — NOT a synchronous "created"
        // verdict; surface og-leader's status verbatim and only report a
        // URL/timestamp when og-leader's response payload actually carries one
        // (never fabricated from a bare ack). Fails closed (no side channel)
        // when the mesh is unreachable.
        let resp = self
            .dispatch(
                "create-repo",
                json!({
                    "name": spec.name,
                    "visibility": if spec.private { "private" } else { "public" },
                    "readme": spec.readme,
                    "license": spec.license,
                    "default_branch": spec.default_branch,
                    "description": spec.description,
                    "org": spec.org,
                    "account": spec.account,
                }),
            )
            .await?;
        Ok(CreateRepoOutput {
            url: resp
                .payload
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            timestamp: resp
                .payload
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            status: resp.status,
            message: resp.payload.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `create-repo` dispatch routes ONLY through og-leader over the A2A
    /// data-mesh offering. With no `DATA_MESH_*` env the facade can't bind, so it
    /// must **fail closed** — never create a repo via a local/GitHub side channel
    /// (code-governance#795), mirroring the a2a fail-closed contract.
    #[tokio::test]
    async fn test_create_repo_fails_closed_when_data_mesh_unconfigured() {
        for k in ["DATA_MESH_URL", "DATA_MESH_TENANT_ID"] {
            std::env::remove_var(k);
        }
        let backend = OgLeaderBackend::new(&crate::config::ProjectConfig::default());
        let spec = CreateRepoSpec {
            name: "acme-web".to_string(),
            private: true,
            readme: true,
            license: Some("MIT".to_string()),
            ..Default::default()
        };
        let res = backend.create_repo(&spec).await;
        assert!(
            res.is_err(),
            "create-repo must fail closed when the mesh is unreachable, never a side channel"
        );
    }
}
