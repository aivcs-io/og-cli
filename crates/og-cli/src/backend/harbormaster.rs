//! Harbormaster fallback backend: the **local last resort** when the sovereign
//! og-leader seam (A2A over the data-mesh offering) is fully unreachable
//! (code-governance#758, resolves #478).
//!
//! og-cli's only *network* seam remains og-leader. Harbormaster is **not** a
//! remote backend — it is harbormaster's orchestrator embedded **in-process** as
//! a library (`harbormaster::run_tool(tool, task)`), running real local tools.
//! There is no external `hm` binary and no subprocess: og-cli ships as a single
//! self-contained binary. It is honest and non-sovereign: it never fabricates a
//! verdict, it reflects the real exit status of the tool it ran. Review scores
//! default to `0` (not gated) — a real gate verdict only ever comes from
//! og-leader.
//!
//! Per-workflow tool routing (code-governance#758 §Dispatch Flow):
//! `design → claude`, `build → propel`, `test → propel`, `review → claude`,
//! `merge → propel`.

use std::time::Instant;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tracing::info;

use crate::exec::CommandResult;

use super::{
    Backend, BuildContext, BuildOutput, CreateRepoOutput, CreateRepoSpec, DesignOutput,
    ExecutionMode, FixConflictsOutput, ForwardOutput, MergeOutput, ReviewOutput, TestOutput,
};

/// In-process harbormaster orchestrator wrapper used as og-cli's fallback
/// executor. Runs real local tools via the embedded `harbormaster::run_tool`
/// library call — no external `hm` binary.
pub struct HarbormasterBackend;

impl HarbormasterBackend {
    pub fn new() -> Self {
        Self
    }

    /// Run `tool` on `task` via harbormaster's embedded `run_tool` and adapt the
    /// captured output into og-cli's [`CommandResult`]. Fails (never fabricates)
    /// when the tool is unknown or cannot be launched — the honest answer is
    /// "no local fallback result". The error propagates through
    /// [`super::FallbackBackend`], which fails closed on it.
    async fn run_hm(tool: &str, task: &str) -> Result<CommandResult> {
        info!("harbormaster (in-process) run_tool: tool={tool} task={task:?}");
        let started = Instant::now();
        let outcome = harbormaster::run_tool(tool, task).await.with_context(|| {
            format!(
                "harbormaster in-process run_tool(tool={tool}) produced no result — \
                 og-leader is unreachable and the local orchestrator could not run the tool. \
                 Failing honestly rather than fabricating a verdict."
            )
        })?;
        Ok(CommandResult {
            success: outcome.success(),
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            duration_secs: started.elapsed().as_secs(),
        })
    }
}

/// Local fallback command for the `build`/`test` operations.
///
/// The `propel` tool runs its task as a shell command inside the repo's nix
/// devshell (`nix develop -c bash -lc <task>`), so the task MUST be a real,
/// shell-safe command — never a prose sentence. A description with shell
/// metacharacters (`(`, `)`, `=`) is a syntax error that failed every local
/// build/test fallback (og-bug-report#41). `og-crab run` is the sovereign local
/// check runner: it reads the repo's propel.toml and performs the real build +
/// tests, og-leader-independent.
const LOCAL_CHECK_CMD: &str = "og-crab run";

/// A leading note for `TestOutput::summary` when the caller asked for a suite
/// scope this backend cannot apply.
///
/// `og-crab run` exposes only `--config` and `--parallel`, so it always runs the
/// full set of checks propel.toml declares. Returning `""` when no scope was
/// requested keeps the common path's output byte-identical to before.
fn scope_note(unit: bool, integration: bool) -> String {
    let requested = match (unit, integration) {
        (true, true) => "--unit --integration",
        (true, false) => "--unit",
        (false, true) => "--integration",
        (false, false) => return String::new(),
    };
    format!(
        "note: {requested} not applied — the local harbormaster fallback runs \
         `{LOCAL_CHECK_CMD}`, which has no suite scoping, so the FULL propel.toml \
         check set ran.\n"
    )
}

/// The `TestOutput::summary` for a finished local check run.
///
/// Deliberately takes `unit`/`integration` rather than a pre-rendered note, so
/// that dropping the scope disclosure is a signature change this function's own
/// tests catch — not a line someone can quietly delete from `test()`.
fn test_summary(unit: bool, integration: bool, r: &CommandResult) -> String {
    format!(
        "{}{}\n{}",
        scope_note(unit, integration),
        r.stdout,
        r.stderr
    )
}

impl Default for HarbormasterBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Backend for HarbormasterBackend {
    fn mode(&self) -> ExecutionMode {
        // Harbormaster runs real local tools on this machine — honest and
        // non-sovereign, which is exactly the `Local` execution semantics.
        ExecutionMode::Local
    }

