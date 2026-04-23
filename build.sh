#!/usr/bin/env bash
set -euo pipefail
# Library crate — build all targets (lib + tests + benches) to verify compilation.
# Pass --release for an optimised build.
if [[ "${1:-}" == "--release" ]]; then
  cargo build --release --all-targets
else
  cargo build --all-targets
fi
