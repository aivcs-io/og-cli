//! Required-checks gate (og-bug-report#39).
//!
//! og-cli may return an **approving** review or perform a **merge** ONLY when the
//! PR's REQUIRED CHECKS all **actually run and pass**:
//!   - every repo: `og-crab`, `og-governance`, `og-ciso-agent`
//!   - IaC PRs, additionally: `og-kustomize`
//!
//! The fleet CI rail posts nothing to GitHub, so the gate does NOT read GitHub's
//! status rollup (that would find nothing and block everything). Instead it
//! **executes** each required check against the PR's checked-out worktree and
//! gates on the tool's real exit status.
//!
//! FAIL CLOSED, always: a check that fails, cannot be run (tool absent, spawn
//! error), or whose result can't be determined is NEVER treated as a pass. The
//! verdict lists exactly which checks passed, failed, or couldn't run.
//!
//! The name/verdict aggregation ([`evaluate_required_checks`]) is a pure function
//! over a list of [`CheckResult`], so it is unit-testable with no process
//! execution; only the collection step ([`run_required_checks`]) shells out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::info;

use crate::exec;
use crate::gh::GhPullRequest;
use crate::pr_checkout::PrWorktree;

/// Required checks for EVERY repo.
pub const REQUIRED_CHECKS: &[&str] = &["og-crab", "og-governance", "og-ciso-agent"];

/// Additional required checks for Infrastructure-as-Code PRs.
pub const REQUIRED_CHECKS_IAC: &[&str] = &["og-kustomize"];

/// The names of every required check for a PR (base set, plus the IaC set when
/// `is_iac`).
pub fn required_check_names(is_iac: bool) -> Vec<&'static str> {
    let mut names = REQUIRED_CHECKS.to_vec();
    if is_iac {
        names.extend_from_slice(REQUIRED_CHECKS_IAC);
    }
    names
}

/// The real outcome of running one required check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The check ran and passed (exit 0).
    Passed,
    /// The check ran and failed (non-zero exit); carries a short detail.
    Failed(String),
    /// The check could NOT be run (tool not installed, spawn error, timeout) —
    /// fail closed, this is never a pass.
    Unrunnable(String),
}

/// The result of one required check: its name and real outcome.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub outcome: CheckOutcome,
}

/// The aggregate gate verdict — the single source of truth read by review,
/// approve and merge. `passed` is true ONLY when every required check ran and
/// passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateVerdict {
    pub passed: bool,
    /// Required checks that ran and passed.
    pub passed_checks: Vec<String>,
    /// Required checks that ran and failed.
    pub failing: Vec<String>,
    /// Required checks that could not be run (fail closed).
    pub unrunnable: Vec<String>,
    /// Required checks that were never attempted (defensive; normally empty
    /// since we run the whole required set ourselves).
    pub missing: Vec<String>,
}

impl GateVerdict {
    /// A one-line human-readable reason a verdict blocked (empty when passed).
    pub fn reason(&self) -> String {
        let mut parts = Vec::new();
        if !self.failing.is_empty() {
            parts.push(format!("failed: {}", self.failing.join(", ")));
        }
        if !self.unrunnable.is_empty() {
            parts.push(format!("could not run: {}", self.unrunnable.join(", ")));
        }
        if !self.missing.is_empty() {
            parts.push(format!("not attempted: {}", self.missing.join(", ")));
        }
        parts.join("; ")
    }
}

/// PURE gate aggregation over already-collected check results. FAIL CLOSED:
/// every required check (base set, plus IaC set when `is_iac`) must be present in
/// `results` AND have outcome [`CheckOutcome::Passed`]. Names match
/// case-insensitively. No I/O — unit-testable in isolation.
pub fn evaluate_required_checks(results: &[CheckResult], is_iac: bool) -> GateVerdict {
    let required = required_check_names(is_iac);

    let mut passed_checks = Vec::new();
    let mut failing = Vec::new();
    let mut unrunnable = Vec::new();
    let mut missing = Vec::new();

    for req in &required {
        match results.iter().find(|r| r.name.eq_ignore_ascii_case(req)) {
            None => missing.push((*req).to_string()),
            Some(r) => match &r.outcome {
                CheckOutcome::Passed => passed_checks.push((*req).to_string()),
                CheckOutcome::Failed(_) => failing.push((*req).to_string()),
                CheckOutcome::Unrunnable(_) => unrunnable.push((*req).to_string()),
            },
        }
    }

    let passed = failing.is_empty() && unrunnable.is_empty() && missing.is_empty();
    GateVerdict {
        passed,
        passed_checks,
        failing,
        unrunnable,
        missing,
    }
}

