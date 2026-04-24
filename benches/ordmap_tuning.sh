#!/usr/bin/env bash
# OrdMap B+ tree node size tuning benchmark.
# Patches ORD_CHUNK_SIZE in src/config.rs, runs ordmap benchmarks, then restores.
#
# Usage:
#   bash benches/ordmap_tuning.sh           # all sizes
#   bash benches/ordmap_tuning.sh 16 32     # specific sizes
#
# Results saved to target/tuning/<size>/

set -euo pipefail

CONFIG="src/config.rs"
RESULTS_DIR="target/tuning"

# Sizes to test — must be even, ≥6 (MEDIAN and THIRD must be ≥1)
if [ $# -gt 0 ]; then
    SIZES=("$@")
else
    SIZES=(16 24 32 48 64)
fi

# Save original config
cp "$CONFIG" "$CONFIG.bak"
trap 'mv "$CONFIG.bak" "$CONFIG"; echo "Restored original config.rs"' EXIT

mkdir -p "$RESULTS_DIR"

for size in "${SIZES[@]}"; do
    echo "=== Benchmarking ORD_CHUNK_SIZE = $size ==="

    # Patch config.rs — replace the non-small-chunks ORD_CHUNK_SIZE line
    sed -i '' "s/^pub(crate) const ORD_CHUNK_SIZE: usize = [0-9].*;/pub(crate) const ORD_CHUNK_SIZE: usize = $size;/" "$CONFIG"

    # Verify the patch took effect
    if ! grep -q "ORD_CHUNK_SIZE: usize = $size;" "$CONFIG"; then
        echo "ERROR: Failed to patch config.rs for size $size"
        exit 1
    fi

    # Run benchmarks, saving to size-specific directory
    cargo bench --bench ordmap -- \
        --save-baseline "node_size_$size" \
        --output-format bencher 2>&1 | tee "$RESULTS_DIR/node_size_${size}.txt"

    echo "=== Done: size $size ==="
    echo
done

echo "All benchmarks complete. Results in $RESULTS_DIR/"
echo "Compare with: cargo bench --bench ordmap -- --baseline node_size_16"
