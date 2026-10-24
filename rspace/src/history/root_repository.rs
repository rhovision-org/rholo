//! Higher-level root commit/validation wrapper.
//!
//! Mirrors `rspace/src/main/scala/coop/rchain/rspace/history/RootRepository.scala`.

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;

use crate::history::history::empty_root_hash_value;
use crate::history::roots_store::RootsStore;

/// The root repository (port of `RootRepository`).
pub struct RootRepository {
    roots_store: RootsStore,
}

impl RootRepository {
    pub fn new(roots_store: RootsStore) -> Self {
        RootRepository { roots_store }
    }

    pub async fn commit(&self, root: Blake2b256Hash) -> Result<(), String> {
        self.roots_store.record_root(root).await
    }

    /// The durable `CURRENT_ROOT`, **without** recording the empty root (the read half of
    /// [`Self::current_root`]).
    pub async fn published_root(&self) -> Result<Option<Blake2b256Hash>, String> {
        self.roots_store.current_root().await
    }

    /// Record `root` as known without moving `CURRENT_ROOT` — the candidate-state half of the split
    /// (law 80, B40, HAZOP F-U17-10).
    pub async fn commit_deferred(&self, root: Blake2b256Hash) -> Result<(), String> {
        self.roots_store.record_known_root(root).await
    }

    /// Move `CURRENT_ROOT` to `root` if the store already knows it; `Ok(false)` when it does not.
    /// This is the acceptance half: the caller has inserted a block whose post-state is `root`.
    pub async fn publish_if_known(&self, root: Blake2b256Hash) -> Result<bool, String> {
        self.roots_store.publish_current_root(root).await
    }

    /// The current root, recording the empty root on first use (port of `currentRoot`).
    pub async fn current_root(&self) -> Result<Blake2b256Hash, String> {
        match self.roots_store.current_root().await? {
            None => {
                let empty = empty_root_hash_value();
                self.roots_store.record_root(empty).await?;
                Ok(empty)
            }
            Some(root) => Ok(root),
        }
    }

    /// Validate `root` is known and set it current; error otherwise (port of
    /// `validateAndSetCurrentRoot`).
    pub async fn validate_and_set_current_root(&self, root: Blake2b256Hash) -> Result<(), String> {
        match self.roots_store.validate_and_set_current_root(root).await? {
            Some(_) => Ok(()),
            None => Err("unknown root".to_string()),
        }
    }

    /// The **same validation, without the publication** — the merge's half of the split.
    ///
    /// A caller that is about to do real work before it has a new root must not advance
    /// `CURRENT_ROOT` to its base first: a crash in between leaves the node naming an **ancestor** on
    /// restart, neither the pre-merge nor the post-merge state. The check is not the thing being
    /// dropped, only the write.
    pub async fn validate_known_root(&self, root: Blake2b256Hash) -> Result<(), String> {
        if self.roots_store.is_known_root(root).await? {
            Ok(())
        } else {
            Err("unknown root".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_shared::store::InMemoryKeyValueStore;
    use std::sync::Arc;

    fn repository() -> RootRepository {
        let store: Box<dyn rchain_shared::store::KeyValueStore + Send + Sync> =
            Box::new(InMemoryKeyValueStore::default());
        RootRepository::new(RootsStore::new(Arc::new(tokio::sync::Mutex::new(store))))
    }

    /// A store with no root is not an error: the **empty root** is recorded and returned, so the
    /// first read establishes the genesis of the trie rather than failing. Pinned because the
    /// alternative (an error on an empty store) would be indistinguishable from a corrupt one.
    #[tokio::test]
    async fn an_empty_store_yields_the_empty_root_and_records_it() {
        let repo = repository();
        let empty = empty_root_hash_value();

        assert_eq!(repo.current_root().await.expect("root"), empty);
        // Now that it has been recorded, the validated setter accepts it — the lazily recorded root
        // is a *known* root, not a special case the setter has to make an exception for.
        assert_eq!(repo.current_root().await.expect("root"), empty);
        assert!(repo.validate_and_set_current_root(empty).await.is_ok());
    }

    /// The wrapper turns the store's refusal into an error rather than a silent no-op, and leaves the
    /// current root where it was — so a caller cannot believe it moved to a root that does not exist.
    #[tokio::test]
    async fn an_unknown_root_is_an_error_and_does_not_move_the_current_root() {
        let repo = repository();
        let known = rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([0x11; 32]);
        repo.commit(known).await.expect("commit");

        let unknown = rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_bytes([0xAB; 32]);
        let err = repo
            .validate_and_set_current_root(unknown)
            .await
            .expect_err("an unknown root must be an error");
        assert!(err.contains("unknown root"), "{err}");
        assert_eq!(
            repo.current_root().await.expect("root"),
            known,
            "a refused root must not become current"
        );
    }

    /// **The validating read must not publish — and this is the level where that is observable**
    /// (the programme's L2, and the merge's half of the split).
    ///
    /// A merge resets to its base and then does real work before the merged trie exists. `reset`
    /// advanced `CURRENT_ROOT` to the base as it went, so a crash in between left the node naming an
    /// **ancestor** on restart — neither the pre-merge nor the post-merge state. `validate_known_root`
    /// is the same refusal `reset` gives an unknown root, with the write removed.
    ///
    /// This lives here rather than on `HistoryRepository` because a fresh repository over an in-memory
    /// manager knows exactly **one** root, so there `before` and the reset target cannot be told apart.
    /// Two recorded roots make the difference observable, which is what a falsifier needs.
    #[tokio::test]
    async fn validate_known_root_refuses_without_publishing() {
        use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
        let repo = repository();
        let first = Blake2b256Hash::from_bytes([0x11; 32]);
        let second = Blake2b256Hash::from_bytes([0x22; 32]);
        repo.commit(first).await.expect("commit first");
        repo.commit(second).await.expect("commit second");
        assert_eq!(
            repo.current_root().await.expect("root"),
            second,
            "the last commit is current"
        );

        // The read: a known root is accepted, an unknown one is refused — the same verdicts the
        // publishing setter gives.
        repo.validate_known_root(first).await.expect("a known root");
        let err = repo
            .validate_known_root(Blake2b256Hash::from_bytes([0x33; 32]))
            .await
            .expect_err("an unknown root must be an error");
        assert!(err.contains("unknown root"), "{err}");

        // **And neither call moved the current root.** Under `validate_and_set_current_root` the
        // first one would have; that difference is the whole of the fix.
        assert_eq!(
            repo.current_root().await.expect("root"),
            second,
            "the validating read must not publish"
        );

        // The publishing setter is unchanged, so the ordinary path still works.
        repo.validate_and_set_current_root(first)
            .await
            .expect("a known root");
        assert_eq!(repo.current_root().await.expect("root"), first);
    }
}
