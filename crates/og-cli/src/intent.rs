//! Natural-language interaction surface — Phase 1.
//!
//! Implements FR_OG_CLI_NATURAL_LANGUAGE_INTERACTIONS (code-governance#613): let a
//! user describe an objective in plain English and have `og` map it onto the
//! **existing** command surface. This is *translation, not a second workflow
//! engine* — every resolved step is one of the canonical `og` commands, side
//! effects are previewed as a dry run first, and ambiguous/unsafe requests get a
//! clarifying question rather than a guess.
//!
//! The parser here is **deterministic** — a bounded keyword/pattern mapper over
//! the known `og` verbs. No model call, no network, no new service (a model-backed
//! helper, if ever added, is a Phase-3 bounded integration, not this layer).
//!
//! Privacy: the machine-readable [`IntentTrace`] carries the *normalized* intent
//! and the command plan, **never the raw prompt text** — raw input stays local.

use crate::session::{parse_session_command, SessionCommand};
use serde::Serialize;

/// A canonical step in a resolved plan. Each maps 1:1 to an existing `og` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum PlanStep {
    Design {
        prompt: String,
    },
    Build,
    Test {
        integration: bool,
        pr: Option<String>,
    },
    Review {
        pr: Option<String>,
    },
    Gate {
        pr: Option<String>,
    },
    Merge {
        pr: Option<String>,
    },
    /// Resolve a PR's merge conflicts via og-fix-merge-conflicts-agent.
    ///
    /// SEAM: other og-* agent verbs are added as sibling variants exactly like
    /// this one (a variant here, a keyword branch in [`parse_intent`], a
    /// [`PlanStep::command_line`] render, and a `Backend` dispatch method):
    ///   Rebase → og-pull-request-rebase, FixChecks → og-fix-failing-checks-agent,
    ///   Diagnose → og-diagnostic, Cleanup → og-cleanup, RedTeam → og-red-teaming.
    FixConflicts {
        pr: Option<String>,
    },
    /// Create a repository via the sovereign flow — og-leader → AOA →
    /// og-agent-forge (code-governance#795). Mutating: provisions a new repo, so
    /// it is confirmation-gated. `name` empty ⇒ the parser asks which name to use.
    CreateRepo {
        name: String,
        private: bool,
        readme: bool,
        license: Option<String>,
        default_branch: Option<String>,
        description: Option<String>,
        org: Option<String>,
    },
    Status,
    AccountShow,
    AccountList,
    AccountSwitch {
        account_id: String,
    },
    AccountLogout {
        account_id: Option<String>,
    },
    OrgList,
    OrgUse {
        org: String,
    },
    /// No local fast-path matched — forward the free-form objective to og-leader,
    /// which (with AOA) plans and selects the required og-* agents over A2A. og-cli
    /// does not choose the agent or hard-wire the action (code-governance#769).
    ForwardToLeader {
        objective: String,
    },
    /// Laptop session verb (`login`, `exit`, `help`, …). Local-only: never
    /// forwarded to og-leader and never requires the data-mesh.
    Session(SessionCommand),
}

impl PlanStep {
    /// Human-readable AivcsOps plan label. This is deliberately not a
    /// re-runnable `og <verb>` command: the CLI has one entry point and the
    /// orchestrator owns execution.
    pub fn command_line(&self) -> String {
        match self {
            PlanStep::Design { prompt } => format!("intent: design ({prompt:?})"),
            PlanStep::Build => "intent: build".to_string(),
            PlanStep::Test { integration, pr } => {
                let mut cmd = "intent: test".to_string();
                if *integration {
                    cmd.push_str(" --integration");
                }
                if let Some(p) = pr {
                    cmd.push_str(&format!(" --pr {p}"));
                }
                cmd
            }
            PlanStep::Review { pr } => match pr {
                Some(p) => format!("intent: review ({p})"),
                None => "intent: review".to_string(),
            },
            PlanStep::Gate { pr } => match pr {
                Some(p) => format!("intent: gate ({p})"),
                None => "intent: gate".to_string(),
            },
            PlanStep::Merge { pr } => match pr {
                Some(p) => format!("intent: merge ({p})"),
                None => "intent: merge".to_string(),
            },
            PlanStep::FixConflicts { pr } => match pr {
                Some(p) => format!("intent: fix-conflicts ({p})"),
                None => "intent: fix-conflicts".to_string(),
            },
            PlanStep::CreateRepo {
                name,
                private,
                readme,
                license,
                default_branch,
                description,
                org,
            } => {
                let mut cmd = format!("intent: create-repo ({name})");
                cmd.push_str(if *private { " --private" } else { " --public" });
                if *readme {
                    cmd.push_str(" --readme");
                }
                if let Some(l) = license {
                    cmd.push_str(&format!(" --license {l}"));
                }
                if let Some(b) = default_branch {
                    cmd.push_str(&format!(" --default-branch {b}"));
                }
                if let Some(d) = description {
                    cmd.push_str(&format!(" --description {d:?}"));
                }
                if let Some(o) = org {
                    cmd.push_str(&format!(" --org {o}"));
                }
                cmd
            }
            PlanStep::Status => "intent: status".to_string(),
            PlanStep::AccountShow => "intent: account-show".to_string(),
            PlanStep::AccountList => "intent: account-list".to_string(),
            PlanStep::AccountSwitch { account_id } => {
                format!("intent: account-switch ({account_id})")
            }
            PlanStep::AccountLogout { account_id } => match account_id {
                Some(id) => format!("intent: account-logout ({id})"),
                None => "intent: account-logout".to_string(),
            },
            PlanStep::OrgList => "intent: org-list".to_string(),
            PlanStep::OrgUse { org } => format!("intent: org-use ({org})"),
            PlanStep::ForwardToLeader { objective } => {
                format!("→ og-leader: {objective}")
            }
            PlanStep::Session(cmd) => format!("session: {}", cmd.label()),
        }
    }

