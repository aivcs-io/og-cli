use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub duration_secs: u64,
}

/// When set, local backend skips shelling out to cargo/gh (used in unit tests).
pub fn stub_mode() -> bool {
    std::env::var("OG_CLI_STUB").is_ok()
}

pub async fn run(cmd: &str, args: &[&str]) -> Result<CommandResult> {
    run_in(None, cmd, args).await
}

/// Same as [`run`], but executes in `dir` when given (e.g. a PR worktree)
/// instead of the current process's working directory.
pub async fn run_in(
    dir: Option<&std::path::Path>,
    cmd: &str,
    args: &[&str],
) -> Result<CommandResult> {
    let started = Instant::now();
    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run {cmd}"))?;

    Ok(CommandResult {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        duration_secs: started.elapsed().as_secs(),
    })
}

pub fn tool_available(name: &str) -> bool {
    which::which(name).is_ok()
}
