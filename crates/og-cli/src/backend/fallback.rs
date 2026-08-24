//! [`FallbackBackend`] — transparent harbormaster fallback wrapper
//! (code-governance#758, resolves #478).
//!
//! Wraps a **primary** backend (og-leader, over A2A on the data-mesh offering)
//! and a **fallback** backend (harbormaster, local `hm` subprocess). Each of the
//! five workflow methods tries the primary first; **only** when the primary fails
//! with a genuine mesh-unreachability error does it transparently fall back to
//! harbormaster. Every other error — auth rejections, validation errors, build
//! failures — **fails closed** (propagates), never silently falling back.
//!
//! This is the security-critical seam: [`FallbackBackend::is_mesh_unreachable`]
//! is deliberately conservative (deny-list of non-transport failures checked
//! first), so a fallback can never mask an authorization or validation failure.

use anyhow::Result;
use async_trait::async_trait;
use tracing::info;

use super::{
    Backend, BuildContext, BuildOutput, CreateRepoOutput, CreateRepoSpec, DesignOutput,
    ExecutionMode, FixConflictsOutput, ForwardOutput, MergeOutput, ReviewOutput, TestOutput,
};

/// A backend that dispatches to `primary`, falling back to `fallback` **only**
/// on mesh-unreachability. Non-generic (holds `Box<dyn Backend>`) so
/// [`Self::is_mesh_unreachable`] is callable without a turbofish.
pub struct FallbackBackend {
    primary: Box<dyn Backend>,
    fallback: Box<dyn Backend>,
}

impl FallbackBackend {
    /// Wrap `primary` with a `fallback` used only when the primary reports the
    /// mesh work-plane unreachable.
    pub fn new(primary: Box<dyn Backend>, fallback: Box<dyn Backend>) -> Self {
        Self { primary, fallback }
    }

    /// Decide whether `err` represents the sovereign work-plane being
    /// **unreachable** (⇒ fall back to harbormaster) versus any other failure
    /// (⇒ fail closed).
    ///
    /// Security-critical and intentionally conservative:
    /// 1. If the error chain names an auth / validation / build failure, return
    ///    `false` immediately — those must never be masked by a fallback, even
    ///    if the surrounding og-leader context mentions the data-mesh.
    /// 2. Otherwise return `true` only when a genuine transport/unreachability
    ///    signal is present.
    pub fn is_mesh_unreachable(err: &anyhow::Error) -> bool {
        // `{:#}` walks the full anyhow source chain so we inspect wrapped causes,
        // not just the top-level context.
        let msg = format!("{err:#}").to_lowercase();

        // (1) Fail-closed deny-list: never fall back on these, regardless of any
        // "data-mesh"/"mesh" wording the og-leader context adds.
        const FAIL_CLOSED: &[&str] = &[
            "auth",
            "unauthor",
            "unauthenticated",
            "forbidden",
            "denied",
            "permission",
            "invalid",
            "validation",
            "not approved",
            "build failed",
            "test failed",
            "compilation",
        ];
        if FAIL_CLOSED.iter().any(|p| msg.contains(p)) {
            return false;
        }

        // (2) Genuine mesh/transport unreachability opts into the fallback.
        const UNREACHABLE: &[&str] = &[
            "unreachable",
            "connection refused",
            "connection reset",
            "failed to connect",
            "cannot connect",
            "cannot reach",
            "no route to host",
            "timed out",
            "timeout",
            "dial",
            "work-plane",
            "data-mesh",
            "mesh unavailable",
            "mesh is unavailable",
        ];
        UNREACHABLE.iter().any(|p| msg.contains(p))
    }
}

/// Run a primary workflow call, transparently falling back to harbormaster on
/// mesh-unreachability and failing closed on everything else.
macro_rules! with_fallback {
    ($wf:literal, $primary:expr, $fallback:expr) => {
        match $primary.await {
            Ok(output) => Ok(output),
            Err(e) if FallbackBackend::is_mesh_unreachable(&e) => {
                info!(concat!(
                    "og-leader unreachable; falling back to harbormaster for ",
                    $wf
                ));
                $fallback.await
            }
            // Fail closed: auth/validation/build errors propagate — no fallback.
            Err(e) => Err(e),
        }
    };
}