    /// True when the step mutates a repository/PR — requires confirmation before
    /// it may run (per the FR's safety-gate requirement).
    pub fn mutates(&self) -> bool {
        // Merge/FixConflicts mutate the PR directly; a forwarded objective may drive
        // mutating work on og-leader's side — all three are confirmation-gated.
        matches!(
            self,
            PlanStep::Merge { .. }
                | PlanStep::FixConflicts { .. }
                | PlanStep::ForwardToLeader { .. }
                | PlanStep::CreateRepo { .. }
        )
    }

    /// Steps that need a specific PR to be actionable.
    fn missing_pr(&self) -> Option<&'static str> {
        match self {
            PlanStep::Review { pr: None } => Some("review"),
            PlanStep::Gate { pr: None } => Some("gate"),
            PlanStep::Merge { pr: None } => Some("merge"),
            PlanStep::FixConflicts { pr: None } => Some("fix conflicts on"),
            _ => None,
        }
    }
}

/// A resolved plan for one natural-language objective.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IntentPlan {
    /// A normalized restatement of what we understood (safe to show/log).
    pub normalized: String,
    /// The canonical command plan, in order.
    pub steps: Vec<PlanStep>,
    /// Follow-up questions when the request is ambiguous, incomplete, or unsafe to
    /// run as written. Non-empty ⇒ do not execute; ask first.
    pub clarifications: Vec<String>,
    /// True when any step mutates a repo/PR — execution requires confirmation.
    pub requires_confirmation: bool,
}

impl IntentPlan {
    /// A plan is runnable only when we understood it and have nothing to ask.
    pub fn is_actionable(&self) -> bool {
        self.clarifications.is_empty() && !self.steps.is_empty()
    }

    /// The machine-readable trace for downstream tooling — **no raw prompt text**.
    pub fn trace(&self) -> IntentTrace<'_> {
        IntentTrace {
            normalized_intent: &self.normalized,
            plan: &self.steps,
            requires_confirmation: self.requires_confirmation,
            clarifications: &self.clarifications,
        }
    }
}

/// Structured, machine-readable intent trace. Deliberately excludes the raw user
/// text (privacy: prompt content is not exported in the clear).
#[derive(Debug, Serialize)]
pub struct IntentTrace<'a> {
    pub normalized_intent: &'a str,
    pub plan: &'a [PlanStep],
    pub requires_confirmation: bool,
    pub clarifications: &'a [String],
}

/// Fleet/multi-target scope markers. Their presence means the objective spans
/// *many* PRs/repos ("test all open PRs across every repo", the og-cli#48 case) —
/// work only og-leader + AOA can decompose into per-target og-* runs. og-cli has
/// no local "test every PR in the org", so such an objective must be forwarded
/// whole, never collapsed onto a single local verb (`og test`, `og build`, …).
const FLEET_MARKERS: &[&str] = &[
    "all prs",
    "all open pr",      // "all open prs" / "all open pull requests"
    "all pull request", // "all pull requests" (sing./plural)
    "every pr",
    "each pr",
    "all repos",
    "all repositor", // "all repositories"
    "across repos",
    "across all repos",
    "across all repositor",
    "across the repos",
    "across every repo",
    "every repo",
    "each repo",
    "org-wide",
    "org wide",
    "orgwide",
    "whole org",
    "entire org",
    "fleet",
];

/// True when `norm` (already lowercased) names a fleet/multi-target scope — see
/// [`FLEET_MARKERS`]. A single-target reference like "test pr 42" contains none of
/// these and stays a local action.
fn is_fleet_scope(norm: &str) -> bool {
    FLEET_MARKERS.iter().any(|m| norm.contains(m))
}

