//! Workflow lifecycle client for og-cli (FR code-governance#635 Phase 2.1).
//!
//! Starts a tracked workflow before build/test/review, surfaces the workflow id
//! to the operator, and completes it when the backend returns.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Active workflow handle returned when a command starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowHandle {
    pub id: String,
    pub action: String,
    pub title: String,
}

pub struct WorkflowClient;

impl WorkflowClient {
    /// Begin a tracked workflow; the shell can use the returned id for follow-up.
    pub async fn start(action: &str, title: &str, tenant: &str) -> Result<WorkflowHandle> {
        let id = format!(
            "{}-{}",
            chrono::Utc::now().timestamp(),
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("wf")
        );

        // The workflow id is local; og-leader coordination happens via the A2A
        // data-mesh dispatch (og-agent-data-mesh offering), not an HTTP probe.
        info!(
            workflow_id = %id,
            action = action,
            tenant = tenant,
            "workflow started"
        );

        let handle = WorkflowHandle {
            id: id.clone(),
            action: action.to_string(),
            title: title.to_string(),
        };

        println!(
            "Workflow: {} (use the AivcsOps session to watch)",
            handle.id
        );
        Ok(handle)
    }

    /// Mark workflow complete and print summary line.
    pub fn complete(handle: &WorkflowHandle, success: bool) {
        let mark = if success { "✓ done" } else { "✗ failed" };
        println!(
            "Workflow: {} {} — {} ({})",
            handle.id, mark, handle.title, handle.action
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_returns_printable_handle() {
        let h = WorkflowClient::start("build", "AivcsOps build: main", "acme")
            .await
            .unwrap();
        assert!(!h.id.is_empty());
        assert_eq!(h.action, "build");
    }
}
