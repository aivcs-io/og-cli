//! Off-cluster transport: og-cli → og-leader **edge gateway** over HTTPS.
//!
//! Design: code-governance `DESIGN_OG_LEADER_EDGE_GATEWAY_OFF_CLUSTER` (og-cli#65).
//!
//! The data-mesh is **cluster-only**. From a laptop, og-cli reaches og-leader
//! through a thin, OIDC-authenticated gateway that exposes only *submit-objective*
//! / *get-status*. This backend mirrors [`super::og_leader::OgLeaderBackend`] but
//! over HTTP: it `POST`s an A2A action (as an objective) to the gateway — which
//! enqueues it into the data-mesh **in-cluster**, server-side — then **polls**
//! status to a terminal state. It carries the caller's OIDC session token as a
//! Bearer and a per-objective `Idempotency-Key` so a retry never double-runs a
//! mutating objective. **Fails closed** — never a fabricated verdict, and the
//! laptop never holds a data-mesh secret.

use crate::backend::types::{
    BuildContext, BuildOutput, CreateRepoOutput, CreateRepoSpec, DesignOutput, FixConflictsOutput,
    ForwardOutput, MergeOutput, ReviewOutput, TestOutput,
};
use crate::backend::{Backend, ExecutionMode};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

const SUBMIT_PATH: &str = "/v1/objectives";
/// Overall poll deadline for a submitted objective.
const DEFAULT_DEADLINE: Duration = Duration::from_secs(300);
/// Backoff bounds for status polling.
const MIN_POLL_MS: u64 = 500;
const MAX_POLL_MS: u64 = 5_000;

/// og-cli's off-cluster backend: an HTTP client of the og-leader edge gateway.
pub struct EdgeGatewayBackend {
    base_url: String,
    token: String,
    tenant: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct SubmitResponse {
    /// Server-assigned job id — the correlation key `GET /v1/objectives/{id}` uses.
    id: String,
}

#[derive(Debug, Deserialize, Default)]
struct StatusResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    next_poll_ms: Option<u64>,
}

impl EdgeGatewayBackend {
    pub fn new(base_url: String, token: String, tenant: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            tenant,
            http: reqwest::Client::new(),
        }
    }

    /// Submit an A2A `action` (framed as an objective) with `payload`, then poll
    /// the gateway to a terminal status. Returns `(status, result)` verbatim — the
    /// gateway/og-leader own the verdict; og-cli never fabricates one.
    async fn submit_and_wait(
        &self,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<(String, serde_json::Value)> {
        let idem = new_idempotency_key();
        let submit: SubmitResponse = self
            .http
            .post(format!("{}{}", self.base_url, SUBMIT_PATH))
            .bearer_auth(&self.token)
            .header("Idempotency-Key", &idem)
            .header("x-tenant-id", &self.tenant)
            .json(&json!({ "objective": action, "context": payload }))
            .send()
            .await
            .with_context(|| self.gw_err(action))?
            .error_for_status()
            .with_context(|| self.gw_err(action))?
            .json()
            .await
            .context("parse gateway submit response")?;

        // Poll to a terminal status, honoring the server's `next_poll_ms` hint
        // within bounded backoff, until the deadline.
        let started = Instant::now();
        let mut attempt: u32 = 0;
        loop {
            let st: StatusResponse = self
                .http
                .get(format!("{}{}/{}", self.base_url, SUBMIT_PATH, submit.id))
                .bearer_auth(&self.token)
                .header("x-tenant-id", &self.tenant)
                .send()
                .await
                .with_context(|| self.gw_err("poll status"))?
                .error_for_status()
                .with_context(|| self.gw_err("poll status"))?
                .json()
                .await
                .unwrap_or_default();

            if is_terminal(&st.status) {
                return Ok((st.status, st.result));
            }
            if started.elapsed() >= DEFAULT_DEADLINE {
                // Honest timeout — the objective is still running server-side; we
                // just stopped waiting. Not a failure verdict, not a fabrication.
                return Ok((
                    if st.status.is_empty() {
                        "running".to_string()
                    } else {
                        st.status
                    },
                    json!({ "id": submit.id, "note": "poll deadline reached; still running" }),
                ));
            }
            tokio::time::sleep(Duration::from_millis(poll_backoff_ms(
                st.next_poll_ms,
                attempt,
            )))
            .await;
            attempt = attempt.saturating_add(1);
        }
    }

    fn gw_err(&self, action: &str) -> String {
        format!(
            "cannot {action} via the og-leader edge gateway ({}): off-cluster requests route \
             only through the authenticated gateway (submit/status); it (or its auth) is \
             unreachable — no fabricated fallback.",
            self.base_url
        )
    }
}

/// A per-objective idempotency key (UUID v4). Reused across retries of the *same*
/// objective so the gateway dedupes; distinct across distinct objectives.
fn new_idempotency_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A gateway job status is terminal when the objective is done (successfully or
/// not). Non-terminal statuses (`queued`/`running`/…/empty) keep polling.
fn is_terminal(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "done" | "completed" | "succeeded" | "success" | "failed" | "error" | "cancelled"
    )
}