    async fn design(&self, prompt: Option<&str>) -> Result<DesignOutput> {
        let task = prompt.unwrap_or("design architecture and acceptance criteria");
        let result = Self::run_hm("claude", task).await?;
        Ok(DesignOutput {
            design: format!("{}\n{}", result.stdout, result.stderr),
            status: if result.success {
                "completed"
            } else {
                "failed"
            }
            .to_string(),
            ..Default::default()
        })
    }

    async fn build(&self, ctx: &BuildContext) -> Result<BuildOutput> {
        // The propel tool runs its task as a shell command in the repo's nix
        // devshell, so it must be a real, shell-safe command — never a prose
        // "build <target>" (og-bug-report#41). A target/service hint doesn't map
        // to a portable build command, so run the repo's configured checks.
        let _ = ctx;
        let result = Self::run_hm("propel", LOCAL_CHECK_CMD).await?;
        Ok(BuildOutput {
            status: if result.success { "success" } else { "failed" }.to_string(),
            log: format!("{}\n{}", result.stdout, result.stderr),
            duration_secs: result.duration_secs,
            ..Default::default()
        })
    }

    async fn test(&self, unit: bool, integration: bool) -> Result<TestOutput> {
        // Real, shell-safe command — NEVER prose. The old
        // `format!("run tests (unit={unit}, …)")` was `bash -lc`'d by the propel
        // tool, and the `(` was a shell syntax error that failed every local
        // test fallback (og-bug-report#41). og-crab run performs the real tests.
        //
        // Suite scoping is NOT silently dropped. Every other backend honours
        // `unit`/`integration` — gateway and og-leader forward them in the
        // dispatch payload, local maps them onto `run_cargo_test`. This backend
        // cannot: `og-crab run` takes only `--config`/`--parallel`, so it always
        // runs whatever propel.toml declares. Hardcoding `cargo test` here to
        // fake the distinction would be wrong twice over — it would bypass
        // propel.toml, and og-cli drives Go and TypeScript repos too.
        //
        // So the scope request is reported instead of discarded: `og test
        // --unit` that actually ran the full suite must not read as if only
        // unit tests ran. Lifting this needs suite scoping in og-crab itself.
        let result = Self::run_hm("propel", LOCAL_CHECK_CMD).await?;
        Ok(TestOutput {
            status: if result.success { "passed" } else { "failed" }.to_string(),
            summary: test_summary(unit, integration, &result),
            duration_secs: result.duration_secs,
            ..Default::default()
        })
    }

    async fn review(&self, pr_url: &str) -> Result<ReviewOutput> {
        // No local fallback. Reviewing a remote PR needs its diff + a sovereign gate
        // score from og-code-review-agent via og-leader. A local `claude` on just the PR
        // URL has no diff and produces no gate verdict, so FAIL CLOSED rather than report a
        // hollow "completed". (Building/testing the *local* repo still falls back — those
        // operate on the checkout in hand; a remote PR review does not.)
        bail!(
            "review has no local fallback: reviewing {pr_url} requires og-code-review-agent (and \
             a sovereign gate score) via og-leader, which is unreachable. Refusing to report a \
             hollow review."
        )
    }

    async fn merge(&self, pr_url: &str, _if_approved: bool) -> Result<MergeOutput> {
        // No local fallback. Merging a remote PR (with the sovereign gate) is the job of
        // og-pr-merge-agent via og-leader; a local tool cannot merge a GitHub/aivcs PR. So
        // FAIL CLOSED — never run a tool and map its exit code to a fabricated "merged".
        bail!(
            "merge has no local fallback: merging {pr_url} requires og-pr-merge-agent (with the \
             sovereign gate) via og-leader, which is unreachable. Refusing to fabricate a merge."
        )
    }

    async fn fix_conflicts(&self, pr_url: &str) -> Result<FixConflictsOutput> {
        // No local fallback. Resolving a remote PR's conflicts means clone → merge/resolve →
        // push — the job of og-fix-merge-conflicts-agent via og-leader. A local one-shot tool
        // (e.g. `claude` on a prompt) has no checkout and cannot push, so its exit code says
        // nothing about the PR's state. FAIL CLOSED rather than fabricate a "resolved" verdict.
        bail!(
            "fix-conflicts has no local fallback: resolving conflicts on {pr_url} requires the \
             sovereign og-fix-merge-conflicts-agent (clone → resolve → push) via og-leader, which \
             is unreachable. Refusing to fabricate a 'resolved' result."
        )
    }

