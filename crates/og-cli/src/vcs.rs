use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

/// Git repository client (github.com) - SEPARATE implementation
pub struct GitClient {
    pub owner: String,
    pub repo: String,
    pub url: String,
    token: Option<String>,
}

impl GitClient {
    pub fn new(owner: String, repo: String) -> Self {
        let url = format!("https://github.com/{}/{}", owner, repo);
        let token = std::env::var("GITHUB_TOKEN").ok();

        Self {
            owner,
            repo,
            url,
            token,
        }
    }

    pub fn from_url(url: &str) -> Result<Self> {
        let url_clean = url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let parts: Vec<&str> = url_clean.split('/').collect();

        if parts.len() < 3 {
            return Err(anyhow!("Invalid GitHub URL format: {}", url));
        }

        Ok(Self::new(
            parts[1].to_string(),
            parts[2].trim_end_matches(".git").to_string(),
        ))
    }

    pub fn api_endpoint(&self) -> String {
        format!("https://api.github.com/repos/{}/{}", self.owner, self.repo)
    }

    pub async fn is_reachable(&self) -> bool {
        info!("Checking GitHub connectivity...");
        match reqwest::Client::new()
            .get("https://api.github.com/")
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                info!("GitHub unreachable: {}", e);
                false
            }
        }
    }

    pub fn authenticate(&self) -> Result<()> {
        if self.token.is_none() {
            return Err(anyhow!("GITHUB_TOKEN not set"));
        }
        info!("Authenticated with GitHub");
        Ok(())
    }

    pub fn auth_header(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("token {}", t))
    }

    pub async fn list_prs(&self) -> Result<Vec<PullRequest>> {
        info!("Fetching PRs from GitHub {}/{}", self.owner, self.repo);
        Ok(vec![PullRequest {
            number: 1,
            title: "example PR".to_string(),
            state: "open".to_string(),
            url: format!("{}/pull/1", self.url),
        }])
    }

    pub async fn merge_pr(&self, pr_number: u32) -> Result<()> {
        info!(
            "Merging GitHub PR #{}/{}/{}",
            self.owner, self.repo, pr_number
        );
        self.authenticate()?;
        // GitHub: PUT /repos/{owner}/{repo}/pulls/{pull_number}/merge
        Ok(())
    }
}

/// AIVCS repository client (aivcs.io) - SEPARATE implementation
pub struct AivcsClient {
    pub owner: String,
    pub repo: String,
    pub url: String,
    token: Option<String>,
}

impl AivcsClient {
    pub fn new(owner: String, repo: String) -> Self {
        let url = format!("https://aivcs.io/{}/{}", owner, repo);
        let token = std::env::var("AIVCS_TOKEN").ok();

        Self {
            owner,
            repo,
            url,
            token,
        }
    }

    pub fn from_url(url: &str) -> Result<Self> {
        let url_clean = url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let parts: Vec<&str> = url_clean.split('/').collect();

        if parts.len() < 3 {
            return Err(anyhow!("Invalid AIVCS URL format: {}", url));
        }

        Ok(Self::new(
            parts[1].to_string(),
            parts[2].trim_end_matches(".git").to_string(),
        ))
    }

    pub fn api_endpoint(&self) -> String {
        format!("https://aivcsd.aivcs.io/repos/{}/{}", self.owner, self.repo)
    }

    pub async fn is_reachable(&self) -> bool {
        info!("Checking AIVCS connectivity...");
        match reqwest::Client::new()
            .get("https://aivcs.io/api/v1/health")
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                info!("AIVCS unreachable: {}", e);
                false
            }
        }
    }

    pub fn authenticate(&self) -> Result<()> {
        if self.token.is_none() {
            return Err(anyhow!("AIVCS_TOKEN not set"));
        }
        info!("Authenticated with AIVCS");
        Ok(())
    }

    pub fn auth_header(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("Bearer {}", t))
    }

    pub async fn list_prs(&self) -> Result<Vec<PullRequest>> {
        info!("Fetching MRs from AIVCS {}/{}", self.owner, self.repo);
        Ok(vec![PullRequest {
            number: 1,
            title: "example MR".to_string(),
            state: "open".to_string(),
            url: format!("{}/merge_requests/1", self.url),
        }])
    }

    pub async fn merge_pr(&self, pr_number: u32) -> Result<()> {
        info!(
            "Merging AIVCS MR #{}/{}/{}",
            self.owner, self.repo, pr_number
        );
        self.authenticate()?;
        // AIVCS: POST /repos/{owner}/{repo}/merge_requests/{merge_request_number}/merge
        Ok(())
    }
}

