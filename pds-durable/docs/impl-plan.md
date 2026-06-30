# pds-durable — Implementation Plan

WAL-backed durability wrapper for `pds` heap-based collections.
Two durability modes: `Strict` (zero data loss, fsync per mutation) and `Relaxed`
(write-behind buffer, may lose mutations since last flush, native heap throughput).

**Dependency direction:** `pds-durable → pds` only. Never the reverse.

---

## Contents

- [Done](#done)
- [Current](#current)
- [Future](#future)
  - [D.0 — Workspace scaffold](#d0--workspace-scaffold)
  - [D.1 — WAL format and append](#d1--wal-format-and-append)
  - [D.2 — Recovery: load checkpoint + replay](#d2--recovery-load-checkpoint--replay)
  - [D.3 — `DurableMap<K,V,Strict>`](#d3--durablemapkvstrict)
  - [D.4 — `DurableMap<K,V,Relaxed>`](#d4--durablemapkvrelaxed)
  - [D.5 — Checkpoint and WAL compaction](#d5--checkpoint-and-wal-compaction)
  - [D.6 — Tests and proptest suite](#d6--tests-and-proptest-suite)
  - [D.7 — Additional collection types](#d7--additional-collection-types)
  - [D.8 — Benchmarks](#d8--benchmarks)
  - [D.9 — `TieredMap`: heap cache + folio backing store](#d9--tieredmap-heap-cache--folio-backing-store)
  - [D.10 — `TierPolicy` trait + `Pipelined` 3-tier preset](#d10--tierpolicy-trait--pipelined-3-tier-preset)

---

## Done {#done}

*Newest first.*

- **[2026-07-01] D.10 — `TierPolicy` trait + `Pipelined` 3-tier preset** —
  `src/policy.rs` created with sealed `TierPolicy` marker trait and four preset
  zero-sized types: `MemOnly`, `WriteBack`, `Pipelined`, `Durable`. Existing
  `TieredMap<K,V,Strict>` renamed to `TieredMap<K,V,Durable>` and
  `TieredMap<K,V,Relaxed>` renamed to `TieredMap<K,V,WriteBack>`; backward-compat
  type aliases (`Strict = Durable`, `Relaxed = WriteBack`) retained. `MemOnlyMap<K,V>`
  (wraps `std::collections::HashMap`, no disk; `into_persistent()` → O(N log N)
  freeze into `pds::HashMap`) and `PipelinedMap<K,V>` (3-tier: t0 `std::HashMap` →
  t1 `pds::HashMap` → t2 `VersionedHamt`) added as concrete exported types.
  `TieredConfig` extended with `commit_every: usize`. `lib.rs` updated with `policy`
  module and all new exports. 15 integration tests in `tests/pipelined.rs` (feature-
  gated on `tiered`), all passing. 5 new criterion benchmarks in `benches/bench.rs`.
  `cargo test --all-features` green (80 total tests); `cargo clippy -D warnings` clean.
  - Benchmark highlights (M5 Max, macOS tmpfs): `mem_only_insert` 57.9 µs / 1 000;
    `pipelined_insert` 3.49 ms / 1 000; `pipelined_commit` 3.28 ms; `pipelined_flush`
    4.35 ms; `policy_comparison/MemOnly` 5.37 µs / 10; `policy_comparison/Pipelined`
    3.21 ms / 10; `policy_comparison/WriteBack` 3.25 ms / 10; `policy_comparison/Durable`
    526 µs / 10.
  - Implementation note: Rust cannot give `TieredMap<K,V,MemOnly>` a different field
    layout than `TieredMap<K,V,WriteBack>` in the same generic struct; `MemOnlyMap` and
    `PipelinedMap` are separate concrete structs exported alongside `TieredMap`.
  - `V: Hash` required by `pds::HashMap::insert`; propagated to all bounds on `MemOnlyMap`
    and `PipelinedMap`.
  - Results recorded in `docs/baselines.md` (D.10 Pipeline section, 2026-07-01).

- **[2026-07-01] D.9 — `TieredMap`** — `src/tiered_map.rs` implemented; `Cargo.toml`
  updated with `pds-merkle-spine` + `folio-core` optional deps under `tiered` feature;
  `lib.rs` exports `TieredMap`, `TieredConfig`, `VersionId`; 22 unit tests (Strict + Relaxed),
  all passing; criterion benchmarks added (6 tiered scenarios, gated on `tiered` feature);
  `cargo fmt` clean; `cargo clippy -D warnings` clean; benchmark results recorded in
  `docs/baselines.md` (2026-07-01 baseline).
  - Benchmark highlights (N=1 000, macOS tmpfs, M5 Max): `tiered_strict_insert` 9.94 ms;
    `tiered_relaxed_insert` 7.73 ms; `tiered_relaxed_flush` (100 entries) 8.09 ms;
    `tiered_get_warm` (500 reads) 25.27 µs; `tiered_get_cold` (10 reads) 505 ns;
    `tiered_eviction` (max_front=100, N=1 000) 8.79 ms.
  - Key design note: `VersionedHamt::insert/remove` are immutable (return new `Self`) so
    `back` is reassigned on each mutation; `get` returns `Option<&V>` from front only —
    cold lookups via `get_or_fetch(&mut self)` which re-warms front.
  - `V: Hash` required by `pds::HashMap`; both mode `impl` bounds include it.
  - Benchmarks gated with `#[cfg(feature = "tiered")]` in `bench.rs`; separate
    `criterion_group!` macro invocations for `tiered` vs default builds.

- **[2026-07-01] D.8 — Benchmarks** — `benches/bench.rs` implemented with 6 criterion
  benchmarks: `durable_map_strict_insert`, `durable_map_relaxed_insert`,
  `durable_map_relaxed_insert_flush`, `durable_map_get`, `durable_map_checkpoint`,
  `heap_reference`; `insert_comparison` group comparing heap_only vs relaxed_no_flush vs
  strict_fsync; `[[bench]]` entry added to `Cargo.toml`; results recorded in
  `docs/baselines.md` (D.1–D.8 baseline, 2026-07-01).
  - Benchmark highlights (M5 Max, macOS tmpfs): strict insert 4.86 s / 1 000 entries;
    relaxed insert 389 µs / 1 000 entries; relaxed+flush 5.63 ms (100 + flush); get 49.3 µs.

- **[2026-07-01] D.7 — Additional collection types** — `src/durable_set.rs`
  (`DurableSet<T, Mode>`) and `src/durable_ordmap.rs` (`DurableOrdMap<K,V,Mode>`)
  implemented; each wraps its `pds` counterpart with the same WAL infrastructure;
  per-collection recovery functions (`recover_set`, `recover_ord_map`); Strict and
  Relaxed impls for each type; feature flags `durable-set`, `durable-ordmap` added to
  `Cargo.toml`; `lib.rs` exports gated on features; 4 unit tests (2 per type, covering
  both modes); all tests green.

- **[2026-07-01] D.6 — Tests and proptest suite** — `tests/durable_map.rs` implemented
  with 3 proptest suites (`strict_round_trip`, `relaxed_flush_semantics`,
  `checkpoint_recovery`) and 8 edge-case unit tests; all 11 integration tests pass;
  proptest default 256 cases; `proptest` added to `dev-dependencies`.

- **[2026-07-01] D.5 — Checkpoint and WAL compaction** — `src/checkpoint.rs`
  implemented; `write_checkpoint` serialises the collection and appends a `Checkpoint`
  entry with fsync; `compact_wal` writes a new WAL at `<path>.wal.tmp` containing only
  the checkpoint entry, then atomically renames it over the original; 2 unit tests
  covering compaction and post-checkpoint recovery; all green.

- **[2026-07-01] D.4 — `DurableMap<K,V,Relaxed>`** — Relaxed impl added to
  `src/durable_map.rs`; write-behind: mutations land in `inner` immediately and
  WAL entries buffer in `wal.pending`; `flush()` drains pending + writes `FlushMarker`
  + `sync_data()`; `pending_count()` exposes buffer depth; auto-flush/checkpoint via
  `DurableConfig`; data loss window documented in doc comments; 6 unit tests green.

- **[2026-07-01] D.3 — `DurableMap<K,V,Strict>`** — `src/durable_map.rs` implemented;
  `DurableConfig` struct; `Strict` and `Relaxed` zero-sized mode tags; `DurableMap`
  struct; Strict impl: `open` (open_or_create + recover_map), `insert`/`remove` (WAL
  append with fsync=true then heap mutation), `get`, `contains_key`, `len`, `is_empty`,
  `checkpoint`, `inner`, `maybe_checkpoint`; 5 unit tests green.

- **[2026-07-01] D.2 — Recovery: load checkpoint + replay** — `src/recovery.rs`
  implemented; `recover_map<K,V>` collects all valid entries, finds last `Checkpoint`,
  deserialises snapshot, replays subsequent Insert/Remove, truncates corrupt tail;
  `compute_entry_end` helper; 6 unit tests covering empty WAL, full replay, checkpoint
  with post-ops, remove idempotency, partial tail truncation, and CRC-corrupt entry;
  all green.

- **[2026-07-01] D.1 — WAL format and append** — `src/wal.rs` and `src/error.rs`
  implemented; magic `b"PDSW"`, version 1, per-entry CRC32C; `WalEntry` enum
  (Insert, Remove, Checkpoint, FlushMarker); `Wal` struct with `create`, `open`,
  `open_or_create`, `append` (fsync flag), `flush` (drain pending), `entries_from`
  (CRC-validated iterator), `file_len`, `truncate`; `DurableError` enum; 8 unit tests
  green.
  - Key fix: pds dep requires `features = ["std", "persist", "serde", "traits"]` —
    `serde` feature gates the actual Serialize/Deserialize impls (separate from `persist`).

- **[2026-07-01] Workspace scaffold** — crate directory, `Cargo.toml`, `src/lib.rs`,
  `docs/impl-plan.md` created; added to workspace `members` in root `Cargo.toml`.

---

## Current {#current}

*(nothing — D.10 complete; all planned items D.0–D.10 done)*

---

## Future {#future}

---

### D.0 — Workspace scaffold

**DONE** — see Done section above.

---

### D.1 — WAL format and append

Implement the append-only WAL file format in `src/wal.rs`.

#### WAL file layout

```
[File header]
  magic:   b"PDSW"    (4 bytes)
  version: 1u32 LE   (4 bytes)

[Entry stream — repeated until EOF]
  entry_len: u64 LE  — byte count of (entry_type + payload + crc32)
  entry_type: u8
  payload:   [entry_len - 5] bytes — postcard-encoded body
  crc32:     u32 LE — CRC32C of (entry_type byte ++ payload bytes)
```

#### Entry types

| Tag | Name | Payload |
|-----|------|---------|
| `0x01` | `Insert` | `postcard((key_bytes, value_bytes))` where `key_bytes = postcard(K)` etc. |
| `0x02` | `Remove` | `postcard(key_bytes)` |
| `0x03` | `Checkpoint` | `postcard(snapshot_bytes)` — full collection serialised |
| `0x04` | `FlushMarker` | empty — marks an explicit flush boundary in Relaxed mode |

#### `Wal` struct (in `src/wal.rs`)

```rust
pub(crate) struct Wal {
    file: File,
    path: PathBuf,
    /// Byte offset of the last valid Checkpoint entry (or 0 if none).
    last_checkpoint_offset: u64,
    /// Pending entries not yet flushed (Relaxed mode only; empty in Strict mode).
    pending: Vec<WalEntry>,
}

impl Wal {
    pub fn create(path: &Path) -> Result<Self, DurableError>
    pub fn open(path: &Path) -> Result<Self, DurableError>

    /// Appends an entry and optionally fsyncs (Strict) or buffers (Relaxed).
    pub fn append(&mut self, entry: &WalEntry, fsync: bool) -> Result<(), DurableError>

    /// Flushes all pending entries to disk (Relaxed mode).
    pub fn flush(&mut self) -> Result<(), DurableError>

    /// Iterates all valid entries from offset `since` onward.
    /// Stops at the first entry with an invalid CRC32 (partial write).
    pub fn entries_from(&self, offset: u64) -> impl Iterator<Item = Result<WalEntry, DurableError>>
}
```

#### CRC32C

Use `crc32c` crate (already in `Cargo.toml`). Compute over the raw bytes of
`entry_type ++ payload`. Verify on read; truncate the WAL at the first failed CRC.

#### Error type (`src/error.rs`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum DurableError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WAL corrupt at offset {offset}: {reason}")]
    Corrupt { offset: u64, reason: &'static str },
    #[error("serialisation error: {0}")]
    Serde(String),
    #[error("WAL version mismatch: expected 1, got {0}")]
    VersionMismatch(u32),
}
```

**Tests:**
- Write a few entries, re-open, verify they round-trip
- Truncate the file mid-entry, verify recovery stops cleanly at the last valid entry
- CRC mismatch: flip a byte in a payload, verify the iterator stops before it

**Acceptance:** `cargo test -p pds-durable` green.

---

### D.2 — Recovery: load checkpoint + replay

Implement recovery logic in `src/recovery.rs`.

```rust
/// Recovers a `pds::HashMap<K, V>` from a WAL file.
///
/// Returns the recovered collection plus the byte offset of the last fully
/// processed entry (for WAL truncation on partial tail writes).
pub fn recover_map<K, V>(wal: &Wal) -> Result<(pds::HashMap<K, V>, u64), DurableError>
where
    K: Clone + Hash + Eq + DeserializeOwned,
    V: Clone + DeserializeOwned,
```

Algorithm:
1. Scan the WAL for the last `Checkpoint` entry. Deserialise its payload to a
   `pds::HashMap<K, V>`.
2. Replay all `Insert` and `Remove` entries after the checkpoint offset.
3. Stop at the first `Corrupt` error (partial tail write); record `last_valid_offset`.
4. Truncate the WAL file to `last_valid_offset` (removes the partial entry).
5. Return the recovered collection.

If no Checkpoint entry exists, start from an empty `HashMap` and replay all entries.

**Tests:**
- Empty WAL: `recover_map` returns empty `HashMap`
- N inserts, no checkpoint: full replay produces correct map
- N inserts, one checkpoint midway, N more inserts: correct replay from checkpoint
- Partial tail entry: recovery stops cleanly; truncation removes partial bytes
- Corrupt payload (bad CRC mid-stream): recovery stops before the corrupt entry

**Acceptance:** All tests green; correct map contents verified against reference.

---

### D.3 — `DurableMap<K,V,Strict>`

Implement in `src/durable_map.rs`.

```rust
pub struct DurableMap<K, V, Mode = Strict> {
    inner: pds::HashMap<K, V>,
    wal: Wal,
    config: DurableConfig,
    checkpoint_counter: usize,
    _mode: PhantomData<Mode>,
}

/// Zero-sized mode tag: WAL entry is fsynced before mutation is applied.
/// Every successful `insert`/`remove` is durable on return.
pub struct Strict;

/// Zero-sized mode tag: mutations are buffered; call `flush()` to persist.
pub struct Relaxed;

pub struct DurableConfig {
    /// Auto-checkpoint every N mutations (0 = manual only).
    pub checkpoint_every: usize,
    /// Auto-flush every N mutations in Relaxed mode (0 = manual only).
    pub flush_every: usize,
    /// Compact the WAL when it exceeds this byte size (default: 64 MB).
    pub wal_max_bytes: u64,
}
```

#### `Strict` implementation

```rust
impl<K, V> DurableMap<K, V, Strict>
where
    K: Clone + Hash + Eq + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
{
    /// Opens an existing WAL or creates a new one.  Replays WAL on open.
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError>

    /// Inserts a key-value pair, syncing the WAL before returning.
    ///
    /// Time: O(log N) heap + O(1) WAL append + fsync.
    pub fn insert(&mut self, k: K, v: V) -> Result<Option<V>, DurableError>

    /// Removes a key, syncing the WAL before returning.
    pub fn remove(&mut self, k: &K) -> Result<Option<V>, DurableError>

    /// Returns a reference to the value for `k`, if present.  O(log N).
    pub fn get(&self, k: &K) -> Option<&V>

    /// Returns `true` if the map contains `k`.
    pub fn contains_key(&self, k: &K) -> bool

    /// Returns the number of entries.
    pub fn len(&self) -> usize

    /// Tests whether the map is empty.
    pub fn is_empty(&self) -> bool

    /// Snapshots the current state to the WAL and compacts old entries.
    pub fn checkpoint(&mut self) -> Result<(), DurableError>

    /// Returns a read-only reference to the underlying heap collection.
    pub fn inner(&self) -> &pds::HashMap<K, V>
}
```

The fsync sequence for `insert` in Strict mode:
1. Serialise `WalEntry::Insert { key_bytes, value_bytes }`.
2. `wal.append(&entry, fsync: true)` — writes and fsyncs the file.
3. `inner.insert(k, v)` — apply to heap collection.
4. If `checkpoint_counter` hits `config.checkpoint_every`, call `self.checkpoint()`.
5. Return previous value.

**Tests:**
- Open empty, insert N keys, close, re-open: all keys present
- insert → remove → re-open: key absent
- Simulate crash after WAL write but before heap apply: recovery restores the key
  (WAL is the ground truth)
- `checkpoint_every = 5`: checkpoint fires automatically after 5 inserts

**Acceptance:** `cargo test -p pds-durable` green; recovery test passes.

---

### D.4 — `DurableMap<K,V,Relaxed>`

Same struct, different mode. Add to `src/durable_map.rs`.

```rust
impl<K, V> DurableMap<K, V, Relaxed>
where
    K: Clone + Hash + Eq + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
{
    pub fn open(path: &Path, config: DurableConfig) -> Result<Self, DurableError>

    /// Inserts a key-value pair.  O(log N) heap + O(1) buffer append.
    /// The mutation is NOT yet durable; call `flush()` to persist.
    pub fn insert(&mut self, k: K, v: V) -> Option<V>

    /// Removes a key.  O(log N) heap + O(1) buffer append.
    pub fn remove(&mut self, k: &K) -> Option<V>

    pub fn get(&self, k: &K) -> Option<&V>
    pub fn contains_key(&self, k: &K) -> bool
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool

    /// Flushes all buffered mutations to the WAL file (no fsync guaranteed).
    /// Writes a `FlushMarker` entry.
    pub fn flush(&mut self) -> Result<(), DurableError>

    /// Returns the number of mutations buffered but not yet flushed.
    pub fn pending_count(&self) -> usize

    /// Checkpoints (includes an implicit flush + fsync).
    pub fn checkpoint(&mut self) -> Result<(), DurableError>

    pub fn inner(&self) -> &pds::HashMap<K, V>
}
```

The write-behind sequence for `insert` in Relaxed mode:
1. `inner.insert(k, v)` — apply to heap collection immediately (O(log N)).
2. Serialise `WalEntry::Insert` and push to `wal.pending` buffer (O(1)).
3. If `flush_every > 0` and `pending.len() >= flush_every`, call `self.flush()`.
4. If `checkpoint_every > 0` and `checkpoint_counter >= checkpoint_every`,
   call `self.checkpoint()`.

Data loss window: mutations in `wal.pending` at the time of crash are lost.
Recovery will restore the state as of the last successful flush or checkpoint.

**Tests:**
- insert N, no flush, simulate crash: on recovery, no inserts present (all lost)
- insert N, explicit flush(), simulate crash: all inserts present on recovery
- insert N, auto-flush (flush_every=3): flush fires at N=3, 6, 9 etc.

**Acceptance:** All tests green; data-loss window documented in doc comments.

---

### D.5 — Checkpoint and WAL compaction

Implement in `src/checkpoint.rs`.

**Checkpoint write:**
1. Serialise `self.inner` to bytes via `postcard::to_allocvec(&self.inner)`.
2. Append `WalEntry::Checkpoint { snapshot_bytes }` to the WAL with fsync.
3. Record `wal.last_checkpoint_offset` = current WAL position.
4. Compact the WAL (see below).
5. Reset `checkpoint_counter = 0`.

**WAL compaction:**
Compaction replaces the WAL with a new file containing only the latest Checkpoint
entry (everything before is redundant). Steps:
1. Write a new WAL file at `path.with_extension("wal.tmp")`.
2. Write the file header.
3. Copy the latest Checkpoint entry verbatim.
4. Atomically rename `.wal.tmp` → `.wal` (replaces the old file).
5. Update `wal.path` and `wal.last_checkpoint_offset = header_size`.

The rename is atomic on POSIX filesystems (Linux, macOS). Windows requires a
different strategy (not yet in scope).

**Auto-compaction:** if `wal.file.metadata()?.len() > config.wal_max_bytes` after a
checkpoint, run compaction automatically.

**Tests:**
- After N inserts + checkpoint: WAL file contains only the Checkpoint entry
- After checkpoint + M more inserts: recovery restores N + M keys
- Crash during compaction (rename not reached): old WAL is intact; recovery succeeds

**Acceptance:** All tests green; WAL file size after compaction == header + checkpoint entry.

---

### D.6 — Tests and proptest suite

Integration and property tests in `tests/durable_map.rs`.

**Proptest: `Strict` round-trip**
```
for N random (Insert(k, v) | Remove(k)) operations:
  apply to DurableMap<Strict>
  apply same to std::HashMap (reference)
  close DurableMap
  re-open DurableMap
  assert DurableMap.inner() == reference
```

**Proptest: `Relaxed` with random flush points**
```
for N random operations with random flush() calls:
  apply to DurableMap<Relaxed>
  track last-flush state in reference HashMap
  simulate crash (drop without flush)
  re-open: assert recovered state == reference at last flush
```

**Proptest: checkpoint + partial post-checkpoint mutations**
```
  random operations → checkpoint → random more operations → crash
  recover: assert state == post-checkpoint reference
```

**Edge cases:**
- Empty map: open, immediately close, re-open → empty map
- Single-entry map: insert one key, checkpoint, remove it, crash → key present on recovery
- Very large values (1 MB payload): WAL handles without truncation
- Unicode keys: serialisation round-trips correctly
- `len()` and `is_empty()` agree with `inner().len()` at all times

**Acceptance:** Proptest passes 256 cases (default); all edge case tests green.

---

### D.7 — Additional collection types

Same WAL format; different collection tag in the file header and Checkpoint payload.

| Type | Wraps | WAL entry body |
|------|-------|---------------|
| `DurableSet<T>` | `pds::HashSet<T>` | `Insert(elem)`, `Remove(elem)` |
| `DurableVector<T>` | `pds::Vector<T>` | `PushBack(elem)`, `PopBack`, `Set(idx, elem)` |
| `DurableOrdMap<K,V>` | `pds::OrdMap<K,V>` | same as DurableMap |

All share the same `Wal` and recovery infrastructure. Only the collection-specific
serialisation and mutation application differ.

Add each as a feature flag: `durable-set`, `durable-vector`, `durable-ordmap`.

**Acceptance:** Each type passes a basic round-trip test; feature flags compile cleanly.

---

### D.8 — Benchmarks

Add `pds-durable/benches/bench.rs` with criterion benchmarks.

**Benchmark scenarios:**

| Name | What it measures |
|------|-----------------|
| `durable_map_strict_insert` | Strict insert throughput: O(log N) heap + WAL fsync |
| `durable_map_relaxed_insert` | Relaxed insert throughput: heap only, no fsync |
| `durable_map_relaxed_insert_flush` | Relaxed: 100 inserts + 1 flush |
| `durable_map_get` | Pure get: O(log N) heap, no WAL involvement |
| `durable_map_checkpoint` | Full checkpoint: serialise + WAL write + compaction |
| `heap_reference` | Raw `pds::HashMap` insert for comparison |

**Expected results (n=1000, MemBackend baseline from pds-folio):**

| Operation | pds-folio HamtMap | DurableMap Strict | DurableMap Relaxed |
|-----------|------------------:|-----------------:|------------------:|
| insert (build from empty) | 6.95 ms | ~7 ms + fsync × N | ~7 ms (heap only) |
| get (n/2) | 720 ns | 720 ns | 720 ns |

Note: `durable_map_strict_insert` times are dominated by fsync latency on real disk
(~100 µs per fsync on NVMe). With `MemBackend` or `tmpfs`, fsync cost is near zero.
Benchmark on `MemBackend` first; add real-disk numbers in a separate run.

**Acceptance:** Bench compiles; results recorded in `docs/baselines.md`.

---

### D.9 — `TieredMap`: heap cache + merkle-spine backing store

**DONE** — see Done section above.

Replaces the flat WAL with `pds-merkle-spine` (which wraps `pds-folio`) as the
durable backing store. The heap collection becomes an L1 write-back cache;
merkle-spine is the crash-safe, versioned L2 store. Each `flush()` produces a
new version with a Merkle root — a complete audit trail of heap state over time.

Using merkle-spine as the default (rather than bare folio) adds versioning for
free: folio is already the storage layer underneath, and the HAMT structural
sharing means versioned snapshots cost nothing extra.

#### Architecture

```
TieredMap<K, V, Mode>   (default backend: VersionedHamt from pds-merkle-spine)
  ├── front: pds::HashMap<K, V>                     — hot tier; RAM-bounded cache
  ├── dirty: HashSet<K>                             — entries not yet flushed to back
  ├── eviction_queue: VecDeque<K>                   — approximate LRU order
  └── back:  pds_merkle_spine::VersionedHamt<K, V>  — cold tier; versioned, crash-safe, unbounded
```

Recovery is O(1): open the VersionedHamt at the latest version; front starts
empty and warms lazily on access. No replay step needed.

Each `flush()` in Relaxed mode (or each mutation in Strict mode) creates a new
`VersionId` in the backing VersionedHamt. Historical versions remain queryable.

#### Mode semantics

| Mode | Write sequence | Recovery | Versioning |
|------|---------------|----------|------------|
| `Strict` | Write to `back` (new version per mutation) then `front` | Latest version → open; front warms on demand | One version per mutation |
| `Relaxed` | Write to `front` only; `flush()` pushes dirty entries to `back` as one new version | Latest version → open; state = last flush | One version per flush |

#### `TieredConfig`

```rust
pub struct TieredConfig {
    /// Evict LRU entries from front when it exceeds this count (0 = unlimited).
    pub max_front_entries: usize,
    /// Auto-flush in Relaxed mode every N mutations (0 = manual only).
    pub flush_every: usize,
    /// Retain this many historical versions in the backing store (0 = all).
    pub max_versions: usize,
}

impl Default for TieredConfig {
    fn default() -> Self {
        Self { max_front_entries: 0, flush_every: 0, max_versions: 0 }
    }
}
```

#### Public API (`src/tiered_map.rs`)

```rust
impl<K, V> TieredMap<K, V, Strict> {
    /// Opens or creates the TieredMap at `path`. Front starts empty; back opens
    /// the VersionedHamt at its latest version.
    pub fn open(path: &Path, config: TieredConfig) -> Result<Self, DurableError>
    /// Writes to back (new version) then front. Durable on return.
    pub fn insert(&mut self, k: K, v: V) -> Result<Option<V>, DurableError>
    pub fn remove(&mut self, k: &K) -> Result<Option<V>, DurableError>
    /// Front first; falls through to back on miss.
    pub fn get(&self, k: &K) -> Option<&V>
    pub fn contains_key(&self, k: &K) -> bool
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool
    pub fn front(&self) -> &pds::HashMap<K, V>
    /// Returns the VersionId of the latest committed version in the back store.
    pub fn latest_version(&self) -> Option<VersionId>
}

impl<K, V> TieredMap<K, V, Relaxed> {
    pub fn open(path: &Path, config: TieredConfig) -> Result<Self, DurableError>
    /// Writes to front only. Not yet durable; call flush() to persist.
    pub fn insert(&mut self, k: K, v: V) -> Option<V>
    pub fn remove(&mut self, k: &K) -> Option<V>
    /// Pushes all dirty entries to back as a single new version. Returns the
    /// new VersionId.
    pub fn flush(&mut self) -> Result<VersionId, DurableError>
    pub fn pending_count(&self) -> usize   // dirty.len()
    pub fn get(&self, k: &K) -> Option<&V>
    pub fn contains_key(&self, k: &K) -> bool
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool
    pub fn front(&self) -> &pds::HashMap<K, V>
    pub fn latest_version(&self) -> Option<VersionId>
}
```

#### Flush produces a version

In Relaxed mode, `flush()` batches all dirty entries into the VersionedHamt as a
single version. The returned `VersionId` contains the Merkle root over the full
collection state at that point — usable for auditing, inclusion proofs, or
point-in-time reads via `back.root_hash_at(version_id)`.

In Strict mode, each mutation creates its own version. Version history is an
append-only log of every individual change.

#### LRU eviction (when `max_front_entries > 0`)

When `front.len() > max_front_entries` after an insert:
1. Pop the head of `eviction_queue` (oldest key).
2. If the key is in `dirty`, write it to `back` immediately (a single-entry version).
3. Remove from `front` and `dirty`.

Future reads that miss `front` fall through to `back` at its latest version.
Not automatically re-warmed (avoids cache thrashing on sequential scans).

Approximate LRU via `VecDeque<K>` is sufficient — push inserted keys to the back,
pop from the front when over capacity.

#### Cargo.toml addition

```toml
[dependencies]
pds-merkle-spine = { path = "../pds-merkle-spine", optional = true }

[features]
tiered = ["dep:pds-merkle-spine"]
```

`TieredMap` is only compiled when the `tiered` feature is enabled. pds-merkle-spine
already depends on pds-folio; no need to list pds-folio separately.

#### Benchmarks (add to `benches/bench.rs`)

| Name | What it measures |
|------|-----------------|
| `tiered_strict_insert` | Strict insert: back version per mutation + front write |
| `tiered_relaxed_insert` | Relaxed insert: front write only — zero back involvement |
| `tiered_relaxed_flush` | 100 inserts + one flush (one new version) |
| `tiered_get_warm` | Get for a front-cached key |
| `tiered_get_cold` | Get for an evicted key (back read at latest version) |
| `tiered_eviction` | Insert beyond `max_front_entries`; eviction + dirty-flush path |

Compare `tiered_relaxed_insert` against `heap_reference` — the difference should
be near zero (both write only to the in-memory HAMT). Compare `tiered_relaxed_flush`
against `durable_map_relaxed_insert_flush` from D.8.

**Prerequisite:** D.1–D.8 complete. pds-merkle-spine `VersionedHamt<K,V>` must
support `insert`, `remove`, `get`, and expose `VersionId` (all present as of H.9).

**Acceptance:**
- `cargo test -p pds-durable --features tiered` green
- Relaxed insert throughput ≥ `durable_map_relaxed_insert` (no folio touch in fast path)
- Strict insert throughput within 2× of folio's own insert (no double-write overhead)
- Eviction test: `front.len()` never exceeds `max_front_entries + 1`
- Cold-get test: evicted key recovered correctly from `back`
- Results recorded in `docs/baselines.md`

---

### D.10 — `TierPolicy` trait + `Pipelined` 3-tier preset

Introduces a sealed `TierPolicy` marker trait so the storage tier of `TieredMap`
is a swappable type parameter with no runtime overhead (statically dispatched).
Adds two new presets — `MemOnly` and `Pipelined` — alongside the existing
`Relaxed`/`Strict` presets (renamed to `WriteBack`/`Durable` for clarity).
The `Pipelined` preset is the 3-tier pipeline: transient → heap → merkle-spine.

**Prerequisite:** D.9 complete.

#### Sealed `TierPolicy` trait (`src/policy.rs`)

```rust
mod sealed { pub trait Sealed {} }

/// Marker trait for `TieredMap` storage presets.
/// Implemented only by the four preset types in this module.
pub trait TierPolicy: sealed::Sealed {}

/// Tier 0 only — `pds` transient in-place HashMap, no disk backing.
/// Fastest possible mutations; no durability.
pub struct MemOnly;

/// Tier 1 + Tier 2 — heap `pds::HashMap` with merkle-spine write-behind.
/// Same as D.9 `Relaxed`. Mutations are heap-speed; `flush()` persists.
pub struct WriteBack;

/// Tier 0 + Tier 1 + Tier 2 — transient buffer → heap snapshot → merkle-spine.
/// `commit()` freezes the transient into the heap (O(1)).
/// `flush()` pushes dirty heap entries to merkle-spine (write-behind).
pub struct Pipelined;

/// Write-through — every mutation goes directly to merkle-spine.
/// Same as D.9 `Strict`. Zero data loss; slower mutation.
pub struct Durable;

impl sealed::Sealed for MemOnly {}
impl sealed::Sealed for WriteBack {}
impl sealed::Sealed for Pipelined {}
impl sealed::Sealed for Durable {}

impl TierPolicy for MemOnly {}
impl TierPolicy for WriteBack {}
impl TierPolicy for Pipelined {}
impl TierPolicy for Durable {}
```

Rename existing `TieredMap<K,V,Strict>` → `TieredMap<K,V,Durable>` and
`TieredMap<K,V,Relaxed>` → `TieredMap<K,V,WriteBack>` for consistency.
`Strict` and `Relaxed` remain as type aliases pointing to `Durable` and
`WriteBack` for backward compatibility within the crate (no public release yet).

#### `TieredMap<K, V, MemOnly>`

Struct holds only a `pds::TransientHashMap<K, V>`. No `back` field.

```rust
impl<K, V> TieredMap<K, V, MemOnly> {
    pub fn new() -> Self
    pub fn insert(&mut self, k: K, v: V) -> Option<V>   // O(log N) in-place
    pub fn remove(&mut self, k: &K) -> Option<V>
    pub fn get(&self, k: &K) -> Option<&V>
    pub fn contains_key(&self, k: &K) -> bool
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool
    /// Freezes the transient into a persistent `pds::HashMap`. O(1).
    pub fn into_persistent(self) -> pds::HashMap<K, V>
}
```

No `path` argument — `MemOnly` has no disk backing. The fastest storage preset.

#### `TieredMap<K, V, Pipelined>` — 3-tier pipeline

```rust
struct TieredMap<K, V, Pipelined> {
    t0: pds::TransientHashMap<K, V>,   // mutable write buffer
    t1: pds::HashMap<K, V>,            // last committed snapshot
    t2: VersionedHamt<K, V>,           // durable replica (merkle-spine)
    dirty: HashSet<K>,                 // keys mutated since last flush()
    config: TieredConfig,
}
```

**Mutations** go to `t0` only (in-place, O(log N), zero structural sharing overhead).

**`commit()`** — freezes t0 into t1:
1. `self.t1 = mem::replace(&mut self.t0, TransientHashMap::new()).into_persistent()`
   — O(1); produces a new persistent snapshot; t0 becomes a fresh empty transient.
2. All keys that were in the old t0 are now dirty w.r.t. t2 — they are already in
   `self.dirty` (tracked on every `insert`/`remove`).

**`flush()`** — pushes dirty entries from t1 to t2:
1. For each key in `dirty`: look up in `t1`; if present → `t2 = t2.insert(k, v)`;
   if absent → `t2 = t2.remove(k)`.
2. Clear `dirty`. Return the new `VersionId`.

**`get`** — waterfall: t0 → t1 → t2 (fallthrough). Keys written to t0 but not
yet committed are visible in t0. Keys committed to t1 but not yet flushed are
visible in t1. Evicted or pre-open keys are in t2.

**Auto-commit / auto-flush** via `TieredConfig`:

```rust
pub struct TieredConfig {
    pub max_front_entries: usize,   // evict from t0 via commit() when exceeded (0 = unlimited)
    pub commit_every: usize,        // auto-commit every N mutations (0 = manual)
    pub flush_every: usize,         // auto-flush every N commits (0 = manual)
    pub max_versions: usize,        // retain N historical versions in t2 (0 = all)
}
```

**Data loss windows:**
- Mutations in t0 not yet committed → lost on crash
- Committed entries in t1 not yet flushed → lost on crash
- t2 (VersionedHamt) is always crash-safe

**Recovery on `open()`:**
- Open t2 (VersionedHamt) from disk at its latest version
- t1 = empty persistent HashMap (warm lazily from t2 on access)
- t0 = fresh empty transient

```rust
impl<K, V> TieredMap<K, V, Pipelined>
where K: Clone + Hash + Eq + Serialize + DeserializeOwned,
      V: Clone + Hash + Serialize + DeserializeOwned,
{
    pub fn open(path: &Path, config: TieredConfig) -> Result<Self, DurableError>
    pub fn insert(&mut self, k: K, v: V) -> Option<V>
    pub fn remove(&mut self, k: &K) -> Option<V>
    pub fn get(&self, k: &K) -> Option<&V>
    pub fn contains_key(&self, k: &K) -> bool
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool
    /// Freezes t0 into t1. O(1). Does NOT flush to disk.
    pub fn commit(&mut self)
    /// Pushes dirty t1 entries to t2 (new version). Returns the VersionId.
    pub fn flush(&mut self) -> Result<VersionId, DurableError>
    /// commit() + flush() in one call.
    pub fn commit_and_flush(&mut self) -> Result<VersionId, DurableError>
    pub fn pending_commit(&self) -> usize   // mutations in t0 since last commit
    pub fn pending_flush(&self) -> usize    // dirty.len()
    pub fn latest_version(&self) -> Option<VersionId>
    pub fn t0(&self) -> &pds::TransientHashMap<K, V>
    pub fn t1(&self) -> &pds::HashMap<K, V>
}
```

#### Tests (`tests/pipelined.rs`, feature-gated on `tiered`)

- `MemOnly`: insert N, `into_persistent()` contains all keys
- `Pipelined`: insert N → crash → re-open: empty (t0 never committed)
- `Pipelined`: insert N → `commit()` → crash → re-open: empty (t1 never flushed)
- `Pipelined`: insert N → `commit()` → `flush()` → crash → re-open: N keys in t2
- `Pipelined`: `commit_and_flush()` round-trip proptest (N random ops)
- `Pipelined`: `get` waterfall — key in t0 / t1 / t2 only, correct value returned
- `Pipelined`: auto-commit (`commit_every = 10`) fires at 10, 20, 30 mutations
- `Pipelined`: `latest_version()` advances on each `flush()`
- `WriteBack` renamed to `WriteBack` (was `Relaxed`) — existing tests renamed, behaviour unchanged
- `Durable` renamed (was `Strict`) — existing tests renamed, behaviour unchanged

#### Benchmarks (add to `benches/bench.rs`, `tiered` feature)

| Name | What it measures |
|------|-----------------|
| `pipelined_insert` | t0 in-place insert — should be fastest of all presets |
| `pipelined_commit` | `commit()` — O(1) transient → persistent |
| `pipelined_flush` | 100 commits + `flush()` — dirty entries → t2 |
| `mem_only_insert` | `MemOnly` insert — baseline for t0 speed |
| `policy_comparison` | Insert N=1000 across all 4 presets side by side |

Expected: `MemOnly` ≈ `Pipelined` insert (both hit only t0/transient). Both should
outperform `WriteBack` insert (which hits persistent heap pds::HashMap).

**Acceptance:**
- `cargo test -p pds-durable --features tiered` green (all presets)
- `pipelined_insert` faster than `tiered_relaxed_insert` from D.9 baseline
- `pipelined_commit` < 1 µs (O(1) operation)
- All 4 presets produce correct values on re-open after crash simulation
- Results appended to `docs/baselines.md`
- `TierPolicy` is sealed (compile error if user tries to implement it externally)

---

## Design decisions

### Why postcard for WAL entries?

postcard is already a pds-durable dependency (via pds's `persist` feature). It
produces compact binary output (no overhead per key–value pair) and is `no_std`
compatible. Alternative: bincode — similar performance, slightly worse for variable-
length types. Alternative: serde_json — human-readable but 3–5× larger output.
Postcard wins on size + speed for this use case.

### Why CRC32C and not BLAKE3?

WAL integrity checking needs to detect accidental corruption (torn writes, bit
flips), not adversarial tampering. CRC32C is hardware-accelerated on all modern
x86_64 and Apple Silicon cores (~1 cycle per byte). BLAKE3 is also fast but overkill
for integrity-only use. CRC32C is the standard choice for WAL formats (RocksDB,
LevelDB, SQLite all use CRC32/CRC32C).

### Why not mmap the WAL?

The WAL is append-only. mmap requires a pre-allocated file region; appending via
mmap requires `ftruncate` + remap on growth, which is fragile and platform-specific.
Sequential `write` + `fsync` is simpler, portable, and sufficient for WAL throughput
requirements.

### Why CRC over each entry, not the whole file?

Per-entry CRC allows recovery to identify the exact point of corruption and replay
all entries before it. A file-level CRC only tells you something is wrong, not where.

---

## Design decisions (continued)

### Write-behind in `TieredMap<Relaxed>` — minimal hot-path overhead

`Relaxed` mode is a write-behind cache: mutations land in `front` only; the folio
write is deferred to `flush()`. The hot path is:

```
insert() → front.insert() + dirty.insert()           O(log N) heap, zero I/O
flush()  → per dirty entry: back.insert(k, v)        amortised over N mutations
```

Per-mutation overhead in Relaxed mode is identical to a bare `pds::HashMap` — no
fsync, no page writes. The durability cost is paid at flush boundaries and amortised
across all mutations since the last flush. Data loss window = mutations since last flush.

### Why merkle-spine as the default backing store (not bare folio)?

merkle-spine wraps folio and adds versioning + Merkle roots at essentially zero
marginal cost — HAMT structural sharing means each new version shares almost all
nodes with the previous one. Using merkle-spine as the default means every
`flush()` or Strict mutation produces a `VersionId` with a Merkle root,
giving a full audit trail and enabling inclusion proofs with no extra work.

A bare-folio variant (`TieredMap<K,V,Mode,FolioBackend>`) could be added later
if the VersionedHamt overhead is measurable — but benchmark first.

### Why `TieredMap` is not a WAL at all

The flat WAL in `DurableMap` is a log of deltas: Insert(k,v), Remove(k), etc.
`TieredMap`'s backing store is a full persistent replica of the heap collection,
updated in write-behind fashion. Each flush writes the current dirty state as a
new HAMT version. Recovery opens the latest version directly — no replay needed.
This is cleaner, faster to recover, and adds history for free.

### Why keep `DurableMap` (WAL) alongside `TieredMap` (merkle-spine)?

For small datasets that always fit in RAM, `DurableMap` with a flat WAL has lower
per-mutation overhead in Strict mode (sequential append + fsync vs. HAMT page
writes). `TieredMap` is the right choice when data may exceed RAM, when versioned
history is useful, or when the write-behind Relaxed mode is the primary path.
Both are useful; neither supersedes the other.

### Approximate vs exact LRU

Exact LRU requires a doubly-linked list threaded through the hash map — complex and
not justified for a cache whose primary purpose is memory bounding, not optimal
hit rate. A `VecDeque<K>` eviction queue is O(1) push/pop and sufficient to keep
`front.len()` bounded. Entries are evicted FIFO within each "generation"; close
enough to LRU for the expected access patterns.

---

## Dependency map

```
pds (heap collections)  ←── pds-durable (WAL wrapper, D.1–D.8)
pds-folio               ←┐
pds-merkle-spine        ←─┴─ pds-durable (tiered backend, D.9, feature = "tiered")
     ↑
  none of the above may depend on pds-durable
```
