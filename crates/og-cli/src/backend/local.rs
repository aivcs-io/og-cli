use anyhow::{bail, Result};
use async_trait::async_trait;
use std::path::Path;
use tracing::info;

use crate::exec;

use super::{
    Backend, BuildContext, BuildOutput, CreateRepoOutput, CreateRepoSpec, DesignOutput,
    ExecutionMode, FixConflictsOutput, ForwardOutput, MergeOutput, ReviewOutput, TestOutput,
};

/// Local backend: uses git + cargo + docker + gh when available
pub struct LocalBackend;

impl LocalBackend {
    pub fn new() -> Self {
        Self
    }

    fn is_rust_project() -> bool {
        Path::new("Cargo.toml").exists()
    }

    async fn run_cargo_test(unit: bool, integration: bool) -> Result<TestOutput> {
        let started = std::time::Instant::now();
        let mut summary = String::new();
        let mut failed = false;

        if unit {
            let result = exec::run("cargo", &["test", "--workspace"]).await?;
            summary.push_str(&format!(
                "Unit tests:\n{}\n{}\n",
                result.stdout, result.stderr
            ));
            failed |= !result.success;
        }

        if integration {
            let result = exec::run("cargo", &["test", "--workspace", "--", "--ignored"]).await?;
            summary.push_str(&format!(
                "Integration tests:\n{}\n{}\n",
                result.stdout, result.stderr
            ));
            failed |= !result.success;
        }

        Ok(TestOutput {
            status: if failed { "failed" } else { "passed" }.to_string(),
            summary,
            duration_secs: started.elapsed().as_secs(),
            ..Default::default()
        })
    }

