#!/usr/bin/env bash
set -euo pipefail
# Quality gate — all steps must pass for work items to be considered done.

echo "--- cargo fmt --check ---"
cargo fmt --check

echo "--- cargo test (default features) ---"
cargo test

echo "--- cargo test (all features) ---"
cargo test --all-features

echo "--- cargo test (small-chunks) ---"
cargo test --features small-chunks

echo "--- cargo check (no-default-features) ---"
# Verify no_std surface compiles without std (directive compliance).
cargo check --no-default-features

echo "--- cargo clippy ---"
cargo clippy --all-features -- -D warnings

echo "--- cargo doc ---"
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

echo "--- cargo audit ---"
cargo audit

echo "--- all checks passed ---"
