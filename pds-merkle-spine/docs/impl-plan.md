# pds-merkle-spine — Implementation Plan

Phased implementation plan for `pds-merkle-spine`: versioned, Merkle-verified
persistent hash map combining `pds-folio` and `merkle-spine`.

---

## Contents

- [Done](#done)
- [Current](#current)
- [Future](#future)

---

## Done {#done}

*Newest first.*

- **[2026-07-01] H.9 — Lazy Merkle root computation.**

  Deferred `compute_merkle_root` from `insert`/`remove` to the first call of
  `root_hash()`, `root_hash_at()`, or `prove_inclusion*()`.  Result cached in
  `VersionEntry` (`root_computed: bool` flag).  `VersionHistory::ensure_root(seq)`
  added as the single computation/cache point.  All callers updated to call
  `ensure_root` through the history lock.

  `VersionId.root_hash` now contains a placeholder (`[0u8;32]`) until the root is
  computed; doc-warning added to the field.  `VersionedHamtError::VersionNotFound(u64)`
  added to support `ensure_root` error reporting.  `get_id` helper removed (no longer
  needed).  `get_snapshot` rewritten to use O(1) index lookup by seq instead of
  `Iterator::find`.

  **Performance (n=1 000 inserts, no root_hash() calls):**

  | Before (eager) | After (lazy) | Change   |
  |----------------|--------------|----------|
  | 119.9 ms       | 18.7 ms      | **−84%** |

  Full before/after table in `docs/baselines.md`.

  **Tests added:** `lazy_root_not_computed_after_insert`,
  `lazy_root_correct_after_root_hash_call`, `prove_inclusion_lazy`,
  `version_not_found_error_formats`.  Updated: `insert_changes_root_hash` (added
  explanatory comment).

  **Acceptance:** 49 unit + integration tests green (38 unit + 11 integration).
  `test.sh` full quality gate passed.  n=1 000 insert: 18.7 ms (O(N log N) confirmed).

- **[2026-07-01] H.0–H.8 — `VersionedHamt` full implementation.**
  All H-phase items completed together.

  **H.0 — Scaffold.**
  Workspace member added: `pds-merkle-spine` in root `Cargo.toml`.
  `Cargo.toml` with deps: `pds-folio` (path), `merkle-spine` (path),
  `pds` (workspace, traits feature), `folio-core` (path), `serde`, `postcard`,
  `thiserror`. Dev-deps: `proptest`.
  `src/lib.rs` with `#![deny(unsafe_code)]`, `#![deny(missing_docs)]`,
  `#![warn(unreachable_pub)]`.

  **H.1 — `VersionId` type.**
  `pub struct VersionId { pub seq: u64, pub root_hash: SpineHash }` where
  `SpineHash = [u8; 32]`. Derives `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`,
  `Hash`. Monotonic `seq` incremented on every mutation; `root_hash` is the
  BLAKE3 Merkle root over all key-value pairs.

  **H.2 — `VersionedHamt` struct.**
  `pub struct VersionedHamt<K, V, C, B>` containing:
  - `data: HamtMap<K, V, C, B>` — current mutable snapshot
  - `current: VersionId` — current version handle
  - `history: Arc<Mutex<VersionHistory<K, V, C, B>>>` — shared append-only log

  Manual `Clone` impl avoids spurious bounds on `C`. Manual `Debug` impl avoids
  requiring `C: Debug`.

  `VersionHistory<K, V, C, B>` stores `Vec<VersionEntry<…>>` where each entry
  holds a full `HamtMap` clone (O(1)) to keep folio refcounts permanently alive.
  Storing only page IDs was tried and discarded: pages are freed when the owning
  `HamtMap` is dropped; historical `get_at` would then silently return `None`.

  **H.3 — Basic CRUD: `insert`, `remove`, `get`, `contains_key`, `len`, `iter`.**
  `insert` returns a new `VersionedHamt` with a new `VersionId` (seq+1, new
  Merkle root). `remove` returns `(VersionedHamt, Option<V>)`. Both append to
  the shared history. `get`, `contains_key`, `len`, `iter` delegate to `self.data`.

  **H.4 — Historical access: `get_at`, `root_hash_at`.**
  `get_at(version, key)` — looks up the stored `HamtMap` snapshot in the history
  log (by `version.seq`) and calls `snapshot.get(key)`. Holds the Mutex lock for
  the duration. O(1) history lookup + O(log N) HAMT get.
  `root_hash_at(version)` — returns the `root_hash` field of the stored
  `VersionEntry`.

  **H.5 — Version checkout and branching: `checkout`.**
  `checkout(version)` — clones the stored `HamtMap` snapshot (O(1)) and returns
  a new `VersionedHamt` with that snapshot as `data` and `current = version`.
  The new instance shares the same `Arc<Mutex<VersionHistory>>` so branching
  from a historical point still appends to the shared log.

  **H.6 — Structural diff: `diff(from, to)`.**
  `diff(from, to)` returns `Vec<DiffEntry<K, V>>` comparing the snapshot at
  `from` with the snapshot at `to`. Iterates both snapshots, collects entries
  into `HashMap`s, then computes the symmetric difference. O(changed × log N)
  overall.
  `DiffEntry<K, V>` enum: `Inserted { key, value }`, `Removed { key, value }`,
  `Updated { key, old_value, new_value }`.

  **H.7 — Merkle proofs: `prove_inclusion`, `prove_inclusion_at`, `verify_proof`.**
  `MerkleProof { root_hash, key_hash, value_hash, siblings: Vec<[u8; 32]> }`.
  `prove_inclusion(key)` — builds an inclusion proof for `key` in the current
  snapshot. Returns `None` for absent keys.
  `prove_inclusion_at(version, key)` — same but at a historical snapshot.
  `verify_proof(root, key, value, proof)` — verifies that the proof's root hash
  matches `root` and that `key_hash` and `value_hash` are consistent with the
  stored proof hashes.

  **H.8 — `pds::traits` impls.**
  `PersistentCollection` — marker impl.
  `PersistentMap<K, V>` — `get_cloned`, `insert`, `remove`, `len`, `contains_key`.
  `VersionedPersistentMap<K, V>` — `version`, `get_at`, `checkout`, `diff`.
  `MerklePersistentMap<K, V>` — `root_hash`, `prove_inclusion`, `verify_proof`.

  Tests:
  - 30 unit tests (`src/versioned_hamt.rs`): all API methods, trait impls, edge cases.
  - 11 integration tests (`tests/versioned_hamt_integration.rs`): large insert/remove
    sequences, non-adjacent diff, snapshot isolation, Merkle proof round-trips,
    cross-crate trait usage.
  - 2 proptest property tests (20 cases each): historical value correctness,
    diff inverse-of-mutations.
  - 41/41 tests green; `cargo fmt --check` clean; `cargo clippy -D warnings` clean;
    full workspace `test.sh` (9 steps) passes.

---

## Current {#current}

*Nothing in progress.*

---

## Future {#future}

---

## Dependency map

```
pds-folio (G.0–G.15) ─────────────────────────────────┐
merkle-spine ──────────────────────────────────────────┤
pds (traits feature) ──────────────────────────────────┤
                                                        ↓
H.0 (scaffold) → H.1 (VersionId) → H.2 (struct)
              → H.3 (CRUD) → H.4 (historical) → H.5 (checkout)
              → H.6 (diff) → H.7 (proofs) → H.8 (traits)
```