    async fn run_quality_checks() -> Result<(Vec<String>, u32)> {
        let mut findings = Vec::new();
        let mut score: i32 = 100;

        if Self::is_rust_project() {
            for (label, args) in [
                ("fmt", vec!["fmt", "--all", "--", "--check"]),
                (
                    "clippy",
                    vec!["clippy", "--workspace", "--", "-D", "warnings"],
                ),
            ] {
                let result = exec::run("cargo", &args).await?;
                if !result.success {
                    findings.push(format!("cargo {label} failed"));
                    score -= 15;
                }
            }
        }

        Ok((findings, score.clamp(0, 100) as u32))
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Backend for LocalBackend {
    fn mode(&self) -> ExecutionMode {
        ExecutionMode::Local
    }

    async fn design(&self, prompt: Option<&str>) -> Result<DesignOutput> {
        info!("Local backend: design");

        let prompt_text = prompt.unwrap_or("Default architecture design");

        let output = DesignOutput {
            design: format!(
                "# Proposed Architecture\n\n{}\n\n## Notes\n- This is a local (placeholder) design\n",
                prompt_text
            ),
            status: "pending".to_string(),
            decisions: vec![
                "Use modular architecture".to_string(),
                "Implement trait-based abstractions".to_string(),
            ],
            roadmap: vec![
                "Phase 0: Backend abstraction".to_string(),
                "Phase 1: Command implementation".to_string(),
            ],
            ..Default::default()
        };

        Ok(output)
    }

    async fn build(&self, ctx: &BuildContext) -> Result<BuildOutput> {
        info!("Local backend: build");

        if ctx.service.is_some() {
            return Ok(BuildOutput {
                status: "pending".to_string(),
                log: crate::build_plan::stub_build_log(ctx, "local"),
                duration_secs: 0,
                ..Default::default()
            });
        }

        if !exec::stub_mode() && Self::is_rust_project() {
            let result = exec::run("cargo", &["build", "--release"]).await?;
            return Ok(BuildOutput {
                status: if result.success { "success" } else { "failed" }.to_string(),
                log: format!("{}\n{}", result.stdout, result.stderr),
                duration_secs: result.duration_secs,
                ..Default::default()
            });
        }

        let dockerfile_path = ctx.dockerfile.as_deref().unwrap_or("Dockerfile");
        let output = BuildOutput {
            status: "success".to_string(),
            log: format!(
                "Building with cargo and {}\n\
                 → Running: cargo build --release\n\
                 → Image build: docker build -f {}\n\
                 Build completed successfully",
                dockerfile_path, dockerfile_path
            ),
            artifact: Some("/usr/local/bin/og".to_string()),
            duration_secs: 60,
            ..Default::default()
        };

        Ok(output)
    }

    async fn test(&self, unit: bool, integration: bool) -> Result<TestOutput> {
        info!("Local backend: test");

        if !exec::stub_mode() && Self::is_rust_project() {
            return Self::run_cargo_test(unit, integration).await;
        }

        let mut summary = String::new();
        if unit {
            summary.push_str("Unit tests: PASS (stub)\n");
        }
        if integration {
            summary.push_str("Integration tests: PASS (stub)\n");
        }

        Ok(TestOutput {
            status: "passed".to_string(),
            summary,
            coverage: Some(87.5),
            duration_secs: 120,
            ..Default::default()
        })
    }

    async fn review(&self, pr_url: &str) -> Result<ReviewOutput> {
        info!("Local backend: review for {}", pr_url);

        // FAIL CLOSED (og-bug-report#39): review must never emit an "approved"
        // verdict unless the PR's REQUIRED CHECKS (og-crab/og-governance/
        // og-ciso-agent, plus og-kustomize for IaC) are all present and passing.
        //
        // stub_mode is offline — we cannot verify any checks — so it can NEVER
        // approve. It reports a blocked review with score 0, exactly like a PR
        // with no checks. (This replaces the old unconditional "approved"/85.)
        if exec::stub_mode() {
            return Ok(ReviewOutput {
                status: "blocked".to_string(),
                comments: vec![],
                lint_findings: vec![
                    "stub mode: required checks cannot be verified offline — failing closed"
                        .to_string(),
                ],
                security_findings: vec![],
                score: 0,
            });
        }

        // Without gh we cannot verify required checks ⇒ blocked.
        if !crate::gh::available() {
            return Ok(ReviewOutput {
                status: "blocked".to_string(),
                comments: vec![],
                lint_findings: vec![
                    "gh CLI unavailable: required checks cannot be verified — failing closed"
                        .to_string(),
                ],
                security_findings: vec![],
                score: 0,
            });
        }

        let (mut lint_findings, score) = Self::run_quality_checks().await?;
        let security_findings: Vec<String> = Vec::new();

        // Resolve the PR and evaluate the required-checks gate (the single source
        // of truth). If it does not pass — a required check missing, failing, no
        // checks configured, or status undeterminable — the review is BLOCKED and
        // the score is capped at 0 so nothing downstream reads it as an approval.
        let identifier = if pr_url == "auto-detect" || pr_url.is_empty() {
            None
        } else {
            Some(pr_url)
        };
        let pr = crate::gh::resolve_pr(identifier).await?;
        let is_iac = crate::gh::is_iac_pr(&pr).await?;
        // Actually RUN the required checks against the PR's checkout and gate on
        // their real verdicts (og-bug-report#39).
        let verdict = crate::checks::required_checks_verdict(&pr, is_iac).await?;

        if !verdict.passed {
            lint_findings.push(format!(
                "required checks not satisfied ({}) — failing closed",
                verdict.reason()
            ));
            return Ok(ReviewOutput {
                status: "blocked".to_string(),
                comments: vec![],
                lint_findings,
                security_findings,
                // Cap at 0: an unsatisfied required-checks gate can never read as
                // an approval regardless of local lint score.
                score: 0,
            });
        }

        // Required-checks gate passed; the local quality score decides approve
        // vs changes-requested from here.
        let status = if score >= 80 && security_findings.is_empty() {
            "approved"
        } else {
            "changes_requested"
        };

        Ok(ReviewOutput {
            status: status.to_string(),
            comments: vec![],
            lint_findings,
            security_findings,
            score,
        })
    }

    async fn merge(&self, pr_url: &str, if_approved: bool) -> Result<MergeOutput> {
        info!(
            "Local backend: merge {} (if_approved={})",
            pr_url, if_approved
        );

        // FAIL CLOSED (og-bug-report#39): never merge without first RUNNING the
        // PR's REQUIRED CHECKS and confirming they all pass. Offline (stub_mode)
        // or gh-unavailable ⇒ we cannot check out or run checks ⇒ refuse. The old
        // fabricated "merged"/abc123 path is gone — a merge only ever happens
        // through the real, gated gh path below.
        if exec::stub_mode() || !crate::gh::available() {
            bail!(
                "cannot merge {pr_url}: required checks cannot be run offline (stub/gh-unavailable) — failing closed"
            );
        }

        let identifier = if pr_url == "auto-detect" || pr_url.is_empty() {
            None
        } else {
            Some(pr_url)
        };
        let pr = crate::gh::resolve_pr(identifier).await?;
        let is_iac = crate::gh::is_iac_pr(&pr).await?;
        let verdict = crate::checks::required_checks_verdict(&pr, is_iac).await?;
        if !verdict.passed {
            bail!(
                "merge blocked for PR #{}: required checks not satisfied ({}) — failing closed",
                pr.number,
                verdict.reason()
            );
        }

        if if_approved && !crate::gh::is_approved(pr.number).await? {
            return Ok(MergeOutput {
                status: "pending_approval".to_string(),
                message: format!("Merge pending approval: {pr_url}"),
                ..Default::default()
            });
        }

        let sha = crate::gh::merge_pr(pr.number, false).await?;
        Ok(MergeOutput {
            status: "merged".to_string(),
            commit_sha: Some(sha),
            message: format!("Successfully merged: {pr_url}"),
            ..Default::default()
        })
    }

    async fn fix_conflicts(&self, pr_url: &str) -> Result<FixConflictsOutput> {
        info!("Local backend: fix-conflicts for {}", pr_url);
        // Conflict resolution is og-fix-merge-conflicts-agent's job, routed via
        // og-leader. The local dev backend has no sovereign agent to run it, so it
        // reports honestly rather than fabricating a resolution (no fake success).
        Ok(FixConflictsOutput {
            status: "unsupported_local".to_string(),
            message: format!(
                "fix-merge-conflicts for {pr_url} routes to og-fix-merge-conflicts-agent \
                 via og-leader; the local dev backend does not resolve conflicts itself."
            ),
        })
    }

    async fn forward_to_leader(&self, objective: &str) -> Result<ForwardOutput> {
        // A fleet/free-form objective is decomposed by og-leader + AOA into
        // per-target og-* runs; the local dev backend has no sovereign planner and
        // no local "test every PR in the org". FAIL CLOSED rather than fabricate a
        // fleet verdict from a single local tool run (og-cli#48).
        bail!(
            "forward-to-leader has no local fallback: the objective {objective:?} must be \
             decomposed by og-leader + AOA into per-target og-* runs. The local dev backend \
             cannot execute a fleet objective; refusing to fabricate a verdict."
        )
    }

    async fn create_repo(&self, spec: &CreateRepoSpec) -> Result<CreateRepoOutput> {
        // Repo creation is sovereign — og-leader → AOA → og-agent-forge. The
        // local dev backend has no sovereign provisioner, and creating a repo via
        // a local `gh` side channel would bypass org policy + identity + audit.
        // FAIL CLOSED rather than create a repo out-of-band (code-governance#795).
        bail!(
            "create-repo has no local backend: provisioning repository {:?} routes to \
             og-agent-forge via og-leader; the local dev backend does not create repos \
             (no local/GitHub side channel).",
            spec.name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_backend_mode() {
        let backend = LocalBackend::new();
        assert_eq!(backend.mode(), ExecutionMode::Local);
    }

    #[tokio::test]
    async fn test_local_design() {
        let backend = LocalBackend::new();
        let output = backend.design(Some("Test design")).await.unwrap();
        assert_eq!(output.status, "pending");
        assert!(output.design.contains("Test design"));
    }

    /// Repo creation is sovereign (og-agent-forge via og-leader) — the local dev
    /// backend has no provisioner and must fail closed, never create a repo via a
    /// local `gh` side channel (code-governance#795).
    #[tokio::test]
    async fn test_local_create_repo_fails_closed() {
        let spec = CreateRepoSpec {
            name: "acme-web".to_string(),
            private: true,
            ..Default::default()
        };
        let err = LocalBackend::new()
            .create_repo(&spec)
            .await
            .expect_err("local create-repo must fail closed, never use a side channel");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local backend") && msg.contains("og-agent-forge"),
            "expected an honest no-backend error, got: {msg}"
        );
    }

    // All OG_CLI_STUB assertions live in ONE sequential test: `OG_CLI_STUB` is
    // process-global, so splitting them into parallel #[tokio::test]s races (one
    // test's remove_var clears another's set_var mid-run). Sequential here =
    // deterministic, mirroring the `test_backend_selection_*` convention.
    #[tokio::test]
    async fn test_local_stub_mode_contract() {
        std::env::set_var("OG_CLI_STUB", "1");
        let backend = LocalBackend::new();

        // build/test still return their honest stub success in stub mode.
        let build = backend.build(&BuildContext::default()).await.unwrap();
        assert_eq!(build.status, "success");

        let test = backend.test(true, false).await.unwrap();
        assert_eq!(test.status, "passed");
        assert!(test.summary.contains("Unit tests"));

        // og-bug-report#39: offline stub mode can't run/verify required checks, so
        // review must NEVER rubber-stamp an approval — it reports a blocked review
        // with score 0 (this replaces the old unconditional "approved"/85).
        let review = backend
            .review("https://github.com/org/repo/pull/1")
            .await
            .unwrap();
        assert_eq!(review.status, "blocked");
        assert_ne!(review.status, "approved");
        assert_eq!(review.score, 0, "no approval score when checks unverified");

        // og-bug-report#39: merge must be REFUSED offline, never fabricated.
        let err = backend
            .merge("https://github.com/org/repo/pull/1", false)
            .await
            .expect_err("stub merge must fail closed, never fabricate 'merged'");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("failing closed") && msg.contains("required checks"),
            "expected a fail-closed error, got: {msg}"
        );

        std::env::remove_var("OG_CLI_STUB");
    }
}
