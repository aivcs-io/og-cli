use crate::backend::{BackendFactory, BuildContext};
use crate::config::ProjectConfig;
use crate::pr_checkout::PrWorktree;
use crate::prs::PrManager;
use crate::workflow_client::WorkflowClient;
use anyhow::Result;
use tracing::info;

pub struct WorkflowOrchestrator;

impl WorkflowOrchestrator {
    pub async fn design(prompt: Option<String>, config: &ProjectConfig) -> Result<()> {
        info!("Orchestrating design workflow...");

        let backend = BackendFactory::select(config).await?;
        let mode = backend.mode();

        println!(
            "🎨 Design: {}",
            prompt
                .as_deref()
                .unwrap_or("Architecture and acceptance criteria")
        );
        println!("→ Backend: {} | VCS: {}", mode, config.vcs.backend.as_str());

        let output = backend.design(prompt.as_deref()).await?;
        println!("✓ Status: {}", output.status);
        println!("✓ Decisions: {}", output.decisions.len());
        println!("✓ Roadmap phases: {}", output.roadmap.len());

        info!("Design complete: {}", output.status);
        Ok(())
    }

    pub async fn build(
        service: Option<String>,
        image_name: Option<String>,
        no_push: bool,
        config: &ProjectConfig,
    ) -> Result<()> {
        info!("Orchestrating build workflow...");

        let backend = BackendFactory::select(config).await?;
        let mode = backend.mode();

        let ctx = BuildContext {
            service: service.clone(),
            image_name: image_name.clone(),
            no_push,
            dockerfile: config.build.dockerfile.clone(),
        };

        let label = service
            .as_deref()
            .or(image_name.as_deref())
            .unwrap_or("auto");

        let tenant = std::env::var("DATA_MESH_TENANT_ID").unwrap_or_else(|_| "default".into());
        let wf =
            WorkflowClient::start("build", &format!("AivcsOps build: {label}"), &tenant).await?;

        println!("🏗️  Build: {} (push={})", label, !no_push);
        println!("→ Backend: {} | VCS: {}", mode, config.vcs.backend.as_str());

        let output = backend.build(&ctx).await?;
        let success = output.status == "success" || output.status == "passed";
        WorkflowClient::complete(&wf, success);
        println!("✓ Status: {}", output.status);
        if let Some(digest) = output.image_digest {
            println!("✓ Image digest: {}", digest);
        }
        println!("✓ Duration: {}s", output.duration_secs);

        info!("Build complete: {}", output.status);
        Ok(())
    }

    pub async fn test(
        unit: bool,
        integration: bool,
        coverage: bool,
        pr: Option<String>,
        config: &ProjectConfig,
    ) -> Result<()> {
        info!("Orchestrating test workflow...");

        // "test this pr X" must actually test PR X's code, not silently test
        // whatever's already checked out locally (code-governance#639
        // follow-up). Fetch it into an isolated worktree first when named.
        let worktree = match &pr {
            Some(pr_ref) => {
                println!("📥 Fetching PR {pr_ref} into an isolated worktree...");
                let wt = PrWorktree::create(pr_ref).await?;
                println!(
                    "✓ Checked out PR #{} ({}) at {}",
                    wt.pr.number,
                    wt.pr.title,
                    wt.path.display()
                );
                Some(wt)
            }
            None => None,
        };

        let original_dir = std::env::current_dir().ok();
        if let Some(wt) = &worktree {
            std::env::set_current_dir(&wt.path)?;
        }

        let run_result: Result<()> = async {
            let backend = BackendFactory::select(config).await?;
            let mode = backend.mode();

            println!(
                "🧪 Test: unit={} integration={} coverage={}",
                unit, integration, coverage
            );
            println!("→ Backend: {} | VCS: {}", mode, config.vcs.backend.as_str());

            let output = backend.test(unit, integration).await?;
            println!("✓ Status: {}", output.status);
            if let Some(cov) = output.coverage {
                println!("✓ Coverage: {:.1}%", cov);
            }
            println!("✓ Duration: {}s", output.duration_secs);

            info!("Test complete: {}", output.status);
            Ok(())
        }
        .await;

        // Always restore cwd + clean up the worktree, even if the test run
        // itself errored — surface that error, don't mask it, but don't leak
        // the temp worktree or leave the caller sitting in it either.
        if let Some(dir) = original_dir {
            let _ = std::env::set_current_dir(dir);
        }
        if let Some(wt) = &worktree {
            wt.remove().await?;
        }

        run_result
    }

