#!/usr/bin/env bash
set -euo pipefail
# Run criterion benchmarks in release mode.
# Usage:
#   bash bench.sh              # all benchmarks
#   bash bench.sh vector       # single benchmark suite
#   bash bench.sh -- --save-baseline before   # pass args to criterion

if [[ "${1:-}" == "--" ]]; then
  shift
  cargo bench -- "$@"
elif [[ -n "${1:-}" ]]; then
  cargo bench --bench "$1" -- "${@:2}"
else
  cargo bench
fi
