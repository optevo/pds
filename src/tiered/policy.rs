//! [`PropagationPolicy`] — controls when the hot tier is flushed to the cold tier.

/// Controls when writes accumulated in the hot tier are propagated to the cold tier.
///
/// Each policy represents a different point on the latency / durability trade-off curve:
///
/// | Policy | Cold-tier lag | Write overhead |
/// |--------|--------------|----------------|
/// | [`Immediate`][PropagationPolicy::Immediate] | Zero | High — flush on every write |
/// | [`Batched`][PropagationPolicy::Batched] | Up to n writes | Low — amortised across n |
/// | [`Timed`][PropagationPolicy::Timed] | Up to one interval | None on write path |
/// | [`Manual`][PropagationPolicy::Manual] | Unbounded | None on write path |
///
/// Pass a `PropagationPolicy` to
/// [`TieredCollection::new`][super::TieredCollection::new].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropagationPolicy {
    /// Cold tier is updated synchronously on every write.
    ///
    /// Every `insert` and `remove` triggers an immediate `flush`. This gives the
    /// cold tier zero lag at the cost of a flush call on every write.
    Immediate,

    /// Propagate after at least `n` writes have accumulated in the hot tier.
    ///
    /// The flush is triggered on the write that causes `write_count` to reach or
    /// exceed `n`. Subsequent writes restart the counter.
    ///
    /// A value of `0` is treated the same as `1` — every write triggers a flush.
    Batched(usize),

    /// A background thread propagates on this interval.
    ///
    /// Call [`TieredCollection::start_background_propagation`][super::TieredCollection::start_background_propagation]
    /// to spawn the background thread. Writes do not trigger flushes on their own
    /// with this policy; the background thread wakes every `d` and calls `flush`.
    ///
    /// Drop the returned [`PropagationHandle`][super::PropagationHandle] to stop
    /// the background thread.
    Timed(std::time::Duration),

    /// Only propagate on an explicit
    /// [`TieredCollection::flush`][super::TieredCollection::flush] call.
    ///
    /// Writes never trigger automatic flushing. This is suitable when the caller
    /// manages propagation explicitly (e.g. at the end of a batch job or on
    /// application shutdown).
    Manual,
}
