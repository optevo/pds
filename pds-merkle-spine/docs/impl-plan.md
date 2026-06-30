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

**H.9 — Lazy Merkle root computation**

**Motivation:** `insert` and `remove` currently call `compute_merkle_root()` on every
mutation — a full O(N) BLAKE3 pass over all key-value pairs, giving O(N²) total cost
for an N-insert build. Benchmarked at n=1 000: 119 ms (vs 7 ms for a plain `HamtMap`
insert, a ~16× overhead). For workflows where callers only occasionally need the root
hash (e.g. periodic checkpoints, proof requests), this computation is wasted.

**Design:** Defer `compute_merkle_root` to the first call of `root_hash()`,
`root_hash_at()`, or `prove_inclusion*()` after a mutation. Cache the result in the
`VersionEntry` so subsequent calls are O(1) without recomputing.

**Implementation changes:**

1. Add `root_computed: bool` flag to `VersionEntry` (private). When `false`, the
   `id.root_hash` field is a placeholder (`SpineHash::default()`); when `true`,
   `id.root_hash` is valid.

2. `VersionHistory::push(snapshot)` — store `root_computed: false`, skip
   `compute_merkle_root`. The genesis version (`new()`) keeps its eager empty-HAMT hash
   (`hash_hamt_node(b"")`) since it's a constant O(1) computation.

3. `VersionHistory::ensure_root(seq)` — new private method: looks up the entry at
   `entries[seq]`, computes `compute_merkle_root(&entry.snapshot)` if not yet computed,
   caches the result in `entry.id.root_hash` and sets `root_computed = true`, returns
   the hash.

4. `VersionedHamt::root_hash()` — acquires the history mutex, calls
   `hist.ensure_root(self.current.seq)`. O(N) on first call, O(1) thereafter.

5. `VersionedHamt::root_hash_at(version)` — acquires the history mutex, calls
   `hist.ensure_root(version.seq)`. Same lazy behaviour.

6. `VersionedHamt::insert()` / `remove()` — remove the `compute_merkle_root` call.
   Call `hist.push(snapshot)` (no `merkle_root` argument). O(log N) total (no change
   in HAMT work; Merkle pass eliminated).

7. `VersionedHamt::diff()` — the shortcut `if from.root_hash == to.root_hash` is
   replaced with a lazy-aware version: acquire the mutex, call `ensure_root` for both
   sides, compare. For subsequent calls where both are cached, the shortcut is O(1).

8. `VersionedHamt::prove_inclusion_at()` — acquire the mutex, call `ensure_root`
   for the requested version before using the root hash in the proof.

9. `PartialEq` — acquires history mutexes for both sides, calls `ensure_root`, then
   compares. O(N) on first call, O(1) thereafter. Documented that PartialEq acquires
   a mutex and should not be called in tight loops before any `root_hash()` call.

**VersionId public API:** `VersionId.root_hash: SpineHash` stays as a public field.
Its value is `SpineHash::default()` (`[0u8;32]`) until `root_hash()` or
`root_hash_at()` is called on the owning `VersionedHamt`. Callers must not read
`version_id.root_hash` directly; always obtain it via `VersionedHamt::root_hash_at()`.
A doc-warning is added to the field.

**Expected performance impact (n=1 000):**

| Operation | Before | After | Note |
|-----------|--------|-------|------|
| insert (build from empty, no root calls) | 119 ms | ~7 ms | O(N²) → O(N log N) |
| root_hash() — first call after N inserts | 0 ms (eager) | ~7 ms | O(N) one-time |
| root_hash() — subsequent calls | O(1) | O(1) + lock | negligible overhead |
| prove_inclusion() — first call | O(log N) (hash eager) | O(N) + O(log N) | O(N) first-call hash |
| prove_inclusion() — subsequent calls | O(log N) | O(log N) + lock | negligible |

**Tests to add / update:**
- `lazy_root_not_computed_on_insert` — after N inserts, check `root_computed == false`
  in the latest entry (via test-only accessor on `VersionHistory`)
- `lazy_root_computed_on_root_hash_call` — `root_hash()` returns correct BLAKE3 hash;
  entry is marked `root_computed = true` afterwards
- `root_hash_matches_eager` — proptest: lazy root matches what the old eager path produced
  (computed by running `compute_merkle_root` directly on the same snapshot)
- `prove_inclusion_after_lazy_compute` — inclusion proofs verify correctly after lazy hash
- Update `insert_changes_root_hash` to call `root_hash()` explicitly (no longer implicit
  from insert)

**Acceptance:** `test.sh` green; `versioned_hamt_insert` bench at n=1 000 ≤ 10 ms.

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
