#!/usr/bin/env bash
# Run criterion benchmarks with thermal management and automatic output capture.
#
# Usage:
#   bash bench.sh                              # all suites
#   bash bench.sh ordmap                       # single suite
#   bash bench.sh ordmap 'lookup|insert_mut'   # single suite with criterion filter
#   bash bench.sh -- --save-baseline before    # pass flags directly to criterion
#   bash bench.sh --cool-down 120 ordmap       # sleep 120 s before benchmarking
#
# Output is always teed to docs/bench_<label>_<YYYYMMDD_HHMMSS>.txt.
# A docs/bench_latest.txt symlink points to the most recent run.
#
# Thermal notes:
#   - caffeinate -i is always started (user-level, no sudo needed).
#   - highpowermode requires sudo; the script tries it silently and warns on failure.
#   - Use --cool-down <seconds> between a warming run and a comparison run.
#   - For reliable A/B comparisons, run BOTH variants in the same binary invocation
#     (criterion baseline workflow) rather than two separate cargo bench calls.

set -euo pipefail

# ---------------------------------------------------------------------------
# Argument: optional cool-down
# ---------------------------------------------------------------------------
COOL_DOWN=0
if [[ "${1:-}" == "--cool-down" ]]; then
    COOL_DOWN="${2:?--cool-down requires a number of seconds}"
    shift 2
fi

# ---------------------------------------------------------------------------
# Thermal management
# ---------------------------------------------------------------------------
CAFFEINATE_PID=
HIGH_POWER_ENABLED=false

cleanup() {
    if [[ -n "${CAFFEINATE_PID:-}" ]]; then
        kill "$CAFFEINATE_PID" 2>/dev/null || true
    fi
    if [[ "${HIGH_POWER_ENABLED:-false}" == "true" ]]; then
        sudo -n pmset -a highpowermode 0 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Prevent CPU sleep and clock down during benchmarks.
caffeinate -i &
CAFFEINATE_PID=$!

# High-power mode keeps the CPU at full frequency. Requires a passwordless sudo
# entry or an active sudo session; fails gracefully otherwise.
if sudo -n pmset -a highpowermode 1 2>/dev/null; then
    HIGH_POWER_ENABLED=true
    echo "[bench.sh] High-power mode enabled."
else
    echo "[bench.sh] Note: could not enable highpowermode (no passwordless sudo)."
    echo "[bench.sh]       For best results: sudo pmset -a highpowermode 1"
fi

# Warn if on battery — measurements are less stable.
if pmset -g batt 2>/dev/null | grep -q "Battery Power"; then
    echo "[bench.sh] Warning: running on battery — consider plugging in."
fi

# ---------------------------------------------------------------------------
# Optional thermal cool-down
# ---------------------------------------------------------------------------
if [[ "$COOL_DOWN" -gt 0 ]]; then
    echo "[bench.sh] Cooling down for ${COOL_DOWN}s before benchmarking …"
    sleep "$COOL_DOWN"
fi

# ---------------------------------------------------------------------------
# CPU codegen flags
# ---------------------------------------------------------------------------
# Appended (not replacing) so lld linking flags are preserved.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=native"

# ---------------------------------------------------------------------------
# Output capture helpers
# ---------------------------------------------------------------------------
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
mkdir -p docs

run_bench() {
    # run_bench <label> <cargo bench args…>
    local label="$1"; shift
    local out="docs/bench_${label}_${TIMESTAMP}.txt"
    echo "[bench.sh] Starting benchmark: $*"
    echo "[bench.sh] Output file:        $out"
    # Run and tee; exit non-zero if cargo bench fails.
    "$@" 2>&1 | tee "$out"
    local exit_code="${PIPESTATUS[0]}"
    if [[ "$exit_code" -ne 0 ]]; then
        echo "[bench.sh] ERROR: cargo bench exited with code $exit_code" >&2
        exit "$exit_code"
    fi
    if [[ ! -s "$out" ]]; then
        echo "[bench.sh] ERROR: output file is empty — something went wrong" >&2
        exit 1
    fi
    # Update latest symlink for quick access.
    ln -sf "bench_${label}_${TIMESTAMP}.txt" docs/bench_latest.txt
    echo "[bench.sh] Done. Results saved to $out"
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--" ]]; then
    # Pass everything after -- directly to criterion.
    shift
    run_bench "custom" cargo bench -- "$@"
elif [[ -n "${1:-}" ]]; then
    # Single suite, with optional criterion filter as second argument.
    SUITE="$1"; shift
    run_bench "$SUITE" cargo bench --bench "$SUITE" -- "$@"
else
    # All suites.
    run_bench "all" cargo bench
fi
