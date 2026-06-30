#!/usr/bin/env bash
set -euo pipefail
# Library crate — build all targets (lib + tests + benches) to verify compilation.
# Pass --release for an optimised build.
# --workspace ensures member crates (pds-folio, pds-merkle-spine) build together
# once they are added in Phase G.0 and H.0.
if [[ "${1:-}" == "--release" ]]; then
  cargo build --workspace --release --all-targets
else
  cargo build --workspace --all-targets
fi
