// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

/// The branching factor of RRB-trees
#[cfg(feature = "small-chunks")]
pub(crate) const VECTOR_CHUNK_SIZE: usize = 4;
#[cfg(not(feature = "small-chunks"))]
pub(crate) const VECTOR_CHUNK_SIZE: usize = 64;

/// The branching factor of B-trees
// Value of 6 chosen improve test coverage, specifically
// so that both deletion node merging and rebalancing are tested.
// Must be an even number!
#[cfg(feature = "small-chunks")]
pub(crate) const ORD_CHUNK_SIZE: usize = 6;
// Value of 32 chosen based on Apple Silicon (M5 Max, 128-byte cache lines) benchmarks across
// sizes 16/24/32/48 — see DEC-017 in docs/decisions.md for full data.
//
// vs size 16: lookup 8-21% faster (larger collections benefit more from fewer tree levels),
// mutable ops 10-37% faster, iteration 10-12% faster. Persistent single-insert/remove is
// 15-25% slower (more bytes copied per path-copy), but the breakeven is only ~6-30 lookups
// per insert depending on collection size — easily exceeded in most real workloads.
// Size 48 shows diminishing lookup returns with accelerating persistent-op regression.
#[cfg(not(feature = "small-chunks"))]
pub(crate) const ORD_CHUNK_SIZE: usize = 32;

/// The level size of HAMTs, in bits
/// Branching factor is 2 ^ HashLevelSize.
// The smallest supported value is 3 currently, as the small node
// (half the size of a full node) requires at least 4 slots.
#[cfg(feature = "small-chunks")]
pub(crate) const HASH_LEVEL_SIZE: usize = 3;
// Value of 5 (branching factor 32) chosen based on performance analysis. Smaller value 4
// (branching factor 16) improves immutable inserts by 16-25% but suffers severe lookup
// regressions. Under typical workloads (e.g. 70% lookup, 25% small mutation, 5% bulk mutation),
// 5 is arguably better overall.
#[cfg(not(feature = "small-chunks"))]
pub(crate) const HASH_LEVEL_SIZE: usize = 5;

/// Width of Merkle hashes in bits. Must be ≥ 64 for positive equality
/// shortcuts to be safe (collision probability ~2⁻⁶⁴). See DEC-023.
pub(crate) const MERKLE_HASH_BITS: usize = 64;

/// Minimum hash width (bits) for Merkle-based positive equality.
/// When `MERKLE_HASH_BITS < MERKLE_POSITIVE_EQ_MIN_BITS`, positive
/// equality checks are disabled — only negative checks (different
/// hash ⇒ definitely different) remain safe.
///
/// **Do not set below 64.** At 32 bits the birthday-bound collision
/// probability is ~1/65k entries — far too high for correctness.
/// For super-conservative deployments, increase to 128 (requires
/// widening Merkle hashes to u128).
pub(crate) const MERKLE_POSITIVE_EQ_MIN_BITS: usize = 64;
