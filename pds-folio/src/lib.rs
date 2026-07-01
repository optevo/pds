//! `pds-folio` — folio-backed persistent data structures with structural sharing.
//!
//! Provides five collection types backed by folio mmap'd pages, all implementing
//! the cross-variant traits from [`pds::traits`] so callers can be generic over
//! the storage backend:
//!
//! | Type | Module | Trait |
//! |------|--------|-------|
//! | `FolioHashMap` (via [`hamt`]) | [`hamt`] | [`pds::traits::PersistentMap`] |
//! | `FolioHashSet` (via [`set`]) | [`set`] | [`pds::traits::PersistentSet`] |
//! | [`folio_vector::FolioVector`] | [`folio_vector`] | [`pds::traits::PersistentVector`] |
//! | [`folio_ordmap::FolioOrdMap`] | [`folio_ordmap`] | [`pds::traits::PersistentOrdMap`] |
//! | [`folio_ordset::FolioOrdSet`] | [`folio_ordset`] | [`pds::traits::PersistentOrdSet`] |
//!
//! # Storage model
//!
//! All five types store nodes in a [`folio_core::store::FolioStore`] backed by a
//! [`folio_core::backend::Backend`] (default: `MemBackend`; production: CoW + WAL).
//! Each collection root is an `Option<u64>` folio page ID.  The empty collection
//! uses `root: None`.
//!
//! # Codec
//!
//! The `C: Codec` type parameter controls how keys and values are encoded
//! into node page bytes.  Two implementations are provided:
//!
//! - [`codec::PodCodec`] — zero-copy for [`bytemuck::Pod`] types (`u64`,
//!   `[u8; 32]`, …)
//! - [`codec::PostcardCodec`] — compact variable-length encoding for any
//!   `#[derive(Serialize, Deserialize)]` type (default)
//!
//! # Structural sharing
//!
//! Path-copy semantics: insert/remove allocate new nodes only along the path
//! from root to the modified leaf (O(log N) pages per operation).  Unchanged
//! subtrees are shared between the old and new roots via a per-store
//! `HashMap<u64, u32>` refcount table.
//!
//! - `Clone` increments the root's refcount — O(1).
//! - `Drop` performs an iterative DFS, decrementing refcounts and batch-freeing
//!   pages that reach zero — O(log N) per snapshot dropped.
//!
//! # Consensus backend
//!
//! `pds-folio` does not implement consensus itself.  The `B: Backend` type
//! parameter allows callers to pass a consensus-aware backend (e.g. Raft-backed
//! folio).  A `consensus` feature flag (`consensus = ["folio-consensus"]`) will be
//! added when a folio consensus backend exists.
//!
//! # Backend selection
//!
//! Choose the right backend for your use case:
//!
//! | Requirement | Use |
//! |-------------|-----|
//! | Pure in-memory, maximum speed | `pds` (`HashMap`, `OrdMap`, `Vector`) |
//! | Disk persistence, large datasets (N > 10M) | `pds-folio` (`HamtMap`, `FolioVec`) |
//! | Cryptographic identity over in-memory data | `pds::MerkleWrapper<C>` |
//! | Versioned history + disk durability + proofs | `pds-merkle-spine` (`VersionedHamt`) |
//! | ACID semantics + explicit checkpoint control | `pds-durable` (`DurableMap`) |
//!
//! **Performance tradeoff:** `pds-folio`'s page indirection adds roughly 2–3×
//! latency vs `pds` at small N (e.g. `HashMap::get` ≈ 78 ns vs `HamtMap::get`
//! ≈ 150–200 ns on MemBackend). `pds-folio` wins when the working set exceeds
//! available RAM, when disk durability is required, or when N > ~10M keys where
//! the page cache amortises the indirection cost.
//!
//! # Status
//!
//! **Phase G complete** — all five collection types implemented with full CRUD,
//! structural sharing, and [`pds::traits`] impls.  G.1–G.12 done.
//! See `docs/impl-plan.md` for the full history.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(unreachable_pub)]

pub mod btree;
pub mod codec;
pub mod folio_ordmap;
pub mod folio_ordset;
pub mod folio_vector;
pub mod hamt;
pub mod hamt_index;
pub mod node;
pub mod set;
pub mod traits;
pub mod vector;
