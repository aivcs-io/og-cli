#[test]
fn test_og_cli_help() {
    let output = std::process::Command::new("cargo")
        .args(["run", "--", "--help"])
        .output();

    assert!(output.is_ok(), "og --help should succeed");
    let output = output.unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("AivcsOps"),
        "help should describe the AivcsOps front door"
    );
    assert!(
        !stdout.contains("Commands:"),
        "help must not advertise subcommands"
    );
}

#[test]
fn test_operational_words_are_prompt_text() {
    let output = std::process::Command::new("cargo")
        .args(["run", "--", "build", "test", "sandlot"])
        .output();

    assert!(output.is_ok());
    let output = output.unwrap();
    assert!(output.status.success());
}

#[test]
fn test_natural_language_sandlot_objective_is_accepted() {
    let output = std::process::Command::new("cargo")
        .args(["run", "--", "test", "this", "on", "sandlot"])
        .output();

    assert!(output.is_ok());
    let output = output.unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("intent") || stdout.contains("og-leader"),
        "natural-language objective should produce an AivcsOps plan"
    );
}
