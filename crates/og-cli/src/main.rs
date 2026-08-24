//! The single `og` AivcsOps front door.
//!
//! The binary intentionally has no subcommands. Once started, users interact
//! with the REPL (`/login`, `/compact`, `/help`, …) or pass one natural-language
//! objective. Agent choice, execution placement, and durable state belong to
//! og-leader → orchestrator/OxidizedGraph.

use clap::Parser;
use og_cli::config::ConfigManager;

#[derive(Parser)]
#[command(name = "og", about = "AivcsOps natural-language front door")]
#[command(
    long_about = "Start the AivcsOps shell with `og`, or pass one natural-language objective. \
                  There are no `og <subcommand>` operational paths; orchestration is owned by \
                  og-leader → orchestrator/OxidizedGraph."
)]
struct Cli {
    /// One natural-language objective. Without this, start the interactive shell.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Non-interactive mode: print result and exit (like `claude --print`)
    #[arg(long)]
    print: bool,

    /// Execute the generated plan instead of previewing it.
    #[arg(long)]
    execute: bool,

    /// Auto-confirm a mutating plan when used with --execute.
    #[arg(long)]
    yes: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("og_cli=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let config = ConfigManager::load_project().await?;

    if cli.prompt.is_empty() {
        og_cli::shell::run(&config).await
    } else {
        run_intent(cli.prompt.join(" "), cli.execute, cli.yes, cli.print, &config).await
    }
}

async fn run_intent(
    text: String,
    execute: bool,
    yes: bool,
    json: bool,
    config: &og_cli::config::ProjectConfig,
) -> anyhow::Result<()> {
    use og_cli::intent::parse_intent;

    let plan = parse_intent(&text);

    let session_only = !plan.steps.is_empty()
        && plan
            .steps
            .iter()
            .all(|s| matches!(s, og_cli::intent::PlanStep::Session(_)));
    if session_only {
        return og_cli::shell::execute_plan(&plan, config).await;
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&plan.trace())?);
    }

    if !plan.clarifications.is_empty() {
        println!("🤔 I need a bit more before I can run this:");
        for clarification in &plan.clarifications {
            println!("   - {clarification}");
        }
        if plan.steps.is_empty() {
            return Ok(());
        }
    }

    println!("🧭 Understood: {}", plan.normalized);
    println!("   Plan:");
    for (index, step) in plan.steps.iter().enumerate() {
        println!("     {}. {}", index + 1, step.command_line());
    }
    if plan.requires_confirmation {
        println!("   ⚠️  This plan mutates a PR/repo and needs confirmation (--yes).");
    }

    if !execute {
        println!("\n(dry run — re-run with --execute to perform these steps)");
        return Ok(());
    }
    if !plan.is_actionable() {
        println!("\nNot executing: resolve the questions above first.");
        return Ok(());
    }
    if plan.requires_confirmation && !yes {
        println!(
            "\nRefusing to run a mutating plan without confirmation. Re-run with --execute --yes."
        );
        return Ok(());
    }

    og_cli::shell::execute_plan(&plan, config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_words_are_prompt_text_not_subcommands() {
        let cli = Cli::parse_from(["og", "build", "test", "sandlot"]);
        assert_eq!(cli.prompt, vec!["build", "test", "sandlot"]);
    }

    #[test]
    fn bare_og_has_no_command_surface() {
        let cli = Cli::parse_from(["og"]);
        assert!(cli.prompt.is_empty());
        assert!(!cli.execute);
        assert!(!cli.yes);
    }
}
