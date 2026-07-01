//! Re-exports [`FolioRope`], [`RopeSnapshot`], and [`RopeError`] from the
//! `folio-rope` crate, making them available as first-class pds-folio types.
//!
//! Create a rope with [`FolioRope::new`]; clone it in O(1) via [`Clone`].
//! No serde dependency — text content is stored as raw UTF-8 folio pages.
pub use folio_rope::error::RopeError;
pub use folio_rope::rope::FolioRope;
pub use folio_rope::snapshot::RopeSnapshot;

#[cfg(test)]
mod tests {
    use super::*;
    use folio_core::{backend::MemBackend, checksum::ChecksumKind, store::FolioStore};

    fn make_rope() -> FolioRope<MemBackend> {
        let store = FolioStore::create(
            MemBackend::new(4096, 256),
            4096,
            0,
            ChecksumKind::Xxh3,
            true,
        )
        .unwrap();
        FolioRope::new(store)
    }

    #[test]
    fn rope_reexport_basic_round_trip() {
        let mut rope = make_rope();
        rope.insert(0, "hello, world").unwrap();
        assert_eq!(format!("{rope}"), "hello, world");
    }

    #[test]
    fn rope_reexport_clone_is_independent() {
        let mut rope = make_rope();
        rope.insert(0, "original").unwrap();
        let snapshot = rope.clone();
        rope.insert(8, " modified").unwrap();
        // snapshot still sees original content
        assert_eq!(format!("{snapshot}"), "original");
        assert_eq!(format!("{rope}"), "original modified");
    }
}
