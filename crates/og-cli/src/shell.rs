//! Interactive `og` shell (REPL).
//!
//! Fundamental flow (FR code-governance#613, revised): running bare `og` drops
//! you into this shell — like launching `claude`. Inside, you type plain English
//! and og maps it to a command plan (via [`crate::intent`]) and, on confirmation,
//! runs it against the existing command primitives. There is no `do` prefix
//! inside the shell — natural language *is* the interaction.
//!
//! Login is **separate and persistent**: use `/login` inside the shell; the
//! token persists across shell sessions. There are no pre-run operational
//! subcommands — the shell is the product surface.

use crate::config::ProjectConfig;
use crate::intent::{extract_pr_reference, parse_intent, IntentPlan, PlanStep};
use crate::session::{parse_session_command, SessionCommand};
use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

/// Outcome of reading one line at the top-level `og>` prompt. Ctrl-C and Ctrl-D
/// both leave the shell (session-local; no data-mesh).
enum ReadOutcome {
    Line(String),
    Interrupted, // Ctrl-C
    Eof,         // Ctrl-D / end of piped input
}

/// Line-input abstraction shared by the main prompt and every sub-prompt.
///
/// On a TTY it wraps a [`rustyline::DefaultEditor`], giving arrow-key history,
/// ←/→ cursor movement, Ctrl-A/E, etc., plus a persistent history file. When
/// stdin is *not* a terminal (e.g. `echo "status" | og`) rustyline cannot drive
/// the terminal, so we fall back to plain buffered `stdin` line reads — scripted
/// and piped usage keeps working unchanged.
enum LineReader {
    Interactive {
        editor: Box<DefaultEditor>,
        history_path: Option<PathBuf>,
    },
    Piped,
}

impl LineReader {
    /// Build a reader, choosing interactive vs. piped from whether stdin is a
    /// TTY. On the interactive path, load history from `~/.og/history`.
    fn new() -> Self {
        if io::stdin().is_terminal() {
            if let Ok(mut editor) = DefaultEditor::new() {
                let history_path = og_history_path();
                if let Some(path) = history_path.as_ref() {
                    // Missing file on first run is fine; ignore the error.
                    let _ = editor.load_history(path);
                }
                return LineReader::Interactive {
                    editor: Box::new(editor),
                    history_path,
                };
            }
            // Editor init failed (no terminal available after all) — fall
            // through to the piped reader so we never hard-fail here.
        }
        LineReader::Piped
    }

    /// Read one command at the top-level prompt, recording non-empty lines in
    /// history and reporting Ctrl-C / Ctrl-D distinctly.
    fn read_command(&mut self, prompt: &str) -> Result<ReadOutcome> {
        match self {
            LineReader::Interactive { editor, .. } => match editor.readline(prompt) {
                Ok(line) => {
                    if !line.trim().is_empty() {
                        let _ = editor.add_history_entry(line.trim());
                    }
                    Ok(ReadOutcome::Line(line))
                }
                Err(ReadlineError::Interrupted) => Ok(ReadOutcome::Interrupted),
                Err(ReadlineError::Eof) => Ok(ReadOutcome::Eof),
                Err(e) => Err(e.into()),
            },
            LineReader::Piped => match read_piped_line(prompt)? {
                Some(line) => Ok(ReadOutcome::Line(line)),
                None => Ok(ReadOutcome::Eof),
            },
        }
    }

    /// Read one line for a sub-prompt (confirm, PR entry, `/config`). EOF and
    /// Ctrl-C both surface as `None`, i.e. "cancel / no answer".
    fn read_line(&mut self, prompt: &str) -> Result<Option<String>> {
        match self {
            LineReader::Interactive { editor, .. } => match editor.readline(prompt) {
                Ok(line) => Ok(Some(line)),
                Err(ReadlineError::Interrupted | ReadlineError::Eof) => Ok(None),
                Err(e) => Err(e.into()),
            },
            LineReader::Piped => read_piped_line(prompt),
        }
    }

