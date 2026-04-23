#!/usr/bin/env bash
set -euo pipefail
# Run criterion benchmarks in release mode with native CPU optimisations.
# Usage:
#   bash bench.sh              # all benchmarks
#   bash bench.sh vector       # single benchmark suite
#   bash bench.sh -- --save-baseline before   # pass args to criterion

# Enable native CPU codegen for benchmarks — lets LLVM use M-series-specific
# instruction scheduling. Measured ~8-15% improvement on HashMap lookups.
# Appended to (not replacing) RUSTFLAGS so lld linking is preserved.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=native"

if [[ "${1:-}" == "--" ]]; then
  shift
  cargo bench -- "$@"
elif [[ -n "${1:-}" ]]; then
  cargo bench --bench "$1" -- "${@:2}"
else
  cargo bench
fi
