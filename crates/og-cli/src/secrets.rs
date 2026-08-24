//! Secrets resolution via ESO (External Secrets Operator) + Vault
//!
//! og-cli reads credentials (GitHub PAT, Cloudflare tokens) from Kubernetes secrets
//! seeded by ESO. No hardcoded credentials in binary or config files.
//!
//! ESO syncs secrets from Vault (via og-agent-secrets) into K8s secrets, which
//! og-cli mounts as environment variables or files.

use anyhow::{anyhow, Result};
use tracing::info;

/// Read a secret from environment (typically injected by ESO).
/// Fails gracefully with helpful error message if secret is missing.
pub fn read_secret(name: &str, env_var: &str) -> Result<String> {
    match std::env::var(env_var) {
        Ok(value) if !value.trim().is_empty() => {
            info!("Secret '{}' resolved from env: {}", name, env_var);
            Ok(value)
        }
        Ok(_) => Err(anyhow!(
            "Secret '{}' (env: {}) is empty — ESO may not have synced yet",
            name,
            env_var
        )),
        Err(_) => Err(anyhow!(
            "Secret '{}' (env: {}) not found — ESO not configured or pod not mounted secret",
            name,
            env_var
        )),
    }
}

/// GitHub PAT for PR operations (gh CLI)
pub fn github_pat() -> Result<String> {
    read_secret("github_pat", "GITHUB_TOKEN")
}

/// Cloudflare API token for DNS operations
pub fn cloudflare_token() -> Result<String> {
    read_secret("cloudflare_token", "CLOUDFLARE_API_TOKEN")
}

/// og-leader A2A credentials (if using mTLS or bearer token)
pub fn og_leader_auth() -> Result<Option<String>> {
    // Optional: og-leader may use OIDC identity instead
    match std::env::var("OG_LEADER_AUTH_TOKEN") {
        Ok(token) if !token.is_empty() => Ok(Some(token)),
        _ => Ok(None),
    }
}

/// How og-cli can reach GitHub for PR operations. A classic PAT is **not**
/// required — an existing `gh auth login` session is accepted just as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubAccess {
    /// A token is present in the environment (`GITHUB_TOKEN` / `GH_TOKEN`).
    TokenEnv,
    /// The `gh` CLI is already authenticated (`gh auth status` succeeds). Carries
    /// the logged-in account when it can be parsed, so the caller can surface
    /// *which* identity gh will act as — `gh auth status` succeeds even when gh
    /// is flipped to the wrong account, so we never assert a bare green.
    GhCli { account: Option<String> },
    /// No GitHub credential detected — the caller should offer setup options.
    None,
}

/// Detect how (if at all) og-cli can authenticate to GitHub, without ever
/// requiring a PAT. Prefers an env token, then an existing `gh auth login`.
pub fn github_access() -> GithubAccess {
    let token_present = github_pat().is_ok()
        || std::env::var("GH_TOKEN")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
    let (gh_ok, gh_account) = match std::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
    {
        Ok(o) if o.status.success() => {
            // gh prints the status to stdout or stderr depending on version.
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            (true, parse_gh_account(&combined))
        }
        _ => (false, None),
    };
    classify_github_access(token_present, gh_ok, gh_account)
}

/// Parse the logged-in account from `gh auth status` output, which reads e.g.
/// `✓ Logged in to github.com account octocat (keyring)`. Best-effort: `None`
/// if the format doesn't match. Pure, so it is unit-tested directly.
fn parse_gh_account(status_output: &str) -> Option<String> {
    let mut toks = status_output.split_whitespace().peekable();
    while let Some(t) = toks.next() {
        if t == "account" {
            if let Some(next) = toks.next() {
                let name = next.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// Pure classifier for [`github_access`] — an env token wins, then a live `gh`
/// login (with its account, if known); neither ⇒ `None`. A PAT is never
/// *required* (og-cli GitHub-PAT bug).
fn classify_github_access(
    token_present: bool,
    gh_ok: bool,
    gh_account: Option<String>,
) -> GithubAccess {
    if token_present {
        GithubAccess::TokenEnv
    } else if gh_ok {
        GithubAccess::GhCli {
            account: gh_account,
        }
    } else {
        GithubAccess::None
    }
}

/// Check that optional ESO-seeded secrets are available. GitHub access is handled
/// separately by [`github_access`] and is NOT a required secret here.
pub fn validate_secrets_available() -> Result<()> {
    info!("Validating secrets availability...");

    // Cloudflare token optional (only needed for DNS operations)
    if std::env::var("OG_REQUIRE_CLOUDFLARE_TOKEN")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
    {
        cloudflare_token()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses a UNIQUELY-named env var so the three cases never share
    // process-global state — env vars are process-wide, so a shared name races
    // under the default parallel `cargo test` (og-bug-report#40).

    #[test]
    fn test_read_secret_missing_returns_error() {
        let var = "TEST_SECRET_VAR_MISSING";
        std::env::remove_var(var);
        let result = read_secret("test_secret", var);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_read_secret_empty_returns_error() {
        let var = "TEST_SECRET_VAR_EMPTY";
        std::env::set_var(var, "");
        let result = read_secret("test_secret", var);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_read_secret_present_succeeds() {
        let var = "TEST_SECRET_VAR_PRESENT";
        std::env::set_var(var, "test-value-123");
        let result = read_secret("test_secret", var);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-value-123");
    }

    #[test]
    fn classify_github_access_prefers_token_then_gh_login() {
        // No PAT required: an env token wins, else an existing `gh auth login`
        // is accepted (case B), else there is nothing and we offer setup.
        assert_eq!(
            classify_github_access(true, true, Some("x".into())),
            GithubAccess::TokenEnv
        );
        assert_eq!(
            classify_github_access(true, false, None),
            GithubAccess::TokenEnv
        );
        assert_eq!(
            classify_github_access(false, true, Some("octocat".into())),
            GithubAccess::GhCli {
                account: Some("octocat".into())
            }
        );
        // gh is logged in but the account couldn't be parsed — still GhCli, no account.
        assert_eq!(
            classify_github_access(false, true, None),
            GithubAccess::GhCli { account: None }
        );
        assert_eq!(
            classify_github_access(false, false, None),
            GithubAccess::None
        );
    }

    #[test]
    fn parse_gh_account_extracts_login_or_none() {
        assert_eq!(
            parse_gh_account("✓ Logged in to github.com account octocat (keyring)").as_deref(),
            Some("octocat")
        );
        assert_eq!(
            parse_gh_account("  ✓ Logged in to github.com account my-org-bot (oauth_token)")
                .as_deref(),
            Some("my-org-bot")
        );
        assert_eq!(
            parse_gh_account("You are not logged into any GitHub hosts."),
            None
        );
    }
}