    /// Persist history on exit (interactive only; best-effort).
    fn save_history(&mut self) {
        if let LineReader::Interactive {
            editor,
            history_path: Some(path),
        } = self
        {
            let _ = editor.save_history(path);
        }
    }
}

/// Prompt + plain buffered line read from stdin, used for non-TTY / piped input.
/// `None` on EOF. Locking `io::stdin()` fresh each call is safe: the lock shares
/// the one global buffer, so bytes read across calls are not lost.
fn read_piped_line(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush().ok();
    let mut buf = String::new();
    if io::stdin().lock().read_line(&mut buf)? == 0 {
        println!();
        return Ok(None);
    }
    Ok(Some(buf))
}

/// Resolve `~/.og/history`, creating the config dir if needed. Reuses the same
/// `~/.og` resolution as the persistent config. `None` if the dir is unusable.
fn og_history_path() -> Option<PathBuf> {
    let dir = crate::persist::ConfigPaths::config_dir().ok()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!("could not create og config dir for history: {e}");
        return None;
    }
    Some(dir.join("history"))
}

/// Run the interactive shell until Ctrl-C, Ctrl-D, or `exit`/`quit`.
pub async fn run(config: &ProjectConfig) -> Result<()> {
    banner();
    let mut reader = LineReader::new();
    loop {
        let line = match reader.read_command("og> ")? {
            ReadOutcome::Line(line) => line,
            ReadOutcome::Interrupted | ReadOutcome::Eof => {
                println!();
                break; // Ctrl-C / Ctrl-D / end of input ⇒ exit cleanly
            }
        };
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        // Local session verbs (login / exit / help / …) — slash optional, any
        // laptop, no data-mesh. Never forwarded to og-leader.
        if let Some(cmd) = parse_session_command(input) {
            if run_session(cmd, &mut reader).await? {
                break;
            }
            continue;
        }
        match input {
            // A leading `!` is a shell escape: run the rest as a local command
            // (e.g. `!gh auth login`) with inherited stdio, then return to the
            // shell — never forwarded to og-leader.
            s if s.starts_with('!') => run_shell_escape(s[1..].trim()),
            // A leading slash addresses the shell, not the fleet: an unrecognized
            // slash-command is a local error — never forward `/foo` to og-leader.
            s if s.starts_with('/') => {
                println!("Unknown command: {s}. Type `help` for available commands.");
            }
            _ => handle_intent(input, config, &mut reader).await?,
        }
    }
    reader.save_history();
    println!("bye");
    Ok(())
}

/// Run a `!`-prefixed shell escape (e.g. `!gh auth login`). Inherits stdio so
/// interactive commands — browser logins, prompts — work, then returns to the
/// shell. Failures are reported, never propagated: a bad `!cmd` must not kill
/// the session.
fn run_shell_escape(cmd: &str) {
    if cmd.is_empty() {
        println!("usage: !<shell command>   e.g.  !gh auth login");
        return;
    }
    match std::process::Command::new("sh").arg("-c").arg(cmd).status() {
        Ok(status) if status.success() => {}
        Ok(status) => println!("(`{cmd}` exited with {status})"),
        Err(e) => println!("failed to run `{cmd}`: {e}"),
    }
}

/// Local session verb. `true` ⇒ leave the REPL. Never touches the data-mesh.
async fn run_session(cmd: SessionCommand, reader: &mut LineReader) -> Result<bool> {
    match cmd {
        SessionCommand::Exit => Ok(true),
        SessionCommand::Help => {
            help();
            Ok(false)
        }
        SessionCommand::Login => {
            let _ = crate::auth::AuthManager::login(crate::auth::LoginProvider::Auto).await;
            Ok(false)
        }
        SessionCommand::Logout => {
            let _ = crate::auth::AuthManager::logout();
            Ok(false)
        }
        SessionCommand::Whoami => {
            let _ = crate::auth::AuthManager::whoami();
            Ok(false)
        }
        SessionCommand::Compact => {
            println!("No local transcript to compact; AivcsOps state remains authoritative.");
            Ok(false)
        }
        SessionCommand::Config => {
            config_menu(reader)?;
            Ok(false)
        }
    }
}