/// The two source transports (ADR: Dual-Transport Architecture, code-governance#634 / RFC #572).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    /// GitHub clone — the always-present, offline-testable bootstrap default.
    Git,
    /// AIVCS native — sovereign, opt-in for production.
    Aivcs,
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Transport::Git => write!(f, "git"),
            Transport::Aivcs => write!(f, "aivcs"),
        }
    }
}

/// A concrete resolved source client, one per transport.
pub enum SourceClient {
    Git(GitClient),
    Aivcs(AivcsClient),
}

impl SourceClient {
    pub fn transport(&self) -> Transport {
        match self {
            SourceClient::Git(_) => Transport::Git,
            SourceClient::Aivcs(_) => Transport::Aivcs,
        }
    }

    pub fn owner(&self) -> &str {
        match self {
            SourceClient::Git(c) => &c.owner,
            SourceClient::Aivcs(c) => &c.owner,
        }
    }

    pub fn repo(&self) -> &str {
        match self {
            SourceClient::Git(c) => &c.repo,
            SourceClient::Aivcs(c) => &c.repo,
        }
    }

    pub fn url(&self) -> &str {
        match self {
            SourceClient::Git(c) => &c.url,
            SourceClient::Aivcs(c) => &c.url,
        }
    }

    pub async fn is_reachable(&self) -> bool {
        match self {
            SourceClient::Git(c) => c.is_reachable().await,
            SourceClient::Aivcs(c) => c.is_reachable().await,
        }
    }
}

/// The **canonical source-resolution router** (ADR code-governance#634, RFC #572).
///
/// This is the fleet's *reference implementation* of dual-transport routing:
/// every service resolves source through one abstraction with two transports and
/// **explicit, fail-closed** routing — never a silent cross-transport fallback.
///
/// Routing (as decided in the ADR):
/// - `SOURCE_URI=git://github.com/<org>/<repo>` → Git transport.
/// - `SOURCE_URI=aivcs://<repo-id>@aivcs.io`    → AIVCS transport.
/// - `SOURCE_URI` unset + `ENABLE_AIVCS_NATIVE=true` → AIVCS.
/// - `SOURCE_URI` unset + no flag → **Git** (safe, offline-testable default).
/// - Any other `SOURCE_URI` scheme → **error** (fail-closed), never a silent Git fallback.
pub struct SourceResolver;

impl SourceResolver {
    /// Decide which transport to use, from the environment only (no I/O,
    /// deterministic). Fail-closed on an unrecognized `SOURCE_URI` scheme.
    pub fn route() -> Result<Transport> {
        if let Ok(uri) = std::env::var("SOURCE_URI") {
            let uri = uri.trim();
            if uri.is_empty() {
                return Self::route_from_flag();
            }
            if uri.starts_with("git://") {
                return Ok(Transport::Git);
            }
            if uri.starts_with("aivcs://") {
                return Ok(Transport::Aivcs);
            }
            // No silent fallback — an unknown scheme is a hard error.
            return Err(anyhow!(
                "unrecognized SOURCE_URI scheme in {uri:?}; expected git:// or aivcs:// \
                 (routing is explicit and fail-closed — no silent fallback)"
            ));
        }
        Self::route_from_flag()
    }

    /// `SOURCE_URI` unset: `ENABLE_AIVCS_NATIVE=true` opts into AIVCS; otherwise Git.
    fn route_from_flag() -> Result<Transport> {
        match std::env::var("ENABLE_AIVCS_NATIVE").ok().as_deref() {
            Some("true") | Some("1") => Ok(Transport::Aivcs),
            _ => Ok(Transport::Git),
        }
    }

