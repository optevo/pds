//! `pds-folio` — folio-backed persistent data structures with structural sharing.
//!
//! Provides [`HashMap`], [`HashSet`], [`Vector`], [`OrdMap`], and [`OrdSet`]
//! backed by folio mmap'd pages. All five types implement the cross-variant traits
//! from [`pds::traits`] so callers can be generic over the storage backend.
//!
//! # Storage model
//!
//! Nodes are stored in a shared [`folio_collections::slab::FolioSlab`] backed
//! by a [`folio_core::backend::Backend`] (default: CoW + WAL). Each collection
//! root is a [`SlabPageId`] (a `u64`). The empty collection uses `root: None`.
//!
//! # Codec
//!
//! The `C: Codec` type parameter controls how keys and values are encoded
//! into node page bytes. Two implementations are provided:
//!
//! - [`codec::PodCodec`] — zero-copy for [`bytemuck::Pod`] types (`u64`, `[u8; 32]`, …)
//! - [`codec::PostcardCodec`] — compact variable-length encoding for any
//!   `#[derive(Serialize, Deserialize)]` type (default)
//!
//! # Structural sharing
//!
//! Path-copy semantics: insert/remove return a new root; unchanged subtrees are
//! shared. A `FolioBTree<SlabPageId, u32>` refcount table tracks sharing.
//! `Clone` increments the root's refcount in O(1). `Drop` frees the path in O(log N).
//!
//! # Status
//!
//! **Phase G scaffold** — types are declared, [`codec`] is implemented.
//! Full CRUD (G.1–G.12) is in progress. See `docs/impl-plan.md`.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(unreachable_pub)]

pub mod codec;
