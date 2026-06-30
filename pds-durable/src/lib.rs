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
//! - `docs/impl-plan.md` — phased implementation plan (D.0–D.10)

pub mod checkpoint;
pub mod durable_map;
pub mod error;
pub(crate) mod recovery;
pub(crate) mod wal;

#[cfg(feature = "durable-set")]
pub mod durable_set;

#[cfg(feature = "durable-ordmap")]
pub mod durable_ordmap;

#[cfg(feature = "tiered")]
pub mod policy;

#[cfg(feature = "tiered")]
pub mod tiered_map;

pub use durable_map::{DurableConfig, DurableMap, Relaxed, Strict};
pub use error::DurableError;

#[cfg(feature = "tiered")]
pub use pds_merkle_spine::VersionId;
#[cfg(feature = "tiered")]
pub use policy::{Durable, MemOnly, Pipelined, TierPolicy, WriteBack};
#[cfg(feature = "tiered")]
pub use tiered_map::{MemOnlyMap, PipelinedMap, TieredConfig, TieredMap};
