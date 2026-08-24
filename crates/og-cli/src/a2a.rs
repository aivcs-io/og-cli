//! og-cli → og-leader over **A2A on the data-mesh standard offering**
//! (`og-agent-data-mesh` / `envelope-data-mesh`), per code-governance#641/#640.
//!
//! og-leader is an **agent, not a server** — there is no HTTP endpoint to POST to.
//! og-cli reaches it by publishing an [`A2AWork`] item onto the shared,
//! tenant-scoped data-mesh inbox collection; the og-leader agent consumes it and
//! translates it into an AOA handoff. og-cli's ONLY outward seam is this
//! offering (it is not the data-mesh impl — it binds the sanctioned facade).
//!
//! **Fails closed** (og-bug-report#31): if the data-mesh work-plane is
//! unreachable/unconfigured, dispatch errors — og-cli never fabricates a result.
//! Dispatch is a *queued ack*, not a synchronous verdict: the agent processes
//! asynchronously, so `test`/`review` surface `"dispatched"` and the PR gate
//! correctly declines until a real outcome exists.

use anyhow::{anyhow, Result};
use envelope_data_mesh::DataMeshClient;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::discovery;

/// Tenant-scoped collection og-cli publishes A2A work into; the og-leader agent
/// consumes it. MUST match `og_leader::agent::A2A_INBOX`.
pub const A2A_INBOX: &str = "a2a_og_leader_inbox";

/// One A2A work item — the shape the og-leader agent consumes
/// (`og_leader::agent::A2AWork`). Keep these in lockstep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AWork {
    pub id: String,
    pub action: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Result of a dispatch. `status` is `"dispatched"` (queued on the inbox) — never
/// a synthesized pass/verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AResponse {
    pub id: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRequest {
    pub service: String,
    pub repo: String,
    pub flake_ref: String,
    pub registry: String,
    pub push: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRequest {
    pub service: String,
    pub cluster: String,
    pub namespace: String,
}

pub struct A2AClient;

impl A2AClient {
    /// Dispatch an action to og-leader by publishing an [`A2AWork`] item onto the
    /// data-mesh inbox (`og-agent-data-mesh` offering, tenant-scoped by the
    /// facade's env config). **Fails closed** when the work-plane is
    /// unreachable/unconfigured. Idempotent by work id.
    pub async fn dispatch(
        action: &str,
        payload: serde_json::Value,
        tenant: &str,
    ) -> Result<A2AResponse> {
        // Resolve data-mesh endpoint for off-cluster use. If DATA_MESH_URL is not
        // already set (or is the in-cluster default), try to discover it via
        // service-discovery (code-governance#1027). Fall back to from_env() if
        // resolution fails (off-cluster will get a clear error; in-cluster uses the default).
        if std::env::var("DATA_MESH_URL").is_err() {
            if let Ok(url) = discovery::resolve_data_mesh_url().await {
                std::env::set_var("DATA_MESH_URL", &url);
                info!("resolved data-mesh endpoint from service-discovery: {url}");
            }
            // If resolution fails, let from_env() handle it (it will either use the
            // in-cluster default if DATA_MESH_URL is unset, or fail with a clear error).
        }

        // Bind the sanctioned data-mesh facade; unconfigured/unreachable ⇒ Err.
        let mesh = DataMeshClient::from_env().map_err(|e| {
            anyhow!(
                "og-cli cannot reach og-leader: the A2A data-mesh work-plane \
                 (og-agent-data-mesh offering) is unavailable — {e}. og-cli routes \
                 only through og-leader over this offering; no fabricated fallback."
            )
        })?;

        let id = uuid::Uuid::new_v4().to_string();
        // Carry the target repo through when the caller included one, so the
        // og-leader agent can scope the campaign envelope.
        let repo = payload
            .get("repo")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let work = A2AWork {
            id: id.clone(),
            action: action.to_string(),
            repo,
            payload,
        };

        mesh.put_idempotent(A2A_INBOX, &id, &work)
            .await
            .map_err(|e| {
                anyhow!("publishing A2A {action} to og-leader inbox {A2A_INBOX:?} failed: {e}")
            })?;
        info!("A2A dispatch: id={id} action={action} -> data-mesh {A2A_INBOX} (tenant {tenant})");

        // Queued ack — the og-leader agent consumes and processes asynchronously.
        // NOT a verdict: never synthesize a pass here.
        Ok(A2AResponse {
            id,
            status: "dispatched".to_string(),
            payload: serde_json::json!({
                "inbox": A2A_INBOX,
                "note": "queued on the data-mesh A2A inbox; og-leader agent processes asynchronously",
            }),
            error: None,
        })
    }

    /// Publish a build request to og-leader over the data-mesh A2A offering.
    pub async fn build_service(req: &BuildRequest, tenant: &str) -> Result<A2AResponse> {
        Self::dispatch("build", serde_json::to_value(req)?, tenant).await
    }

    /// Publish a deploy request to og-leader over the data-mesh A2A offering.
    pub async fn deploy_service(req: &DeployRequest, tenant: &str) -> Result<A2AResponse> {
        Self::dispatch("deploy", serde_json::to_value(req)?, tenant).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_request_serialization() {
        let req = BuildRequest {
            service: "og-leader".to_string(),
            repo: "aivcs://acme/og-leader".to_string(),
            flake_ref: ".#image".to_string(),
            registry: "registry.example".to_string(),
            push: true,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["service"], "og-leader");
    }

    #[tokio::test]
    async fn test_dispatch_fails_closed_when_data_mesh_unconfigured() {
        // No DATA_MESH_* env → the data-mesh facade can't bind → dispatch must
        // FAIL CLOSED, never fabricate a "queued (stub)" success (og-bug-report#31).
        for k in ["DATA_MESH_URL", "DATA_MESH_TENANT_ID"] {
            std::env::remove_var(k);
        }
        let req = BuildRequest {
            service: "og-leader".to_string(),
            repo: "aivcs://acme/og-leader".to_string(),
            flake_ref: ".#image".to_string(),
            registry: "registry.example".to_string(),
            push: true,
        };
        let resp = A2AClient::build_service(&req, "acme").await;
        assert!(resp.is_err(), "must fail closed, not fabricate a stub");
    }
}
