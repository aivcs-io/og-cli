# og-cli

AIVCS Ops front door — one interactive `og` command for natural-language objectives.

## Features

- One executable: `og`
- Interactive slash commands such as `/login`, `/whoami`, `/compact`, and `/exit`
- Natural-language objectives routed through og-leader → envelope-data-fabric-graph
- Unified authentication (OIDC via the identity layer)

## Status

Early development (Phase 1 MVP target: Oct 2026)

## Build

`og-cli` depends on crates published on the [aivcs sparse registry](https://registry.aivcs.io/). Export a read token and build:

```bash
export CARGO_REGISTRIES_AIVCS_TOKEN="your-token"
cargo build --release -p og-cli
```

The binary installs as `target/release/og`.

Or run from the public container (Docker Hub: [hello0aivcs0io](https://hub.docker.com/repositories/hello0aivcs0io)):

```bash
docker run --rm hello0aivcs0io/og-cli:latest --version
docker run --rm -it hello0aivcs0io/og-cli:latest
```

Optional local fallback (internal fleets only) requires the `harbormaster-fallback` feature:

```bash
cargo build --release -p og-cli --features harbormaster-fallback
```

## Interaction model

Run `og`, then type an objective. Agent selection, execution placement, and durable state are decided behind the AivcsOps orchestration seam.

```text
$ og
og> /login
og> run the test suite on this repository
og> /compact
og> /exit
```

## Architecture

The CLI is deliberately a single AivcsOps front door. Slash commands control the local session; natural language is forwarded to the sovereign orchestrator:

```
og-cli ↔ og-cli-service ↔ og-leader-gw ↔ og-leader ↔ graph
```

## Public vs forge

| Surface | Location |
|---------|----------|
| This repo | `https://github.com/aivcs-io/og-cli` |
| Docker Hub | `docker.io/hello0aivcs0io/og-cli` |
| GHCR | `ghcr.io/aivcs-io/og-cli` |
| Sovereign forge (SoT) | `aivcs://aivcs/og-cli` |
| Homebrew | `brew install aivcs-io/tap/og` (after first release) |

See [GEMINI.md](./GEMINI.md) for release/sync policy, deployment wiring, and the scan-first release gate.

## License

Apache-2.0 — see [LICENSE](./LICENSE).