async fn run_session_oneshot(cmd: SessionCommand) -> Result<()> {
    match cmd {
        SessionCommand::Exit => {
            println!("bye");
            Ok(())
        }
        SessionCommand::Help => {
            help();
            Ok(())
        }
        SessionCommand::Login => {
            crate::auth::AuthManager::login(crate::auth::LoginProvider::Auto).await
        }
        SessionCommand::Logout => crate::auth::AuthManager::logout(),
        SessionCommand::Whoami => crate::auth::AuthManager::whoami(),
        SessionCommand::Compact => {
            println!("No local transcript to compact; AivcsOps state remains authoritative.");
            Ok(())
        }
        SessionCommand::Config => {
            println!("config is interactive — start `og` and type `config`.");
            Ok(())
        }
    }
}

fn banner() {
    println!("og — aivcs.io og-cli");
    match crate::auth::AuthManager::session_summary() {
        Some(s) => println!("signed in: {s}"),
        None => println!("not signed in — type `login` (slash optional)"),
    }
    println!("Type what you want in plain English. `help` for tips, `exit` or Ctrl-C to quit.\n");
}

fn help() {
    println!("Just describe what you want, e.g.:");
    println!("  build and test this repo");
    println!("  review #42");
    println!("  run the gate then merge https://github.com/lornu-ai/og-cli/pull/16");
    println!("  what's the status?");
    println!("  show my account");
    println!("  switch to account alice");
    println!("  use org team-x");
    println!("Commands (slash optional): login · logout · whoami · compact · config · help · exit");
    println!("Shell escape: prefix a shell command with `!`, e.g. `!gh auth login`.");
    println!("If og asks for a PR, answer with #123 or a URL, or type cancel.");
    println!("Quit: `exit`, Ctrl-C, or Ctrl-D. Mutating actions (merge) always ask before running.\n");
}

/// Parse one line of natural language, preview the plan, and run it on confirmation.
async fn handle_intent(text: &str, config: &ProjectConfig, reader: &mut LineReader) -> Result<()> {
    let plan = parse_intent(text);

    if !plan.clarifications.is_empty() {
        for c in &plan.clarifications {
            println!("  🤔 {c}");
        }
        if plan.steps.is_empty() {
            return Ok(());
        }
    }

    if has_missing_pr(&plan) {
        if let Some(pr) = prompt_for_pr(reader)? {
            let resolved = apply_pr_answer(&plan, &pr);
            return run_plan(&resolved, config, reader).await;
        }
        println!("  (skipped)\n");
        return Ok(());
    }

    if !plan.is_actionable() {
        println!("  (resolve the questions above, then try again)\n");
        return Ok(());
    }

    run_plan(&plan, config, reader).await
}

async fn run_plan(
    plan: &IntentPlan,
    config: &ProjectConfig,
    reader: &mut LineReader,
) -> Result<()> {
    println!("  → {}", plan.normalized);
    for (i, step) in plan.steps.iter().enumerate() {
        println!("     {}. {}", i + 1, step.command_line());
    }

    if plan
        .steps
        .iter()
        .all(|s| matches!(s, PlanStep::Session(_)))
        && !plan.steps.is_empty()
    {
        execute_plan(plan, config).await?;
        println!();
        return Ok(());
    }

    let prompt = if plan.requires_confirmation {
        "  ⚠️  This mutates a PR/repo. Run it? [y/N] "
    } else {
        "  Run this? [Y/n] "
    };
    if !confirm(
        reader,
        prompt,
        /* default_yes = */ !plan.requires_confirmation,
    )? {
        println!("  (skipped)\n");
        return Ok(());
    }

    execute_plan(plan, config).await?;
    println!();
    Ok(())
}

