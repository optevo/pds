// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! # pds — Persistent Data Structures for Rust
//!
//! This library implements persistent (immutable) data structures with
//! structural sharing for Rust.
//!
//! ## What are persistent data structures?
//!
//! Persistent data structures are data structures which can be copied and
//! modified efficiently without altering the original. The most uncomplicated
//! example of this is the venerable [cons list][cons-list]. This crate offers a
//! selection of more modern and flexible data structures with similar
//! properties, tuned for the needs of Rust developers.
//!
//! Briefly, the following data structures are provided:
//!
//! * [`Vector<A>`][vector::Vector] — RRB tree sequence
//! * [`HashMap<K, V>`][hashmap::HashMap] / [`HashSet<A>`][hashset::HashSet] — HAMT-based unordered map and set
//! * [`OrdMap<K, V>`][ordmap::OrdMap] / [`OrdSet<A>`][ordset::OrdSet] — B+ tree sorted map and set
//! * [`InsertionOrderMap<K, V>`][crate::InsertionOrderMap] / [`InsertionOrderSet<A>`][crate::InsertionOrderSet] — insertion-ordered map and set
//! * [`Bag<A>`][crate::Bag] / [`OrdBag<A>`][crate::OrdBag] — persistent multiset tracking element counts
//! * [`HashMultiMap<K, V>`][crate::HashMultiMap] / [`OrdMultiMap<K, V>`][crate::OrdMultiMap] — key → set of values multimap
//! * [`BiMap<K, V>`][crate::BiMap] / [`OrdBiMap<K, V>`][crate::OrdBiMap] — bidirectional bijection map
//! * [`SymMap<A>`][crate::SymMap] / [`OrdSymMap<A>`][crate::OrdSymMap] — symmetric bidirectional map
//! * [`Trie<K, V>`][crate::Trie] / [`OrdTrie<K, V>`][crate::OrdTrie] — persistent prefix tree
//! * [`OrdInsertionOrderMap<K, V>`][crate::OrdInsertionOrderMap] / [`OrdInsertionOrderSet<A>`][crate::OrdInsertionOrderSet] — `Ord`-only insertion-ordered collections
//! * [`UniqueVector<A>`][crate::UniqueVector] — persistent sequence with element uniqueness guarantee
//!
//! ## Why Would I Want This?
//!
//! While immutable data structures can be a game changer for other
//! programming languages, the most obvious benefit - avoiding the
//! accidental mutation of data - is already handled so well by Rust's
//! type system that it's just not something a Rust programmer needs
//! to worry about even when using data structures that would send a
//! conscientious Clojure, Haskell, Scala, or F# programmer into a panic.
//!
//! Immutable data structures offer other benefits, though, some of
//! which are useful even in a language like Rust. The most prominent
//! is *structural sharing*, which means that if two data structures
//! are mostly copies of each other, most of the memory they take up
//! will be shared between them. This implies that making copies of an
//! immutable data structure is cheap: it's really only a matter of
//! copying a pointer and increasing a reference counter, where in the
//! case of [`Vec`] you have to allocate the same
//! amount of memory all over again and make a copy of every element
//! it contains. For immutable data structures, extra memory isn't
//! allocated until you modify either the copy or the original, and
//! then only the memory needed to record the difference.
//!
//! Another goal of this library has been the idea that you shouldn't
//! even have to think about what data structure to use in any given
//! situation, until the point where you need to start worrying about
//! optimisation - which, in practice, often never comes. Beyond the
//! shape of your data (ie. whether to use a list or a map), it should
//! be fine not to think too carefully about data structures - you can
//! just pick the one that has the right shape and it should have
//! acceptable performance characteristics for every operation you
//! might need. Specialised data structures will always be faster at
//! what they've been specialised for, but `pds` aims to provide the
//! data structures which deliver the least chance of accidentally
//! using them for the wrong thing.
//!
//! For instance, [`Vec`] beats everything at memory
//! usage, indexing and operations that happen at the back of the
//! list, but is terrible at insertion and removal, and gets worse the
//! closer to the front of the list you get.
//! [`VecDeque`](std::collections::VecDeque) adds a little bit of
//! complexity in order to make operations at the front as efficient
//! as operations at the back, but is still bad at insertion and
//! especially concatenation. [`Vector`] adds another
//! bit of complexity, and could never match [`Vec`] at
//! what it's best at, but in return every operation you can throw at
//! it can be completed in a reasonable amount of time - even normally
//! expensive operations like copying and especially concatenation are
//! reasonably cheap when using a [`Vector`].
//!
//! It should be noted, however, that because of its simplicity,
//! [`Vec`] actually beats [`Vector`] even at its
//! strongest operations at small sizes, just because modern CPUs are
//! hyperoptimised for things like copying small chunks of contiguous memory -
//! you actually need to go past a certain size (usually in the vicinity of
//! several hundred elements) before you get to the point where
//! [`Vec`] isn't always going to be the fastest choice.
//! [`Vector`] attempts to overcome this by actually just being
//! an array at very small sizes, and being able to switch efficiently to the
//! full data structure when it grows large enough. Thus,
//! [`Vector`] will actually be equivalent to
//! [Vec] until it grows past the size of a single chunk.
//!
//! The maps - [`HashMap`] and
//! [`OrdMap`] - generally perform similarly to their
//! equivalents in the standard library, but tend to run a bit slower
//! on the basic operations ([`HashMap`] is almost
//! neck and neck with its counterpart, while
//! [`OrdMap`] currently tends to run 2-3x slower). On
//! the other hand, they offer the cheap copy and structural sharing
//! between copies that you'd expect from immutable data structures.
//!
//! In conclusion, the aim of this library is to provide a safe
//! default choice for the most common kinds of data structures,
//! allowing you to defer careful thinking about the right data
//! structure for the job until you need to start looking for
//! optimisations - and you may find, especially for larger data sets,
//! that immutable data structures are still the right choice.
//!
//! ## Values
//!
//! Because we need to make copies of shared nodes in these data structures
//! before updating them, the values you store in them must implement
//! [`Clone`].  For primitive values that implement
//! [`Copy`], such as numbers, everything is fine: this is
//! the case for which the data structures are optimised, and performance is
//! going to be great.
//!
//! On the other hand, if you want to store values for which cloning is
//! expensive, or values that don't implement [`Clone`], you
//! need to wrap them in [`Rc`][std::rc::Rc] or [`Arc`][std::sync::Arc]. Thus,
//! if you have a complex structure `BigBlobOfData` and you want to store a list
//! of them as a `Vector<BigBlobOfData>`, you should instead use a
//! `Vector<Rc<BigBlobOfData>>`, which is going to save you not only the time
//! spent cloning the big blobs of data, but also the memory spent keeping
//! multiple copies of it around, as [`Rc`][std::rc::Rc] keeps a single
//! reference counted copy around instead.
//!
//! If you're storing smaller values that aren't
//! [`Copy`]able, you'll need to exercise judgement: if your
//! values are going to be very cheap to clone, as would be the case for short
//! [`String`] or small [`Vec`]s, you're probably better off storing them directly
//! without wrapping them in an [`Rc`][std::rc::Rc], because, like the [`Rc`][std::rc::Rc],
//! they're just pointers to some data on the heap, and that data isn't expensive to clone -
//! you might actually lose more performance from the extra redirection of
//! wrapping them in an [`Rc`][std::rc::Rc] than you would from occasionally
//! cloning them.
//!
//! ### When does cloning happen?
//!
//! So when will your values actually be cloned? The easy answer is only if you
//! [`clone`][Clone::clone] the data structure itself, and then only
//! lazily as you change it. Values are stored in tree nodes inside the data
//! structure, each node of which contains up to 32 or 64 values
//! (depending on the collection type). When you
//! [`clone`][Clone::clone] a data structure, nothing is actually
//! copied - it's just the reference count on the root node that's incremented,
//! to indicate that it's shared between two data structures. It's only when you
//! actually modify one of the shared data structures that nodes are cloned:
//! when you make a change somewhere in the tree, the node containing the change
//! needs to be cloned, and then its parent nodes need to be updated to contain
//! the new child node instead of the old version, and so they're cloned as
//! well.
//!
//! We can call this "lazy" cloning - if you make two copies of a data structure
//! and you never change either of them, there's never any need to clone the
//! data they contain. It's only when you start making changes that cloning
//! starts to happen, and then only on the specific tree nodes that are part of
//! the change. Note that the implications of lazily cloning the data structure
//! extend to memory usage as well as the CPU workload of copying the data
//! around - cloning an immutable data structure means both copies share the
//! same allocated memory, until you start making changes.
//!
//! Most crucially, if you never clone the data structure, the data inside it is
//! also never cloned, and in this case it acts just like a mutable data
//! structure, with minimal performance differences (but still non-zero, as we
//! still have to check for shared nodes).
//!
//! ## Data Structures
//!
//! We'll attempt to provide a comprehensive guide to the available
//! data structures below.
//!
//! ### Performance Notes
//!
//! "Big O notation" is the standard way of talking about the time
//! complexity of data structure operations. If you're not familiar
//! with big O notation, here's a quick cheat sheet:
//!
//! *O(1)* means an operation runs in constant time: it will take the
//! same time to complete regardless of the size of the data
//! structure.
//!
//! *O(n)* means an operation runs in linear time: if you double the
//! size of your data structure, the operation will take twice as long
//! to complete; if you quadruple the size, it will take four times as
//! long, etc.
//!
//! *O(log n)* means an operation runs in logarithmic time: for
//! *log<sub>2</sub>*, if you double the size of your data structure,
//! the operation will take one step longer to complete; if you
//! quadruple the size, it will need two steps more; and so on.
//! However, the data structures in this library generally run in
//! *log<sub>32</sub>* or *log<sub>64</sub>* time (branching factor
//! 32 for maps and sets, 64 for vectors), meaning you have to make
//! your data structure 32–64 times bigger to need one extra step.
//! This means that, while they still count
//! as O(log n), operations on all but really large data sets will run
//! at near enough to O(1) that you won't usually notice.
//!
//! *O(n log n)* is the most expensive operation you'll see in this
//! library: it means that for every one of the *n* elements in your
//! data structure, you have to perform *log n* operations. In our
//! case, as noted above, this is often close enough to O(n) that it's
//! not usually as bad as it sounds, but even O(n) isn't cheap and the
//! cost still increases logarithmically, if slowly, as the size of
//! your data increases. O(n log n) basically means "are you sure you
//! need to do this?" Operations in this class include: sorting a
//! [`Vector`]; constructing an [`OrdMap`] or [`OrdSet`] from an
//! iterator (each of the *n* insertions costs O(log n)); and all bulk
//! set operations — `union`, `intersection`, `difference`,
//! `symmetric_difference` — across every collection type, because each
//! iterates *n* elements and inserts each into a new collection at
//! O(log n) per insert.
//!
//! *O(1)** means 'amortised O(1),' which means that an operation
//! usually runs in constant time but will occasionally be more
//! expensive: for instance,
//! [`Vector::push_back`], if called in
//! sequence, will be O(1) most of the time but every 64th time it
//! will be O(log n), as it fills up its tail chunk and needs to
//! insert it into the tree. Please note that the O(1) with the
//! asterisk attached is not a common notation; it's just a convention
//! I've used in these docs to save myself from having to type
//! 'amortised' everywhere.
//!
//! ### Lists
//!
//! Lists are sequences of single elements which maintain the order in
//! which you inserted them. The only list in this library is
//! [`Vector`], which offers the best all round
//! performance characteristics: it's pretty good at everything, even
//! if there's always another kind of list that's better at something.
//!
//! | Type | Algorithm | Constraints | Order | Clone | Eq | Push | Pop | Split | Append | Lookup |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | [`Vector<A>`] | [RRB tree][rrb-tree] | [`Clone`] | insertion | O(1) | O(1)† | O(1)\* | O(1)\* | O(log n) | O(log n) | O(log n) |
//!
//! ### Maps
//!
//! Maps are mappings of keys to values, where the most common read
//! operation is to find the value associated with a given key. Maps
//! may or may not have a defined order. Any given key can only occur
//! once inside a map, and setting a key to a different value will
//! overwrite the previous value.
//!
//! | Type | Algorithm | Key Constraints | Order | Clone | Eq | Insert | Remove | Lookup |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | [`HashMap<K, V>`] | [HAMT][hamt] | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] | undefined | O(1) | O(1)† | O(log n) | O(log n) | O(log n) |
//! | [`OrdMap<K, V>`] | [B+tree][b+tree] | [`Clone`] + [`Ord`] | sorted | O(1) | O(n)‡ | O(log n) | O(log n) | O(log n) |
//!
//! ### Sets
//!
//! Sets are collections of unique values, and may or may not have a
//! defined order. Their crucial property is that any given value can
//! only exist once in a given set.
//!
//! | Type | Algorithm | Constraints | Order | Clone | Eq | Insert | Remove | Lookup |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | [`HashSet<A>`] | [HAMT][hamt] | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] | undefined | O(1) | O(1)† | O(log n) | O(log n) | O(log n) |
//! | [`OrdSet<A>`] | [B+tree][b+tree] | [`Clone`] + [`Ord`] | sorted | O(1) | O(n)‡ | O(log n) | O(log n) | O(log n) |
//!
//! † Merkle-accelerated equality: O(1) when both collections have valid
//! Merkle hashes (common after clone-and-modify workflows). Falls back
//! to O(n) element-by-element comparison otherwise. For `HashMap`, requires
//! both maps to share a hasher instance (common ancestor via `clone`).
//!
//! ‡ O(1) when both collections have a valid cached content hash (requires
//! the `ord-hash` feature, enabled by default). Falls back to O(n) sorted
//! scan otherwise.
//!
//! ### Other Collections
//!
//! | Type | Description | Key Constraints |
//! | --- | --- | --- |
//! | [`Bag<A>`][crate::Bag] | Persistent multiset (bag) — tracks element counts | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`OrdBag<A>`][crate::OrdBag] | Sorted multiset — `Ord`, `Hash`, and `range()` | [`Clone`] + [`Ord`] |
//! | [`OrdMultiMap<K, V>`][crate::OrdMultiMap] | Sorted key → sorted set of values multimap | [`Clone`] + [`Ord`] |
//! | [`OrdSymMap<A>`][crate::OrdSymMap] | Sorted symmetric bidirectional map | [`Clone`] + [`Ord`] |
//! | [`OrdBiMap<K, V>`][crate::OrdBiMap] | Sorted bidirectional map — bijection between two types | [`Clone`] + [`Ord`] |
//! | [`OrdTrie<K, V>`][crate::OrdTrie] | Sorted prefix tree — lexicographic path iteration | [`Clone`] + [`Ord`] |
//! | [`OrdInsertionOrderMap<K, V>`][crate::OrdInsertionOrderMap] | Insertion-ordered map — `Ord`-only, no tombstones | [`Clone`] + [`Ord`] |
//! | [`OrdInsertionOrderSet<A>`][crate::OrdInsertionOrderSet] | Insertion-ordered set — `Ord`-only, no tombstones | [`Clone`] + [`Ord`] |
//! | [`HashMultiMap<K, V>`][crate::HashMultiMap] | Key → set of values multimap | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`InsertionOrderMap<K, V>`][crate::InsertionOrderMap] | Map that iterates in insertion order | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`InsertionOrderSet<A>`][crate::InsertionOrderSet] | Set that iterates in insertion order | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`BiMap<K, V>`][crate::BiMap] | Bidirectional map — bijection between two types | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`SymMap<A>`][crate::SymMap] | Symmetric bidirectional map with O(1) swap | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`Trie<K, V>`][crate::Trie] | Persistent prefix tree (trie) — paths to values | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//! | [`UniqueVector<A>`][crate::UniqueVector] | Persistent sequence with uniqueness — dedup queue/stack with index access | [`Clone`] + [`Hash`][std::hash::Hash] + [`Eq`] |
//!
//! ### Ord-backed variants
//!
//! Several compound types have an `Ord`-backed variant (prefix `Ord`) alongside the
//! default hash-backed variant. The `Ord` variants:
//!
//! - Require only `Clone + Ord` — no `Hash + Eq` on keys or elements.
//! - Iterate in **sorted order by definition**, which means they implement
//!   `PartialOrd`, `Ord`, and `Hash` without an order-independent combiner.
//! - Support **range queries** over elements or keys.
//! - Have **no hasher type parameter** (`S`) — the generic signature is simpler.
//! - Work in `no_std` without the `foldhash` feature because `OrdMap` needs no hasher.
//!
//! Where a hash-backed type has both a map and a set form, the `Ord` variant follows the
//! same pattern: `OrdBag` pairs with `Bag`; `OrdMultiMap` pairs with `HashMultiMap`;
//! `OrdSymMap` pairs with `SymMap`; `OrdBiMap` pairs with `BiMap`; `OrdTrie` pairs with
//! `Trie`; `OrdInsertionOrderMap` and `OrdInsertionOrderSet` pair with
//! `InsertionOrderMap` and `InsertionOrderSet`.
//!
//! Use the `Ord` variant when you need ordering, range queries, or the simpler type
//! signature; use the hash variant when `Hash + Eq` is available and you do not need order.
//!
//! ## Deterministic hashing
//!
//! By default `HashMap` and `HashSet` use a randomised hasher
//! (`RandomState`) seeded differently on every process start — a deliberate
//! defence against hash-flooding attacks. This means that the HAMT node
//! layout, and therefore every node pointer and Merkle hash, varies between
//! runs even for identical key sets.
//!
//! When cross-session consistency matters — reproducible test snapshots,
//! node deduplication across serialised pools, or merging an `InternPool`
//! loaded from disk with one built at runtime — you can opt into deterministic
//! hashing by choosing a fixed-seed hasher:
//!
//! | Use case | Recommended hasher |
//! |----------|--------------------|
//! | Integer keys (UUIDs, content hashes, random `u64`) | [`IdentityBuildHasher`](identity_hasher::IdentityBuildHasher) |
//! | String or composite keys | A seeded instance of `AHasher`, `FxBuildHasher`, or `foldhash` |
//!
//! With a fixed-seed hasher the same key always produces the same hash, so:
//!
//! - The HAMT trie path is identical across sessions.
//! - Merkle hashes computed at runtime match those deserialised from a
//!   previous run — enabling the `hash-intern` + `persist` features to merge
//!   loaded nodes with in-memory nodes by pointer after verifying Merkle
//!   equality.
//! - Tests that assert on internal node structure or serialised bytes are
//!   reproducible without controlling random seeds externally.
//!
//! **Security caveat:** A fixed-seed hasher is vulnerable to Hash DoS from
//! untrusted input. Use fixed seeds only when all keys come from a trusted
//! source (your own code, a closed serialisation format, internal integers).
//! For untrusted user input, keep `RandomState`.
//!
//! The `Ord`-backed collections (`OrdMap`, `OrdBag`, `OrdMultiMap`, etc.)
//! are always deterministic: they have no hasher and iterate in sorted order,
//! so their content hashes (when `K: Hash, V: Hash`) are canonical across
//! sessions without any special configuration.
//!
//! ## In-place Mutation
//!
//! All of these data structures support in-place copy-on-write
//! mutation, which means that if you're the sole user of a data
//! structure, you can update it in place without taking the
//! performance hit of making a copy of the data structure before
//! modifying it (this is about an order of magnitude faster than
//! immutable operations, almost as fast as
//! [`std::collections`]'s mutable data structures).
//!
//! Thanks to [`Rc`][std::rc::Rc]'s reference counting, we are able to
//! determine whether a node in a data structure is being shared with
//! other data structures, or whether it's safe to mutate it in place.
//! When it's shared, we'll automatically make a copy of the node
//! before modifying it. The consequence of this is that cloning a
//! data structure becomes a lazy operation: the initial clone is
//! instant, and as you modify the cloned data structure it will clone
//! chunks only where you change them, so that if you change the
//! entire thing you will eventually have performed a full clone.
//!
//! This also gives us a couple of other optimisations for free:
//! implementations of immutable data structures in other languages
//! often have the idea of local mutation, like Clojure's transients
//! or Haskell's `ST` monad - a managed scope where you can treat an
//! immutable data structure like a mutable one, gaining a
//! considerable amount of performance because you no longer need to
//! copy your changed nodes for every operation, just the first time
//! you hit a node that's sharing structure. In Rust, we don't need to
//! think about this kind of managed scope, it's all taken care of
//! behind the scenes because of our low level access to the garbage
//! collector (which, in our case, is just a simple
//! [`Rc`](std::rc::Rc)).
//!
//! ## pds vs the standard library
//!
//! The standard library provides `HashMap`, `BTreeMap`, and `Vec` as mutable, owned
//! containers. Every `clone()` allocates fresh memory and copies every element — O(n)
//! in both time and space. pds collections use structural sharing: clone is always O(1),
//! and a modification touches only the path from the root to the changed node.
//!
//! ### Maps
//!
//! | Operation | `std::HashMap` | `pds::HashMap` | `std::BTreeMap` | `pds::OrdMap` |
//! |-----------|:--------------:|:--------------:|:---------------:|:-------------:|
//! | `clone()` | O(n) | **O(1)** | O(n) | **O(1)** |
//! | Lookup | **O(1) avg** | O(log n) | O(log n) | O(log n) |
//! | Insert | **O(1) avg** | O(log n) | O(log n) | O(log n) |
//! | Remove | **O(1) avg** | O(log n) | O(log n) | O(log n) |
//! | Iterate | O(n) | O(n) | O(n) | O(n) |
//! | Equality | O(n) | **O(1)†** | O(n) | **O(1)‡** |
//!
//! † Merkle hash fast-path — same-lineage maps with equal length and equal Merkle hash compare in O(1).
//! ‡ Cached content hash (`ord-hash` feature, on by default) — O(1) when the hash is valid.
//!
//! The trade-off is clone cost versus point-lookup speed. `std::HashMap` wins on random
//! lookups (roughly 2× faster than [`HashMap`]). pds wins on any operation that involves
//! copying: every clone that would cost O(n) with a standard map becomes O(1).
//!
//! When a single thread owns a map and mutates it in a tight loop with no snapshotting,
//! `std::HashMap` is the right tool. When you need snapshots, undo/redo, versioning, or
//! shared state between threads — pds collections win.
//!
//! ### Vectors
//!
//! | Operation | `std::Vec` | `pds::Vector` |
//! |-----------|:----------:|:-------------:|
//! | `clone()` | O(n) | **O(1)** |
//! | Push (back) | **O(1) avg** | O(1) avg |
//! | Random access | **O(1)** | O(log n) |
//! | Insert (middle) | O(n) | **O(log n)** |
//! | Split | O(n) | **O(log n)** |
//! | Concat | O(n) | **O(log n)** |
//!
//! `std::Vec` is unbeatable for purely sequential workloads: appending and reading by
//! index in a tight loop. [`Vector`] trades a constant factor on random access (the RRB
//! tree depth) for dramatically cheaper structural operations — split and concat are
//! O(log n) rather than O(n), and clone is O(1). Use [`Vector`] when you need to branch
//! on a sequence: taking a snapshot before a speculative edit, passing an independent
//! view to another thread, or producing multiple output variants from a single input.
//!
//! ### Multi-threading
//!
//! Rust's ownership model prevents data races at compile time. pds extends this
//! advantage: because clone is O(1), you can hand a complete, independent snapshot to
//! another thread with no synchronisation overhead.
//!
//! With a standard library map:
//!
//! ```ignore
//! // Every reader must acquire the lock — even for read-only access.
//! let shared: Arc<Mutex<std::collections::HashMap<K, V>>> = ...;
//! let guard = shared.lock().unwrap();
//! let value = guard.get(&key);
//! ```
//!
//! With a pds map:
//!
//! ```ignore
//! // Clone the current snapshot in O(1) — no lock held during processing.
//! let snapshot: pds::HashMap<K, V> = current_state.clone();
//! let value = snapshot.get(&key);
//! ```
//!
//! Because each modification produces a new root without touching the old one, multiple
//! threads can hold snapshots at different points in time — all sharing structure, all
//! independent, none blocking the others. Common patterns:
//!
//! - **Worker pools** — distribute independent snapshots to workers; merge results back
//!   with `par_union`.
//! - **Speculative execution** — clone before a tentative operation; discard the clone
//!   on rollback, keep it on commit.
//! - **Event sourcing** — each state transition produces a new snapshot; prior states
//!   are retained cheaply because unchanged subtrees are shared.
//! - **Read scale-out** — any number of readers hold the latest snapshot with zero
//!   contention; the writer atomically publishes a new root.
//!
//! ## Thread Safety
//!
//! The data structures in `pds` are thread safe by default using
//! [`triomphe::Arc`](https://docs.rs/triomphe/latest/triomphe/struct.Arc.html)
//! (a drop-in replacement for `std::sync::Arc` without the weak reference
//! count — saves 8 bytes per node and eliminates one atomic operation per
//! clone/drop). Disable the `triomphe` feature to fall back to `std::sync::Arc`.
//!
//! `pds` also supports `Rc` as the pointer type through the [`archery`]
//! crate, just like `im-rc` in the original `im` crate. If you prioritise
//! speed over thread safety, you can use
//! [`GenericVector<T, archery::shared_pointer::RcK>`](vector::GenericVector) with
//! non-threadsafe but faster `Rc`, instead of the type alias [`Vector`].
//! The same pattern works on all other collection types.
//!
//! ## `no_std` Support
//!
//! `pds` supports `no_std` environments that provide `alloc`. Disable the
//! default `std` feature:
//!
//! ```no_compile
//! [dependencies]
//! pds = { version = "*", default-features = false, features = ["triomphe"] }
//! ```
//!
//! In `no_std` mode, convenience type aliases (`HashMap`, `HashSet`, `Bag`,
//! `HashMultiMap`, `InsertionOrderMap`, `InsertionOrderSet`, `BiMap`, `SymMap`,
//! `Trie`) are not available because they depend on
//! `std::collections::hash_map::RandomState`. Use the generic variants
//! (`GenericHashMap`, `GenericHashSet`, `GenericBag`, etc.) with your own
//! [`BuildHasher`](core::hash::BuildHasher) implementation instead.
//! `OrdMap`, `OrdSet`, and `Vector` are always available.
//!
//! ## Feature Flags
//!
//! | Feature | Default | Description |
//! | ------- | :-----: | ----------- |
//! | `std` | Yes | Enables `std`-dependent functionality: `RandomState`-based type aliases (`HashMap`, `HashSet`, etc.), `From<std::collections::*>` conversions, and `Mutex`-based locking. Disable for `no_std + alloc` environments. |
//! | [`triomphe`](https://crates.io/crates/triomphe/) | Yes | Use [`triomphe::Arc`](https://docs.rs/triomphe/latest/triomphe/struct.Arc.html) as the default shared pointer — faster than `std::sync::Arc` (no weak reference count). |
//! | [`proptest`](https://crates.io/crates/proptest) | No | Proptest strategies for all 20 collection types. |
//! | [`quickcheck`](https://crates.io/crates/quickcheck) | No | [`quickcheck::Arbitrary`](https://docs.rs/quickcheck/latest/quickcheck/trait.Arbitrary.html) implementations for all collection types. |
//! | [`rayon`](https://crates.io/crates/rayon) | No | Parallel iterators, parallel set operations (`par_union`, `par_intersection`, `par_difference`, `par_symmetric_difference`), and parallel transform operations (`par_filter`, `par_map_values`, `par_map_values_with_key`) for all eligible collection types. See [Parallel operations](#parallel-operations) below. |
//! | [`serde`](https://crates.io/crates/serde) | No | [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html) and [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html) implementations for all `pds` datatypes |
//! | [`arbitrary`](https://crates.io/crates/arbitrary/) | No | [`arbitrary::Arbitrary`](https://docs.rs/arbitrary/latest/arbitrary/trait.Arbitrary.html) implementations for all collection types. |
//! | [`foldhash`](https://crates.io/crates/foldhash/) | No | Enables `HashMap`, `HashSet`, etc. type aliases in `no_std` environments using `foldhash::fast::RandomState` as the default hasher. |
//! | [`atom`](https://crates.io/crates/arc-swap/) | No | Thread-safe shared values via `arc-swap` (requires `std`) |
//! | `hash-intern` | No | Hash consing / node interning for HAMT collections via `InternPool`. Deduplicates structurally identical subtrees to save memory and enable O(1) equality via pointer comparison. Requires `std`. |
//! | `persist` | No | Structural-sharing-preserving serialisation via `HashMapPool`. Serialises HAMT node trees with deduplication and reconstructs with hash consing on deserialisation. Requires `std` and `hash-intern`. |
//! | `ord-hash` | Yes | Cached content hash on `OrdMap` and `OrdSet` — enables O(1) `PartialEq` fast-path negative check (different hash → definitely unequal), a `content_hash()` method, and `Hash` impls for `K: Hash, V: Hash`. One atomic store per mutation; overhead is unmeasurable for typical workloads. See DEC-036. |
//! | `small-chunks` | No | Reduces internal chunk sizes so tree structures can be exercised with small collections. For testing only — not intended for production use. |
//! | `debug` | No | Enables internal invariant-checking methods on `Vector` (RRB tree validation). For testing and debugging only. |
//!
//! ## Parallel operations
//!
//! Enable the `rayon` feature to unlock parallel capabilities on all collection types.
//!
//! ### Parallel iteration
//!
//! Every collection with a sequential iterator also has a `par_iter()` method that returns
//! a Rayon `ParallelIterator`. Collections that support unordered collection also implement
//! `FromParallelIterator` and `ParallelExtend`.
//!
//! | Collection | `par_iter` | `FromParallelIterator` | `ParallelExtend` |
//! |------------|:----------:|:---------------------:|:----------------:|
//! | `HashMap` | ✓ | ✓ | ✓ |
//! | `HashSet` | ✓ | ✓ | ✓ |
//! | `OrdMap` | ✓ | ✓ | ✓ |
//! | `OrdSet` | ✓ | ✓ | ✓ |
//! | `Vector` | ✓ | ✓ | ✓ |
//! | `Bag` | ✓ | ✓ | ✓ |
//! | `HashMultiMap` | ✓ | ✓ | ✓ |
//! | `BiMap` | ✓ | ✓ | ✓ |
//! | `SymMap` | ✓ | ✓ | ✓ |
//! | `InsertionOrderMap` | ✓ | — | — |
//! | `InsertionOrderSet` | ✓ | — | — |
//! | `Trie` | — | — | — |
//!
//! ### Parallel set operations
//!
//! Collections with set semantics gain `par_union`, `par_intersection`, `par_difference`,
//! and `par_symmetric_difference`. These use Rayon's work-stealing thread pool and apply
//! the same O(1) structural fast-paths (pointer equality, Merkle hash) as the sequential
//! versions where applicable.
//!
//! | Collection | `par_union` | `par_intersection` | `par_difference` | `par_symmetric_difference` |
//! |------------|:-----------:|:-----------------:|:----------------:|:---------------------------:|
//! | `HashMap` | ✓ | ✓ | ✓ | ✓ |
//! | `HashSet` | ✓ | ✓ | ✓ | ✓ |
//! | `OrdMap` | ✓ | ✓ | ✓ | ✓ |
//! | `OrdSet` | ✓ | ✓ | ✓ | ✓ |
//! | `Bag` | ✓ | ✓ | ✓ | ✓ |
//! | `HashMultiMap` | ✓† | ✓ | ✓ | ✓ |
//! | `BiMap` | ✓† | ✓ | ✓ | ✓ |
//! | `SymMap` | ✓† | ✓ | ✓ | ✓ |
//!
//! † `par_union` delegates to the sequential implementation for these types because their
//! invariants (bijection, symmetry, value-set merging) require sequential conflict resolution.
//!
//! ### Parallel transform operations
//!
//! Map and set types additionally expose parallel higher-order transforms. These fall into
//! two categories depending on whether the implementation can exploit the internal data
//! structure directly.
//!
//! #### Implementation-optimised methods
//!
//! These methods walk the internal tree natively rather than going through a
//! `par_iter().collect()` round-trip. Because keys are not modified, the tree topology
//! (separator keys, hash positions, node layout) is preserved without re-insertion or
//! re-sorting, giving **O(n/p)** work instead of **O(n/p + n log n)**:
//!
//! | Method | Available on | Complexity | Notes |
//! |--------|-------------|-----------|-------|
//! | `par_map_values(f)` | `HashMap`, `OrdMap` | O(n/p) | Values replaced by `f(&value)`; tree structure preserved |
//! | `par_map_values_with_key(f)` | `HashMap`, `OrdMap` | O(n/p) | Values replaced by `f(&key, &value)`; tree structure preserved |
//!
//! For `HashMap` the HAMT node graph is walked directly: leaf entries are transformed
//! in-place and the key-hash Merkle values (which depend only on keys) are copied verbatim.
//! For `OrdMap` the B+ tree leaf children are processed in parallel via Rayon and the
//! separator keys are cloned unchanged.
//!
//! #### Convenience methods (collect-based)
//!
//! These methods change the tree topology (filtering removes entries, which can
//! trigger rebalancing), so they must reconstruct the collection via insertion.
//! They parallelise the scan/predicate evaluation but the rebuild step is sequential:
//!
//! | Method | Available on | Complexity | Notes |
//! |--------|-------------|-----------|-------|
//! | `par_filter(f)` | `HashMap`, `HashSet`, `OrdMap`, `OrdSet` | O(n/p) scan + O(k log k) rebuild | Surviving entries re-inserted (k = count kept) |
//!
//! All transform methods are immutable — the original collection is unchanged.
//!
//! ### Vector parallel operations
//!
//! [`Vector`] additionally provides:
//!
//! | Method | Description |
//! |--------|-------------|
//! | `par_sort()` | Sort in parallel (natural order) |
//! | `par_sort_by(f)` | Sort in parallel with a comparator |
//!
//! [rrb-tree]: https://infoscience.epfl.ch/record/213452/files/rrbvector.pdf
//! [hamt]: https://en.wikipedia.org/wiki/Hash_array_mapped_trie
//! [b+tree]: https://en.wikipedia.org/wiki/B%2B_tree
//! [cons-list]: https://en.wikipedia.org/wiki/Cons#Lists

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(rust_2018_idioms)]
#![deny(nonstandard_style)]
#![warn(unreachable_pub, missing_docs)]
#![deny(unsafe_code)]

