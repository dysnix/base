# dysnix/base — community fork of base/base

This is an **unofficial community fork** of [base/base](https://github.com/base/base),
maintained by [dysnix](https://github.com/dysnix). It is **not affiliated with
Coinbase or the Base team**.

## Why this fork exists

Upstream `base/base` ships an official container image only for the `client`
service (`ghcr.io/base/node-reth-dev`), even though the build tooling defines
8 service targets (`client`, `consensus`, `builder`, `batcher`, `proposer`,
`websocket-proxy`, `ingress-rpc`, `audit-archiver`). To run a Base node from
container images today, the `consensus` binary in particular has to be built
from source.

This fork publishes multi-arch (`linux/amd64` + `linux/arm64`) images for
those services to `ghcr.io/dysnix/<service-name>`, so they can be consumed
directly. We intend to upstream the build pipeline; this fork exists to
unblock our own deployments in the meantime, and to prove the pipeline
before opening that PR.

## Image layout

| Service          | Image                                |
| ---------------- | ------------------------------------ |
| base-reth-node   | `ghcr.io/dysnix/base-reth-node`      |
| base-consensus   | `ghcr.io/dysnix/base-consensus`      |

Phase 2 will add the remaining services (`base-builder`, `base-batcher`,
`base-proposer`, `websocket-proxy`, `ingress-rpc`, `audit-archiver`).

Tags:
- `vX.Y.Z` — built from the upstream tag of the same name
- `latest` — alias for the most recent non-rc release we've built
- `MAJOR.MINOR` — rolling tag for non-rc releases

## Branch layout

- **`main`** — kept as a clean mirror of upstream `base/base` `main`. Synced
  via the [merge-upstream API](https://docs.github.com/en/rest/branches/branches#sync-a-fork-branch-with-the-upstream-repository)
  every 6 hours.
- **`dysnix/ci`** (default branch) — equals `main` plus the dysnix CI
  workflows (`.github/workflows/dysnix-*.yml`) and this file. Rebased on
  top of `main` automatically; never carries edits to upstream files.

## Workflows

- [`.github/workflows/dysnix-sync-upstream.yml`](.github/workflows/dysnix-sync-upstream.yml)
  — keeps `main`, `dysnix/ci`, and tags in sync with upstream.
- [`.github/workflows/dysnix-images.yml`](.github/workflows/dysnix-images.yml)
  — builds and pushes images. Trigger manually from the Actions tab with a
  `version` input (e.g. `v0.7.6`); it checks out that upstream tag and
  builds.

## Trademarks

"Base" is a trademark of Coinbase. This fork uses the name to identify
which software it builds; it does not claim affiliation with or endorsement
by Coinbase.

## License

Same as upstream — MIT. See [LICENSE](LICENSE).
