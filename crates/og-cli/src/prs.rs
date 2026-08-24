use crate::backend::{BackendFactory, ExecutionMode};
use crate::config::ProjectConfig;
use crate::gh::{self, GhPullRequest};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub author: String,
    pub base_branch: String,
    pub checks_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrGateRecord {
    pub pr_number: u32,
    pub pr_url: String,
    pub test_status: String,
    pub review_status: String,
    pub review_score: u32,
    pub checks_passed: bool,
    pub backend: String,
    pub head_sha: String,
    pub gated_at: String,
}

pub struct PrManager;

impl PrManager {
    pub async fn list() -> Result<()> {
        info!("Listing pull requests");

        if gh::available() {
            let prs = gh::list_prs("open").await?;
            print_prs(
                &prs.iter()
                    .map(GhPullRequest::as_summary)
                    .collect::<Vec<_>>(),
            );
            return Ok(());
        }

        let prs = sample_prs();
        println!("⚠️  gh not available; showing sample data");
        print_prs(&prs);
        Ok(())
    }

    /// Run test + review gates for one PR or all open PRs. Must pass before approve/merge.
    pub async fn gate(
        pr: Option<String>,
        all_open: bool,
        min_score: u32,
        config: &ProjectConfig,
    ) -> Result<()> {
        if all_open && pr.is_some() {
            bail!("use either --pr or --all-open, not both");
        }

        let prs = if all_open {
            if !gh::available() {
                bail!("--all-open requires gh CLI");
            }
            gh::list_prs("open").await?
        } else if gh::available() {
            vec![gh::resolve_pr(pr.as_deref()).await?]
        } else {
            bail!("gh CLI required for `og pr gate`; install and run `gh auth login`");
        };

        if prs.is_empty() {
            println!("No open PRs to gate");
            return Ok(());
        }

        let mut failed = 0u32;
        for pr_info in prs {
            match Self::gate_one(&pr_info, min_score, config).await {
                Ok(record) => {
                    Self::save_gate_record(&record)?;
                    println!(
                        "✅ PR #{} passed gate (score={}, backend={})",
                        record.pr_number, record.review_score, record.backend
                    );
                }
                Err(err) => {
                    failed += 1;
                    eprintln!("❌ PR #{} gate failed: {err:#}", pr_info.number);
                }
            }
            println!();
        }

        if failed > 0 {
            bail!("{failed} PR(s) failed gate");
        }

        Ok(())
    }

    async fn gate_one(
        pr: &GhPullRequest,
        min_score: u32,
        config: &ProjectConfig,
    ) -> Result<PrGateRecord> {
        println!("🚧 Gating PR #{}: {}", pr.number, pr.title);

        // Multi-org (code-governance#785): route the A2A tenant to the PR's org.
        let org = crate::org::OrgManager::org_from_url(&pr.url);
        let backend = BackendFactory::select_for_org(config, org).await?;
        let backend_name = backend.mode().to_string();
        if backend.mode() != ExecutionMode::Local {
            // The AOA/Kubernetes/Harbormaster backends are the deprecated
            // direct-dispatch path (code-governance#631) — og-cli is meant to
            // route through og-leader instead. Kubernetes in particular is a
            // known Phase-0 stub that reports a canned pass regardless of
            // actual PR content (code-governance#639) until that migration
            // lands, so a non-Local result here is advisory, not verified.
            eprintln!(
                "⚠️  gate ran via the deprecated `{backend_name}` direct-backend path \
                 (code-governance#631) — treat this result as advisory only, not a \
                 verified pass, until og-cli routes PR gating through og-leader."
            );
        }

        println!("  [1/3] Test");
        let test = backend.test(true, false).await?;
        if test.status != "passed" {
            bail!("tests failed: {}", test.summary.trim());
        }
        println!("        ✓ {}", test.status);

        println!("  [2/3] Review");
        let review = backend.review(&pr.url).await?;
        let review_ok = review.status == "approved" || review.score >= min_score;
        if !review_ok {
            bail!(
                "review score {} below minimum {} (status={})",
                review.score,
                min_score,
                review.status
            );
        }
        if !review.security_findings.is_empty() {
            bail!(
                "security findings block gate: {}",
                review.security_findings.join("; ")
            );
        }
        println!(
            "        ✓ score={}/100 status={}",
            review.score, review.status
        );

        println!("  [3/3] Required checks");
        // Actually RUN the required checks (og-crab/og-governance/og-ciso-agent,
        // + og-kustomize for IaC) against the PR checkout and gate on their real
        // verdicts — fail closed (og-bug-report#39).
        let is_iac = gh::is_iac_pr(pr).await?;
        let verdict = crate::checks::required_checks_verdict(pr, is_iac).await?;
        if !verdict.passed {
            bail!(
                "required checks not satisfied for PR #{}: {}",
                pr.number,
                verdict.reason()
            );
        }
        let checks_passed = verdict.passed;
        println!(
            "        ✓ required checks passed: {}",
            verdict.passed_checks.join(", ")
        );

        Ok(PrGateRecord {
            pr_number: pr.number,
            pr_url: pr.url.clone(),
            test_status: test.status,
            review_status: review.status,
            review_score: review.score,
            checks_passed,
            backend: backend_name,
            head_sha: pr.head_sha.clone(),
            gated_at: Utc::now().to_rfc3339(),
        })
    }