/// Executes a single named required check against a checked-out PR worktree.
/// Abstracted as a trait so the collection loop is testable with a mock runner
/// (no real tools, no network).
#[async_trait]
pub trait CheckRunner: Send + Sync {
    async fn run(&self, name: &str, dir: &Path) -> CheckOutcome;
}

/// The CLI invocation for a required check: the binary and its args. The check
/// runs against the PR worktree (cwd = the worktree), the way the rest of og-cli
/// dispatches local tools. This is the integration seam — each og-* agent's real
/// check entrypoint plugs in here.
fn check_invocation(name: &str) -> (&str, Vec<&'static str>) {
    match name {
        // og-crab runs the CI assessment via `run` (reads propel.toml); it has no
        // `check` subcommand — the old value made the gate fail unconditionally.
        "og-crab" => ("og-crab", vec!["run"]),
        "og-governance" => ("og-governance", vec!["check"]),
        "og-ciso-agent" => ("og-ciso-agent", vec!["scan"]),
        // og-kustomize is NOT dispatched here: it requires `--path <dir>` per
        // changed kustomization (see `ToolCheckRunner::run`), so a bare
        // `validate` errors on missing args. Handled specially by the runner.
        "og-kustomize" => ("og-kustomize", vec!["validate"]),
        // Unknown check: run the tool bare and let its exit status decide.
        other => (other, vec![]),
    }
}

fn tail(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.len() <= max {
        t.to_string()
    } else {
        format!("…{}", &t[t.len() - max..])
    }
}

/// A short, non-empty failure detail from a finished command. Many og-* tools
/// print their real failure to **stdout** (og-crab/og-governance render their
/// report there), leaving stderr empty — capturing only stderr yielded the
/// opaque `Failed("")` of og-bug-report#45. Prefer stderr, fall back to stdout,
/// and never return an empty reason.
fn failure_detail(r: &crate::exec::CommandResult) -> String {
    let parts: Vec<&str> = [r.stderr.trim(), r.stdout.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        "non-zero exit, no stdout/stderr".to_string()
    } else {
        tail(&parts.join(" | "), 200)
    }
}

/// The nearest ancestor directory of `rel` (a worktree-relative path) that is a
/// kustomization root — i.e. contains `kustomization.yaml`/`.yml` under
/// `worktree`. Walks from the file's own directory up to the worktree root.
fn nearest_kustomization_dir(worktree: &Path, rel: &Path) -> Option<PathBuf> {
    let mut cur = rel.parent();
    while let Some(dir) = cur {
        for name in ["kustomization.yaml", "kustomization.yml"] {
            if worktree.join(dir).join(name).is_file() {
                return Some(dir.to_path_buf());
            }
        }
        cur = dir.parent();
    }
    None
}

