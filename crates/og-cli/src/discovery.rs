//! Resolve production service endpoints via the `service-discovery` standard offering.
//!
//! Consumers call `GET {DISCOVERY_URL}/v1/discover?brand=aivcs&capability=…` and connect
//! to the returned publication (SD5). With `DISCOVERY_REQUIRED=1`, resolution failures
//! are fatal (SD-fail-closed).

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tracing::{info, warn};

const DEFAULT_DISCOVERY_URL: &str = "https://discovery.aivcs.io";
const DEFAULT_BRAND: &str = "aivcs";

/// Well-known public routes when the catalog omits `routes` (discovery-api gap, 2026-08-01).
const KNOWN_PUBLIC_ROUTES: &[(&str, &str)] = &[
    ("propel/propel", "https://propel.aivcs.io"),
    (
        "identity-broker/identity-broker",
        "https://identity-broker.aivcs.io",
    ),
    ("sandlot/sandlot", "https://sandlot.uk"),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Publication {
    pub service_id: String,
    pub capability: String,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub exposure_class: Option<String>,
    #[serde(default)]
    pub cluster: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
}

pub fn discovery_url() -> String {
    std::env::var("DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string())
}

pub fn discovery_required() -> bool {
    matches!(
        std::env::var("DISCOVERY_REQUIRED").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

pub fn brand() -> String {
    std::env::var("DISCOVERY_BRAND").unwrap_or_else(|_| DEFAULT_BRAND.to_string())
}

/// Resolve the identity-broker HTTPS base URL for `og login`.
pub async fn resolve_identity_broker_url() -> Result<String> {
    if let Ok(url) = std::env::var("OG_IDENTITY_BROKER_URL") {
        let trimmed = url.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    match resolve_https_endpoint("identity", Some("identity-broker/identity-broker")).await {
        Ok(url) => Ok(url),
        Err(err) if discovery_required() => Err(err),
        Err(err) => {
            info!("identity broker discovery failed ({err:#}); using documented public route");
            Ok("https://identity-broker.aivcs.io".to_string())
        }
    }
}

/// Resolve the **og-leader edge gateway** base URL — the off-cluster A2A
/// submit/status front (og-cli#65, capability `og-leader`). Env override
/// `OG_LEADER_GATEWAY_URL`. Errors (no fabricated default) when the catalog has
/// no publication — off-cluster has nothing to route to, and must fail closed.
pub async fn resolve_og_leader_gateway_url() -> Result<String> {
    if let Ok(url) = std::env::var("OG_LEADER_GATEWAY_URL") {
        let trimmed = url.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    resolve_https_endpoint("og-leader", None).await
}

/// Resolve the **externally-reachable data-mesh endpoint** for off-cluster A2A dispatch.
/// Env override `OG_DATA_MESH_URL`. Errors (no fabricated default) when the catalog has
/// no publication — off-cluster cannot reach the in-cluster default, must fail closed
/// with actionable guidance (code-governance#1027).
pub async fn resolve_data_mesh_url() -> Result<String> {
    if let Ok(url) = std::env::var("OG_DATA_MESH_URL") {
        let trimmed = url.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    match resolve_https_endpoint("data-mesh", None).await {
        Ok(url) => Ok(url),
        Err(err) => Err(anyhow!(
            "data-mesh discovery failed: {err}. \
             The service-discovery catalog must publish an externally-reachable data-mesh endpoint \
             (capability=data-mesh) for og-cli to dispatch A2A work from off-cluster (code-governance#1027). \
             Set OG_DATA_MESH_URL to override, or work from within the cluster."
        )),
    }
}

/// Resolve sandlot.uk HTTPS base URL for remote CI/build (code-governance#1176).
pub async fn resolve_sandlot_endpoint() -> Result<String> {
    resolve_https_endpoint("sandlot", Some("sandlot/sandlot")).await
}

/// Resolve a public HTTPS base URL for a catalog capability (e.g. `propel`, `identity`).
pub async fn resolve_https_endpoint(capability: &str, service_id: Option<&str>) -> Result<String> {
    let publications = fetch_publications(capability).await?;
    let publication = select_publication(publications, service_id)?;
    endpoint_from_publication(&publication)
}

async fn fetch_publications(capability: &str) -> Result<Vec<Publication>> {
    let base = discovery_url();
    let url = format!("{}/v1/discover", base.trim_end_matches('/'));
    info!("service-discovery: GET {} capability={}", url, capability);

    let response = reqwest::Client::new()
        .get(&url)
        .query(&[("brand", brand()), ("capability", capability.to_string())])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .with_context(|| format!("discovery request to {url} failed"))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "discovery returned {} for capability={capability}",
            response.status()
        ));
    }

    response
        .json::<Vec<Publication>>()
        .await
        .context("failed to parse discovery catalog response")
}

fn select_publication(
    publications: Vec<Publication>,
    service_id: Option<&str>,
) -> Result<Publication> {
    if publications.is_empty() {
        return Err(anyhow!("no publications for requested capability"));
    }

    if let Some(id) = service_id {
        return publications
            .into_iter()
            .find(|p| p.service_id == id)
            .ok_or_else(|| anyhow!("service-id {id} not found in catalog"));
    }

    let fallback = publications.first().cloned();
    publications
        .into_iter()
        .find(|p| p.exposure_class.as_deref() == Some("Public"))
        .or(fallback)
        .ok_or_else(|| anyhow!("no publication matched"))
}

fn endpoint_from_publication(publication: &Publication) -> Result<String> {
    for route in &publication.routes {
        let host = route.trim();
        if host.is_empty() {
            continue;
        }
        if !is_public_route(host) {
            warn!(
                "discovery: ignoring non-public route {host:?} for {} (not a public hostname)",
                publication.service_id
            );
            continue;
        }
        return Ok(normalize_public_url(host));
    }

    for (id, url) in KNOWN_PUBLIC_ROUTES {
        if publication.service_id == *id {
            return Ok((*url).to_string());
        }
    }

    Err(anyhow!(
        "publication {} has no usable public route and no known public hostname",
        publication.service_id
    ))
}

fn is_public_route(route: &str) -> bool {
    let authority = route
        .strip_prefix("https://")
        .or_else(|| route.strip_prefix("http://"))
        .unwrap_or(route);
    let host = authority
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    !host.is_empty() && host.contains('.') && !host.contains(' ')
}

fn normalize_public_url(route: &str) -> String {
    if route.starts_with("http://") || route.starts_with("https://") {
        route.to_string()
    } else {
        format!("https://{route}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_public_url_adds_https() {
        assert_eq!(
            normalize_public_url("propel.aivcs.io"),
            "https://propel.aivcs.io"
        );
    }

    #[test]
    fn identity_broker_known_route_fallback() {
        let pub_ = Publication {
            service_id: "identity-broker/identity-broker".to_string(),
            capability: "identity".to_string(),
            brand: None,
            port: None,
            routes: vec![],
            exposure_class: Some("Public".to_string()),
            cluster: None,
            namespace: None,
        };
        assert_eq!(
            endpoint_from_publication(&pub_).unwrap(),
            "https://identity-broker.aivcs.io"
        );
    }

    #[test]
    fn sandlot_known_route_fallback() {
        let pub_ = Publication {
            service_id: "sandlot/sandlot".to_string(),
            capability: "sandlot".to_string(),
            brand: None,
            port: None,
            routes: vec![],
            exposure_class: Some("Public".to_string()),
            cluster: None,
            namespace: None,
        };
        assert_eq!(
            endpoint_from_publication(&pub_).unwrap(),
            "https://sandlot.uk"
        );
    }

    #[test]
    fn is_public_route_rejects_internal_route_ids() {
        assert!(!is_public_route(
            "gateway-routes/internal-cluster-identity-broker-identity-broker-identity-broker"
        ));
        assert!(!is_public_route("gateway-routes"));
        assert!(!is_public_route("identity-broker"));
        assert!(!is_public_route(""));
        assert!(is_public_route("identity-broker.aivcs.io"));
        assert!(is_public_route("https://identity-broker.aivcs.io"));
    }

    #[test]
    fn internal_route_is_skipped_for_known_public_fallback() {
        let pub_ = Publication {
            service_id: "identity-broker/identity-broker".to_string(),
            capability: "identity".to_string(),
            brand: None,
            port: None,
            routes: vec![
                "gateway-routes/internal-cluster-identity-broker-identity-broker-identity-broker"
                    .to_string(),
            ],
            exposure_class: Some("Public".to_string()),
            cluster: None,
            namespace: None,
        };
        assert_eq!(
            endpoint_from_publication(&pub_).unwrap(),
            "https://identity-broker.aivcs.io"
        );
    }

    #[test]
    fn internal_route_only_and_unknown_service_errors_not_bogus_url() {
        let pub_ = Publication {
            service_id: "mystery/mystery".to_string(),
            capability: "identity".to_string(),
            brand: None,
            port: None,
            routes: vec!["gateway-routes/whatever".to_string()],
            exposure_class: None,
            cluster: None,
            namespace: None,
        };
        assert!(endpoint_from_publication(&pub_).is_err());
    }

    #[test]
    fn select_publication_by_service_id() {
        let pubs = vec![
            Publication {
                service_id: "other/other".to_string(),
                capability: "propel".to_string(),
                brand: None,
                port: None,
                routes: vec![],
                exposure_class: None,
                cluster: None,
                namespace: None,
            },
            Publication {
                service_id: "propel/propel".to_string(),
                capability: "propel".to_string(),
                brand: None,
                port: None,
                routes: vec!["propel.aivcs.io".to_string()],
                exposure_class: Some("Public".to_string()),
                cluster: None,
                namespace: None,
            },
        ];
        let picked = select_publication(pubs, Some("propel/propel")).unwrap();
        assert_eq!(picked.service_id, "propel/propel");
    }
}