/// Deterministically parse a plain-language objective into a plan.
pub fn parse_intent(text: &str) -> IntentPlan {
    let norm = text.trim().to_lowercase();

    // Session verbs stay on the laptop: `login` / `exit` / `help` (slash optional)
    // must never become ForwardToLeader and must never require DATA_MESH_*.
    if let Some(cmd) = parse_session_command(text) {
        let steps = vec![PlanStep::Session(cmd)];
        return IntentPlan {
            normalized: describe(&steps),
            steps,
            clarifications: Vec::new(),
            requires_confirmation: false,
        };
    }

    // Scope-aware forwarding (og-cli#48). A fleet/multi-target objective MUST reach
    // og-leader (→ AOA decomposes it into per-PR/per-repo og-* runs, e.g. an
    // og-crab fan-out). Detected BEFORE the local keyword fast-paths so
    // "test all open PRs across all repos" never collapses to a single local
    // `og test` on the current checkout. A fleet objective has NO local action, so
    // we forward the full text verbatim and emit nothing else.
    if is_fleet_scope(&norm) {
        let steps = vec![PlanStep::ForwardToLeader {
            objective: text.trim().to_string(),
        }];
        let requires_confirmation = steps.iter().any(PlanStep::mutates);
        let normalized = describe(&steps);
        return IntentPlan {
            normalized,
            steps,
            clarifications: Vec::new(),
            requires_confirmation,
        };
    }

    // Repo creation → sovereign og-agent-forge via og-leader (code-governance#795).
    // A dedicated branch (not additive): "create a private repo named X with a
    // README and MIT license" is one whole objective, so we emit a single
    // CreateRepo step and nothing else. Requires a create/new/make verb AND a
    // repo/repository noun so ordinary "review"/"build" asks never trip it.
    // Checked BEFORE the generic keyword fast-paths for the same reason.
    if let Some(step) = parse_create_repo(text, &norm) {
        let mut clarifications = Vec::new();
        if let PlanStep::CreateRepo { name, .. } = &step {
            if name.is_empty() {
                clarifications.push(
                    "What should the new repository be named? Add `named <name>`.".to_string(),
                );
            }
        }
        let steps = vec![step];
        // Provisioning a repo mutates → always confirmation-gated.
        let requires_confirmation = true;
        let normalized = describe(&steps);
        return IntentPlan {
            normalized,
            steps,
            clarifications,
            requires_confirmation,
        };
    }

    let pr = extract_pr_reference(text);
    let has = |kws: &[&str]| kws.iter().any(|k| norm.contains(k));

    let mut steps: Vec<PlanStep> = Vec::new();
    let mut clarifications: Vec<String> = Vec::new();

    // Sandlot is an AivcsOps execution substrate, not an og-cli backend or
    // user-facing automation command.  Any objective that names Sandlot must
    // cross the sovereign og-leader seam so orchestrator/OxidizedGraph
    // can select the required agents and runner.  In particular, do not let
    // "test on sandlot" fall through to the local Test fast path.
    if norm.contains("sandlot")
        && ["test", "build", "check", "verify", "run"]
            .iter()
            .any(|word| norm.contains(word))
    {
        let steps = vec![PlanStep::ForwardToLeader {
            objective: text.trim().to_string(),
        }];
        return IntentPlan {
            normalized: describe(&steps),
            steps,
            clarifications,
            requires_confirmation: false,
        };
    }

    // Account management
    if has(&["show account", "which account", "current account"]) {
        steps.push(PlanStep::AccountShow);
    }
    if has(&["list accounts", "all accounts"]) {
        steps.push(PlanStep::AccountList);
    }
    if has(&["switch account", "switch to account"]) {
        if let Some(account_id) = extract_identifier(text, &["to"]) {
            steps.push(PlanStep::AccountSwitch { account_id });
        } else {
            clarifications.push("Which account should I switch to?".to_string());
        }
    }
    if has(&["logout", "log out"]) && has(&["account"]) {
        steps.push(PlanStep::AccountLogout {
            account_id: extract_identifier(text, &["from"]),
        });
    }

    // Org management
    if has(&["list orgs", "available orgs", "show orgs"]) {
        steps.push(PlanStep::OrgList);
    }
    if has(&["switch org", "use org", "switch to org", "use the org"]) {
        if let Some(org) = extract_identifier(text, &["to"]) {
            steps.push(PlanStep::OrgUse { org });
        } else {
            clarifications.push("Which organization should I switch to?".to_string());
        }
    }

    if has(&["design", "architect", "spec out", "draft a spec"]) {
        steps.push(PlanStep::Design {
            prompt: text.trim().to_string(),
        });
    }
    if has(&["build", "compile", "image"]) {
        steps.push(PlanStep::Build);
    }
    if has(&["test", "run the checks", "verify"]) {
        steps.push(PlanStep::Test {
            integration: has(&["integration", "e2e", "end-to-end"]),
            pr: pr.clone(),
        });
    }
    if has(&["review", "code review", "crr", "why", "failed"]) {
        steps.push(PlanStep::Review { pr: pr.clone() });
    }
    // "after checks pass" / "gate" / "safe path" ⇒ run the PR gate first.
    if has(&["gate", "checks pass", "safe path", "before merg"]) {
        steps.push(PlanStep::Gate { pr: pr.clone() });
    }
    // Bare "merge" is a substring of "merge conflict(s)" — "fix merge conflicts
    // on <url>" must NOT be read as "please merge <url>" (code-governance#639).
    // Checked word-by-word (not `norm.contains("merge conflict")`) so a typo'd
    // "conflicts" (the exact wording in #639's transcript: "merge conflucts")
    // is still caught — the word right after "merge" only has to *look like*
    // "conflict...", not match it exactly.
    let wants_merge = has_bare_merge_word(&norm) || has(&["land it", "ship it", "open a pr"]);
    if wants_merge {
        steps.push(PlanStep::Merge { pr: pr.clone() });
    }
    // Fix merge conflicts → og-fix-merge-conflicts-agent (via og-leader). This is
    // deliberately distinct from a *merge* request: "merge conflict(s)" phrasing
    // must resolve conflicts, never merge (code-governance#639) — `has_bare_merge_word`
    // above already declines to read it as a merge. "fmc" is the fleet abbreviation.
    //
    // SEAM: the other og-* agent verbs slot in right here, each a short keyword
    // branch pushing its own PlanStep (rebase → og-pull-request-rebase,
    // fix-failing-checks → og-fix-failing-checks-agent, diagnose → og-diagnostic,
    // cleanup → og-cleanup, red-team → og-red-teaming). See PlanStep::FixConflicts.
    if has(&[
        "fix merge conflicts",
        "resolve merge conflicts",
        "resolve conflicts",
        "merge conflicts",
        "fix conflicts",
        "fmc",
    ]) {
        steps.push(PlanStep::FixConflicts { pr: pr.clone() });
    }
    if steps.is_empty() && has(&["status", "health", "what's happening", "what is happening"]) {
        steps.push(PlanStep::Status);
    }

    if steps.is_empty() {
        // No local fast-path matched. Do NOT hard-error: og-cli is a thin NL front —
        // forward the free-form objective to og-leader, which (with AOA) plans and
        // selects the required og-* agents over A2A (code-governance#769). The CLI is
        // not the authority on what's allowed; new fleet capabilities need no CLI change.
        // (Recognized fast-paths like `fix-conflicts` from #45 still match earlier and
        // are not forwarded.)
        steps.push(PlanStep::ForwardToLeader {
            objective: text.trim().to_string(),
        });
    }
    for step in &steps {
        if let Some(verb) = step.missing_pr() {
            clarifications.push(format!(
                "Which PR should I {verb}? Add a PR number (#123) or URL."
            ));
        }
    }
    dedup(&mut clarifications);

    let requires_confirmation = steps.iter().any(PlanStep::mutates);
    let normalized = describe(&steps);

    IntentPlan {
        normalized,
        steps,
        clarifications,
        requires_confirmation,
    }
}

