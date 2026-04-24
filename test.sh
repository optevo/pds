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
# Pre-existing upstream violations — allow until dedicated clippy cleanup pass.
# collapsible_match: ord/map.rs, ord/set.rs
# enum_variant_names: nodes/hamt.rs IterItem, IterMutItem
# unnecessary_cast: nodes/hamt.rs u8 cast
cargo clippy --all-features -- -D warnings \
  -A clippy::enum_variant_names \
  -A clippy::collapsible_match \
  -A clippy::unnecessary_cast

echo "--- cargo doc ---"
cargo doc --no-deps

echo "--- all checks passed ---"