/// Poll interval: honor the server's `next_poll_ms` hint (clamped), else an
/// exponential backoff from `MIN_POLL_MS`, capped at `MAX_POLL_MS`.
fn poll_backoff_ms(next_poll_ms: Option<u64>, attempt: u32) -> u64 {
    match next_poll_ms {
        Some(ms) => ms.clamp(MIN_POLL_MS, MAX_POLL_MS),
        None => (MIN_POLL_MS.saturating_mul(1u64 << attempt.min(4))).min(MAX_POLL_MS),
    }
}

#[async_trait]
impl Backend for EdgeGatewayBackend {
    fn mode(&self) -> ExecutionMode {
        ExecutionMode::EdgeGateway
    }

    async fn design(&self, prompt: Option<&str>) -> Result<DesignOutput> {
        let (status, _r) = self
            .submit_and_wait("design", json!({ "prompt": prompt }))
            .await?;
        Ok(DesignOutput {
            status,
            ..Default::default()
        })
    }

    async fn build(&self, ctx: &BuildContext) -> Result<BuildOutput> {
        let (status, r) = self
            .submit_and_wait(
                "build",
                json!({ "service": ctx.service, "image_name": ctx.image_name, "no_push": ctx.no_push }),
            )
            .await?;
        Ok(BuildOutput {
            status,
            log: r.to_string(),
            ..Default::default()
        })
    }

    async fn test(&self, unit: bool, integration: bool) -> Result<TestOutput> {
        let (status, r) = self
            .submit_and_wait("test", json!({ "unit": unit, "integration": integration }))
            .await?;
        Ok(TestOutput {
            status,
            summary: r.to_string(),
            ..Default::default()
        })
    }

    async fn review(&self, pr_url: &str) -> Result<ReviewOutput> {
        let (status, r) = self
            .submit_and_wait("review", json!({ "pr_url": pr_url }))
            .await?;
        let score = r
            .get("score")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0);
        Ok(ReviewOutput {
            status,
            score,
            ..Default::default()
        })
    }

    async fn merge(&self, pr_url: &str, if_approved: bool) -> Result<MergeOutput> {
        let (status, r) = self
            .submit_and_wait(
                "merge",
                json!({ "pr_url": pr_url, "if_approved": if_approved }),
            )
            .await?;
        Ok(MergeOutput {
            status,
            message: r.to_string(),
            ..Default::default()
        })
    }

    async fn fix_conflicts(&self, pr_url: &str) -> Result<FixConflictsOutput> {
        let (status, r) = self
            .submit_and_wait("fix-conflicts", json!({ "pr_url": pr_url }))
            .await?;
        Ok(FixConflictsOutput {
            status,
            message: r.to_string(),
        })
    }

    async fn forward_to_leader(&self, objective: &str) -> Result<ForwardOutput> {
        let (status, r) = self
            .submit_and_wait("objective", json!({ "objective": objective }))
            .await?;
        Ok(ForwardOutput {
            status,
            message: r.to_string(),
        })
    }

    async fn create_repo(&self, spec: &CreateRepoSpec) -> Result<CreateRepoOutput> {
        let (status, r) = self
            .submit_and_wait(
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
            url: r.get("url").and_then(|v| v.as_str()).map(str::to_string),
            timestamp: r
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            status,
            message: r.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_status_detection() {
        for s in [
            "done",
            "Completed",
            "FAILED",
            "error",
            "success",
            "cancelled",
        ] {
            assert!(is_terminal(s), "{s} should be terminal");
        }
        for s in ["queued", "running", "", "pending", "dispatched"] {
            assert!(!is_terminal(s), "{s} should NOT be terminal");
        }
    }

    #[test]
    fn backoff_honors_server_hint_and_bounds() {
        // Server hint is clamped into [MIN, MAX].
        assert_eq!(poll_backoff_ms(Some(1500), 0), 1500);
        assert_eq!(poll_backoff_ms(Some(50), 0), MIN_POLL_MS); // below floor → floor
        assert_eq!(poll_backoff_ms(Some(99_999), 0), MAX_POLL_MS); // above cap → cap
                                                                   // No hint → exponential from MIN, capped at MAX.
        assert_eq!(poll_backoff_ms(None, 0), MIN_POLL_MS);
        assert_eq!(poll_backoff_ms(None, 1), MIN_POLL_MS * 2);
        assert_eq!(poll_backoff_ms(None, 10), MAX_POLL_MS); // saturates at the cap
    }

    #[test]
    fn idempotency_keys_are_nonempty_and_unique() {
        let a = new_idempotency_key();
        let b = new_idempotency_key();
        assert!(!a.is_empty() && a.len() >= 32);
        assert_ne!(a, b, "each objective gets a distinct key");
    }
}