    async fn forward_to_leader(&self, objective: &str) -> Result<ForwardOutput> {
        // No local fallback — even with OG_FALLBACK enabled. A fleet/free-form
        // objective ("test every open PR across the org") is decomposed by
        // og-leader + AOA into per-target og-* runs; there is no local equivalent.
        // A one-shot local `propel`/`claude` here would touch only the current
        // checkout and fabricate a verdict for work that never happened. FAIL
        // CLOSED, mirroring merge/review/fix_conflicts (#49), so the fallback can
        // never mask og-leader being unreachable as a bogus local result (og-cli#48).
        bail!(
            "og-leader unreachable; a fleet objective has no local fallback: {objective:?} must be \
             decomposed by og-leader + AOA into per-target og-* runs (e.g. an og-crab fan-out). \
             Refusing to run a local tool and fabricate a fleet verdict."
        )
    }

    async fn create_repo(&self, spec: &CreateRepoSpec) -> Result<CreateRepoOutput> {
        // No local fallback — even with OG_FALLBACK enabled. Provisioning a repo
        // is a sovereign action: og-leader → AOA → og-agent-forge creates it with
        // the org's policy, identity, and audit trail. A local `gh repo create`
        // side channel would bypass all of that and fabricate provenance, so FAIL
        // CLOSED rather than create a repo out-of-band (code-governance#795),
        // mirroring merge/review/fix_conflicts/forward_to_leader.
        bail!(
            "create-repo has no local fallback: provisioning repository {:?} requires the \
             sovereign og-agent-forge (with org policy + audit) via og-leader, which is \
             unreachable. Refusing to create a repo via a local/GitHub side channel.",
            spec.name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_harbormaster_mode_is_local() {
        assert_eq!(HarbormasterBackend::new().mode(), ExecutionMode::Local);
    }

    #[test]
    fn local_check_cmd_is_shell_safe() {
        // og-bug-report#41: the propel tool `bash -lc`s its task, so the
        // build/test fallback command must contain NO shell metacharacters —
        // the old prose "run tests (unit=…, integration=…)" broke on `(`.
        for ch in ['(', ')', '=', ';', '&', '|', '$', '`'] {
            assert!(
                !LOCAL_CHECK_CMD.contains(ch),
                "LOCAL_CHECK_CMD {LOCAL_CHECK_CMD:?} must be shell-safe; found {ch:?}"
            );
        }
    }

    #[test]
    fn scope_note_is_empty_when_no_scope_requested() {
        // The common path must stay byte-identical to before: no scope asked
        // for, no note prepended to the summary.
        assert_eq!(scope_note(false, false), "");
    }

    #[test]
    fn scope_note_reports_every_unhonoured_scope() {
        // `og test --unit` running the FULL suite and saying nothing is the
        // "green but wrong" shape: a verdict that reads narrower than what ran.
        for (unit, integration, expect) in [
            (true, false, "--unit"),
            (false, true, "--integration"),
            (true, true, "--unit --integration"),
        ] {
            let note = scope_note(unit, integration);
            assert!(
                note.contains(expect),
                "scope_note({unit},{integration}) must name the requested scope; got {note:?}"
            );
            assert!(
                note.contains("not applied") && note.contains("FULL"),
                "the note must say the scope was ignored AND that everything ran; got {note:?}"
            );
            assert!(
                note.ends_with('\n'),
                "the note is a prefix line and must not run into stdout; got {note:?}"
            );
        }
    }

    #[test]
    fn test_summary_carries_the_scope_note_and_the_output() {
        let r = CommandResult {
            success: false,
            stdout: "3 passed, 1 failed".to_string(),
            stderr: "boom".to_string(),
            duration_secs: 0,
        };
        // Scoped run: the disclosure leads, the real output still follows.
        let scoped = test_summary(true, false, &r);
        assert!(scoped.starts_with("note: --unit not applied"), "{scoped:?}");
        assert!(scoped.contains("3 passed, 1 failed") && scoped.contains("boom"));

        // Unscoped run: byte-identical to the pre-change format.
        assert_eq!(test_summary(false, false, &r), "3 passed, 1 failed\nboom");
    }

    #[test]
    fn scope_note_never_reaches_a_shell() {
        // Guards the mistake this replaced: an earlier attempt encoded suite
        // scoping INTO the propel task string. Anything built from `unit`/
        // `integration` must stay in the summary and out of the command, so the
        // og-bug-report#41 class cannot come back through a new door.
        for (u, i) in [(true, false), (false, true), (true, true)] {
            assert!(
                !LOCAL_CHECK_CMD.contains(&scope_note(u, i)),
                "scope text must never be spliced into LOCAL_CHECK_CMD"
            );
        }
        assert_eq!(
            LOCAL_CHECK_CMD, "og-crab run",
            "the task sent to propel must not vary with suite flags"
        );
    }

    /// The fallback runs its tool **in-process** via `harbormaster::run_tool`
    /// (no external `hm` subprocess). Point harbormaster's registry at a stub
    /// tool mapped to `echo` (always on PATH, always exits 0) and assert the
    /// captured output flows back through `run_hm` as a real `CommandResult`.
    #[tokio::test]
    async fn test_run_hm_runs_tool_in_process() {
        let dir = std::env::temp_dir().join(format!("og-hm-inproc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tools.toml"),
            "[tools.stub-echo]\nbinary = \"echo\"\nexec_args = [\"{prompt}\"]\n",
        )
        .unwrap();
        std::env::set_var("HARBORMASTER_HOME", &dir);

        let result = HarbormasterBackend::run_hm("stub-echo", "hello in-process")
            .await
            .expect("in-process run_tool should succeed for a stub echo tool");
        assert!(result.success, "stub echo tool should exit 0");
        assert!(
            result.stdout.contains("hello in-process"),
            "captured stdout should echo the task, got: {:?}",
            result.stdout
        );

        std::env::remove_var("HARBORMASTER_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unknown tool fails closed (no fabrication): `run_hm` returns an error
    /// rather than a fake success.
    #[tokio::test]
    async fn test_run_hm_unknown_tool_fails_closed() {
        let err = match HarbormasterBackend::run_hm("definitely-not-a-real-tool", "x").await {
            Ok(_) => panic!("unknown tool must error, never fabricate a result"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("run_tool") || msg.to_lowercase().contains("unknown tool"),
            "expected an honest failure, got: {msg}"
        );
    }

    /// fix-conflicts has NO local fallback: resolving a remote PR needs clone→resolve→push
    /// via og-fix-merge-conflicts-agent. It must fail closed, never fabricate "resolved"
    /// from a local tool's exit code (the bug where `og` said "resolved" while the PR's
    /// conflicts were untouched).
    #[tokio::test]
    async fn test_fix_conflicts_fails_closed_no_fabrication() {
        let err = HarbormasterBackend::new()
            .fix_conflicts("https://github.com/lornu-ai/x/pull/1")
            .await
            .expect_err("fix-conflicts must fail closed, never fabricate 'resolved'");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback") && msg.contains("og-fix-merge-conflicts-agent"),
            "expected an honest no-fallback error, got: {msg}"
        );
    }

    /// merge likewise has no local fallback — no fabricated "merged".
    #[tokio::test]
    async fn test_merge_fails_closed_no_fabrication() {
        let err = HarbormasterBackend::new()
            .merge("https://github.com/lornu-ai/x/pull/1", true)
            .await
            .expect_err("merge must fail closed, never fabricate 'merged'");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback") && msg.contains("og-pr-merge-agent"),
            "expected an honest no-fallback error, got: {msg}"
        );
    }

    /// review of a remote PR has no local fallback — no hollow "completed".
    #[tokio::test]
    async fn test_review_fails_closed_no_fabrication() {
        let err = HarbormasterBackend::new()
            .review("https://github.com/lornu-ai/x/pull/1")
            .await
            .expect_err("review must fail closed, never report a hollow 'completed'");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback") && msg.contains("og-code-review-agent"),
            "expected an honest no-fallback error, got: {msg}"
        );
    }

    /// A fleet/forward objective has NO local fallback: decomposing "test every PR
    /// in the org" into per-target runs is og-leader + AOA's job. The harbormaster
    /// backend must fail closed here, never run a local tool and fabricate a fleet
    /// verdict — the og-cli#48 masking bug.
    #[tokio::test]
    async fn test_forward_to_leader_fails_closed_no_fabrication() {
        let err = HarbormasterBackend::new()
            .forward_to_leader("test all open PRs across all repos")
            .await
            .expect_err("forward-to-leader must fail closed, never fabricate a fleet verdict");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback") && msg.contains("og-leader unreachable"),
            "expected an honest no-fallback error, got: {msg}"
        );
    }

    /// Repo creation has NO local fallback: provisioning a repo is og-agent-forge's
    /// job (with org policy + audit) via og-leader. The harbormaster backend must
    /// fail closed here, never create a repo via a local `gh` side channel
    /// (code-governance#795).
    #[tokio::test]
    async fn test_create_repo_fails_closed_no_side_channel() {
        let spec = CreateRepoSpec {
            name: "acme-web".to_string(),
            private: true,
            readme: true,
            license: Some("MIT".to_string()),
            ..Default::default()
        };
        let err = HarbormasterBackend::new()
            .create_repo(&spec)
            .await
            .expect_err("create-repo must fail closed, never create via a local side channel");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("no local fallback") && msg.contains("og-agent-forge"),
            "expected an honest no-fallback error, got: {msg}"
        );
    }
}