    /// Approve a PR after gate passes (requires gh).
    pub async fn approve(pr: Option<String>, body: Option<String>) -> Result<()> {
        let pr_info = gh::resolve_pr(pr.as_deref()).await?;
        Self::require_gate(pr_info.number, Some(&pr_info.head_sha))?;

        // Belt-and-suspenders (og-bug-report#39): re-run the required checks live
        // before submitting an approval so a stale/otherwise-satisfied gate record
        // can never let an approval through when checks don't actually pass now.
        let is_iac = gh::is_iac_pr(&pr_info).await?;
        let verdict = crate::checks::required_checks_verdict(&pr_info, is_iac).await?;
        if !verdict.passed {
            bail!(
                "approval blocked for PR #{}: required checks not satisfied ({}) — failing closed",
                pr_info.number,
                verdict.reason()
            );
        }

        println!(
            "✅ Gate satisfied for PR #{} — submitting approval",
            pr_info.number
        );

        gh::approve_pr(pr_info.number, body.as_deref()).await?;
        println!("✓ PR #{} approved", pr_info.number);
        Ok(())
    }

    /// Ensure a PR has a valid gate record matching the current head SHA.
    pub fn require_gate(pr_number: u32, head_sha: Option<&str>) -> Result<PrGateRecord> {
        let record = Self::load_gate_record(pr_number)?
            .ok_or_else(|| anyhow::anyhow!("PR #{pr_number} has not passed `og pr gate`"))?;

        if let Some(sha) = head_sha {
            if record.head_sha != sha {
                bail!(
                    "gate is stale for PR #{pr_number}: re-run `og pr gate --pr {pr_number}` after new commits"
                );
            }
        }

        if record.test_status != "passed" {
            bail!("gate test status is {}", record.test_status);
        }

        if !record.checks_passed {
            bail!("gate recorded failing CI checks");
        }

        Ok(record)
    }