#[async_trait]
impl Backend for FallbackBackend {
    fn mode(&self) -> ExecutionMode {
        // The fallback is transparent: present the primary (sovereign) mode.
        self.primary.mode()
    }

    async fn design(&self, prompt: Option<&str>) -> Result<DesignOutput> {
        with_fallback!(
            "design",
            self.primary.design(prompt),
            self.fallback.design(prompt)
        )
    }

    async fn build(&self, ctx: &BuildContext) -> Result<BuildOutput> {
        with_fallback!("build", self.primary.build(ctx), self.fallback.build(ctx))
    }

    async fn test(&self, unit: bool, integration: bool) -> Result<TestOutput> {
        with_fallback!(
            "test",
            self.primary.test(unit, integration),
            self.fallback.test(unit, integration)
        )
    }

    async fn review(&self, pr_url: &str) -> Result<ReviewOutput> {
        with_fallback!(
            "review",
            self.primary.review(pr_url),
            self.fallback.review(pr_url)
        )
    }

    async fn merge(&self, pr_url: &str, if_approved: bool) -> Result<MergeOutput> {
        with_fallback!(
            "merge",
            self.primary.merge(pr_url, if_approved),
            self.fallback.merge(pr_url, if_approved)
        )
    }

    async fn fix_conflicts(&self, pr_url: &str) -> Result<FixConflictsOutput> {
        with_fallback!(
            "fix-conflicts",
            self.primary.fix_conflicts(pr_url),
            self.fallback.fix_conflicts(pr_url)
        )
    }

    async fn forward_to_leader(&self, objective: &str) -> Result<ForwardOutput> {
        // Wraps the primary like the other verbs, but the fallback (harbormaster)
        // has NO local execution for a fleet objective — its `forward_to_leader`
        // bails. So even on a genuine mesh-unreachable miss this fails closed with
        // an honest "no local fallback" error, never a fabricated fleet verdict
        // (og-cli#48). This holds with OG_FALLBACK enabled.
        with_fallback!(
            "forward-to-leader",
            self.primary.forward_to_leader(objective),
            self.fallback.forward_to_leader(objective)
        )
    }

    async fn create_repo(&self, spec: &CreateRepoSpec) -> Result<CreateRepoOutput> {
        // Wraps the primary like the other verbs, but the fallback (harbormaster)
        // has NO local repo creation — its `create_repo` bails. So even on a
        // genuine mesh-unreachable miss this fails closed with an honest "no local
        // fallback" error, never a repo created out-of-band via a GitHub side
        // channel (code-governance#795). This holds with OG_FALLBACK enabled.
        with_fallback!(
            "create-repo",
            self.primary.create_repo(spec),
            self.fallback.create_repo(spec)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// How a [`MockBackend`] responds to every workflow call.
    #[derive(Clone, Copy)]
    enum Behavior {
        /// Succeed, tagging outputs with `tag` so we can tell primary vs fallback.
        Ok,
        /// Fail with a mesh-unreachability error (⇒ should trigger fallback).
        MeshErr,
        /// Fail with a non-mesh error (auth) (⇒ must fail closed, no fallback).
        AuthErr,
    }

    struct MockBackend {
        behavior: Behavior,
        tag: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl MockBackend {
        fn new(behavior: Behavior, tag: &'static str) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    behavior,
                    tag,
                    calls: calls.clone(),
                },
                calls,
            )
        }

        fn record(&self) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.behavior {
                Behavior::Ok => Ok(()),
                Behavior::MeshErr => Err(anyhow::anyhow!(
                    "og-leader unreachable: data-mesh connection refused"
                )),
                Behavior::AuthErr => Err(anyhow::anyhow!("authorization denied: token expired")),
            }
        }
    }