/// A human restatement of the plan (safe to log).
fn describe(steps: &[PlanStep]) -> String {
    if steps.is_empty() {
        return "no recognized action".to_string();
    }
    let parts: Vec<String> = steps
        .iter()
        .map(|s| match s {
            PlanStep::Design { .. } => "design".to_string(),
            PlanStep::Build => "build".to_string(),
            PlanStep::Test {
                integration: true,
                pr: None,
            } => "test (incl. integration)".to_string(),
            PlanStep::Test {
                integration: false,
                pr: None,
            } => "test".to_string(),
            PlanStep::Test {
                integration,
                pr: Some(p),
            } => format!(
                "test PR {p}{}",
                if *integration {
                    " (incl. integration)"
                } else {
                    ""
                }
            ),
            PlanStep::Review { .. } => "review".to_string(),
            PlanStep::Gate { .. } => "gate".to_string(),
            PlanStep::Merge { .. } => "merge".to_string(),
            PlanStep::FixConflicts { .. } => "fix merge conflicts".to_string(),
            PlanStep::CreateRepo { name, private, .. } => {
                let vis = if *private { "private" } else { "public" };
                if name.is_empty() {
                    format!("create {vis} repo")
                } else {
                    format!("create {vis} repo {name}")
                }
            }
            PlanStep::Status => "status".to_string(),
            PlanStep::AccountShow => "show account".to_string(),
            PlanStep::AccountList => "list accounts".to_string(),
            PlanStep::AccountSwitch { account_id } => format!("switch account to {account_id}"),
            PlanStep::AccountLogout { .. } => "logout account".to_string(),
            PlanStep::OrgList => "list orgs".to_string(),
            PlanStep::OrgUse { org } => format!("use org {org}"),
            PlanStep::ForwardToLeader { objective } => {
                format!("forward to og-leader: {objective}")
            }
            PlanStep::Session(cmd) => format!("session {}", cmd.label()),
        })
        .collect();
    format!("run: {}", parts.join(" → "))
}

/// True if `norm` contains the word "merge" *not* immediately followed by a
/// word that looks like "conflict(s)" — i.e. an actual merge request, not the
/// noun phrase "merge conflict(s)" (code-governance#639). Matched on whole
/// words (not a bare substring) so "emerge"/"submerge" don't false-trigger
/// either. The next-word check is a `conf`-prefix match rather than an exact
/// "conflict" comparison, so a typo like "conflucts" is still caught.
fn has_bare_merge_word(norm: &str) -> bool {
    let words: Vec<&str> = norm.split_whitespace().collect();
    words.iter().enumerate().any(|(i, w)| {
        w.trim_matches(|c: char| !c.is_alphanumeric()) == "merge"
            && !words.get(i + 1).is_some_and(|next| {
                next.trim_start_matches(|c: char| !c.is_alphanumeric())
                    .starts_with("conf")
            })
    })
}

/// Deterministically detect a repo-creation objective and build its [`PlanStep`].
///
/// Requires a create/new/make verb AND a repo/repository noun; visibility
/// defaults to private unless "public" is named; README and an SPDX license are
/// picked up when mentioned. `name` may be empty — the caller then asks which
/// name to use (mirrors the missing-PR clarification path). This is a bounded
/// keyword mapper: no model call, no network (see the module docs).
fn parse_create_repo(text: &str, norm: &str) -> Option<PlanStep> {
    let has = |kws: &[&str]| kws.iter().any(|k| norm.contains(k));
    let is_create = has(&["create", "make", "new ", "spin up", "provision", "set up"]);
    // Whole-word match: a bare `contains("repo")` also fires on "report",
    // "reporter", … so "create a status report" would wrongly trip repo creation.
    let is_repo = norm.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        matches!(w, "repo" | "repos" | "repository" | "repositories")
    });
    if !(is_create && is_repo) {
        return None;
    }
    Some(PlanStep::CreateRepo {
        // Default private unless the user explicitly says "public".
        private: !has(&["public"]),
        readme: has(&["readme", "read me", "read-me"]),
        license: extract_license(norm),
        default_branch: None,
        description: None,
        org: None,
        name: extract_repo_name(text).unwrap_or_default(),
    })
}

