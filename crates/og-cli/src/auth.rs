use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

use crate::discovery;
use crate::secrets;

const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_CLIENT_ID: &str = "og-cli";
const DEVICE_AUTHORIZE_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum LoginProvider {
    #[default]
    Auto,
    Google,
    Github,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub token: String,
    pub user: String,
    pub email: String,
    pub expires_at: u64,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub audiences: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceAuthorizeResponse {
    device_code: String,
    user_code: String,
    // Part of the RFC 8628 response contract; we present `verification_uri_complete`.
    #[allow(dead_code)]
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceTokenSuccess {
    access_token: String,
    // Always "Bearer" per RFC 8628; retained for wire-contract completeness.
    #[serde(default)]
    #[allow(dead_code)]
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceTokenError {
    error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecodedJwtClaims {
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    aud: JwtAudience,
    #[serde(default)]
    pub exp: u64,
}

impl DecodedJwtClaims {
    pub fn audiences(&self) -> Vec<String> {
        self.aud.clone().into_vec()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
enum JwtAudience {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl JwtAudience {
    fn into_vec(self) -> Vec<String> {
        match self {
            JwtAudience::None => vec![],
            JwtAudience::One(v) => vec![v],
            JwtAudience::Many(v) => v,
        }
    }
}

pub struct AuthManager;

impl AuthManager {
    pub async fn login(provider: LoginProvider) -> Result<()> {
        info!("Initiating RFC 8628 device authorization flow");

        let auth_dir = Self::auth_dir()?;
        fs::create_dir_all(&auth_dir).context("Failed to create ~/.og/auth directory")?;

        let broker = discovery::resolve_identity_broker_url().await?;
        info!("identity-broker base URL: {broker}");

        let client_id =
            std::env::var("OG_AUTH_CLIENT_ID").unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_string());
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        let authorize = Self::device_authorize(&http, &broker, &client_id).await?;
        let verification_url = append_provider_hint(&authorize.verification_uri_complete, provider);

        println!("→ Sign in at: {verification_url}");
        println!("→ Code: {}", authorize.user_code);

        if browser_launch_disabled() || is_loopback_http_url(&verification_url) {
            info!("skipping browser launch (OG_AUTH_NO_BROWSER or loopback verify URL)");
            println!("→ Open the URL above manually to complete SSO (Google or GitHub).");
        } else {
            println!("→ Opening browser…");
            if let Err(err) = open_browser(&verification_url) {
                info!("could not open browser: {err:#}");
                println!("→ Open the URL above manually to complete SSO (Google or GitHub).");
            }
        }

        let token_response = Self::poll_device_token(
            &http,
            &broker,
            &client_id,
            &authorize.device_code,
            authorize.interval,
            authorize.expires_in,
        )
        .await?;

        let claims = decode_jwt_claims(&token_response.access_token)?;
        let now = now_secs();
        let expires_at = if claims.exp > now {
            claims.exp
        } else {
            now + token_response.expires_in
        };

        let email = claims.sub.clone();

        let scopes = if token_response.scope.is_empty() {
            vec![]
        } else {
            token_response
                .scope
                .split_whitespace()
                .map(String::from)
                .collect()
        };

        let stored = AuthToken {
            token: token_response.access_token,
            user: email.clone(),
            email,
            expires_at,
            scopes,
            audiences: claims.audiences(),
        };

        let token_path = Self::token_path()?;
        fs::write(
            &token_path,
            serde_json::to_string_pretty(&stored).context("serialize auth token")?,
        )
        .context("write ~/.og/auth/token")?;

        println!("✓ Authenticated as {}", stored.email);
        if !stored.audiences.is_empty() {
            println!("✓ Audiences: {}", stored.audiences.join(", "));
        }
        println!("✓ Token stored in {}", token_path.display());

        // GitHub access for PR operations. og-cli does NOT require a classic PAT:
        // an env token works, and an existing `gh auth login` session is accepted
        // just as well. If neither is present, offer the two supported setup paths
        // instead of a scary warning.
        match secrets::github_access() {
            secrets::GithubAccess::TokenEnv => {
                println!("✓ GitHub access via environment token");
            }
            secrets::GithubAccess::GhCli { account } => match account {
                // Surface the identity — `gh auth status` succeeds even when gh is
                // flipped to the wrong account, so we show *who* rather than assert
                // a bare green (og-cli#60 review).
                Some(login) => println!(
                    "✓ GitHub access via `gh` login as '{login}' — if that's the wrong account, \
                     run  !gh auth switch  from the og shell"
                ),
                None => println!(
                    "✓ GitHub access via existing `gh` login (run  !gh auth status  to confirm \
                     the account)"
                ),
            },
            secrets::GithubAccess::None => {
                println!("ℹ️  No GitHub credential yet — og-cli needs GitHub access for PR");
                println!("   operations. No personal access token required; set up either:");
                println!("     • a GitHub MCP server (add it to your MCP config), or");
                println!(
                    "     • run  !gh auth login  from the og shell (uses gh's own browser login)"
                );
            }
        }

        // Sync Global OIDC/WIF token to ~/.cargo/credentials.toml for sovereign registry.aivcs.io
        if let Err(err) = Self::sync_cargo_credentials(&stored.token) {
            info!("could not sync cargo credentials: {err}");
        }

        // Other optional ESO-seeded secrets (e.g. Cloudflare) — soft check only.
        if let Err(err) = secrets::validate_secrets_available() {
            eprintln!("⚠️  {err}");
        }

        Ok(())
    }

    fn sync_cargo_credentials(token: &str) -> Result<()> {
        let home = dirs::home_dir().context("resolve home dir")?;
        let cargo_dir = home.join(".cargo");
        fs::create_dir_all(&cargo_dir)?;
        let creds_path = cargo_dir.join("credentials.toml");
        let content = if creds_path.exists() {
            fs::read_to_string(&creds_path).unwrap_or_default()
        } else {
            String::new()
        };
        let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        // Remove existing [registries.aivcs] section if present
        let mut in_aivcs = false;
        lines.retain(|line| {
            if line.trim() == "[registries.aivcs]" {
                in_aivcs = true;
                return false;
            }
            if in_aivcs {
                if line.trim().starts_with('[') {
                    in_aivcs = false;
                    return true;
                }
                return false;
            }
            true
        });
        lines.push(String::new());
        lines.push("[registries.aivcs]".to_string());
        lines.push(format!("token = \"{}\"", token));
        fs::write(&creds_path, lines.join("\n").trim())?;
        println!("✓ Cargo credentials synced for [registries.aivcs] (registry.aivcs.io)");
        Ok(())
    }

    /// The stored OIDC access token (for `Authorization: Bearer`), if a **valid,
    /// non-expired** session exists. Read-only; never triggers a login. Used by
    /// the off-cluster edge-gateway transport (og-cli#65).
    pub fn access_token() -> Option<String> {
        let path = Self::token_path().ok()?;
        let token: AuthToken = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
        if token.expires_at <= now_secs() {
            return None;
        }
        Some(token.token)
    }

    /// One-line session summary for the shell banner; `None` if not signed in.
    /// Read-only — never triggers a login.
    pub fn session_summary() -> Option<String> {
        let path = Self::token_path().ok()?;
        if !path.exists() {
            return None;
        }
        let token: AuthToken = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
        let remaining = token.expires_at.saturating_sub(now_secs());
        Some(if remaining == 0 {
            format!(
                "{} — session expired, use `/login` inside `og`",
                token.email
            )
        } else {
            format!(
                "{} — {}h{}m left",
                token.email,
                remaining / 3600,
                (remaining % 3600) / 60
            )
        })
    }

    /// Require a valid authenticated session, returning the stored token.
    ///
    /// Fail-closed gate for sovereign, mutating actions (e.g. repo creation,
    /// code-governance#795): bails if no token is stored **or** the token has
    /// expired. Read-only — never triggers a login. The returned [`AuthToken`]
    /// supplies the actor (email) for an audit record; callers must NEVER proceed
    /// without it.
    pub fn require_session() -> Result<AuthToken> {
        let token_path = Self::token_path()?;
        if !token_path.exists() {
            bail!("not logged in — use `/login` inside `og` first (authentication required)");
        }
        let token: AuthToken =
            serde_json::from_str(&fs::read_to_string(&token_path)?).context("parse token file")?;
        if token.expires_at.saturating_sub(now_secs()) == 0 {
            bail!("session expired — use `/login` inside `og` again (authentication required)");
        }
        Ok(token)
    }

    pub fn logout() -> Result<()> {
        let token_path = Self::token_path()?;
        if token_path.exists() {
            fs::remove_file(&token_path).context("remove token file")?;
            println!("✓ Logged out. Credentials removed from ~/.og/auth");
        } else {
            println!("Not currently logged in");
        }
        Ok(())
    }

    /// Bearer access token for sandlot/propel HTTP calls. Errors if not logged in or expired.
    pub fn bearer_token() -> Result<String> {
        let token_path = Self::token_path()?;
        if !token_path.exists() {
            bail!("Not logged in. Use `/login` inside `og` first");
        }
        let token: AuthToken =
            serde_json::from_str(&fs::read_to_string(&token_path)?).context("parse token file")?;
        if token.expires_at <= now_secs() {
            bail!("session expired — use `/login` inside `og` again");
        }
        Ok(token.token)
    }

    pub fn whoami() -> Result<()> {
        let token_path = Self::token_path()?;
        if !token_path.exists() {
            bail!("Not logged in. Use `/login` inside `og` first");
        }

        let token: AuthToken =
            serde_json::from_str(&fs::read_to_string(&token_path)?).context("parse token file")?;

        println!("User: {} ({})", token.user, token.email);

        let now = now_secs();
        let seconds_remaining = token.expires_at.saturating_sub(now);
        if seconds_remaining == 0 {
            println!("Token: expired — use `/login` inside `og` again");
        } else {
            let hours = seconds_remaining / 3600;
            let mins = (seconds_remaining % 3600) / 60;
            println!("Token expires in: {hours}h {mins}m ({})", token.expires_at);
        }

        if !token.audiences.is_empty() {
            println!("Audiences: {}", token.audiences.join(", "));
        }
        if !token.scopes.is_empty() {
            println!("Scopes: {}", token.scopes.join(", "));
        }
        Ok(())
    }

    async fn device_authorize(
        http: &reqwest::Client,
        broker: &str,
        client_id: &str,
    ) -> Result<DeviceAuthorizeResponse> {
        let url = format!("{}/auth/device", broker.trim_end_matches('/'));
        for attempt in 0..DEVICE_AUTHORIZE_ATTEMPTS {
            let response = match http
                .post(&url)
                .form(&[("client_id", client_id)])
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) if attempt + 1 < DEVICE_AUTHORIZE_ATTEMPTS => {
                    info!(
                        "device authorize attempt {} failed ({err}); retrying",
                        attempt + 1
                    );
                    tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                    continue;
                }
                Err(err) => {
                    bail!(
                        "device authorize request failed after {} attempts: {err}",
                        DEVICE_AUTHORIZE_ATTEMPTS
                    );
                }
            };

            if response.status().is_success() {
                return response
                    .json::<DeviceAuthorizeResponse>()
                    .await
                    .context("parse device authorize response");
            }

            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if (status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
                && attempt + 1 < DEVICE_AUTHORIZE_ATTEMPTS
            {
                info!(
                    "device authorize returned {status} on attempt {}; retrying",
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                continue;
            }

            let detail = if body.trim().is_empty() {
                "empty response body"
            } else {
                body.trim()
            };
            bail!("device authorize failed with HTTP {status}: {detail}");
        }

        unreachable!("device authorization loop always returns or bails")
    }

    async fn poll_device_token(
        http: &reqwest::Client,
        broker: &str,
        client_id: &str,
        device_code: &str,
        mut interval: u64,
        expires_in: u64,
    ) -> Result<DeviceTokenSuccess> {
        let url = format!("{}/auth/device/token", broker.trim_end_matches('/'));
        let deadline = std::time::Instant::now() + Duration::from_secs(expires_in.max(1));

        loop {
            if std::time::Instant::now() >= deadline {
                bail!("device authorization timed out");
            }

            tokio::time::sleep(Duration::from_secs(interval.max(1))).await;

            let response = http
                .post(&url)
                .form(&[
                    ("grant_type", DEVICE_GRANT_TYPE),
                    ("device_code", device_code),
                    ("client_id", client_id),
                ])
                .send()
                .await
                .with_context(|| format!("POST {url} failed"))?;

            if response.status().is_success() {
                return response
                    .json::<DeviceTokenSuccess>()
                    .await
                    .context("parse device token response");
            }

            let body = response.text().await.unwrap_or_default();
            let error = serde_json::from_str::<DeviceTokenError>(&body)
                .map(|e| e.error)
                .unwrap_or_else(|_| body.clone());

            match error.as_str() {
                "authorization_pending" => {
                    print!(".");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    continue;
                }
                "slow_down" => {
                    interval = interval.saturating_add(5);
                    continue;
                }
                "expired_token" => bail!("device code expired — use `/login` inside `og` again"),
                "access_denied" => bail!("sign-in denied at verification URI"),
                "invalid_grant" => bail!("invalid device grant"),
                other => bail!("device token error: {other}"),
            }
        }
    }

    fn auth_dir() -> Result<PathBuf> {
        dirs::home_dir()
            .map(|h| h.join(".og").join("auth"))
            .context("home directory not found")
    }

    fn token_path() -> Result<PathBuf> {
        Ok(Self::auth_dir()?.join("token"))
    }
}

/// Append SSO IdP hint for the fleet proxy (`?provider=google|github`).
pub fn append_provider_hint(url: &str, provider: LoginProvider) -> String {
    let hint = match provider {
        LoginProvider::Auto => return url.to_string(),
        LoginProvider::Google => "google",
        LoginProvider::Github => "github",
    };

    if url.contains("provider=") {
        return url.to_string();
    }

    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}provider={hint}")
}

pub fn decode_jwt_claims(token: &str) -> Result<DecodedJwtClaims> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow!("JWT missing payload segment"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| {
            base64::engine::general_purpose::URL_SAFE.decode(payload.trim_end_matches('='))
        })
        .context("JWT payload base64 decode")?;
    serde_json::from_slice(&bytes).context("JWT payload JSON")
}

/// Skip auto-open for tests (`OG_AUTH_NO_BROWSER=1`) and for loopback verify
/// URLs (wiremock / local mocks have no UI — opening them yields a blank tab).
fn browser_launch_disabled() -> bool {
    matches!(
        std::env::var("OG_AUTH_NO_BROWSER").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn is_loopback_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    [
        "http://127.0.0.1:",
        "http://127.0.0.1/",
        "http://localhost:",
        "http://localhost/",
        "http://[::1]:",
        "http://[::1]/",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .context("spawn open")?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .context("spawn xdg-open")?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("spawn cmd start")?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        bail!("unsupported platform for browser launch");
    }
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_verify_urls_are_detected() {
        assert!(is_loopback_http_url(
            "http://127.0.0.1:54221/auth/device/verify?user_code=BCDF-GHJK&provider=google"
        ));
        assert!(is_loopback_http_url(
            "http://localhost:1234/auth/device/verify?user_code=ABCD-EFGH"
        ));
        assert!(!is_loopback_http_url(
            "https://identity-broker.aivcs.io/auth/device/verify?user_code=ABCD-EFGH"
        ));
    }

    #[test]
    fn append_provider_hint_google() {
        let url = "https://identity-broker.aivcs.io/auth/device/verify?user_code=ABCD-EFGH";
        assert_eq!(
            append_provider_hint(url, LoginProvider::Google),
            "https://identity-broker.aivcs.io/auth/device/verify?user_code=ABCD-EFGH&provider=google"
        );
    }

    #[test]
    fn append_provider_hint_auto_unchanged() {
        let url = "https://example.com/verify";
        assert_eq!(append_provider_hint(url, LoginProvider::Auto), url);
    }

    #[test]
    fn decode_jwt_claims_parses_sub_and_aud() {
        let payload = serde_json::json!({
            "sub": "user@example.com",
            "aud": ["aivcs", "github"],
            "exp": 9999999999_u64
        });
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());
        let token = format!("header.{encoded}.sig");
        let claims = decode_jwt_claims(&token).unwrap();
        assert_eq!(claims.sub, "user@example.com");
        assert_eq!(claims.audiences(), vec!["aivcs", "github"]);
    }
}