    #[async_trait]
    impl Backend for MockBackend {
        fn mode(&self) -> ExecutionMode {
            ExecutionMode::OgLeader
        }
        async fn design(&self, _prompt: Option<&str>) -> Result<DesignOutput> {
            self.record()?;
            Ok(DesignOutput {
                design: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn build(&self, _ctx: &BuildContext) -> Result<BuildOutput> {
            self.record()?;
            Ok(BuildOutput {
                log: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn test(&self, _unit: bool, _integration: bool) -> Result<TestOutput> {
            self.record()?;
            Ok(TestOutput {
                summary: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn review(&self, _pr_url: &str) -> Result<ReviewOutput> {
            self.record()?;
            Ok(ReviewOutput {
                status: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn merge(&self, _pr_url: &str, _if_approved: bool) -> Result<MergeOutput> {
            self.record()?;
            Ok(MergeOutput {
                message: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn fix_conflicts(&self, _pr_url: &str) -> Result<FixConflictsOutput> {
            self.record()?;
            Ok(FixConflictsOutput {
                message: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn forward_to_leader(&self, _objective: &str) -> Result<ForwardOutput> {
            self.record()?;
            Ok(ForwardOutput {
                message: self.tag.to_string(),
                ..Default::default()
            })
        }
        async fn create_repo(&self, _spec: &CreateRepoSpec) -> Result<CreateRepoOutput> {
            self.record()?;
            Ok(CreateRepoOutput {
                message: self.tag.to_string(),
                ..Default::default()
            })
        }
    }

    fn fb(
        primary: Behavior,
        fallback: Behavior,
    ) -> (FallbackBackend, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let (p, pc) = MockBackend::new(primary, "primary");
        let (f, fc) = MockBackend::new(fallback, "harbormaster");
        (FallbackBackend::new(Box::new(p), Box::new(f)), pc, fc)
    }

    // (1) Detection: mesh errors → true; non-mesh errors → false (fail closed).
    #[test]
    fn test_fallback_backend_mesh_detection() {
        for err in [
            anyhow::anyhow!("og-leader unreachable: data-mesh connection refused"),
            anyhow::anyhow!("failed to connect to mesh service"),
            anyhow::anyhow!("the A2A data-mesh work-plane is unavailable"),
        ] {
            assert!(
                FallbackBackend::is_mesh_unreachable(&err),
                "should detect mesh-unreachable: {err:#}"
            );
        }
    }

    // (2) Fail-closed detection: auth/validation/build errors → false.
    #[test]
    fn test_is_mesh_unreachable_fails_closed_for_non_transport() {
        for err in [
            anyhow::anyhow!("invalid PR URL"),
            anyhow::anyhow!("authorization denied"),
            anyhow::anyhow!("validation failed: missing field"),
            anyhow::anyhow!("build failed: compilation error"),
        ] {
            assert!(
                !FallbackBackend::is_mesh_unreachable(&err),
                "must fail closed (no fallback) for: {err:#}"
            );
        }
    }

    // (3) A non-mesh (auth) primary error must fail closed: fallback untouched.
    #[tokio::test]
    async fn test_fallback_non_mesh_error_fails_closed() {
        let (backend, pc, fc) = fb(Behavior::AuthErr, Behavior::Ok);
        let res = backend.test(true, false).await;
        assert!(res.is_err(), "auth error must propagate, not fall back");
        assert_eq!(pc.load(Ordering::SeqCst), 1, "primary attempted once");
        assert_eq!(fc.load(Ordering::SeqCst), 0, "fallback must NOT be invoked");
    }

    // (4) Primary success passes straight through; fallback untouched.
    #[tokio::test]
    async fn test_fallback_ok_passes_through() {
        let (backend, pc, fc) = fb(Behavior::Ok, Behavior::Ok);
        let out = backend.design(Some("x")).await.unwrap();
        assert_eq!(out.design, "primary", "primary output returned");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 0, "fallback must NOT be invoked");
    }

    // (5) mode() reports the primary (sovereign) mode.
    #[tokio::test]
    async fn test_fallback_mode_reports_primary() {
        let (backend, _pc, _fc) = fb(Behavior::Ok, Behavior::Ok);
        assert_eq!(backend.mode(), ExecutionMode::OgLeader);
    }

    // (6-10) One per workflow: mesh-unreachable primary → fallback is taken.
    #[tokio::test]
    async fn test_fallback_design_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend.design(Some("x")).await.unwrap();
        assert_eq!(out.design, "harbormaster", "fallback output returned");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1, "fallback invoked once");
    }

    #[tokio::test]
    async fn test_fallback_build_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend.build(&BuildContext::default()).await.unwrap();
        assert_eq!(out.log, "harbormaster");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_fallback_test_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend.test(true, false).await.unwrap();
        assert_eq!(out.summary, "harbormaster");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_fallback_review_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend.review("#1").await.unwrap();
        assert_eq!(out.status, "harbormaster");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_fallback_merge_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend.merge("#1", true).await.unwrap();
        assert_eq!(out.message, "harbormaster");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_fallback_fix_conflicts_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend.fix_conflicts("#1").await.unwrap();
        assert_eq!(out.message, "harbormaster");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_fallback_create_repo_on_mesh_unreachable() {
        let (backend, pc, fc) = fb(Behavior::MeshErr, Behavior::Ok);
        let out = backend
            .create_repo(&CreateRepoSpec {
                name: "acme-web".to_string(),
                private: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.message, "harbormaster");
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 1);
    }

    // Fail-closed parity: an auth error on fix-conflicts must NOT fall back.
    #[tokio::test]
    async fn test_fallback_fix_conflicts_auth_error_fails_closed() {
        let (backend, pc, fc) = fb(Behavior::AuthErr, Behavior::Ok);
        assert!(backend.fix_conflicts("#1").await.is_err());
        assert_eq!(pc.load(Ordering::SeqCst), 1);
        assert_eq!(fc.load(Ordering::SeqCst), 0, "fallback must NOT be invoked");
    }

    /// A fleet/forward objective fails closed **even with the fallback enabled**:
    /// the primary reports the mesh unreachable, so the wrapper reaches for the
    /// fallback — but the REAL harbormaster backend has no local fleet execution
    /// and bails. The net result is an honest "no local fallback" error, never a
    /// fabricated local verdict (og-cli#48). Uses the real `HarbormasterBackend`
    /// as the fallback so this exercises the true fail-closed path.
    #[cfg(feature = "harbormaster-fallback")]
    #[tokio::test]
    async fn test_fallback_forward_to_leader_fails_closed_even_when_enabled() {
        let (primary, pc) = MockBackend::new(Behavior::MeshErr, "primary");
        let backend = FallbackBackend::new(
            Box::new(primary),
            Box::new(crate::backend::HarbormasterBackend::new()),
        );
        let err = backend
            .forward_to_leader("test all open PRs across all repos")
            .await
            .expect_err("fleet forward must fail closed, never fabricate a local verdict");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback"),
            "expected an honest no-fallback error, got: {msg}"
        );
        assert_eq!(
            pc.load(Ordering::SeqCst),
            1,
            "primary attempted once before the (bailing) fallback"
        );
    }

    /// Repo creation fails closed **even with the fallback enabled**: the primary
    /// reports the mesh unreachable, the wrapper reaches for the fallback — but the
    /// REAL harbormaster backend has no local repo creation and bails. The net
    /// result is an honest "no local fallback" error, never a repo created
    /// out-of-band via a GitHub side channel (code-governance#795). Uses the real
    /// `HarbormasterBackend` so this exercises the true fail-closed path.
    #[cfg(feature = "harbormaster-fallback")]
    #[tokio::test]
    async fn test_fallback_create_repo_fails_closed_even_when_enabled() {
        let (primary, pc) = MockBackend::new(Behavior::MeshErr, "primary");
        let backend = FallbackBackend::new(
            Box::new(primary),
            Box::new(crate::backend::HarbormasterBackend::new()),
        );
        let err = backend
            .create_repo(&CreateRepoSpec {
                name: "acme-web".to_string(),
                private: true,
                ..Default::default()
            })
            .await
            .expect_err("create-repo must fail closed, never create via a local side channel");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback"),
            "expected an honest no-fallback error, got: {msg}"
        );
        assert_eq!(
            pc.load(Ordering::SeqCst),
            1,
            "primary attempted once before the (bailing) fallback"
        );
    }
}