/// Extract a repository name following "named"/"called"/"name" — e.g.
/// "create a repo named acme-web" → "acme-web". Returns the next token, stripped
/// of surrounding quotes/punctuation (keeps `-`, `_`, `.`, `/` — legal in names).
fn extract_repo_name(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let key = w.to_lowercase();
        let key = key.trim_end_matches(':');
        if matches!(key, "named" | "called" | "name") {
            if let Some(next) = words.get(i + 1) {
                let cleaned = next.trim_matches(|c: char| {
                    !c.is_alphanumeric() && !matches!(c, '-' | '_' | '.' | '/')
                });
                if !cleaned.is_empty() {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

/// Extract an SPDX license id when named as a standalone token (e.g. "MIT
/// license" → "MIT", "Apache-2.0" → "Apache-2.0"). Word-boundary matched (not a
/// bare substring) so "commit"/"submit"/"limit" never read as "MIT". Returns the
/// canonical SPDX casing.
fn extract_license(norm: &str) -> Option<String> {
    const SPDX: &[(&str, &str)] = &[
        ("apache-2.0", "Apache-2.0"),
        ("mit", "MIT"),
        ("bsd-3-clause", "BSD-3-Clause"),
        ("bsd-2-clause", "BSD-2-Clause"),
        ("gpl-3.0", "GPL-3.0"),
        ("gpl-2.0", "GPL-2.0"),
        ("lgpl-3.0", "LGPL-3.0"),
        ("mpl-2.0", "MPL-2.0"),
        ("agpl-3.0", "AGPL-3.0"),
        ("unlicense", "Unlicense"),
    ];
    let tokens: Vec<&str> = norm
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '.')))
        .filter(|s| !s.is_empty())
        .collect();
    for (id, canonical) in SPDX {
        if tokens.iter().any(|t| t == id) {
            return Some(canonical.to_string());
        }
    }
    None
}

/// Extract a PR reference if present, accepting every form a person naturally
/// types (og-bug-report#44) — the CLI must not require a `#` prefix:
///
///   - a full URL `.../pull/123[/subpage]` → kept as `owner/repo/pull/123`
///   - a self-marked token: `#123`, `pr#123`, `pr123`, `PR-123`
///   - a `PR` / `pull request` keyword + number: `PR 1539`, `pull request 1539`
///   - a bare number where the verb implies a PR: `test 1539`, `review 1539`
///
/// Non-URL forms normalize to `#123`, matching what `og … --pr` already emits.
pub fn extract_pr_reference(text: &str) -> Option<String> {
    // 1. Full PR URL — keep the owner/repo/pull/123 prefix (drop trailing subpages).
    for tok in text.split_whitespace() {
        let trimmed = tok.trim_end_matches(['.', ',', ')']);
        if let Some(idx) = trimmed.find("/pull/") {
            let after = &trimmed[idx + "/pull/".len()..];
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                let end = idx + "/pull/".len() + digits.len();
                return Some(trimmed[..end].to_string());
            }
        }
    }

    // Tokenize once, stripping surrounding punctuation, for the number-form passes.
    let toks: Vec<&str> = text
        .split_whitespace()
        .map(|t| t.trim_matches(['.', ',', ')', '(', ':', ';', '!', '?']))
        .filter(|t| !t.is_empty())
        .collect();

    // 2. A token that carries its own PR marker: `#123`, `pr#123`, `PR123`, `pr-123`.
    for tok in &toks {
        if let Some(digits) = pr_number_in_token(tok) {
            return Some(format!("#{digits}"));
        }
    }

    // 3. A `pr` / `pull` / `request` keyword followed by a separate number token:
    //    "PR 1539", "pull request 1539", "pull 1539".
    for (i, tok) in toks.iter().enumerate() {
        let low = tok.to_ascii_lowercase();
        if matches!(low.as_str(), "pr" | "prs" | "pull" | "request") {
            if let Some(next) = toks.get(i + 1) {
                if is_all_digits(next) {
                    return Some(format!("#{next}"));
                }
            }
        }
    }

    // 4. Last resort: a lone all-digit token is a PR reference in this CLI's
    //    command context ("test 1539", "review 1539").
    for tok in &toks {
        if is_all_digits(tok) {
            return Some(format!("#{tok}"));
        }
    }

    None
}

/// A PR number carried inside a single token: `#123`, `pr123`, `PR#123`, `pr-123`.
/// Requires an explicit `#` or `pr` marker — a bare number is handled separately
/// as a last resort — so ordinary words/versions (`gp3`, `us-east-2`) never match.
fn pr_number_in_token(tok: &str) -> Option<String> {
    let low = tok.to_ascii_lowercase();
    let rest = match low.strip_prefix('#') {
        Some(r) => r.to_string(),
        None => low
            .strip_prefix("pr")?
            .trim_start_matches(['#', '-'])
            .to_string(),
    };
    is_all_digits(&rest).then_some(rest)
}

fn is_all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Extract an identifier (account ID or org name) that follows a keyword like "to" or "from".
/// E.g., "switch to alice" → "alice", "use org team-x" → "team-x".
fn extract_identifier(text: &str, after_keywords: &[&str]) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if after_keywords.contains(&word.to_lowercase().as_str().trim_end_matches('s')) {
            // Return the next word if it exists
            if i + 1 < words.len() {
                return Some(words[i + 1].to_string());
            }
        }
    }
    // Fallback: try to extract a hyphenated identifier (e.g., "team-x")
    for word in words {
        if word.contains('-') && !word.contains("://") {
            return Some(word.to_string());
        }
    }
    None
}

