/// Organization (tenant) management
use crate::account::AccountManager;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgContext {
    pub active_org: String,
    pub available_orgs: Vec<String>,
}

impl Default for OrgContext {
    fn default() -> Self {
        Self {
            active_org: "lornu-ai".to_string(),
            available_orgs: vec!["lornu-ai".to_string()],
        }
    }
}

pub struct OrgManager;

impl OrgManager {
    /// List available organizations for active account
    pub fn list() -> Result<()> {
        let context = Self::load_context()?;

        println!("Available organizations:");
        for org in &context.available_orgs {
            let marker = if org == &context.active_org {
                "* (active)"
            } else {
                ""
            };
            println!("  {} {}", org, marker);
        }
        Ok(())
    }

    /// Switch to a different organization (code-governance#785).
    ///
    /// `available_orgs` is a **convenience cache of orgs seen before**, not an
    /// allowlist. It used to gate this call, which made `og org use` the odd
    /// one out among the three ways to select an org:
    ///
    /// | path | any org? |
    /// |---|---|
    /// | URL detection (`org_from_url`) | yes |
    /// | `OG_ORG=acme-corp` | yes |
    /// | `og org use acme-corp` | **no — rejected** |
    ///
    /// Nothing ever populated the cache (`fetch_available_orgs` had no
    /// callers), so it was permanently `["lornu-ai"]` and every other org was
    /// refused — while the same org worked fine via a URL or the env var. The
    /// membership check belongs to the backend that can actually authorize it
    /// (aivcs.io / og-leader), not to an unpopulated local list.
    ///
    /// Switching now records the org in the cache so `og org list` shows it.
    pub fn use_org(org: &str) -> Result<()> {
        let mut context = Self::load_context()?;
        let org = Self::apply_use_org(&mut context, org)?;
        Self::save_context(&context)?;
        info!("Switched to org: {}", org);
        println!("✓ Switched to org: {}", org);
        Ok(())
    }

    /// The switch itself, separated from disk I/O so it is directly testable —
    /// `context_path()` resolves through `dirs::config_dir()` and cannot be
    /// pointed at a temp dir from a unit test.
    fn apply_use_org(context: &mut OrgContext, org: &str) -> Result<String> {
        let org = Self::validate_org_name(org)?;
        context.active_org = org.clone();
        if !context.available_orgs.contains(&org) {
            context.available_orgs.push(org.clone());
            context.available_orgs.sort();
        }
        Ok(org)
    }

    /// An org name is a tenant id AND a URL path segment, so reject anything
    /// that cannot safely be both. Empty/whitespace is the important case: it
    /// would otherwise persist a blank tenant, and a blank tenant is exactly
    /// the "absence read as permission" failure the fleet keeps hitting
    /// (code-governance#1059).
    fn validate_org_name(org: &str) -> Result<String> {
        let org = org.trim();
        if org.is_empty() {
            return Err(anyhow!("Organization name cannot be empty"));
        }
        if org.contains('/') || org.contains('\\') || org.contains(char::is_whitespace) {
            return Err(anyhow!(
                "Invalid organization name {org:?}: no slashes or whitespace \
                 (it is used as a tenant id and a URL path segment)"
            ));
        }
        Ok(org.to_string())
    }

