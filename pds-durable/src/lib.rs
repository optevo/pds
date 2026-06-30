//! Durable persistence for `pds` heap collections.
//!
//! Wraps the heap-based `pds` collections with a write-ahead log (WAL) to
//! provide crash-safe durability without sacrificing in-memory performance.
//!
//! # Durability modes
//!
//! Two modes are available as zero-sized type parameters:
//!
//! | Mode | Data loss on crash | Write latency | Use case |
//! |------|-------------------|--------------|----------|
//! | [`Strict`] | None | fsync per mutation | Correct-by-default |
//! | [`Relaxed`] | Mutations since last flush | Native heap speed | High-throughput / cache workloads |
//!
//! # Architecture
//!
//! ```text
//! DurableMap<K, V, Mode>
//!   ├── inner: pds::HashMap<K, V>     — heap collection; O(log N) insert, O(1) read
//!   └── wal:   Wal                    — append-only WAL file
//!         ├── Strict: fsync after every entry
//!         └── Relaxed: buffer in memory; flush on flush() or auto-threshold
//! ```
//!
//! # Recovery
//!
//! On `open()`, the WAL is replayed:
//! 1. Scan for the last valid `Checkpoint` entry and deserialise it.
//! 2. Replay all valid `Insert` / `Remove` entries after the checkpoint.
//! 3. Partial (corrupt) entries at the tail are silently truncated.
//!
//! # See also
//!
//! - `docs/impl-plan.md` — phased implementation plan (D.0–D.8)
