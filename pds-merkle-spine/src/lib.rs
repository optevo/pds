//! `pds-merkle-spine` — versioned, Merkle-verified persistent hash map.
//!
//! Combines two lower-level libraries to produce [`VersionedHamt<K, V, B>`]:
//!
//! - **[`pds_folio`]** — provides `HamtMap<K, V, C, B>`: folio-backed persistent
//!   HAMT with structural sharing and `HamtIndex<B>: PageIndexBackend`
//! - **[`merkle_spine`]** — provides BLAKE3 hash primitives and the
//!   `PageIndexBackend` trait
//!
//! The result is a persistent, versioned, cryptographically-identified hash map
//! with:
//!
//! - O(log N) point reads and writes with structural sharing between versions
//! - O(1) historical version checkout
//! - O(log N) historical point lookup without materialising the full historical map
//! - O(changed × log N) structural diff between any two versions
//! - O(log N) Merkle inclusion proofs, verifiable without folio access
//!
//! # Status
//!
//! **Phase H** — pds-merkle-spine implementation.  H.0 (scaffold) done.
//! See `docs/impl-plan.md` for the full history.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(unreachable_pub)]

pub mod versioned_hamt;

pub use versioned_hamt::{VersionId, VersionedHamt, VersionedHamtError};
