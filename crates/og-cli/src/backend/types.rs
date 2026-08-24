use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Design workflow output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignOutput {
    /// Design proposal (markdown or JSON)
    pub design: String,
    /// Architecture decisions
    pub decisions: Vec<String>,
    /// Implementation roadmap
    pub roadmap: Vec<String>,
    /// Cost/performance estimates
    pub estimates: HashMap<String, String>,
    /// Status: "pending" | "approved" | "rejected"
    pub status: String,
}

impl Default for DesignOutput {
    fn default() -> Self {
        Self {
            design: String::new(),
            decisions: vec![],
            roadmap: vec![],
            estimates: HashMap::new(),
            status: "pending".to_string(),
        }
    }
}

/// Inputs for backend build execution (FR code-governance#603).
#[derive(Debug, Clone, Default)]
pub struct BuildContext {
    /// Named og-* fleet service from the embedded catalog.
    pub service: Option<String>,
    /// Override image tag/name (catalog default when unset).
    pub image_name: Option<String>,
    /// Skip registry push (og-propel / og-builds).
    pub no_push: bool,
    /// Legacy dockerfile path for local builds.
    pub dockerfile: Option<String>,
}

/// Build workflow output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOutput {
    /// Build status: "success" | "failure"
    pub status: String,
    /// Build log URL or content
    pub log: String,
    /// Artifact path (binary, image, etc.)
    pub artifact: Option<String>,
    /// Image digest (if OCI build)
    pub image_digest: Option<String>,
    /// Build duration in seconds
    pub duration_secs: u64,
}

impl Default for BuildOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            log: String::new(),
            artifact: None,
            image_digest: None,
            duration_secs: 0,
        }
    }
}

/// Test workflow output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutput {
    /// Test status: "passed" | "failed" | "skipped"
    pub status: String,
    /// Test results summary
    pub summary: String,
    /// Code coverage percentage
    pub coverage: Option<f32>,
    /// List of failed tests
    pub failures: Vec<String>,
    /// Test duration in seconds
    pub duration_secs: u64,
}

impl Default for TestOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            summary: String::new(),
            coverage: None,
            failures: vec![],
            duration_secs: 0,
        }
    }
}

/// Code review workflow output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOutput {
    /// Review status: "approved" | "changes_requested" | "pending"
    pub status: String,
    /// Code review comments
    pub comments: Vec<ReviewComment>,
    /// Lint findings (if any)
    pub lint_findings: Vec<String>,
    /// Security findings (if any)
    pub security_findings: Vec<String>,
    /// Overall score (0-100)
    pub score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    /// File path
    pub file: String,
    /// Line number
    pub line: u32,
    /// Comment text
    pub text: String,
    /// Severity: "info" | "warning" | "error"
    pub severity: String,
}

impl Default for ReviewOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            comments: vec![],
            lint_findings: vec![],
            security_findings: vec![],
            score: 0,
        }
    }
}

/// Merge workflow output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeOutput {
    /// Merge status: "merged" | "failed" | "pending_approval"
    pub status: String,
    /// Commit SHA after merge
    pub commit_sha: Option<String>,
    /// Merge message
    pub message: String,
    /// Post-merge actions (if any)
    pub post_merge_actions: Vec<String>,
}

impl Default for MergeOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            commit_sha: None,
            message: String::new(),
            post_merge_actions: vec![],
        }
    }
}

/// Fix-merge-conflicts workflow output (routed to og-fix-merge-conflicts-agent
/// via og-leader). Like every og-leader dispatch this is a *queued ack*, not a
/// synchronous verdict — `status` carries og-leader's verbatim status
/// ("dispatched"), never a fabricated "resolved".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixConflictsOutput {
    /// Status: "dispatched" (og-leader ack) or an honest local backend verdict.
    pub status: String,
    /// Human-readable message / dispatch note (the agent works asynchronously).
    pub message: String,
}

impl Default for FixConflictsOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            message: String::new(),
        }
    }
}

/// Forward-to-og-leader output — the result of handing a fleet/free-form
/// objective to og-leader over A2A (og-leader + AOA decompose it into per-target
/// og-* runs). Like every og-leader dispatch this is a queued *ack*, not a
/// synchronous verdict — `status` carries og-leader's verbatim status
/// ("dispatched"). A fleet objective has NO local fallback, so any non-og-leader
/// backend fails closed rather than filling this in with a fabricated verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardOutput {
    /// Status: "dispatched" (og-leader ack). Never a fabricated local verdict.
    pub status: String,
    /// Human-readable dispatch note (og-leader + AOA work asynchronously).
    pub message: String,
}

impl Default for ForwardOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            message: String::new(),
        }
    }
}

/// Inputs for a **sovereign** repository-creation dispatch (code-governance#795).
///
/// Repo creation is a remote/sovereign action: og-cli → og-leader → AOA →
/// og-agent-forge. This struct carries the request to the backend seam; the
/// og-leader backend serializes it into the `create-repo` A2A payload. og-cli
/// NEVER creates a repo via a local/GitHub side channel — any non-og-leader
/// backend fails closed with this input rather than provisioning out-of-band.
#[derive(Debug, Clone, Default)]
pub struct CreateRepoSpec {
    /// Repository name (required).
    pub name: String,
    /// Visibility: `true` ⇒ private (the default), `false` ⇒ public.
    pub private: bool,
    /// Seed an initial README commit.
    pub readme: bool,
    /// SPDX license identifier to seed (e.g. `MIT`, `Apache-2.0`), if any.
    pub license: Option<String>,
    /// Default branch name override (e.g. `main`), if any.
    pub default_branch: Option<String>,
    /// Repository description, if any.
    pub description: Option<String>,
    /// Owning org/tenant (active org context when unset).
    pub org: Option<String>,
    /// Owning account (active account context when unset).
    pub account: Option<String>,
}

/// Repo-creation dispatch output (routed to og-agent-forge via og-leader).
///
/// Like every og-leader dispatch this is a queued *ack*, not a synchronous
/// verdict — `status` carries og-leader's verbatim status ("dispatched"). The
/// repo `url` and `timestamp` are surfaced ONLY when og-leader's response
/// payload actually includes them; og-cli never fabricates a URL/timestamp from
/// a bare dispatch-ack. A non-og-leader backend has NO local repo creation, so
/// it fails closed rather than filling this in with a fabricated result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRepoOutput {
    /// Status: "dispatched" (og-leader ack). Never a fabricated local verdict.
    pub status: String,
    /// Human-readable message / dispatch note (og-agent-forge works async).
    pub message: String,
    /// Created repository URL — present only if og-leader returned one.
    pub url: Option<String>,
    /// Timestamp from og-leader's response, if any (never fabricated).
    pub timestamp: Option<String>,
}

impl Default for CreateRepoOutput {
    fn default() -> Self {
        Self {
            status: "pending".to_string(),
            message: String::new(),
            url: None,
            timestamp: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_outputs() {
        let design = DesignOutput::default();
        assert_eq!(design.status, "pending");

        let build = BuildOutput::default();
        assert_eq!(build.status, "pending");

        let test = TestOutput::default();
        assert_eq!(test.status, "pending");

        let review = ReviewOutput::default();
        assert_eq!(review.status, "pending");

        let merge = MergeOutput::default();
        assert_eq!(merge.status, "pending");

        let create_repo = CreateRepoOutput::default();
        assert_eq!(create_repo.status, "pending");
        assert!(create_repo.url.is_none());
    }
}
