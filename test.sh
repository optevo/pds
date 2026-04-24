#!/usr/bin/env bash
set -euo pipefail
# Quality gate — all steps must pass for work items to be considered done.

echo "--- cargo test (default features) ---"
cargo test

echo "--- cargo test (all features) ---"
cargo test --all-features

echo "--- cargo test (small-chunks) ---"
cargo test --features small-chunks

echo "--- cargo clippy ---"
cargo clippy --all-features -- -D warnings

echo "--- cargo doc ---"
cargo doc --no-deps

echo "--- all checks passed ---"
