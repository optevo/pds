#!/usr/bin/env bash
set -euo pipefail
# Quality gate — all steps must pass for work items to be considered done.
#
# Workspace note: once member crates are added (pds-folio at G.0, pds-merkle-spine
# at H.0), the `--workspace` smoke-check step below catches cross-crate regressions.
# Per-crate steps target the root `pds` crate only (no --workspace) because member
# crates define their own feature sets and may not share all features with the root.

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

echo "--- cargo test --workspace (smoke check) ---"
# Smoke check across the full workspace — catches cross-crate breakage.
# Member crates (pds-folio, pds-merkle-spine) are included once added.
cargo test --workspace

echo "--- cargo audit ---"
cargo audit

echo "--- all checks passed ---"
