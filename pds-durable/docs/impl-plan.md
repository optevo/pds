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

---

## Done {#done}

*Newest first.*

- **[2026-07-01] D.9 — `TieredMap`** — `src/tiered_map.rs` implemented; `Cargo.toml`
  updated with `pds-merkle-spine` + `folio-core` optional deps under `tiered` feature;
  `lib.rs` exports `TieredMap`, `TieredConfig`, `VersionId`; 22 unit tests (Strict + Relaxed),
  all passing; criterion benchmarks added (6 tiered scenarios, gated on `tiered` feature);
  `cargo fmt` clean; `cargo clippy -D warnings` clean.
  - Key design note: `VersionedHamt::insert/remove` are immutable (return new `Self`) so
    `back` is reassigned on each mutation; `get` returns `Option<&V>` from front only —
    cold lookups via `get_or_fetch(&mut self)` which re-warms front.
  - `V: Hash` required by `pds::HashMap`; both mode `impl` bounds include it.
  - Benchmarks gated with `#[cfg(feature = "tiered")]` in `bench.rs`; separate
    `criterion_group!` macro invocations for `tiered` vs default builds.

- **[2026-07-01] Workspace scaffold** — crate directory, `Cargo.toml`, `src/lib.rs`,
  `docs/impl-plan.md` created; added to workspace `members` in root `Cargo.toml`.

---

## Current {#current}

*(nothing — begin with D.1)*

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