/// Dispatch a resolved plan to the existing command primitives. Shared with the
/// non-interactive one-shot path so both behave identically.
pub async fn execute_plan(plan: &IntentPlan, config: &ProjectConfig) -> Result<()> {
    use crate::account::AccountManager;
    use crate::org::OrgManager;
    use crate::prs::PrManager;
    use crate::status::StatusManager;
    use crate::workflow::WorkflowOrchestrator;

    for step in &plan.steps {
        match step {
            PlanStep::Design { prompt } => {
                WorkflowOrchestrator::design(Some(prompt.clone()), config).await?
            }
            PlanStep::Build => WorkflowOrchestrator::build(None, None, false, config).await?,
            PlanStep::Test { integration, pr } => {
                WorkflowOrchestrator::test(true, *integration, false, pr.clone(), config).await?
            }
            PlanStep::Review { pr } => WorkflowOrchestrator::review(pr.clone(), config).await?,
            PlanStep::Gate { pr } => PrManager::gate(pr.clone(), false, 80, config).await?,
            PlanStep::Merge { pr } => {
                PrManager::merge_gated(pr.clone(), false, false, config).await?
            }
            PlanStep::FixConflicts { pr } => {
                WorkflowOrchestrator::fix_conflicts(pr.clone(), config).await?
            }
            PlanStep::CreateRepo {
                name,
                private,
                readme,
                license,
                default_branch,
                description,
                org,
            } => {
                // Sovereign repo creation (code-governance#795): og-cli → og-leader
                // → AOA → og-agent-forge. Routed through the same orchestrator as
                // `og repo create`, which enforces the fail-closed auth check,
                // resolves org/account, dispatches over A2A, and prints the audit
                // line. og-cli never creates a repo via a local gh/GitHub side channel.
                let spec = crate::backend::CreateRepoSpec {
                    name: name.clone(),
                    private: *private,
                    readme: *readme,
                    license: license.clone(),
                    default_branch: default_branch.clone(),
                    description: description.clone(),
                    org: org.clone(),
                    account: None,
                };
                WorkflowOrchestrator::create_repo(spec, config).await?;
            }
            PlanStep::Status => StatusManager::show(None)?,
            PlanStep::AccountShow => AccountManager::show()?,
            PlanStep::AccountList => AccountManager::list()?,
            PlanStep::AccountSwitch { account_id } => AccountManager::switch(account_id)?,
            PlanStep::AccountLogout { account_id } => {
                AccountManager::logout(account_id.as_deref())?
            }
            PlanStep::OrgList => OrgManager::list()?,
            PlanStep::OrgUse { org } => OrgManager::use_org(org)?,
            PlanStep::ForwardToLeader { objective } => {
                // og-cli is a thin NL front: hand the free-form / fleet-scoped
                // objective to og-leader over A2A. og-leader + AOA plan and select
                // the required og-* agents; the CLI never chooses the agent
                // (code-governance#769). Routed through the backend seam (not a raw
                // A2A call) so a fleet objective FAILS CLOSED — with the fallback
                // enabled, harbormaster has no local "test every PR in the org" and
                // bails, never a fabricated local verdict (og-cli#48).
                WorkflowOrchestrator::forward_to_leader(objective, config).await?;
            }
            PlanStep::Session(cmd) => {
                run_session_oneshot(*cmd).await?;
            }
        }
    }
    Ok(())
}