extern crate alloc;

#[cfg(test)]
#[macro_use]
extern crate pretty_assertions;

#[macro_use]
mod util;

mod config;
pub mod hash_width;
pub mod identity_hasher;
mod nodes;
mod sort;
mod sync;

#[macro_use]
mod ord;
pub use crate::ord::map as ordmap;
pub use crate::ord::set as ordset;

#[macro_use]
mod hash;
pub use crate::hash::map as hashmap;
pub use crate::hash::set as hashset;

#[macro_use]
pub mod vector;

pub mod iter;

#[cfg(any(test, feature = "proptest"))]
pub mod proptest;

#[cfg(any(test, feature = "serde"))]
#[doc(hidden)]
pub mod ser;

#[cfg(feature = "arbitrary")]
#[doc(hidden)]
pub mod arbitrary;

#[cfg(feature = "quickcheck")]
#[doc(hidden)]
pub mod quickcheck;

pub mod shared_ptr;

#[cfg(feature = "atom")]
pub mod atom;

#[cfg(feature = "hash-intern")]
pub mod intern;

#[cfg(feature = "persist")]
pub mod persist;

#[macro_use]
pub mod bag;
pub mod bimap;
pub mod hash_multimap;
#[macro_use]
pub mod insertion_order_map;
#[macro_use]
pub mod insertion_order_set;
#[macro_use]
pub mod ord_bag;
pub mod ord_bimap;
#[macro_use]
pub mod ord_insertion_order_map;
#[macro_use]
pub mod ord_insertion_order_set;
pub mod ord_multimap;
pub mod ord_symmap;
pub mod ord_trie;
pub mod symmap;
pub mod trie;
#[macro_use]
pub mod unique_vector;

