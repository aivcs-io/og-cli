//! Backend selection contract (post og-bug-report#31 / code-governance#630).
//!
//! og-cli routes only through **og-leader** (A2A). The former fabricating
//! AOA/Kubernetes/Harbormaster backends are gone: the default backend is
//! `OgLeader`, `OG_BACKEND=local` opts into honest local-tool execution, and the
//! workflow calls **fail closed** when og-leader is unreachable — they never
//! return a synthesized "passed"/"merged" verdict.

use og_cli::backend::BackendFactory;
use og_cli::config::ProjectConfig;

// One sequential test: `OG_BACKEND`/`OG_LEADER_ENDPOINT` are process-global, so
// parallel env-mutating tests race. This covers the whole selection contract.
#[tokio::test]
async fn test_backend_selection_contract() {
    let config = ProjectConfig::default();

    // Hermetic: snapshot every env var this contract touches, drive them to known
    // values, and restore on exit — so an ambient DATA_MESH_* (a dev's real mesh
    // config) can't make the fail-closed assertions pass or fail by accident.
    const KEYS: [&str; 6] = [
        "OG_BACKEND",
        "OG_FALLBACK",
        "DATA_MESH_URL",
        "DATAMESH_URL",
        "DATA_MESH_TENANT_ID",
        "DATA_MESH_TENANT_ROLE",
    ];
    let saved: Vec<(&str, Option<String>)> =
        KEYS.iter().map(|k| (*k, std::env::var(k).ok())).collect();
    for k in KEYS {
        std::env::remove_var(k);
    }

    // Default is the sole sovereign seam — never a fabricating backend.
    let backend = BackendFactory::select(&config).await.expect("infallible");
    assert_eq!(backend.mode(), og_cli::ExecutionMode::OgLeader);

    // `OG_BACKEND=local` opts into honest local-tool execution.
    std::env::set_var("OG_BACKEND", "local");
    let local = BackendFactory::select(&config).await.expect("infallible");
    assert_eq!(local.mode(), og_cli::ExecutionMode::Local);
    std::env::remove_var("OG_BACKEND");

    // Fail-closed contract. Two knobs must be pinned or this is non-hermetic:
    //   1. OG_FALLBACK=off — force the STRICT og-leader seam. Otherwise a dev's
    //      ~/.og/config.toml `fallback_enabled = true` makes select() return the
    //      harbormaster-fallback backend, which *degrades to local* on a mesh
    //      miss (Ok(status:"failed")) instead of failing closed. Env wins over config.
    //   2. DATA_MESH_URL + DATA_MESH_TENANT_ID unset — the envelope-data-mesh
    //      facade can't bind, so the A2A dispatch errors.
    // Together: every workflow action propagates the error — never a fabricated
    // or local success (og-bug-report#31).
    std::env::set_var("OG_FALLBACK", "off");
    std::env::remove_var("DATA_MESH_URL");
    std::env::remove_var("DATA_MESH_TENANT_ID");
    let leader = BackendFactory::select(&config).await.expect("infallible");
    assert!(leader.test(true, false).await.is_err(), "test fails closed");
    assert!(
        leader
            .review("https://github.com/org/repo/pull/1")
            .await
            .is_err(),
        "review fails closed"
    );
    assert!(
        leader
            .merge("https://github.com/org/repo/pull/1", true)
            .await
            .is_err(),
        "merge fails closed"
    );
    assert!(
        leader
            .fix_conflicts("https://github.com/org/repo/pull/1")
            .await
            .is_err(),
        "fix-conflicts fails closed"
    );
    // A fleet/forward objective has no local action at all — it must fail closed
    // too (og-cli#48), never a fabricated local verdict.
    assert!(
        leader
            .forward_to_leader("test all open PRs across all repos")
            .await
            .is_err(),
        "forward-to-leader fails closed"
    );

    // ...and it STILL fails closed with the harbormaster fallback ENABLED: a fleet
    // objective has no local "test every PR in the org", so harbormaster bails
    // rather than fabricating a local verdict (og-cli#48). DATA_MESH_* stay unset,
    // so og-leader is unreachable and the fallback path is exercised.
    std::env::set_var("OG_FALLBACK", "1");
    let with_fb = BackendFactory::select(&config).await.expect("infallible");
    let forward_err = with_fb
        .forward_to_leader("test all open PRs across all repos")
        .await
        .expect_err("fleet forward must fail closed even with the fallback enabled");
    assert!(
        format!("{forward_err:#}")
            .to_lowercase()
            .contains("no local fallback"),
        "expected an honest no-fallback error, got: {forward_err:#}"
    );

    // Restore the caller's environment exactly (hermetic — #46).
    for (k, v) in saved {
        match v {
            Some(val) => std::env::set_var(k, val),
            None => std::env::remove_var(k),
        }
    }
}

#[tokio::test]
#[ignore = "requires a reachable og-leader A2A listener; run in-cluster or with --ignored"]
async fn test_dispatch_reaches_og_leader() {
    // Against a live og-leader this dispatches (async handoff). We assert only
    // that og-leader answered with a status — never a hardcoded verdict.
    let config = ProjectConfig::default();
    let backend = BackendFactory::select(&config)
        .await
        .expect("selection is infallible");

    let output = backend
        .test(true, false)
        .await
        .expect("og-leader reachable");
    assert!(!output.status.is_empty(), "og-leader returns a status");
}