/// Interactive `/config` menu (code-governance#758). Reads/writes the persistent
/// global config at `~/.og/config.toml` (via [`crate::persist::GlobalConfig`]);
/// on a write failure the change stays session-scoped and is reported as such.
fn config_menu(reader: &mut LineReader) -> Result<()> {
    use crate::persist::GlobalConfig;

    loop {
        // Reload each iteration so displayed values reflect what's persisted.
        let mut cfg = GlobalConfig::load().unwrap_or_default();
        println!("  Configuration Menu:");
        println!(
            "    1. Model choice (claude, codex, …)        [{}]",
            cfg.model.as_deref().unwrap_or("default")
        );
        println!(
            "    2. Fallback orchestrator (og-leader → harbormaster)  [{}]",
            if cfg.fallback_enabled {
                "enabled"
            } else {
                "disabled"
            }
        );
        println!(
            "    3. Backend mode (og-leader, local)        [{}]",
            cfg.default_backend.as_deref().unwrap_or("auto")
        );
        println!(
            "    4. Data mesh endpoint                     [{}]",
            cfg.data_mesh_url
                .as_deref()
                .unwrap_or("(env DATA_MESH_URL)")
        );
        println!("    q. Back to shell");

        let Some(choice) = reader.read_line("  Select: ")? else {
            return Ok(());
        };
        match choice.trim() {
            "1" => {
                if let Some(v) = reader.read_line("    Model (claude, codex, …): ")? {
                    let v = v.trim();
                    if !v.is_empty() {
                        cfg.model = Some(v.to_string());
                        persist_config(&cfg, &format!("model: {v}"))?;
                    }
                }
            }
            "2" => {
                let enable = confirm(
                    reader,
                    "    Enable harbormaster fallback on mesh down? [Y/n]: ",
                    /* default_yes = */ true,
                )?;
                cfg.fallback_enabled = enable;
                persist_config(
                    &cfg,
                    &format!(
                        "harbormaster fallback: {}",
                        if enable { "enabled" } else { "disabled" }
                    ),
                )?;
            }
            "3" => {
                if let Some(v) = reader.read_line("    Backend mode (og-leader | local): ")? {
                    let v = v.trim().to_lowercase();
                    match v.as_str() {
                        "og-leader" | "local" => {
                            cfg.default_backend = Some(v.clone());
                            persist_config(&cfg, &format!("backend mode: {v}"))?;
                        }
                        "" => {}
                        other => println!("    ✗ unknown mode {other:?} (use og-leader or local)"),
                    }
                }
            }
            "4" => {
                if let Some(v) = reader.read_line("    Data mesh endpoint (blank to clear): ")? {
                    let v = v.trim();
                    cfg.data_mesh_url = if v.is_empty() {
                        None
                    } else {
                        Some(v.to_string())
                    };
                    persist_config(
                        &cfg,
                        &format!(
                            "data-mesh endpoint: {}",
                            if v.is_empty() { "(cleared)" } else { v }
                        ),
                    )?;
                }
            }
            "q" | "quit" | "" => return Ok(()),
            other => println!("  Unknown option: {other}"),
        }
        println!();
    }
}

/// Save `cfg` and report the outcome; falls back to a session-scoped note if the
/// on-disk write fails (so the menu stays usable in read-only environments).
fn persist_config(cfg: &crate::persist::GlobalConfig, label: &str) -> Result<()> {
    match cfg.save() {
        Ok(()) => println!("    ✓ {label} (saved to ~/.og/config.toml)"),
        Err(e) => println!("    ✓ {label} (session only — could not persist: {e})"),
    }
    Ok(())
}

/// Read a y/n answer via `reader` with the given default. EOF/cancel ⇒ do not run.
fn confirm(reader: &mut LineReader, prompt: &str, default_yes: bool) -> Result<bool> {
    match reader.read_line(prompt)? {
        Some(answer) => Ok(interpret_confirm(&answer, default_yes)),
        None => Ok(false), // EOF / Ctrl-C ⇒ do not run
    }
}

