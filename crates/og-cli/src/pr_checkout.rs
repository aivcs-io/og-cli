//! Isolated PR checkout — fetch a PR into a temporary `git worktree` so
//! `test`/etc. run against its actual code instead of silently testing
//! whatever's already checked out locally. Closes the gap behind
//! code-governance#639's follow-up ask: "all an og-cli user needs to do is
//! say test this pr X" — `test --pr <ref>` previously accepted (and silently
//! discarded) a PR reference without ever fetching that PR's code.
//!
//! Must be created from within a clone of the PR's own repo — the worktree is
//! added relative to the current repo's `.git` (cross-repo PR references are
//! out of scope here; `git worktree add` requires a common repo).

use crate::exec;
use crate::gh;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// A PR checked out into an isolated `git worktree`. Call [`Self::remove`]
/// when done — it does not clean up on drop, since removal is itself a
/// fallible git operation the caller should be able to see fail.
pub struct PrWorktree {
    pub path: PathBuf,
    pub pr: gh::GhPullRequest,
}

impl PrWorktree {
    /// Resolve `pr_ref` (a `#123`, a bare number, or a PR URL) and check it
    /// out into a fresh temporary worktree of the current repo.
    pub async fn create(pr_ref: &str) -> Result<Self> {
        if !exec::tool_available("git") {
            bail!("git not found in PATH");
        }
        if !gh::available() {
            bail!("gh CLI not found; install GitHub CLI and authenticate with `gh auth login`");
        }

        let pr = gh::resolve_pr(Some(pr_ref)).await?;

        let dir =
            std::env::temp_dir().join(format!("og-cli-pr-{}-{}", pr.number, std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }

        let add = exec::run(
            "git",
            &["worktree", "add", "--detach", &dir.to_string_lossy()],
        )
        .await
        .context("failed to run git worktree add")?;
        if !add.success {
            bail!("git worktree add failed: {}", add.stderr.trim());
        }

        // Fetch the PR's head via `refs/pull/<N>/head` — GitHub always keeps
        // this ref on the base repo, so it works even when the head branch was
        // deleted (merged PR) or lives on a fork. `gh pr checkout` fetched by
        // branch name and failed closed on both ("couldn't find remote ref
        // refs/heads/<branch>"). When the PR came in as a URL we fetch from its
        // own repo, so `test --pr <url-in-another-repo>` no longer silently
        // targets whatever repo the CLI was run from.
        let cleanup = |dir: PathBuf| async move {
            let _ = exec::run(
                "git",
                &["worktree", "remove", "--force", &dir.to_string_lossy()],
            )
            .await;
        };
        let source = pr
            .repo
            .as_deref()
            .map(|r| format!("https://github.com/{r}.git"))
            .unwrap_or_else(|| "origin".to_string());
        let pull_ref = format!("refs/pull/{}/head", pr.number);
        let fetch = exec::run_in(Some(&dir), "git", &["fetch", &source, &pull_ref])
            .await
            .context("failed to run git fetch for PR head")?;
        if !fetch.success {
            cleanup(dir).await;
            bail!(
                "fetching PR #{} head ({pull_ref} from {source}) failed: {}",
                pr.number,
                fetch.stderr.trim()
            );
        }
        let checkout = exec::run_in(Some(&dir), "git", &["checkout", "--detach", "FETCH_HEAD"])
            .await
            .context("failed to run git checkout FETCH_HEAD")?;
        if !checkout.success {
            cleanup(dir).await;
            bail!(
                "checking out PR #{} head failed: {}",
                pr.number,
                checkout.stderr.trim()
            );
        }

        Ok(Self { path: dir, pr })
    }

    /// Remove the temporary worktree.
    pub async fn remove(&self) -> Result<()> {
        let result = exec::run(
            "git",
            &[
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ],
        )
        .await?;
        if !result.success {
            bail!("git worktree remove failed: {}", result.stderr.trim());
        }
        Ok(())
    }
}
