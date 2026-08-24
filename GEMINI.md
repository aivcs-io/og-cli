# og-cli Repository Rules

## Exceptions to VCS / OCI policy

While the organization enforces a strict "Exit GitHub" and "No Docker Hub" policy for general repositories:

* **The `og-cli` public GitHub repo is an exception** (alongside `aivcs-io/aivcs`).
* **GitHub sync**: `https://github.com/aivcs-io/og-cli` is the public open-source upstream.
* **Docker Hub publishing**: Version tags trigger release automation → Docker Hub (`hello0aivcs0io/og-cli`) and GHCR (`ghcr.io/aivcs-io/og-cli`). Org: https://hub.docker.com/repositories/hello0aivcs0io

## Public upstream vs sovereign forge

| Surface | Location |
|---------|----------|
| **Public GitHub** | `https://github.com/aivcs-io/og-cli` (this repo) |
| **Public Docker Hub** | `docker.io/hello0aivcs0io/og-cli` |
| **Public GHCR** | `ghcr.io/aivcs-io/og-cli` |
| **Sovereign forge (SoT)** | `aivcs://aivcs/og-cli` (internal — not mirrored verbatim) |
| **Homebrew** | `https://github.com/aivcs-io/homebrew-tap` (release automation) |

## Release process (scan-first, fail closed)

Every merge to `main` and every tag release **must pass** the `Release Gate` workflow before artifacts publish.

### Gate checks (in order)

1. **Secret scan** — gitleaks across the full tree (GitHub tokens, AWS keys, PEM private keys, etc.).
2. **Privacy scan** — ripgrep deny-list for personal paths, internal cluster DNS, private registry hosts, internal org slugs, and credential patterns.
3. **Build check** — `cargo check` + `cargo test` when `CARGO_REGISTRIES_AIVCS_TOKEN` is configured in repo secrets.

### Publishing a version

1. Export from sovereign forge `aivcs://aivcs/og-cli@main` (strip forge-only files: `.aivcsignore`, `propel.toml`, `state.json`, path patches, internal fleet catalogs).
2. Run the gate locally (optional but recommended):
   ```bash
   gitleaks detect --source . --no-git
   ```
3. Open PR → `Release Gate` must pass on GitHub.
4. Merge to `main`.
5. Tag `v*.*.*` and push — `Release` workflow runs the gate again, then publishes GitHub Release binaries.

**Never tag before the gate passes on `main`.**

### Public container usage

```bash
docker run --rm hello0aivcs0io/og-cli:latest --version
docker run --rm -it hello0aivcs0io/og-cli:latest
```

Prefer immutable version tags (`hello0aivcs0io/og-cli:v0.1.0`) in CI; `:latest` is for local developer convenience only.

## Build dependencies

Public builds resolve `envelope-data-mesh` from the **aivcs sparse registry** (`registry.aivcs.io`). Set `CARGO_REGISTRIES_AIVCS_TOKEN` before `cargo build`.

Optional `harbormaster-fallback` requires registry access to the harbormaster crate.

## Deployment wiring (public and internal)

Fleet-specific endpoints, execution substrates, and service catalogs are **never hardcoded in source**. Wire them at deploy time:

| Mechanism | Examples |
|-----------|----------|
| **Env overrides** | `OG_LEADER_GW_URL`, `OG_DATA_MESH_URL`, `OG_IDENTITY_BROKER_URL`, `DISCOVERY_URL` |
| **Service discovery** | Publications from `discovery.aivcs.io` per capability |
| **`OG_SERVICES_CATALOG`** | JSON file mounted or referenced in the cluster overlay |

Internal fleet deploys set these in `infra-code-micro` ConfigMaps / ExternalSecrets. The sovereign forge tree may contain additional wiring manifests; the **GitHub export** stays free of internal service names. Substrate selection (remote CI, runners, etc.) is **og-leader's** job after og-cli forwards the objective.

## Do not

- Publish internal swarm agents to Docker Hub — only the public `og` CLI runner belongs in `hello0aivcs0io`.
- Commit forge credentials, internal fleet catalogs, personal paths, or CI worktree patches here.
- Skip the release gate for any public publish.