fn prompt_for_pr(reader: &mut LineReader) -> Result<Option<String>> {
    loop {
        let Some(answer) = reader.read_line("  PR> ")? else {
            return Ok(None);
        };
        let answer = answer.trim();
        if answer.is_empty() {
            continue;
        }
        match answer.to_lowercase().as_str() {
            "cancel" | "abort" | "skip" => return Ok(None),
            _ => {
                if let Some(pr) = parse_pr_answer(answer) {
                    return Ok(Some(pr));
                }
                println!("  Please answer with #123 or a PR URL, or type cancel.");
            }
        }
    }
}

fn parse_pr_answer(answer: &str) -> Option<String> {
    let trimmed = answer.trim().trim_end_matches(['.', ',', ')']);
    if trimmed.is_empty() {
        return None;
    }

    if let Some(pr) = extract_pr_reference(trimmed) {
        return Some(pr);
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("#{trimmed}"));
    }

    None
}

fn has_missing_pr(plan: &IntentPlan) -> bool {
    plan.steps.iter().any(|step| {
        matches!(
            step,
            PlanStep::Review { pr: None }
                | PlanStep::Gate { pr: None }
                | PlanStep::Merge { pr: None }
                | PlanStep::FixConflicts { pr: None }
        )
    })
}

fn apply_pr_answer(plan: &IntentPlan, pr: &str) -> IntentPlan {
    let pr = pr.to_string();
    let mut resolved = plan.clone();
    resolved.steps = plan
        .steps
        .iter()
        .map(|step| match step {
            PlanStep::Review { pr: None } => PlanStep::Review {
                pr: Some(pr.clone()),
            },
            PlanStep::Gate { pr: None } => PlanStep::Gate {
                pr: Some(pr.clone()),
            },
            PlanStep::Merge { pr: None } => PlanStep::Merge {
                pr: Some(pr.clone()),
            },
            PlanStep::FixConflicts { pr: None } => PlanStep::FixConflicts {
                pr: Some(pr.clone()),
            },
            other => other.clone(),
        })
        .collect();
    resolved.clarifications.clear();
    resolved.requires_confirmation = resolved.steps.iter().any(PlanStep::mutates);
    resolved
}

/// Pure y/n interpretation (default applied on empty input). Extracted for tests.
fn interpret_confirm(answer: &str, default_yes: bool) -> bool {
    match answer.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        "" => default_yes,
        _ => false, // anything unexpected ⇒ do not run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_defaults_and_explicit() {
        assert!(interpret_confirm("y", false));
        assert!(interpret_confirm("YES", false));
        assert!(!interpret_confirm("n", true));
        // empty applies the default
        assert!(interpret_confirm("", true));
        assert!(!interpret_confirm("", false));
        // anything unexpected is a refusal
        assert!(!interpret_confirm("maybe", true));
    }

    #[test]
    fn parse_pr_answer_accepts_url_number_and_bare_integer() {
        assert_eq!(parse_pr_answer("#42"), Some("#42".to_string()));
        assert_eq!(
            parse_pr_answer("https://github.com/lornu-ai/og-cli/pull/13"),
            Some("https://github.com/lornu-ai/og-cli/pull/13".to_string())
        );
        assert_eq!(
            parse_pr_answer("https://github.com/lornu-ai/og-cli/pull/13/conflicts"),
            Some("https://github.com/lornu-ai/og-cli/pull/13".to_string())
        );
        assert_eq!(parse_pr_answer("1"), Some("#1".to_string()));
        assert_eq!(parse_pr_answer("merge"), None);
    }

    #[test]
    fn apply_pr_answer_fills_missing_merge_pr() {
        let plan = parse_intent("merge this");
        let resolved = apply_pr_answer(&plan, "#7");
        assert_eq!(
            resolved.steps,
            vec![PlanStep::Merge {
                pr: Some("#7".to_string())
            }]
        );
        assert!(resolved.is_actionable());
        assert!(resolved.clarifications.is_empty());
    }
}