    pub async fn review(pr_url: Option<String>, config: &ProjectConfig) -> Result<()> {
        info!("Orchestrating review workflow...");

        // Multi-org (code-governance#785): route the A2A tenant to the PR URL's
        // org when present; else OG_ORG / active context.
        let org = pr_url
            .as_deref()
            .and_then(crate::org::OrgManager::org_from_url);
        let backend = BackendFactory::select_for_org(config, org).await?;
        let mode = backend.mode();

        let url = pr_url.as_deref().unwrap_or("auto-detect");
        println!("👁️  Review: {}", url);
        println!("→ Backend: {} | VCS: {}", mode, config.vcs.backend.as_str());

        let output = backend.review(url).await?;
        println!("✓ Status: {}", output.status);
        println!("✓ Comments: {}", output.comments.len());
        println!("✓ Security findings: {}", output.security_findings.len());
        println!("✓ Score: {}/100", output.score);

        info!("Review complete: {}", output.status);
        Ok(())
    }

    pub async fn merge(
        pr_url: Option<String>,
        if_approved: bool,
        config: &ProjectConfig,
    ) -> Result<()> {
        PrManager::merge_gated(pr_url, if_approved, false, config).await
    }

    /// Resolve a PR's merge conflicts by dispatching to og-fix-merge-conflicts-agent
    /// (og-cli → og-leader → AOA → agent). Mirrors [`Self::review`] — select the
    /// backend and hand off; og-cli never resolves conflicts locally.
    pub async fn fix_conflicts(pr_url: Option<String>, config: &ProjectConfig) -> Result<()> {
        info!("Orchestrating fix-merge-conflicts workflow...");

        let backend = BackendFactory::select(config).await?;
        let mode = backend.mode();

        let url = pr_url.as_deref().unwrap_or("auto-detect");
        println!("🔀 Fix merge conflicts: {}", url);
        println!("→ Backend: {} | VCS: {}", mode, config.vcs.backend.as_str());

        let output = backend.fix_conflicts(url).await?;
        println!("✓ Status: {}", output.status);

        info!("Fix-conflicts dispatched: {}", output.status);
        Ok(())
    }

    /// Forward a free-form / **fleet-scoped** objective to og-leader over A2A
    /// (og-cli#48 / code-governance#769). Selecting the backend routes it through
    /// the same seam as every other verb: og-leader primary, optionally wrapped by
    /// the harbormaster [`crate::backend::FallbackBackend`]. Crucially the
    /// fleet/forward path has **no local fallback** — when og-leader is unreachable
    /// the fallback bails, so this fails closed with an honest error instead of a
    /// fabricated local verdict.
    pub async fn forward_to_leader(objective: &str, config: &ProjectConfig) -> Result<()> {
        info!("Forwarding objective to og-leader...");

        let backend = BackendFactory::select(config).await?;
        let mode = backend.mode();

        println!("🛰️  Forwarding to og-leader: {objective}");
        println!("→ Backend: {} | VCS: {}", mode, config.vcs.backend.as_str());

        let output = backend.forward_to_leader(objective).await?;
        println!("  → status: {}", output.status);
        println!(
            "    og-leader + agent-orchestrate-ai will plan and dispatch the required og-* agents."
        );

        info!("Objective forwarded to og-leader: {}", output.status);
        Ok(())
    }

