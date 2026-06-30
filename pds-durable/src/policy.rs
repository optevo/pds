// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Sealed `TierPolicy` trait and the four built-in tier-policy presets.
//!
//! The trait is sealed — only the four preset types in this module may implement
//! it.  This guarantees that `TieredMap<K, V, Mode>` can only be instantiated
//! with a known, supported policy.
//!
//! # Presets
//!
//! | Policy | Tiers | Durability | Speed |
//! |--------|-------|------------|-------|
//! | [`MemOnly`] | t0 only | None — no disk | Fastest |
//! | [`WriteBack`] | t1 + t2 | Write-behind | Heap speed |
//! | [`Pipelined`] | t0 + t1 + t2 | Write-behind, 2-stage | Near-heap speed |
//! | [`Durable`] | t1 + t2 | Write-through | Durable on return |

// ── Sealed module ─────────────────────────────────────────────────────────────

mod sealed {
    /// Private supertrait — prevents external types from implementing [`TierPolicy`].
    pub trait Sealed {}
}

// ── TierPolicy trait ──────────────────────────────────────────────────────────

/// Marker trait for `TieredMap` storage presets.
///
/// This trait is sealed: only the four preset types in this module may
/// implement it.  Attempting to implement `TierPolicy` for an external type
/// will produce a compile error.
pub trait TierPolicy: sealed::Sealed {}

// ── Preset zero-sized types ───────────────────────────────────────────────────

/// Tier 0 only — in-place `std::collections::HashMap`, no disk backing.
///
/// The fastest possible storage preset.  Mutations land in a standard mutable
/// `HashMap` with no structural-sharing overhead.  Call
/// [`into_persistent()`][crate::TieredMap::into_persistent] to freeze the
/// contents into a persistent `pds::HashMap`.
///
/// There is no durability — all data is lost on drop.
pub struct MemOnly;

/// Tier 1 + Tier 2 — heap `pds::HashMap` with merkle-spine write-behind.
///
/// Equivalent to the D.9 `Relaxed` policy (renamed for clarity).  Mutations
/// write only to the persistent heap front cache at heap speed; call `flush()`
/// to push dirty entries to the `VersionedHamt` backing store as a single new
/// version.
///
/// Data loss window: mutations in `front` since the last `flush()`.
pub struct WriteBack;

/// Tier 0 + Tier 1 + Tier 2 — transient write buffer → heap snapshot → merkle-spine.
///
/// A 3-tier pipeline:
/// - **t0** (`std::collections::HashMap`): low-overhead write buffer; mutations land here.
/// - **t1** (`pds::HashMap`): last committed snapshot; `commit()` freezes t0 into t1.
/// - **t2** (`VersionedHamt`): durable replica; `flush()` pushes dirty t1 entries here.
///
/// Data loss windows:
/// - t0 mutations not yet committed → lost on crash.
/// - t1 mutations not yet flushed → lost on crash.
/// - t2 is always crash-safe.
pub struct Pipelined;

/// Write-through — every mutation goes directly to the `VersionedHamt` backing store.
///
/// Equivalent to the D.9 `Strict` policy (renamed for clarity).  Every mutation
/// creates a new version in the backing store and is durable on return.
///
/// This is the lowest-throughput, highest-durability preset.
pub struct Durable;

// ── Sealed impl ──────────────────────────────────────────────────────────────

impl sealed::Sealed for MemOnly {}
impl sealed::Sealed for WriteBack {}
impl sealed::Sealed for Pipelined {}
impl sealed::Sealed for Durable {}

// ── TierPolicy impl ───────────────────────────────────────────────────────────

impl TierPolicy for MemOnly {}
impl TierPolicy for WriteBack {}
impl TierPolicy for Pipelined {}
impl TierPolicy for Durable {}