    /// Resolve to a concrete client. When `SOURCE_URI` carries an identity it is
    /// authoritative; otherwise the caller-supplied `default_owner`/`default_repo`
    /// (e.g. the active org + repo) are used. Fail-closed — never falls back to the
    /// other transport if the chosen one can't be constructed.
    pub fn resolve(default_owner: &str, default_repo: &str) -> Result<SourceClient> {
        let transport = Self::route()?;
        let uri = std::env::var("SOURCE_URI")
            .ok()
            .filter(|u| !u.trim().is_empty());

        match transport {
            Transport::Git => {
                let (owner, repo) = match &uri {
                    Some(u) => Self::parse_git_uri(u)?,
                    None => (default_owner.to_string(), default_repo.to_string()),
                };
                Ok(SourceClient::Git(GitClient::new(owner, repo)))
            }
            Transport::Aivcs => {
                let (owner, repo) = match &uri {
                    Some(u) => Self::parse_aivcs_uri(u, default_owner)?,
                    None => (default_owner.to_string(), default_repo.to_string()),
                };
                Ok(SourceClient::Aivcs(AivcsClient::new(owner, repo)))
            }
        }
    }

    /// `git://github.com/<org>/<repo>` → (org, repo).
    fn parse_git_uri(uri: &str) -> Result<(String, String)> {
        let rest = uri
            .strip_prefix("git://")
            .ok_or_else(|| anyhow!("not a git:// URI: {uri}"))?;
        // Expect host/<org>/<repo>; ignore the host segment.
        let segs: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        if segs.len() < 3 {
            return Err(anyhow!(
                "malformed git SOURCE_URI {uri:?}; expected git://github.com/<org>/<repo>"
            ));
        }
        let owner = segs[segs.len() - 2].to_string();
        let repo = segs[segs.len() - 1].trim_end_matches(".git").to_string();
        Ok((owner, repo))
    }

