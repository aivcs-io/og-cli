# og-cli Repository Rules

## Exceptions to VCS / OCI policy

While the organization enforces a strict "Exit GitHub" and "No Docker Hub" policy for general repositories:

* **The `og-cli` public GitHub repo is an exception** (alongside `aivcs-io/aivcs`).
* **GitHub sync**: `https://github.com/aivcs-io/og-cli` is the public open-source upstream. When releasing, push the branch and version tags to `origin` (GitHub).
* **Docker Hub publishing**: Version tags on GitHub trigger release automation that publishes the official public CLI image to Docker Hub (`hello0aivcs0io/og-cli`) and GHCR (`ghcr.io/aivcs-io/og-cli`). Docker Hub org: https://hub.docker.com/repositories/hello0aivcs0io

## Public upstream vs sovereign forge

| Surface | Location |
|---------|----------|
| **Public GitHub** | `https://github.com/aivcs-io/og-cli` (this repo) |
| **Public Docker Hub** | `docker.io/hello0aivcs0io/og-cli` |
| **Public GHCR** | `ghcr.io/aivcs-io/og-cli` |
| **Sovereign forge (SoT)** | `aivcs://aivcs/og-cli` @ `aivcsd.aivcs.io` |
| **Homebrew** | `https://github.com/aivcs-io/homebrew-tap` (release automation) |

This repository is the **public open-source mirror** of the forge `og-cli` tree. Internal development and fleet deploys use the forge head; GitHub receives tagged release exports.

## GitHub sync

When releasing a new public version:

1. Export from `aivcs://aivcs/og-cli@main` (strip forge-only files: `.aivcsignore`, `propel.toml`, `state.json`, path patches).
2. Push the branch and version tags (`v*.*.*`) to the `origin` remote (GitHub).
3. Release automation publishes OCI images to Docker Hub + GHCR and binary artifacts for Homebrew.

## Public container usage

```bash
# Verify version (after first release)
docker run --rm hello0aivcs0io/og-cli:latest --version

# Interactive session (mount workspace as needed)
docker run --rm -it -v "$PWD:/work" -w /work hello0aivcs0io/og-cli:latest
```

Prefer immutable version tags (`hello0aivcs0io/og-cli:v0.1.0`) in CI pipelines; `:latest` is for local developer convenience only.

## Build dependencies

Public builds resolve `envelope-data-mesh` (and optional `harbormaster`) from the **aivcs sparse registry** (`registry.aivcs.io`). Set `CARGO_REGISTRIES_AIVCS_TOKEN` before `cargo build`.

The default public build does **not** include harbormaster fallback. Enable `--features harbormaster-fallback` only when you have registry access to the harbormaster crate.

## Do not

- Publish internal swarm agents (`og-leader`, `og-crab`, `data-fabric`, etc.) to Docker Hub — only the public `og` CLI runner belongs in `hello0aivcs0io`.
- Commit forge credentials, `propel.toml`, or internal CI worktree patches here.