#[cfg(any(test, feature = "rayon"))]
mod rayon;

#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::bag::Bag;
pub use crate::bag::GenericBag;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::bimap::BiMap;
pub use crate::bimap::GenericBiMap;
pub use crate::hash_multimap::GenericHashMultiMap;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::hash_multimap::HashMultiMap;
pub use crate::hashmap::GenericHashMap;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::hashmap::HashMap;
pub use crate::hashset::GenericHashSet;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::hashset::HashSet;
pub use crate::insertion_order_map::GenericInsertionOrderMap;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::insertion_order_map::InsertionOrderMap;
pub use crate::insertion_order_set::GenericInsertionOrderSet;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::insertion_order_set::InsertionOrderSet;
pub use crate::ord_bag::{GenericOrdBag, OrdBag};
pub use crate::ord_bimap::{GenericOrdBiMap, OrdBiMap};
pub use crate::ord_insertion_order_map::{GenericOrdInsertionOrderMap, OrdInsertionOrderMap};
pub use crate::ord_insertion_order_set::{GenericOrdInsertionOrderSet, OrdInsertionOrderSet};
pub use crate::ord_multimap::{GenericOrdMultiMap, OrdMultiMap};
pub use crate::ord_symmap::{GenericOrdSymMap, OrdSymMap};
pub use crate::ord_trie::{GenericOrdTrie, OrdTrie};
pub use crate::ordmap::{GenericOrdMap, OrdMap};
pub use crate::ordset::{GenericOrdSet, OrdSet};
pub use crate::symmap::GenericSymMap;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::symmap::{Direction, SymMap};
pub use crate::trie::GenericTrie;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::trie::Trie;
pub use crate::unique_vector::GenericUniqueVector;
#[cfg(any(feature = "std", feature = "foldhash"))]
pub use crate::unique_vector::UniqueVector;
#[doc(inline)]
pub use crate::vector::{GenericVector, Vector};