fn dedup(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_test_maps_to_two_steps() {
        let p = parse_intent("build and test this repo");
        assert_eq!(
            p.steps,
            vec![
                PlanStep::Build,
                PlanStep::Test {
                    integration: false,
                    pr: None
                }
            ]
        );
        assert!(p.is_actionable());
        assert!(!p.requires_confirmation);
    }

    #[test]
    fn integration_keyword_flips_test_mode() {
        let p = parse_intent("run the integration tests");
        assert_eq!(
            p.steps,
            vec![PlanStep::Test {
                integration: true,
                pr: None
            }]
        );
    }

    #[test]
    fn test_this_pr_resolves_both_the_verb_and_the_pr() {
        // The ask this closes (code-governance#639 follow-up): "all an og-cli
        // user needs to do is say test this pr X".
        let p = parse_intent("test this pr #42");
        assert_eq!(
            p.steps,
            vec![PlanStep::Test {
                integration: false,
                pr: Some("#42".to_string())
            }]
        );
        assert!(p.is_actionable());
        assert_eq!(p.steps[0].command_line(), "intent: test --pr #42");
    }

    #[test]
    fn pr_reference_accepts_every_natural_form() {
        // og-bug-report#44: a person may type the PR with or without `#`, spelled
        // out or abbreviated, or as a bare number — all must resolve identically.
        for input in [
            "test pull request #1539",
            "test PR 1539",
            "test infra-code pull request 1539",
            "test pr#1539",
            "test PR-1539",
            "review 1539",
            "test 1539",
            "#1539",
        ] {
            assert_eq!(
                extract_pr_reference(input).as_deref(),
                Some("#1539"),
                "input {input:?} should resolve to #1539"
            );
        }
        // URL form keeps the owner/repo/pull/N prefix.
        assert_eq!(
            extract_pr_reference("merge https://github.com/acme/og-cli/pull/1539/files")
                .as_deref(),
            Some("https://github.com/acme/og-cli/pull/1539")
        );
    }

    #[test]
    fn pr_reference_ignores_non_pr_words_and_versions() {
        // Ordinary words, version-ish tokens, and PR-less prose must NOT be
        // mistaken for a PR number (guards the bare-number last resort).
        assert_eq!(extract_pr_reference("run the checks"), None);
        assert_eq!(extract_pr_reference("build with gp3 in us-east-2"), None);
        assert_eq!(extract_pr_reference("deploy og-leader to aivcs-core"), None);
    }

    #[test]
    fn review_with_bare_pr_number_no_longer_prompts() {
        // Previously "review PR 1539" dropped the number and prompted for one.
        let p = parse_intent("review PR 1539");
        assert_eq!(
            p.steps,
            vec![PlanStep::Review {
                pr: Some("#1539".to_string())
            }]
        );
        assert!(p.is_actionable());
    }

    #[test]
    fn test_with_no_pr_named_is_still_actionable() {
        // A bare "test" must keep working exactly as before — Test is the one
        // PR-carrying step that doesn't *require* a PR (unlike Review/Gate/Merge).
        let p = parse_intent("run the checks");
        assert_eq!(
            p.steps,
            vec![PlanStep::Test {
                integration: false,
                pr: None
            }]
        );
        assert!(p.is_actionable());
    }

    #[test]
    fn fleet_objective_forwards_to_leader_not_local_test() {
        // og-cli#48: the exact reported phrase. A fleet-scoped "test all open PRs
        // across all repos" must reach og-leader (→ AOA fans out per-PR og-crab
        // runs), NOT collapse to a single local `og test` on the current checkout.
        let obj = "test all open pull requests in all repositories in the acme org";
        let p = parse_intent(obj);
        assert_eq!(
            p.steps,
            vec![PlanStep::ForwardToLeader {
                objective: obj.to_string()
            }],
            "fleet scope must forward whole, got: {:?}",
            p.steps
        );
        assert!(
            !p.steps.iter().any(|s| matches!(s, PlanStep::Test { .. })),
            "must NOT emit a local Test for a fleet objective"
        );
        assert!(p.is_actionable());
        assert!(
            p.requires_confirmation,
            "forwarded work is confirmation-gated"
        );
    }

    #[test]
    fn fleet_scope_variants_all_forward() {
        for obj in [
            "test all open PRs across all repos",
            "review every repo in the org",
            "run the checks org-wide",
            "build each repo in the fleet",
            "test all pull requests across the repos",
            "gate all prs in the whole org",
        ] {
            let p = parse_intent(obj);
            assert!(
                matches!(p.steps.as_slice(), [PlanStep::ForwardToLeader { .. }]),
                "fleet phrase {obj:?} must forward to og-leader, got: {:?}",
                p.steps
            );
        }
    }

    #[test]
    fn non_fleet_test_pr_still_maps_to_local_test() {
        // A single-target "test pr 42" carries no fleet marker → stays a local Test.
        let p = parse_intent("test pr #42");
        assert_eq!(
            p.steps,
            vec![PlanStep::Test {
                integration: false,
                pr: Some("#42".to_string())
            }],
            "single-PR test must stay local, got: {:?}",
            p.steps
        );
    }

    #[test]
    fn unknown_request_forwards_to_og_leader() {
        // No local fast-path → og-cli forwards the free-form objective to og-leader
        // instead of hard-erroring (code-governance#769). Actionable, no clarification.
        let p = parse_intent("make me a sandwich");
        assert_eq!(
            p.steps,
            vec![PlanStep::ForwardToLeader {
                objective: "make me a sandwich".to_string()
            }]
        );
        assert!(p.is_actionable());
        assert!(p.clarifications.is_empty());
        assert!(
            p.requires_confirmation,
            "forwarded work is confirmation-gated"
        );
    }

    #[test]
    fn login_and_exit_stay_local_session_never_forward() {
        for text in [
            "login",
            "/login",
            "LOGIN",
            "log in",
            "exit",
            "/exit",
            "quit",
            "help",
            "/help",
            "logout",
            "whoami",
        ] {
            let p = parse_intent(text);
            assert!(
                matches!(p.steps.as_slice(), [PlanStep::Session(_)]),
                "{text:?} must be a local session verb, got {:?}",
                p.steps
            );
            assert!(p.is_actionable());
            assert!(
                !p.requires_confirmation,
                "{text:?} must not look like a mutating fleet plan"
            );
        }
    }

    #[test]
    fn create_private_repo_with_readme_and_license_maps_to_create_repo() {
        // The FR example (code-governance#795): the whole objective becomes ONE
        // sovereign CreateRepo step — never additive noise from "review"/"build".
        let p = parse_intent("create a private repo named acme-web with a README and MIT license");
        assert_eq!(
            p.steps,
            vec![PlanStep::CreateRepo {
                name: "acme-web".to_string(),
                private: true,
                readme: true,
                license: Some("MIT".to_string()),
                default_branch: None,
                description: None,
                org: None,
            }]
        );
        assert!(p.is_actionable());
        assert!(p.requires_confirmation, "provisioning a repo is mutating");
        assert_eq!(
            p.steps[0].command_line(),
            "intent: create-repo (acme-web) --private --readme --license MIT"
        );
    }

    #[test]
    fn create_public_repo_flips_visibility() {
        let p = parse_intent("create a public repo named widgets");
        assert_eq!(
            p.steps,
            vec![PlanStep::CreateRepo {
                name: "widgets".to_string(),
                private: false,
                readme: false,
                license: None,
                default_branch: None,
                description: None,
                org: None,
            }]
        );
        assert!(p.is_actionable());
        assert_eq!(
            p.steps[0].command_line(),
            "intent: create-repo (widgets) --public"
        );
    }

    #[test]
    fn create_repo_without_a_name_asks_for_one() {
        // No `named <x>` ⇒ still parses to CreateRepo, but not actionable — it asks
        // which name to use (mirrors bare "merge" asking for a PR).
        let p = parse_intent("create a new private repository with a readme");
        assert!(
            matches!(p.steps.as_slice(), [PlanStep::CreateRepo { name, .. }] if name.is_empty()),
            "expected a nameless CreateRepo, got: {:?}",
            p.steps
        );
        assert!(!p.is_actionable(), "missing name ⇒ must clarify");
        assert!(p.clarifications.iter().any(|c| c.contains("named")));
    }

    #[test]
    fn license_word_boundary_does_not_false_match_mit() {
        // "commit" must NOT be read as an "MIT" license; with no license word the
        // parsed step carries license: None.
        let p = parse_intent("create a repo named foo then commit the readme");
        assert_eq!(
            p.steps,
            vec![PlanStep::CreateRepo {
                name: "foo".to_string(),
                private: true,
                readme: true,
                license: None,
                default_branch: None,
                description: None,
                org: None,
            }]
        );
    }

    #[test]
    fn create_repo_is_not_tripped_by_ordinary_verbs() {
        // "review #42" has no repo noun → stays a Review, never a CreateRepo.
        let p = parse_intent("review #42");
        assert!(!p
            .steps
            .iter()
            .any(|s| matches!(s, PlanStep::CreateRepo { .. })));

        // "repo" must match as a whole word — "report" (which *contains* "repo")
        // plus a create verb must NOT trip repo creation.
        for text in [
            "create a status report",
            "make a bug report",
            "generate a report",
        ] {
            let p = parse_intent(text);
            assert!(
                !p.steps
                    .iter()
                    .any(|s| matches!(s, PlanStep::CreateRepo { .. })),
                "{text:?} should not be CreateRepo"
            );
        }
    }

    #[test]
    fn merge_requires_confirmation_and_a_pr() {
        let p = parse_intent("merge this");
        assert_eq!(p.steps, vec![PlanStep::Merge { pr: None }]);
        assert!(p.requires_confirmation);
        assert!(!p.is_actionable(), "missing PR ⇒ must clarify");
        assert!(p.clarifications.iter().any(|c| c.contains("merge")));
    }

    #[test]
    fn merge_with_pr_url_is_actionable() {
        let p = parse_intent("merge https://github.com/acme/og-cli/pull/13 now");
        assert_eq!(
            p.steps,
            vec![PlanStep::Merge {
                pr: Some("https://github.com/acme/og-cli/pull/13".to_string())
            }]
        );
        assert!(p.is_actionable());
        assert!(p.requires_confirmation);
    }

    #[test]
    fn pr_url_with_suffix_is_normalized() {
        let p = parse_intent("merge https://github.com/acme/og-cli/pull/13/conflicts");
        assert_eq!(
            p.steps,
            vec![PlanStep::Merge {
                pr: Some("https://github.com/acme/og-cli/pull/13".to_string())
            }]
        );
        assert!(p.is_actionable());
    }

    #[test]
    fn pr_number_form_is_extracted() {
        let p = parse_intent("review #42");
        assert_eq!(
            p.steps,
            vec![PlanStep::Review {
                pr: Some("#42".to_string())
            }]
        );
        assert!(p.is_actionable());
    }

    #[test]
    fn gate_before_merge_orders_gate_then_merge() {
        let p = parse_intent("run the gate then merge #7");
        assert_eq!(
            p.steps,
            vec![
                PlanStep::Gate {
                    pr: Some("#7".to_string())
                },
                PlanStep::Merge {
                    pr: Some("#7".to_string())
                },
            ]
        );
        assert!(p.requires_confirmation);
    }

    #[test]
    fn fix_merge_conflicts_does_not_false_trigger_a_merge() {
        // Regression (code-governance#639) — exact wording (incl. typo) from the
        // bug report's transcript: "merge" here means "merge conflicts", not
        // "please merge this PR". It must NOT become a Merge; with no other
        // recognized verb it is forwarded to og-leader (code-governance#769),
        // which selects og-fix-merge-conflicts-agent.
        let p = parse_intent(
            "fix merge conflucts on https://github.com/acme/og-cli/pull/23/conflicts",
        );
        assert!(
            !p.steps.contains(&PlanStep::Merge {
                pr: Some("https://github.com/acme/og-cli/pull/23".to_string())
            }),
            "must not false-trigger Merge from 'merge conflucts' phrasing: {:?}",
            p.steps
        );
        assert!(
            matches!(p.steps.as_slice(), [PlanStep::ForwardToLeader { .. }]),
            "merge-conflict phrasing forwards to og-leader, not a local action: {:?}",
            p.steps
        );
    }

    #[test]
    fn unrecognized_objective_forwards_to_leader() {
        // An objective with no local fast-path (code-governance#769) forwards verbatim.
        // "fix merge conflicts" is a recognized fast-path (#45) → FixConflicts, not forwarded.
        let obj = "rotate the payments database credentials and notify the on-call rota";
        let p = parse_intent(obj);
        assert_eq!(
            p.steps,
            vec![PlanStep::ForwardToLeader {
                objective: obj.to_string()
            }]
        );
        assert!(p.is_actionable());
        assert!(p.requires_confirmation);
    }

    #[test]
    fn merge_conflict_phrasing_does_not_suppress_a_real_merge_request() {
        // The exclusion is scoped to "merge conflict" specifically — an actual
        // merge request elsewhere in the same sentence still works.
        let p = parse_intent("no conflicts here, please merge #9");
        assert_eq!(
            p.steps,
            vec![PlanStep::Merge {
                pr: Some("#9".to_string())
            }]
        );
    }

    #[test]
    fn pr_url_with_a_sub_page_suffix_is_extracted() {
        // Regression (code-governance#639): a `.../pull/<n>/conflicts` URL (the
        // exact shape GitHub's own conflicts page uses) must still resolve to
        // the PR, trimmed back to the bare `.../pull/<n>` form.
        let p = parse_intent("review https://github.com/acme/og-cli/pull/23/conflicts");
        assert_eq!(
            p.steps,
            vec![PlanStep::Review {
                pr: Some("https://github.com/acme/og-cli/pull/23".to_string())
            }]
        );
        assert!(p.is_actionable());
    }

    #[test]
    fn status_only_when_no_other_verb() {
        assert_eq!(
            parse_intent("what's the status?").steps,
            vec![PlanStep::Status]
        );
        // "build" wins — status is a fallback, not additive noise.
        assert_eq!(
            parse_intent("build and show status").steps,
            vec![PlanStep::Build]
        );
    }

    #[test]
    fn trace_excludes_raw_text() {
        let p = parse_intent("merge https://github.com/acme/og-cli/pull/13");
        let json = serde_json::to_string(&p.trace()).unwrap();
        assert!(json.contains("normalized_intent"));
        assert!(json.contains("merge"));
        // the raw sentence must not appear in the exported trace
        assert!(!json.contains("merge https://github.com/acme/og-cli/pull/13"));
    }

    #[test]
    fn command_line_rendering_is_stable() {
        assert_eq!(PlanStep::Build.command_line(), "intent: build");
        assert_eq!(
            PlanStep::Gate {
                pr: Some("#7".into())
            }
            .command_line(),
            "intent: gate (#7)"
        );
        assert_eq!(
            PlanStep::Merge {
                pr: Some("#7".into())
            }
            .command_line(),
            "intent: merge (#7)"
        );
    }

    #[test]
    fn fix_merge_conflicts_with_pr_url_routes_to_fix_conflicts() {
        // The reported gap (og shell): "fix merge conflicts on <PR>" must map to
        // FixConflicts (→ og-fix-merge-conflicts-agent), not the unmapped
        // clarification and NOT a merge.
        let p =
            parse_intent("fix merge conflicts on https://github.com/acme/bookkeeping/pull/39");
        assert_eq!(
            p.steps,
            vec![PlanStep::FixConflicts {
                pr: Some("https://github.com/acme/bookkeeping/pull/39".to_string())
            }]
        );
        assert!(
            !p.steps.iter().any(|s| matches!(s, PlanStep::Merge { .. })),
            "must never resolve conflicts by merging"
        );
        assert!(p.is_actionable());
        assert!(
            p.requires_confirmation,
            "writes to the PR branch ⇒ mutating"
        );
        assert_eq!(
            p.steps[0].command_line(),
            "intent: fix-conflicts (https://github.com/acme/bookkeeping/pull/39)"
        );
    }

    #[test]
    fn fix_conflicts_recognizes_all_phrasings_with_pr() {
        for phrase in [
            "fix merge conflicts on #39",
            "resolve merge conflicts #39",
            "resolve conflicts on #39",
            "there are merge conflicts on #39",
            "fix conflicts #39",
            "fmc #39",
        ] {
            let p = parse_intent(phrase);
            assert_eq!(
                p.steps,
                vec![PlanStep::FixConflicts {
                    pr: Some("#39".to_string())
                }],
                "phrase {phrase:?} should map to FixConflicts(#39)"
            );
        }
    }

    #[test]
    fn bare_fix_merge_conflicts_parses_but_asks_for_pr() {
        // No PR named ⇒ still parses to FixConflicts(None) (mirrors bare "merge"),
        // and asks which PR rather than acting.
        let p = parse_intent("fix merge conflicts");
        assert_eq!(p.steps, vec![PlanStep::FixConflicts { pr: None }]);
        assert!(!p.is_actionable(), "missing PR ⇒ must clarify");
        assert!(p
            .clarifications
            .iter()
            .any(|c| c.contains("fix conflicts on")));
    }

    #[test]
    fn fix_conflicts_command_line_renders_with_and_without_pr() {
        assert_eq!(
            PlanStep::FixConflicts {
                pr: Some("#39".into())
            }
            .command_line(),
            "intent: fix-conflicts (#39)"
        );
        assert_eq!(
            PlanStep::FixConflicts { pr: None }.command_line(),
            "intent: fix-conflicts"
        );
    }
}
