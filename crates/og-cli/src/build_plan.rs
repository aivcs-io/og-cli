//! Build helpers — shape og-propel requests from CLI build context.

use crate::backend::BuildContext;
use crate::build::{BuildConfig, PropelBuildRequest};
use crate::services::{self, OgService};
use anyhow::Result;

/// Resolve a propel build request from CLI flags + optional fleet service catalog entry.
pub fn propel_request(ctx: &BuildContext) -> Result<PropelBuildRequest> {
    let cfg = BuildConfig::default();
    let registry = cfg.registry();

    if let Some(ref name) = ctx.service {
        let svc = services::get(name)?;
        return Ok(propel_request_for_service(&svc, ctx, &registry));
    }

    Ok(PropelBuildRequest {
        repo_url: std::env::var("GIT_URL")
            .unwrap_or_else(|_| "https://github.com/lornu-ai/og-cli".to_string()),
        dockerfile: ctx
            .dockerfile
            .clone()
            .unwrap_or_else(|| "Dockerfile".to_string()),
        image_name: ctx
            .image_name
            .clone()
            .unwrap_or_else(|| "og-cli:latest".to_string()),
        platforms: cfg.platforms(),
        registry,
        push: !ctx.no_push,
        service_id: None,
        flake_ref: None,
    })
}

fn propel_request_for_service(
    svc: &OgService,
    ctx: &BuildContext,
    registry: &str,
) -> PropelBuildRequest {
    let image = ctx
        .image_name
        .clone()
        .unwrap_or_else(|| format!("{}/{}", registry.trim_end_matches('/'), svc.image_name));

    PropelBuildRequest {
        repo_url: svc.repo.to_string(),
        dockerfile: ctx.dockerfile.clone().unwrap_or_default(),
        image_name: image,
        platforms: BuildConfig::default().platforms(),
        registry: registry.to_string(),
        push: !ctx.no_push,
        service_id: Some(svc.name.to_string()),
        flake_ref: Some(svc.flake_ref.to_string()),
    }
}

/// Human-readable stub log for non-propel backends during fleet build development.
pub fn stub_build_log(ctx: &BuildContext, backend: &str) -> String {
    if let Some(ref name) = ctx.service {
        if let Ok(svc) = services::get(name) {
            return format!(
                "{backend} build stub for fleet service {}\n→ source: {}\n→ flake: {}\n→ cluster: {}",
                svc.name, svc.repo, svc.flake_ref, svc.cluster
            );
        }
    }
    let dockerfile = ctx.dockerfile.as_deref().unwrap_or("Dockerfile");
    format!("{backend} build stub\n→ Dockerfile: {dockerfile}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_service_request_uses_aivcs_source() {
        let ctx = BuildContext {
            service: Some("og-leader".to_string()),
            no_push: true,
            ..Default::default()
        };
        let req = propel_request(&ctx).unwrap();
        assert_eq!(req.repo_url, "aivcs://lornu-ai/og-leader");
        assert_eq!(req.flake_ref.as_deref(), Some(".#image"));
        assert_eq!(req.service_id.as_deref(), Some("og-leader"));
        assert!(!req.push);
    }
}