#[cfg(test)]
mod test;

#[cfg(test)]
mod tests;

/// Update a value inside multiple levels of data structures.
///
/// This macro takes a [`Vector`], [`OrdMap`] or [`HashMap`],
/// a key or a series of keys, and a value, and returns the data structure with the
/// new value at the location described by the keys.
///
/// If one of the keys in the path doesn't exist, the macro will panic.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use std::sync::Arc;
/// # use pds::Vector;
/// # fn main() {
/// let vec_inside_vec = vector![vector![1, 2, 3], vector![4, 5, 6]];
///
/// let expected = vector![vector![1, 2, 3], vector![4, 5, 1337]];
///
/// assert_eq!(expected, update_in![vec_inside_vec, 1 => 2, 1337]);
/// # }
/// ```
///
#[macro_export]
macro_rules! update_in {
    ($target:expr, $path:expr => $($tail:tt) => *, $value:expr ) => {{
        let inner = $target.get($path).expect("update_in! macro: key not found in target");
        $target.update($path, update_in!(inner, $($tail) => *, $value))
    }};

    ($target:expr, $path:expr, $value:expr) => {
        $target.update($path, $value)
    };
}

/// Get a value inside multiple levels of data structures.
///
/// This macro takes a [`Vector`], [`OrdMap`] or [`HashMap`],
/// along with a key or a series of keys, and returns the value at the location inside
/// the data structure described by the key sequence, or `None` if any of the keys didn't
/// exist.
///
/// # Examples
///
/// ```
/// # #[macro_use] extern crate pds;
/// # use std::sync::Arc;
/// # use pds::Vector;
/// # fn main() {
/// let vec_inside_vec: Vector<Vector<i64>> = vector![vector![1, 2, 3], vector![4, 5, 6]];
///
/// assert_eq!(Some(&6), get_in![vec_inside_vec, 1 => 2]);
/// # }
/// ```
#[macro_export]
macro_rules! get_in {
    ($target:expr, $path:expr => $($tail:tt) => * ) => {{
        $target.get($path).and_then(|v| get_in!(v, $($tail) => *))
    }};

    ($target:expr, $path:expr) => {
        $target.get($path)
    };
}