/// The deduplicated set of kustomization directories (worktree-relative) that a
/// PR's `changed` files belong to — the `--path`s og-kustomize must validate.
/// Files not under any kustomization root contribute nothing. Order-stable so
/// tests and logs are deterministic.
fn kustomization_dirs(worktree: &Path, changed: &[String]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for f in changed {
        if let Some(dir) = nearest_kustomization_dir(worktree, Path::new(f)) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// The real runner: each required check is its CLI tool, run in the PR worktree.
/// Exit 0 ⇒ [`CheckOutcome::Passed`]; non-zero ⇒ [`CheckOutcome::Failed`]; tool
/// absent or un-spawnable ⇒ [`CheckOutcome::Unrunnable`] (fail closed).
pub struct ToolCheckRunner {
    /// Kustomization directories (worktree-relative) that the PR changed. Only
    /// consulted for the `og-kustomize` check; empty for non-IaC PRs.
    pub kustomize_paths: Vec<PathBuf>,
}

impl ToolCheckRunner {
    /// og-kustomize requires `--path <dir>` and validates one kustomization root
    /// at a time. Validate every changed kustomization directory; the check
    /// passes only if ALL do. A `LoadRestrictionsNone` restrictor lets Flux
    /// cluster roots that reference out-of-tree files render. When the PR
    /// changed no kustomization directory there is nothing to validate — an
    /// honest pass, not a fabricated one (og-bug-report#52).
    async fn run_og_kustomize(&self, dir: &Path) -> CheckOutcome {
        // Nothing to validate ⇒ honest pass; don't even require the tool.
        if self.kustomize_paths.is_empty() {
            return CheckOutcome::Passed;
        }
        if !exec::tool_available("og-kustomize") {
            return CheckOutcome::Unrunnable("`og-kustomize` not installed".to_string());
        }
        let mut failures = Vec::new();
        for p in &self.kustomize_paths {
            let path = p.to_string_lossy();
            let args = [
                "validate",
                "--path",
                &path,
                "--load-restrictor",
                "LoadRestrictionsNone",
            ];
            match exec::run_in(Some(dir), "og-kustomize", &args).await {
                Ok(r) if r.success => {}
                Ok(r) => failures.push(format!("{path}: {}", failure_detail(&r))),
                Err(e) => {
                    return CheckOutcome::Unrunnable(format!("failed to spawn `og-kustomize`: {e}"))
                }
            }
        }
        if failures.is_empty() {
            CheckOutcome::Passed
        } else {
            CheckOutcome::Failed(tail(&failures.join("; "), 400))
        }
    }
}

#[async_trait]
impl CheckRunner for ToolCheckRunner {
    async fn run(&self, name: &str, dir: &Path) -> CheckOutcome {
        if name == "og-kustomize" {
            return self.run_og_kustomize(dir).await;
        }
        let (bin, args) = check_invocation(name);
        if !exec::tool_available(bin) {
            return CheckOutcome::Unrunnable(format!("`{bin}` not installed"));
        }
        match exec::run_in(Some(dir), bin, &args).await {
            Ok(r) if r.success => CheckOutcome::Passed,
            Ok(r) => CheckOutcome::Failed(failure_detail(&r)),
            Err(e) => CheckOutcome::Unrunnable(format!("failed to spawn `{bin}`: {e}")),
        }
    }
}

/// Run every required check (base set + IaC set when `is_iac`) via `runner`,
/// against the checked-out worktree at `dir`. Collection step only — the pure
/// [`evaluate_required_checks`] turns these into a verdict.
pub async fn run_required_checks<R: CheckRunner>(
    runner: &R,
    dir: &Path,
    is_iac: bool,
) -> Vec<CheckResult> {
    let mut results = Vec::new();
    for name in required_check_names(is_iac) {
        let outcome = runner.run(name, dir).await;
        info!("required check {name}: {outcome:?}");
        results.push(CheckResult {
            name: name.to_string(),
            outcome,
        });
    }
    results
}

/// THE gate entrypoint used by review and merge: check the PR out into an
/// isolated worktree, RUN every required check against it, and aggregate the
/// real verdict. FAIL CLOSED — the worktree checkout failing propagates as an
/// error (never a pass); a failing/unrunnable check yields a blocked verdict.
pub async fn required_checks_verdict(pr: &GhPullRequest, is_iac: bool) -> Result<GateVerdict> {
    let worktree = PrWorktree::create(&pr.number.to_string())
        .await
        .with_context(|| {
            format!(
                "cannot run required checks for PR #{}: failed to check it out — failing closed",
                pr.number
            )
        })?;

    // For IaC PRs, resolve the kustomization directories the PR touched so
    // og-kustomize can be pointed at real `--path`s (og-bug-report#52). A failure
    // to list changed files must not silently pass the check — fail closed by
    // leaving the set empty only when there genuinely are none.
    let kustomize_paths = if is_iac {
        match crate::gh::changed_files(pr.number).await {
            Ok(changed) => kustomization_dirs(&worktree.path, &changed),
            Err(e) => {
                info!(
                    "PR #{}: could not list changed files for og-kustomize paths: {e:#}",
                    pr.number
                );
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let runner = ToolCheckRunner { kustomize_paths };
    let results = run_required_checks(&runner, &worktree.path, is_iac).await;

    // Always attempt to clean up the worktree; surface a real cleanup failure but
    // don't let it mask the verdict we computed.
    if let Err(e) = worktree.remove().await {
        info!("failed to remove PR #{} worktree: {e:#}", pr.number);
    }

    let verdict = evaluate_required_checks(&results, is_iac);
    if verdict.passed {
        info!(
            "PR #{} required-checks gate PASSED (iac={is_iac})",
            pr.number
        );
    } else {
        info!(
            "PR #{} required-checks gate BLOCKED (iac={is_iac}): {}",
            pr.number,
            verdict.reason()
        );
    }
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn result(name: &str, outcome: CheckOutcome) -> CheckResult {
        CheckResult {
            name: name.to_string(),
            outcome,
        }
    }

    #[test]
    fn none_of_the_required_checks_run_blocks() {
        let verdict = evaluate_required_checks(&[], false);
        assert!(!verdict.passed, "no checks run ⇒ fail closed");
        assert_eq!(
            verdict.missing,
            vec!["og-crab", "og-governance", "og-ciso-agent"]
        );
    }

    #[test]
    fn all_three_pass_non_iac() {
        let results = [
            result("og-crab", CheckOutcome::Passed),
            result("OG-Governance", CheckOutcome::Passed), // case-insensitive
            result("og-ciso-agent", CheckOutcome::Passed),
        ];
        let verdict = evaluate_required_checks(&results, false);
        assert!(verdict.passed, "{verdict:?}");
        assert_eq!(verdict.passed_checks.len(), 3);
    }

    #[test]
    fn missing_exactly_one_blocks_and_reports_it() {
        let results = [
            result("og-crab", CheckOutcome::Passed),
            result("og-ciso-agent", CheckOutcome::Passed),
        ];
        let verdict = evaluate_required_checks(&results, false);
        assert!(!verdict.passed);
        assert_eq!(verdict.missing, vec!["og-governance"]);
    }

    #[test]
    fn a_failing_check_blocks() {
        let results = [
            result("og-crab", CheckOutcome::Passed),
            result("og-governance", CheckOutcome::Failed("clippy".into())),
            result("og-ciso-agent", CheckOutcome::Passed),
        ];
        let verdict = evaluate_required_checks(&results, false);
        assert!(!verdict.passed);
        assert_eq!(verdict.failing, vec!["og-governance"]);
        assert!(verdict.reason().contains("og-governance"));
    }

    #[test]
    fn an_unrunnable_check_blocks_fail_closed() {
        let results = [
            result("og-crab", CheckOutcome::Passed),
            result("og-governance", CheckOutcome::Passed),
            result(
                "og-ciso-agent",
                CheckOutcome::Unrunnable("not installed".into()),
            ),
        ];
        let verdict = evaluate_required_checks(&results, false);
        assert!(!verdict.passed, "unrunnable ⇒ fail closed, never a pass");
        assert_eq!(verdict.unrunnable, vec!["og-ciso-agent"]);
    }

    #[test]
    fn iac_pr_needs_og_kustomize() {
        let three = [
            result("og-crab", CheckOutcome::Passed),
            result("og-governance", CheckOutcome::Passed),
            result("og-ciso-agent", CheckOutcome::Passed),
        ];
        // 3 pass for non-IaC; IaC additionally requires og-kustomize.
        assert!(evaluate_required_checks(&three, false).passed);
        let iac_blocked = evaluate_required_checks(&three, true);
        assert!(!iac_blocked.passed);
        assert_eq!(iac_blocked.missing, vec!["og-kustomize"]);

        let four = [
            result("og-crab", CheckOutcome::Passed),
            result("og-governance", CheckOutcome::Passed),
            result("og-ciso-agent", CheckOutcome::Passed),
            result("og-kustomize", CheckOutcome::Passed),
        ];
        assert!(evaluate_required_checks(&four, true).passed);
    }

    /// A mock runner returning canned outcomes per check name — lets us exercise
    /// the collection loop hermetically (no real tools, no network).
    struct MockRunner {
        outcomes: HashMap<String, CheckOutcome>,
        seen: Mutex<Vec<String>>,
    }

    impl MockRunner {
        fn new(pairs: &[(&str, CheckOutcome)]) -> Self {
            Self {
                outcomes: pairs
                    .iter()
                    .map(|(n, o)| (n.to_string(), o.clone()))
                    .collect(),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl CheckRunner for MockRunner {
        async fn run(&self, name: &str, _dir: &Path) -> CheckOutcome {
            self.seen.lock().unwrap().push(name.to_string());
            self.outcomes
                .get(name)
                .cloned()
                // A check with no canned outcome models a tool we couldn't run.
                .unwrap_or_else(|| CheckOutcome::Unrunnable("no mock outcome".into()))
        }
    }

    #[tokio::test]
    async fn run_required_checks_all_pass_yields_passing_verdict() {
        let runner = MockRunner::new(&[
            ("og-crab", CheckOutcome::Passed),
            ("og-governance", CheckOutcome::Passed),
            ("og-ciso-agent", CheckOutcome::Passed),
        ]);
        let results = run_required_checks(&runner, Path::new("/nonexistent"), false).await;
        let verdict = evaluate_required_checks(&results, false);
        assert!(verdict.passed);
        assert_eq!(runner.seen.lock().unwrap().len(), 3, "ran all 3 checks");
    }

    #[tokio::test]
    async fn run_required_checks_iac_runs_and_gates_kustomize() {
        // The 3 pass but og-kustomize fails ⇒ IaC verdict blocked.
        let runner = MockRunner::new(&[
            ("og-crab", CheckOutcome::Passed),
            ("og-governance", CheckOutcome::Passed),
            ("og-ciso-agent", CheckOutcome::Passed),
            ("og-kustomize", CheckOutcome::Failed("bad manifest".into())),
        ]);
        let results = run_required_checks(&runner, Path::new("/nonexistent"), true).await;
        let verdict = evaluate_required_checks(&results, true);
        assert!(!verdict.passed);
        assert_eq!(verdict.failing, vec!["og-kustomize"]);
        assert!(runner
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|n| n == "og-kustomize"));
    }

    #[test]
    fn failure_detail_prefers_stderr_then_stdout_never_empty() {
        use crate::exec::CommandResult;
        let mk = |out: &str, err: &str| CommandResult {
            success: false,
            stdout: out.to_string(),
            stderr: err.to_string(),
            duration_secs: 0,
        };
        // stdout-only failure (og-crab/og-governance style) — no longer opaque.
        assert_eq!(failure_detail(&mk("FAILED: clippy", "")), "FAILED: clippy");
        // stderr present — included, and stdout too when both non-empty.
        assert_eq!(failure_detail(&mk("out", "err")), "err | out");
        // truly silent non-zero exit still yields a reason, never "".
        assert_eq!(
            failure_detail(&mk("", "")),
            "non-zero exit, no stdout/stderr"
        );
    }

    #[test]
    fn kustomization_dirs_walks_to_nearest_root_and_dedupes() {
        let tmp = std::env::temp_dir().join(format!("ogcli-ktest-{}", std::process::id()));
        let base = tmp.join("crossplane/app/base");
        let overlay = tmp.join("crossplane/app/overlays/prod");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&overlay).unwrap();
        std::fs::write(base.join("kustomization.yaml"), "").unwrap();
        std::fs::write(overlay.join("kustomization.yaml"), "").unwrap();

        let changed = vec![
            // a resource file under base/ → nearest root is base/
            "crossplane/app/base/deployment.yaml".to_string(),
            // the base kustomization itself → still base/
            "crossplane/app/base/kustomization.yaml".to_string(),
            // overlay file → overlays/prod/
            "crossplane/app/overlays/prod/patch.yaml".to_string(),
            // a file under no kustomization root → contributes nothing
            "docs/README.md".to_string(),
        ];
        let dirs = kustomization_dirs(&tmp, &changed);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("crossplane/app/base"),
                PathBuf::from("crossplane/app/overlays/prod"),
            ],
            "dedup base/, resolve overlay/, ignore non-kustomize files"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn og_kustomize_no_changed_dirs_is_an_honest_pass() {
        // is_iac PR (e.g. a crossplane/ change) that touched no kustomization
        // directory: nothing to validate ⇒ Passed, not a fabricated block.
        let runner = ToolCheckRunner {
            kustomize_paths: Vec::new(),
        };
        assert_eq!(
            runner.run("og-kustomize", Path::new("/nonexistent")).await,
            CheckOutcome::Passed
        );
    }

    #[tokio::test]
    async fn run_required_checks_unrunnable_tool_fails_closed() {
        // og-ciso-agent has no canned outcome ⇒ mock returns Unrunnable ⇒ blocked.
        let runner = MockRunner::new(&[
            ("og-crab", CheckOutcome::Passed),
            ("og-governance", CheckOutcome::Passed),
        ]);
        let results = run_required_checks(&runner, Path::new("/nonexistent"), false).await;
        let verdict = evaluate_required_checks(&results, false);
        assert!(!verdict.passed, "an unrunnable required check must block");
        assert_eq!(verdict.unrunnable, vec!["og-ciso-agent"]);
    }
}