    /// `aivcs://<repo-id>@aivcs.io` → (owner, repo). `<repo-id>` is `<owner>/<repo>`
    /// (multi-tenant, www.aivcs.io/<org>/<repo>) or a bare repo (uses default owner).
    fn parse_aivcs_uri(uri: &str, default_owner: &str) -> Result<(String, String)> {
        let rest = uri
            .strip_prefix("aivcs://")
            .ok_or_else(|| anyhow!("not an aivcs:// URI: {uri}"))?;
        // Drop the optional @host suffix.
        let repo_id = rest.split('@').next().unwrap_or(rest).trim_matches('/');
        if repo_id.is_empty() {
            return Err(anyhow!(
                "malformed aivcs SOURCE_URI {uri:?}; expected aivcs://<owner>/<repo>@aivcs.io"
            ));
        }
        match repo_id.split_once('/') {
            Some((owner, repo)) if !owner.is_empty() && !repo.is_empty() => {
                Ok((owner.to_string(), repo.trim_end_matches(".git").to_string()))
            }
            _ => Ok((
                default_owner.to_string(),
                repo_id.trim_end_matches(".git").to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_client_from_url() {
        let client = GitClient::from_url("https://github.com/lornu-ai/og-cli").unwrap();
        assert_eq!(client.owner, "lornu-ai");
        assert_eq!(client.repo, "og-cli");
    }

    #[test]
    fn test_git_client_api_endpoint() {
        let client = GitClient::new("lornu-ai".to_string(), "og-cli".to_string());
        assert!(client.api_endpoint().contains("api.github.com"));
    }

    #[test]
    fn test_aivcs_client_from_url() {
        let client = AivcsClient::from_url("https://aivcs.io/lornu-ai/og-cli").unwrap();
        assert_eq!(client.owner, "lornu-ai");
        assert_eq!(client.repo, "og-cli");
    }

    #[test]
    fn test_aivcs_client_api_endpoint() {
        let client = AivcsClient::new("lornu-ai".to_string(), "og-cli".to_string());
        assert!(client.api_endpoint().contains("aivcsd.aivcs.io"));
    }

    #[test]
    fn test_git_auth_header() {
        let mut client = GitClient::new("lornu-ai".to_string(), "og-cli".to_string());
        client.token = Some("test-token".to_string());
        assert!(client.auth_header().unwrap().contains("token"));
    }

    #[test]
    fn test_aivcs_auth_header() {
        let mut client = AivcsClient::new("lornu-ai".to_string(), "og-cli".to_string());
        client.token = Some("test-token".to_string());
        assert!(client.auth_header().unwrap().contains("Bearer"));
    }

    // URI parsing is pure (no env) — safe to test independently.
    #[test]
    fn test_parse_git_uri() {
        let (o, r) = SourceResolver::parse_git_uri("git://github.com/lornu-ai/og-cli").unwrap();
        assert_eq!((o.as_str(), r.as_str()), ("lornu-ai", "og-cli"));
        assert!(SourceResolver::parse_git_uri("git://github.com/only-one").is_err());
    }

    #[test]
    fn test_parse_aivcs_uri() {
        let (o, r) =
            SourceResolver::parse_aivcs_uri("aivcs://lornu-ai/og-cli@aivcs.io", "fallback")
                .unwrap();
        assert_eq!((o.as_str(), r.as_str()), ("lornu-ai", "og-cli"));
        // Bare repo-id falls back to the default owner (tenant).
        let (o, r) =
            SourceResolver::parse_aivcs_uri("aivcs://og-cli@aivcs.io", "lornu-ai").unwrap();
        assert_eq!((o.as_str(), r.as_str()), ("lornu-ai", "og-cli"));
    }

    // `SOURCE_URI` / `ENABLE_AIVCS_NATIVE` are process-global; drive the whole
    // routing contract from ONE sequential test so parallel tests don't race.
    #[test]
    fn test_source_resolver_routing_contract() {
        for k in ["SOURCE_URI", "ENABLE_AIVCS_NATIVE"] {
            std::env::remove_var(k);
        }

        // No env → Git (safe, offline-testable default).
        assert_eq!(SourceResolver::route().unwrap(), Transport::Git);
        let c = SourceResolver::resolve("lornu-ai", "og-cli").unwrap();
        assert_eq!(c.transport(), Transport::Git);
        assert_eq!((c.owner(), c.repo()), ("lornu-ai", "og-cli"));

        // ENABLE_AIVCS_NATIVE=true (no SOURCE_URI) → AIVCS.
        std::env::set_var("ENABLE_AIVCS_NATIVE", "true");
        assert_eq!(SourceResolver::route().unwrap(), Transport::Aivcs);
        assert_eq!(
            SourceResolver::resolve("lornu-ai", "og-cli")
                .unwrap()
                .transport(),
            Transport::Aivcs
        );
        std::env::remove_var("ENABLE_AIVCS_NATIVE");

        // SOURCE_URI scheme is authoritative and overrides the flag.
        std::env::set_var("SOURCE_URI", "git://github.com/lornu-ai/og-cli");
        std::env::set_var("ENABLE_AIVCS_NATIVE", "true"); // must NOT win over an explicit git:// URI
        let c = SourceResolver::resolve("x", "y").unwrap();
        assert_eq!(c.transport(), Transport::Git);
        assert_eq!((c.owner(), c.repo()), ("lornu-ai", "og-cli"));
        std::env::remove_var("ENABLE_AIVCS_NATIVE");

        std::env::set_var("SOURCE_URI", "aivcs://lornu-ai/og-cli@aivcs.io");
        let c = SourceResolver::resolve("x", "y").unwrap();
        assert_eq!(c.transport(), Transport::Aivcs);
        assert_eq!((c.owner(), c.repo()), ("lornu-ai", "og-cli"));

        // Unknown scheme → fail-closed (NOT a silent Git fallback).
        std::env::set_var("SOURCE_URI", "ftp://example.com/x/y");
        assert!(SourceResolver::route().is_err());
        assert!(SourceResolver::resolve("lornu-ai", "og-cli").is_err());

        std::env::remove_var("SOURCE_URI");
    }
}
