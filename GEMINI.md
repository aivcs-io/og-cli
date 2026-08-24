# og-cli Repository Rules

## Public upstream vs sovereign forge

| Surface | Location |
|---------|----------|
| **Public GitHub** | `https://github.com/aivcs-io/og-cli` (this repo) |
| **Sovereign forge (SoT)** | `aivcs://aivcs/og-cli` @ `aivcsd.aivcs.io` |
| **Homebrew** | `https://github.com/aivcs-io/homebrew-tap` (release automation) |

This repository is the **public open-source mirror** of the forge `og-cli` tree. Internal development and fleet deploys use the forge head; GitHub receives tagged release exports.

## GitHub sync

When releasing a new public version:

1. Export from `aivcs://aivcs/og-cli@main` (strip forge-only files: `.aivcsignore`, `propel.toml`, `state.json`, path patches).
2. Push the branch and version tags to the `origin` remote (GitHub).
3. Homebrew tap automation picks up release artifacts from GitHub Releases.

## Build dependencies

Public builds resolve `envelope-data-mesh` (and optional `harbormaster`) from the **aivcs sparse registry** (`registry.aivcs.io`). Set `CARGO_REGISTRIES_AIVCS_TOKEN` before `cargo build`.

The default public build does **not** include harbormaster fallback. Enable `--features harbormaster-fallback` only when you have registry access to the harbormaster crate.

## Do not

- Add Dockerfiles or `docker build` workflows (use Nix OCI / dockworker in fleet repos).
- Commit forge credentials, `propel.toml`, or internal CI worktree patches here.