    /// Get active org.
    ///
    /// Precedence (multi-org support, code-governance#785):
    /// 1. `OG_ORG` environment override (per-invocation, non-persistent), then
    /// 2. the persisted active-org context (`og org use <org>`), else the default.
    pub fn get_active() -> Result<String> {
        if let Ok(env_org) = std::env::var("OG_ORG") {
            let trimmed = env_org.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        let context = Self::load_context()?;
        Ok(context.active_org)
    }

    /// Detect the org (tenant) from a repository/PR URL (code-governance#785).
    ///
    /// Recognises GitHub and the sovereign aivcs.io forge:
    /// - `https://github.com/<org>/<repo>/...`
    /// - `https://aivcs.io/<org>/<repo>/...` (and `www.aivcs.io`)
    /// - `aivcs://<org>/<repo>`
    ///
    /// Returns `None` for a non-URL reference (e.g. `#123`) or an unrecognised
    /// host, so the caller can fall back to the env/context org.
    pub fn org_from_url(url: &str) -> Option<String> {
        let url = url.trim();

        // Native aivcs transport: aivcs://<org>/<repo>
        if let Some(rest) = url.strip_prefix("aivcs://") {
            return rest
                .split('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }

        // http(s)://<host>/<org>/<repo>/...  — only for known hosts.
        let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        let (host, path) = after_scheme.split_once('/')?;
        if !(host.contains("github.com") || host.contains("aivcs.io")) {
            return None;
        }
        path.split('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// Resolve the org for a command, given an optional URL reference.
    ///
    /// Precedence: URL-detected org → `OG_ORG` env → active context → default
    /// (`lornu-ai`). This is the single runtime resolver for A2A tenant routing.
    pub fn resolve_org(url: Option<&str>) -> String {
        if let Some(org) = url.and_then(Self::org_from_url) {
            return org;
        }
        Self::get_active().unwrap_or_else(|_| "lornu-ai".to_string())
    }

    /// Orgs known locally, plus any the active account implies.
    ///
    /// **This is not an authorization boundary.** Real membership lives in
    /// aivcs.io and is enforced by the backend that serves the request; this is
    /// a local convenience list for `og org list`. It deliberately no longer
    /// gates [`Self::use_org`] — see that function for why.
    ///
    /// Querying aivcs.io for actual memberships is FR #785 Phase 3 and needs
    /// the identity token; until then this reports what we have seen locally
    /// rather than pretending to know the full set.
    pub fn fetch_available_orgs() -> Result<Vec<String>> {
        let mut orgs = Self::load_context()?.available_orgs;
        if AccountManager::get_active()?.is_some() {
            // Placeholder for the aivcs.io membership query (Phase 3).
            // Intentionally additive: never shrink the local list from here, or
            // a failed/partial lookup would silently hide an org the user has.
        }
        if orgs.is_empty() {
            orgs.push("lornu-ai".to_string());
        }
        orgs.sort();
        orgs.dedup();
        Ok(orgs)
    }

    /// Update available orgs for active account
    pub fn update_available_orgs(orgs: Vec<String>) -> Result<()> {
        let mut context = Self::load_context()?;
        context.available_orgs = orgs;
        Self::save_context(&context)?;
        Ok(())
    }

    /// Get org-scoped tenant header for API calls (data-mesh consistent)
    pub fn get_tenant_header() -> Result<String> {
        let org = Self::get_active()?;
        Ok(format!("org: {}", org))
    }

    // Private helpers
    fn context_path() -> Result<PathBuf> {
        let config_dir =
            dirs::config_dir().ok_or_else(|| anyhow!("Cannot find config directory"))?;
        Ok(config_dir.join("og").join("org_context.json"))
    }

    fn load_context() -> Result<OrgContext> {
        let path = Self::context_path()?;
        if path.exists() {
            let data = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(OrgContext::default())
        }
    }

    fn save_context(context: &OrgContext) -> Result<()> {
        let path = Self::context_path()?;
        fs::create_dir_all(path.parent().unwrap())?;
        let data = serde_json::to_string_pretty(context)?;
        fs::write(&path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_org_context_default() {
        let context = OrgContext::default();
        assert_eq!(context.active_org, "lornu-ai");
        assert!(context.available_orgs.contains(&"lornu-ai".to_string()));
    }

    #[test]
    fn org_from_github_url() {
        assert_eq!(
            OrgManager::org_from_url("https://github.com/acme-corp/code-governance/pull/123")
                .as_deref(),
            Some("acme-corp")
        );
        assert_eq!(
            OrgManager::org_from_url("https://github.com/lornu-ai/infra-code/pull/5").as_deref(),
            Some("lornu-ai")
        );
    }

    #[test]
    fn org_from_aivcs_urls() {
        assert_eq!(
            OrgManager::org_from_url("https://aivcs.io/acme-corp/infra-code/pull/45").as_deref(),
            Some("acme-corp")
        );
        assert_eq!(
            OrgManager::org_from_url("https://www.aivcs.io/team-x/repo").as_deref(),
            Some("team-x")
        );
        assert_eq!(
            OrgManager::org_from_url("aivcs://team-y/og-leader").as_deref(),
            Some("team-y")
        );
    }

    #[test]
    fn org_from_url_rejects_non_urls_and_unknown_hosts() {
        assert_eq!(OrgManager::org_from_url("#123"), None);
        assert_eq!(OrgManager::org_from_url("og-cli"), None);
        assert_eq!(
            OrgManager::org_from_url("https://gitlab.com/acme/repo"),
            None
        );
        assert_eq!(OrgManager::org_from_url("https://github.com/"), None);
    }

    /// Regression (code-governance#785): all three ways of selecting an org
    /// must accept the same set. `use_org` used to be gated on an
    /// `available_orgs` list that nothing ever populated, so a URL or `OG_ORG`
    /// could select `acme-corp` while `og org use acme-corp` was rejected.
    #[test]
    fn an_org_name_valid_by_url_is_also_valid_by_name() {
        for org in ["acme-corp", "team-x", "lornu-ai"] {
            let from_url =
                OrgManager::org_from_url(&format!("https://github.com/{org}/repo/pull/1"));
            assert_eq!(from_url.as_deref(), Some(org));
            // The same name must survive the `og org use` validator.
            assert_eq!(OrgManager::validate_org_name(org).unwrap(), org);
        }
    }

    /// The behaviour that regressed: switching to an org the local cache has
    /// never seen must succeed and record it. Previously `use_org` rejected
    /// anything not already in `available_orgs`, and nothing ever populated
    /// that list, so only `lornu-ai` was reachable.
    #[test]
    fn switching_to_an_unknown_org_succeeds_and_records_it() {
        let mut ctx = OrgContext::default();
        assert_eq!(ctx.available_orgs, vec!["lornu-ai".to_string()]);

        let got = OrgManager::apply_use_org(&mut ctx, "acme-corp").expect("switch must succeed");

        assert_eq!(got, "acme-corp");
        assert_eq!(ctx.active_org, "acme-corp");
        assert!(
            ctx.available_orgs.contains(&"acme-corp".to_string()),
            "the org should be remembered for `og org list`: {:?}",
            ctx.available_orgs
        );
        // Idempotent: switching again must not duplicate the entry.
        OrgManager::apply_use_org(&mut ctx, "acme-corp").unwrap();
        assert_eq!(
            ctx.available_orgs
                .iter()
                .filter(|o| *o == "acme-corp")
                .count(),
            1
        );
    }

    /// An org name becomes a tenant id and a URL segment, so the blank and
    /// separator cases must be refused rather than persisted.
    #[test]
    fn invalid_org_names_are_rejected() {
        for bad in ["", "   ", "acme corp", "acme/corp", "acme\\corp"] {
            assert!(
                OrgManager::validate_org_name(bad).is_err(),
                "{bad:?} was accepted"
            );
        }
        // Surrounding whitespace is trimmed, not rejected.
        assert_eq!(
            OrgManager::validate_org_name("  acme-corp  ").unwrap(),
            "acme-corp"
        );
    }

    #[test]
    fn resolve_org_prefers_url_over_everything() {
        // URL org wins regardless of env/context — pure, does not read env.
        assert_eq!(
            OrgManager::resolve_org(Some("https://github.com/acme-corp/x/pull/1")),
            "acme-corp"
        );
        // A non-URL reference falls through to the env/context resolver — assert
        // it agrees with that path rather than a literal, so a machine-local
        // OG_ORG / org_context.json can't flake the test.
        let fallback = OrgManager::get_active().unwrap_or_else(|_| "lornu-ai".to_string());
        assert_eq!(OrgManager::resolve_org(Some("#7")), fallback);
        assert_eq!(OrgManager::resolve_org(None), fallback);
    }
}
