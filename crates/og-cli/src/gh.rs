use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::exec;

#[derive(Debug, Clone)]
pub struct GhPullRequest {
    pub number: u32,
    pub url: String,
    pub title: String,
    pub state: String,
    pub author: String,
    pub base_branch: String,
    pub head_sha: String,
    /// `owner/name` when the PR was given as a URL — so downstream ops (view,
    /// checkout) target the PR's own repo, not whatever repo the CLI happens to
    /// be run from. `None` for a bare `#123`/number (resolved against the
    /// current repo).
    pub repo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhPrViewJson {
    number: u32,
    url: String,
    title: String,
    state: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    author: GhAuthor,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

pub fn available() -> bool {
    exec::tool_available("gh")
}

pub async fn resolve_pr(identifier: Option<&str>) -> Result<GhPullRequest> {
    if !available() {
        bail!("gh CLI not found; install GitHub CLI and authenticate with `gh auth login`");
    }

    match identifier {
        None => current_branch_pr().await,
        Some(raw) => parse_and_fetch(raw).await,
    }
}

async fn current_branch_pr() -> Result<GhPullRequest> {
    let result = exec::run(
        "gh",
        &[
            "pr",
            "view",
            "--json",
            "number,url,title,state,headRefOid,author,baseRefName",
        ],
    )
    .await?;

    if !result.success {
        bail!("no PR for current branch: {}", result.stderr.trim());
    }

    parse_pr_json(&result.stdout, None)
}

async fn parse_and_fetch(raw: &str) -> Result<GhPullRequest> {
    let trimmed = raw.trim();
    if trimmed.starts_with("http") {
        // A URL carries the repo (owner/name) — resolve against THAT repo, not
        // whatever repo the CLI is being run from.
        let (repo, number) = extract_repo_and_number_from_url(trimmed)?;
        fetch_pr(number, Some(&repo)).await
    } else {
        let number: u32 = trimmed
            .trim_start_matches('#')
            .parse()
            .with_context(|| format!("invalid PR number: {raw}"))?;
        fetch_pr(number, None).await
    }
}

async fn fetch_pr(number: u32, repo: Option<&str>) -> Result<GhPullRequest> {
    let number_s = number.to_string();
    let mut args = vec!["pr", "view", &number_s];
    // Target the PR's own repo when we know it (URL form).
    if let Some(repo) = repo {
        args.push("-R");
        args.push(repo);
    }
    args.extend_from_slice(&[
        "--json",
        "number,url,title,state,headRefOid,author,baseRefName",
    ]);
    let result = exec::run("gh", &args).await?;

    if !result.success {
        let where_ = repo.map(|r| format!(" in {r}")).unwrap_or_default();
        bail!("PR #{number} not found{where_}: {}", result.stderr.trim());
    }

    parse_pr_json(&result.stdout, repo.map(|r| r.to_string()))
}

fn parse_pr_json(json: &str, repo: Option<String>) -> Result<GhPullRequest> {
    let view: GhPrViewJson =
        serde_json::from_str(json).context("failed to parse gh pr view JSON")?;

    Ok(GhPullRequest {
        number: view.number,
        url: view.url,
        title: view.title,
        state: view.state,
        author: view.author.login,
        base_branch: view.base_ref_name,
        head_sha: view.head_ref_oid,
        repo,
    })
}

/// Parse `https://github.com/<owner>/<repo>/pull/<N>` into (`owner/repo`, N).
fn extract_repo_and_number_from_url(url: &str) -> Result<(String, u32)> {
    let trimmed = url.trim_end_matches('/');
    let segments: Vec<&str> = trimmed.split('/').collect();
    // Find the `pull` segment; owner/repo precede it, the number follows it.
    let pull_idx = segments
        .iter()
        .position(|s| *s == "pull" || *s == "pulls")
        .ok_or_else(|| anyhow!("could not parse PR URL (no /pull/ segment): {url}"))?;
    let number: u32 = segments
        .get(pull_idx + 1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("could not parse PR number from URL: {url}"))?;
    let owner = segments
        .get(pull_idx.wrapping_sub(2))
        .filter(|_| pull_idx >= 2)
        .ok_or_else(|| anyhow!("could not parse repo owner from URL: {url}"))?;
    let name = segments
        .get(pull_idx - 1)
        .ok_or_else(|| anyhow!("could not parse repo name from URL: {url}"))?;
    Ok((format!("{owner}/{name}"), number))
}

pub async fn list_prs(state: &str) -> Result<Vec<GhPullRequest>> {
    if !available() {
        bail!("gh CLI not found; install GitHub CLI and authenticate with `gh auth login`");
    }

    let result = exec::run(
        "gh",
        &[
            "pr",
            "list",
            "--state",
            state,
            "--json",
            "number,url,title,state,headRefOid,author,baseRefName",
            "--limit",
            "100",
        ],
    )
    .await?;

    if !result.success {
        bail!("gh pr list failed: {}", result.stderr.trim());
    }

    let views: Vec<GhPrViewJson> =
        serde_json::from_str(&result.stdout).context("failed to parse gh pr list JSON")?;

    Ok(views
        .into_iter()
        .map(|view| GhPullRequest {
            number: view.number,
            repo: repo_from_url(&view.url),
            url: view.url,
            title: view.title,
            state: view.state,
            author: view.author.login,
            base_branch: view.base_ref_name,
            head_sha: view.head_ref_oid,
        })
        .collect())
}

/// Parse the repository name (the path segment before `/pull/`) out of a PR URL.
fn repo_from_url(url: &str) -> Option<String> {
    let (before, _) = url.split_once("/pull/")?;
    before.rsplit('/').next().map(str::to_string)
}

/// Pure predicate: does any changed path make this an IaC PR? True when a path is
/// under `crossplane/` or `flux/`, or is a kustomize file (`kustomization.yaml`
/// / `kustomization.yml`) or a kustomize overlay path.
fn paths_indicate_iac(paths: &[String]) -> bool {
    paths.iter().any(|p| {
        let lp = p.to_lowercase();
        lp.starts_with("crossplane/")
            || lp.contains("/crossplane/")
            || lp.starts_with("flux/")
            || lp.contains("/flux/")
            || lp.ends_with("kustomization.yaml")
            || lp.ends_with("kustomization.yml")
            || lp.contains("/overlays/")
            || lp.starts_with("overlays/")
    })
}

/// The repo itself is IaC (matched case-insensitively).
fn repo_is_iac(repo: &str) -> bool {
    repo.eq_ignore_ascii_case("infra-code")
}

/// Fetch the list of file paths a PR changes via `gh pr view --json files`.
pub async fn changed_files(number: u32) -> Result<Vec<String>> {
    let result = exec::run(
        "gh",
        &["pr", "view", &number.to_string(), "--json", "files"],
    )
    .await?;
    if !result.success {
        bail!(
            "cannot list changed files for PR #{number} (gh pr view failed: {}) — failing closed",
            result.stderr.trim()
        );
    }

    #[derive(Deserialize)]
    struct FilesView {
        files: Option<Vec<FileEntry>>,
    }
    #[derive(Deserialize)]
    struct FileEntry {
        path: String,
    }

    let view: FilesView =
        serde_json::from_str(&result.stdout).context("failed to parse files JSON")?;
    Ok(view
        .files
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.path)
        .collect())
}

/// Decide whether a PR is Infrastructure-as-Code: true if the repo is
/// `infra-code`, OR the PR changes files under `crossplane/`, `flux/`, or any
/// `kustomization.yaml`/kustomize overlay path. Used to add `og-kustomize` to the
/// required checks. Fails closed (errors) if the changed-file list can't be read.
pub async fn is_iac_pr(pr: &GhPullRequest) -> Result<bool> {
    if let Some(repo) = repo_from_url(&pr.url) {
        if repo_is_iac(&repo) {
            return Ok(true);
        }
    }
    let files = changed_files(pr.number).await?;
    Ok(paths_indicate_iac(&files))
}

pub async fn approve_pr(number: u32, body: Option<&str>) -> Result<()> {
    let number_str = number.to_string();
    let mut args: Vec<&str> = vec!["pr", "review", &number_str, "--approve"];
    if let Some(comment) = body {
        args.push("--body");
        args.push(comment);
    }

    let result = exec::run("gh", &args).await?;
    if !result.success {
        bail!("gh pr review failed: {}", result.stderr.trim());
    }

    Ok(())
}

pub async fn merge_pr(number: u32, squash: bool) -> Result<String> {
    let flag = if squash { "--squash" } else { "--merge" };
    let number_str = number.to_string();
    let result = exec::run("gh", &["pr", "merge", &number_str, flag, "--delete-branch"]).await?;

    if !result.success {
        bail!("gh pr merge failed: {}", result.stderr.trim());
    }

    let sha_result = exec::run(
        "gh",
        &["pr", "view", &number.to_string(), "--json", "mergeCommit"],
    )
    .await?;

    if sha_result.success {
        #[derive(Deserialize)]
        struct MergeView {
            #[serde(rename = "mergeCommit")]
            merge_commit: Option<MergeCommit>,
        }
        #[derive(Deserialize)]
        struct MergeCommit {
            oid: String,
        }

        if let Ok(view) = serde_json::from_str::<MergeView>(&sha_result.stdout) {
            if let Some(commit) = view.merge_commit {
                return Ok(commit.oid);
            }
        }
    }

    Ok(result.stdout.trim().to_string())
}

pub async fn is_approved(number: u32) -> Result<bool> {
    let result = exec::run(
        "gh",
        &[
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "reviewDecision",
        ],
    )
    .await?;

    if !result.success {
        bail!("gh pr view failed: {}", result.stderr.trim());
    }

    #[derive(Deserialize)]
    struct ReviewView {
        #[serde(rename = "reviewDecision")]
        review_decision: Option<String>,
    }

    let view: ReviewView =
        serde_json::from_str(&result.stdout).context("failed to parse reviewDecision")?;

    Ok(matches!(view.review_decision.as_deref(), Some("APPROVED")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_pr_number_from_github_url() {
        // Pre-existing test referenced a renamed helper; the parser is
        // `extract_repo_and_number_from_url` → (repo, number).
        //
        // Assert BOTH halves, not just the number: the repo is what makes
        // `og <verb> <url>` act on the PR's own repository rather than whatever
        // is checked out locally (#67), and a number-only assertion would pass
        // even if the repo were parsed wrong.
        assert_eq!(
            extract_repo_and_number_from_url("https://github.com/lornu-ai/og-cli/pull/42").unwrap(),
            ("lornu-ai/og-cli".to_string(), 42)
        );
    }

    #[test]
    fn repo_from_url_and_repo_is_iac() {
        assert_eq!(
            repo_from_url("https://github.com/lornu-ai/infra-code/pull/5").as_deref(),
            Some("infra-code")
        );
        assert!(repo_is_iac("infra-code"));
        assert!(repo_is_iac("Infra-Code"));
        assert!(!repo_is_iac("og-cli"));
    }

    #[test]
    fn paths_indicate_iac_detects_iac_paths() {
        assert!(paths_indicate_iac(&["flux/clusters/prod/app.yaml".into()]));
        assert!(paths_indicate_iac(&["crossplane/composition.yaml".into()]));
        assert!(paths_indicate_iac(&[
            "apps/web/overlays/prod/patch.yaml".into()
        ]));
        assert!(paths_indicate_iac(&["base/kustomization.yaml".into()]));
        assert!(!paths_indicate_iac(&[
            "src/main.rs".into(),
            "README.md".into()
        ]));
    }
}