    pub async fn merge_gated(
        pr: Option<String>,
        if_approved: bool,
        skip_gate: bool,
        _config: &ProjectConfig,
    ) -> Result<()> {
        // FAIL CLOSED (og-bug-report#39): merging requires gh to check the PR out
        // and RUN its required checks. Without gh we can neither, so refuse — no
        // stub/fabricated merge path exists anymore.
        if !gh::available() {
            bail!(
                "gh CLI required to run required checks before merge — failing closed (og-bug-report#39)"
            );
        }

        let pr_info = gh::resolve_pr(pr.as_deref()).await?;

        // The live required-checks verdict is ALWAYS enforced and can NEVER be
        // bypassed — not even by --skip-gate. Actually runs og-crab/og-governance/
        // og-ciso-agent (+ og-kustomize for IaC) against the PR checkout.
        let is_iac = gh::is_iac_pr(&pr_info).await?;
        let verdict = crate::checks::required_checks_verdict(&pr_info, is_iac).await?;
        if !verdict.passed {
            bail!(
                "merge blocked for PR #{}: required checks not satisfied ({}) — failing closed",
                pr_info.number,
                verdict.reason()
            );
        }
        println!(
            "✓ Required checks passed for PR #{}: {}",
            pr_info.number,
            verdict.passed_checks.join(", ")
        );

        // --skip-gate only skips the *recorded* `og pr gate` artifact (test+review
        // history); it does NOT and CANNOT skip the live required-checks verdict
        // enforced above. Absent/failing required checks always block the merge.
        if skip_gate {
            println!(
                "⚠️  --skip-gate: skipping the recorded `og pr gate` artifact, but the live \
                 required-checks verdict above is still enforced (and passed)."
            );
        } else {
            Self::require_gate(pr_info.number, Some(&pr_info.head_sha))?;
            println!("✓ Gate record verified for PR #{}", pr_info.number);
        }

        if if_approved && !gh::is_approved(pr_info.number).await? {
            bail!(
                "PR #{} is not approved; run `og pr approve` or merge without --if-approved",
                pr_info.number
            );
        }

        let sha = gh::merge_pr(pr_info.number, false).await?;
        println!("✓ Merged PR #{} (commit {})", pr_info.number, sha);
        Self::clear_gate_record(pr_info.number)?;
        Ok(())
    }

    fn gate_dir() -> PathBuf {
        PathBuf::from(".og/gates")
    }

    fn gate_path(pr_number: u32) -> PathBuf {
        Self::gate_dir().join(format!("{pr_number}.json"))
    }

    fn save_gate_record(record: &PrGateRecord) -> Result<()> {
        fs::create_dir_all(Self::gate_dir()).context("create .og/gates")?;
        let path = Self::gate_path(record.pr_number);
        let json = serde_json::to_string_pretty(record)?;
        fs::write(path, json).context("write gate record")?;
        Ok(())
    }

    fn load_gate_record(pr_number: u32) -> Result<Option<PrGateRecord>> {
        let path = Self::gate_path(pr_number);
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(path).context("read gate record")?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    fn clear_gate_record(pr_number: u32) -> Result<()> {
        let path = Self::gate_path(pr_number);
        if path.exists() {
            fs::remove_file(path).context("remove gate record")?;
        }
        Ok(())
    }
}

impl GhPullRequest {
    fn as_summary(&self) -> PullRequest {
        PullRequest {
            number: self.number,
            title: self.title.clone(),
            state: self.state.clone(),
            author: self.author.clone(),
            base_branch: self.base_branch.clone(),
            checks_status: "see gh pr checks".to_string(),
        }
    }
}

fn print_prs(prs: &[PullRequest]) {
    println!("📤 Pull Requests ({} total):", prs.len());
    println!();

    for pr in prs {
        let status_icon = match pr.state.as_str() {
            "MERGED" | "merged" => "✅",
            "OPEN" | "open" => "🟢",
            "CLOSED" | "closed" => "❌",
            _ => "❓",
        };

        println!("{} #{} {}", status_icon, pr.number, pr.title);
        println!("   Author: {}", pr.author);
        println!("   Target: {}", pr.base_branch);
        println!("   Checks: {}", pr.checks_status);
        println!();
    }
}

fn sample_prs() -> Vec<PullRequest> {
    vec![PullRequest {
        number: 88,
        title: "og-cli: dual-transport VCS (git + aivcs) support".to_string(),
        state: "open".to_string(),
        author: "principle".to_string(),
        base_branch: "main".to_string(),
        checks_status: "✅ all checks passed".to_string(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_record_serializes() {
        let record = PrGateRecord {
            pr_number: 99,
            pr_url: "https://github.com/lornu-ai/og-cli/pull/99".to_string(),
            test_status: "passed".to_string(),
            review_status: "approved".to_string(),
            review_score: 92,
            checks_passed: true,
            backend: "local".to_string(),
            head_sha: "abc123".to_string(),
            gated_at: Utc::now().to_rfc3339(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let loaded: PrGateRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.pr_number, 99);
        assert_eq!(loaded.review_score, 92);
    }
}