/// Centralised `Send + Sync` static assertions for all 20 public collection types.
///
/// Individual modules also carry their own assertions; this module provides a single
/// place to verify the complete set compiles together under `--all-features`.
#[cfg(test)]
mod send_sync_tests {
    use static_assertions::assert_impl_all;

    // --- Hash-based collections ---
    assert_impl_all!(crate::HashMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::HashSet<String>: Send, Sync);
    assert_impl_all!(crate::Bag<String>: Send, Sync);
    assert_impl_all!(crate::HashMultiMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::BiMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::SymMap<String>: Send, Sync);
    assert_impl_all!(crate::InsertionOrderMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::InsertionOrderSet<String>: Send, Sync);
    assert_impl_all!(crate::Trie<String, i32>: Send, Sync);
    assert_impl_all!(crate::UniqueVector<String>: Send, Sync);

    // --- Ord-based collections ---
    assert_impl_all!(crate::OrdMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::OrdSet<String>: Send, Sync);
    assert_impl_all!(crate::OrdBag<String>: Send, Sync);
    assert_impl_all!(crate::OrdMultiMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::OrdBiMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::OrdSymMap<String>: Send, Sync);
    assert_impl_all!(crate::OrdInsertionOrderMap<String, i32>: Send, Sync);
    assert_impl_all!(crate::OrdInsertionOrderSet<String>: Send, Sync);
    assert_impl_all!(crate::OrdTrie<String, i32>: Send, Sync);