    /// Create a repository through the sovereign flow (code-governance#795):
    /// og-cli → og-leader → agent-orchestrate-ai → og-agent-forge. Shared by
    /// `og repo create` and the NL shell / `og do` path so both behave
    /// identically.
    ///
    /// Fail-closed: requires an authenticated session (the actor for the audit
    /// line) and resolves the active org/account context. Selecting the backend
    /// routes it through the same seam as every other verb — og-leader primary,
    /// optionally wrapped by the harbormaster [`crate::backend::FallbackBackend`].
    /// Crucially repo creation has **no local side channel**: when og-leader is
    /// unreachable the fallback bails, so this fails closed with an honest error
    /// instead of provisioning a repo out-of-band. og-cli only *dispatches* and
    /// surfaces og-leader's status verbatim; it never fabricates a repo URL.
    pub async fn create_repo(
        mut spec: crate::backend::CreateRepoSpec,
        config: &ProjectConfig,
    ) -> Result<()> {
        info!("Orchestrating repo-creation workflow...");

        // Fail-closed auth: a sovereign repo-creation dispatch requires a valid
        // session. The actor (email) is recorded in the audit line — never
        // fabricated, never proceed without it.
        let session = crate::auth::AuthManager::require_session()?;

        // Resolve org/account from the active context unless the caller overrode
        // it. Never invented — falls back to the persisted active org / account.
        if spec.org.is_none() {
            spec.org = Some(crate::org::OrgManager::get_active()?);
        }
        if spec.account.is_none() {
            spec.account = crate::account::AccountManager::get_active()?.map(|a| a.id);
        }
        let org = spec.org.clone().unwrap_or_default();

        let backend = BackendFactory::select(config).await?;
        let mode = backend.mode();

        println!(
            "📦 Create repo: {} ({})",
            spec.name,
            if spec.private { "private" } else { "public" }
        );
        println!("→ Backend: {} | org: {}", mode, org);

        let output = backend.create_repo(&spec).await?;
        println!("✓ Status: {}", output.status);

        // Success: print the created URL only if og-leader actually returned one
        // (never fabricate a URL from a bare dispatch-ack), plus an audit-friendly
        // record line (action, actor, org/account, repo, status, timestamp).
        if let Some(url) = &output.url {
            println!("✓ Repository: {url}");
        }
        println!(
            "📝 audit: action=create-repo actor={} org={} account={} repo={} status={} timestamp={}",
            session.email,
            org,
            spec.account.as_deref().unwrap_or("(active)"),
            spec.name,
            output.status,
            output.timestamp.as_deref().unwrap_or("(pending og-leader)"),
        );

        info!("Repo-creation dispatched: {}", output.status);
        Ok(())
    }

    pub async fn workflow(
        design: bool,
        build: bool,
        test: bool,
        review: bool,
        merge: bool,
        prompt: Option<String>,
        config: &ProjectConfig,
    ) -> Result<()> {
        info!("Starting workflow pipeline...");

        println!(
            "📋 Workflow: design={} build={} test={} review={} merge={}",
            design, build, test, review, merge
        );
        println!("→ VCS: {}", config.vcs.backend.as_str());
        println!();

        if design {
            println!("[Stage 1/5] Design");
            Self::design(prompt.clone(), config).await?;
            println!();
        }

        if build {
            println!("[Stage 2/5] Build");
            Self::build(None, None, false, config).await?;
            println!();
        }

        if test {
            println!("[Stage 3/5] Test");
            Self::test(true, false, false, None, config).await?;
            println!();
        }

        if review && !merge {
            println!("[Stage 4/5] Review");
            Self::review(None, config).await?;
            println!();
        }

        if merge {
            println!("[Stage 4/5] Gate (test + review + required checks)");
            // FAIL CLOSED (og-bug-report#39): no gh ⇒ no gate ⇒ no merge. The gate
            // must run (it checks out the PR and runs the required checks); a
            // missing gate can NEVER be skipped into a merge.
            if !crate::gh::available() {
                anyhow::bail!(
                    "gh CLI unavailable — cannot run the required-checks gate; refusing to merge (failing closed)"
                );
            }
            PrManager::gate(None, false, 80, config).await?;
            println!();
            println!("[Stage 5/5] Merge");
            Self::merge(None, true, config).await?;
            println!();
        }

        println!("✓ Workflow complete");
        info!("Workflow pipeline complete");
        Ok(())
    }
}
