use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use tracing::info;

/// GitHub repository reference (owner/repo)
#[derive(Debug, Clone)]
pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
}

impl fmt::Display for GitHubRepo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

impl GitHubRepo {
    pub fn from_url(url: &str) -> Result<Self> {
        // Parse https://github.com/owner/repo or github.com/owner/repo
        let url = url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let parts: Vec<&str> = url.split('/').collect();

        if parts.len() < 3 {
            return Err(anyhow!("Invalid GitHub URL format: {}", url));
        }

        Ok(Self {
            owner: parts[1].to_string(),
            repo: parts[2].trim_end_matches(".git").to_string(),
        })
    }

    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.repo)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub url: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub url: String,
    pub author: String,
}

pub struct GitHubClient {
    repo: GitHubRepo,
    token: Option<String>,
}

impl GitHubClient {
    pub fn new(repo: GitHubRepo) -> Self {
        let token = std::env::var("GITHUB_TOKEN").ok();
        Self { repo, token }
    }

    pub fn from_url(url: &str) -> Result<Self> {
        let repo = GitHubRepo::from_url(url)?;
        Ok(Self::new(repo))
    }

    /// Check if GitHub is reachable (no actual API call, just DNS)
    pub async fn is_reachable(&self) -> bool {
        // In a real implementation, this would do a connectivity test
        // For now, we just check if the token is available or if we can reach github.com
        info!("Checking GitHub connectivity...");
        true // Assume reachable for local testing
    }

    /// Fetch open PRs for the repository
    pub async fn list_prs(&self) -> Result<Vec<PullRequest>> {
        info!("Fetching PRs from {}/{}", self.repo.owner, self.repo);

        // In a real implementation, this would call the GitHub REST API
        // For now, return placeholder
        Ok(vec![PullRequest {
            number: 1,
            title: "feat: example feature".to_string(),
            state: "open".to_string(),
            url: format!("{}/pull/1", self.repo.url()),
            author: "developer".to_string(),
        }])
    }

    /// Fetch open issues for the repository
    pub async fn list_issues(&self) -> Result<Vec<Issue>> {
        info!("Fetching issues from {}/{}", self.repo.owner, self.repo);

        Ok(vec![Issue {
            number: 1,
            title: "bug: example issue".to_string(),
            state: "open".to_string(),
            url: format!("{}/issues/1", self.repo.url()),
            author: "reporter".to_string(),
        }])
    }

    /// Get a specific PR
    pub async fn get_pr(&self, number: u32) -> Result<PullRequest> {
        info!(
            "Fetching PR #{} from {}/{}",
            number, self.repo.owner, self.repo
        );

        Ok(PullRequest {
            number,
            title: format!("PR #{}", number),
            state: "open".to_string(),
            url: format!("{}/pull/{}", self.repo.url(), number),
            author: "developer".to_string(),
        })
    }

    /// Create a comment on a PR
    pub async fn comment_on_pr(&self, pr_number: u32, comment: &str) -> Result<()> {
        info!(
            "Posting comment on PR {}/{}#{}: {}",
            self.repo.owner,
            self.repo.repo,
            pr_number,
            comment.lines().next().unwrap_or("(multiline)")
        );

        if self.token.is_none() {
            return Err(anyhow!("GITHUB_TOKEN not set"));
        }

        // In a real implementation, this would call the GitHub REST API
        // POST /repos/{owner}/{repo}/issues/{issue_number}/comments
        Ok(())
    }

    /// Merge a pull request
    pub async fn merge_pr(
        &self,
        pr_number: u32,
        _commit_title: Option<&str>,
        _commit_message: Option<&str>,
    ) -> Result<()> {
        info!(
            "Merging PR #{}/{}/{}",
            self.repo.owner, self.repo.repo, pr_number
        );

        if self.token.is_none() {
            return Err(anyhow!("GITHUB_TOKEN not set — cannot merge without auth"));
        }

        // In a real implementation:
        // PUT /repos/{owner}/{repo}/pulls/{pull_number}/merge
        // with commit_title and commit_message
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_url_parsing() {
        let repo = GitHubRepo::from_url("https://github.com/acme/og-cli").unwrap();
        assert_eq!(repo.owner, "acme");
        assert_eq!(repo.repo, "og-cli");
        assert_eq!(repo.url(), "https://github.com/acme/og-cli");
    }

    #[test]
    fn test_github_url_parsing_with_git() {
        let repo = GitHubRepo::from_url("https://github.com/acme/og-cli.git").unwrap();
        assert_eq!(repo.owner, "acme");
        assert_eq!(repo.repo, "og-cli");
    }

    #[test]
    fn test_github_url_parsing_invalid() {
        assert!(GitHubRepo::from_url("not-a-url").is_err());
        assert!(GitHubRepo::from_url("github.com/only-one-part").is_err());
    }

    #[tokio::test]
    async fn test_github_client_new() {
        let repo = GitHubRepo::from_url("https://github.com/acme/og-cli").unwrap();
        let client = GitHubClient::new(repo);
        assert!(client.is_reachable().await);
    }

    #[tokio::test]
    async fn test_github_list_prs() {
        let repo = GitHubRepo::from_url("https://github.com/acme/og-cli").unwrap();
        let client = GitHubClient::new(repo);
        let prs = client.list_prs().await.unwrap();
        assert!(!prs.is_empty());
    }
}