    // --- Sequence ---
    assert_impl_all!(crate::Vector<String>: Send, Sync);
}

#[cfg(test)]
mod lib_test {
    #[test]
    fn update_in() {
        let vector = vector![1, 2, 3, 4, 5];
        assert_eq!(vector![1, 2, 23, 4, 5], update_in!(vector, 2, 23));
        let hashmap = hashmap![1 => 1, 2 => 2, 3 => 3];
        assert_eq!(
            hashmap![1 => 1, 2 => 23, 3 => 3],
            update_in!(hashmap, 2, 23)
        );
        let ordmap = ordmap![1 => 1, 2 => 2, 3 => 3];
        assert_eq!(ordmap![1 => 1, 2 => 23, 3 => 3], update_in!(ordmap, 2, 23));

        let vecs = vector![vector![1, 2, 3], vector![4, 5, 6], vector![7, 8, 9]];
        let vecs_target = vector![vector![1, 2, 3], vector![4, 5, 23], vector![7, 8, 9]];
        assert_eq!(vecs_target, update_in!(vecs, 1 => 2, 23));
    }

    #[test]
    fn get_in() {
        let vector = vector![1, 2, 3, 4, 5];
        assert_eq!(Some(&3), get_in!(vector, 2));
        let hashmap = hashmap![1 => 1, 2 => 2, 3 => 3];
        assert_eq!(Some(&2), get_in!(hashmap, &2));
        let ordmap = ordmap![1 => 1, 2 => 2, 3 => 3];
        assert_eq!(Some(&2), get_in!(ordmap, &2));

        let vecs = vector![vector![1, 2, 3], vector![4, 5, 6], vector![7, 8, 9]];
        assert_eq!(Some(&6), get_in!(vecs, 1 => 2));
    }
}
