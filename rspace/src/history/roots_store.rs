//! Persists the current root hash under fixed keys.
//!
//! Mirrors `rspace/src/main/scala/coop/rchain/rspace/history/RootsStore.scala`.

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_shared::typed_store::SharedStore;

const CURRENT_ROOT: &[u8] = b"current-root";
const ROOT_TAG: &[u8] = b"root";

/// The roots store (port of `RootsStore`).
pub struct RootsStore {
    store: SharedStore,
}

impl RootsStore {
    pub fn new(store: SharedStore) -> Self {
        RootsStore { store }
    }

    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let vals = self.store.lock().await.get(&[key.to_vec()])?;
        Ok(vals.into_iter().next().flatten())
    }

    async fn put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), String> {
        self.store.lock().await.put(vec![(key, value)])
    }

    /// The current root, if set (port of `currentRoot`).
    ///
    /// **The length is checked before the hash is built, and that is the guard** (AUDIT C164).
    /// `Blake2b256Hash::from_byte_array` *asserts* its length (`crypto/src/hash/blake2b256_hash.rs:49-56`),
    /// so a truncated or corrupted `current-root` value was a **panic on the read path** — the path
    /// every state read goes through — rather than an error saying what was wrong. `codecs.rs`, the
    /// key codec for this same store, already refuses a wrong length by name, and its own test says
    /// why: "a corrupted or truncated store value must be an error naming the length, not a hash built
    /// from whatever bytes arrived". This is that discipline one file over, where it was missing.
    pub async fn current_root(&self) -> Result<Option<Blake2b256Hash>, String> {
        match self.get(CURRENT_ROOT).await? {
            None => Ok(None),
            Some(b) if b.len() == 32 => Ok(Some(Blake2b256Hash::from_byte_array(&b))),
            Some(b) => Err(format!(
                "current-root holds {} bytes, not the 32 a Blake2b256Hash is",
                b.len()
            )),
        }
    }

    /// Set the current root if `key` is a known root (port of `validateAndSetCurrentRoot`).
    pub async fn validate_and_set_current_root(
        &self,
        key: Blake2b256Hash,
    ) -> Result<Option<Blake2b256Hash>, String> {
        let bytes = key.to_byte_array().to_vec();
        if self.get(&bytes).await?.is_some() {
            self.put(CURRENT_ROOT.to_vec(), bytes).await?;
            Ok(Some(key))
        } else {
            Ok(None)
        }
    }

    /// Whether `key` has been recorded as a root, **without** changing which root is current.
    ///
    /// The read half of [`Self::validate_and_set_current_root`], split out for the merge: it must be
    /// able to refuse a root the store does not hold without *publishing* the one it is resetting to
    /// (see `HistoryRepository::reset_volatile`).
    pub async fn is_known_root(&self, key: Blake2b256Hash) -> Result<bool, String> {
        Ok(self.get(&key.to_byte_array()).await?.is_some())
    }

    /// Record `key` as a known root **without** moving the current one (law 80, B40).
    ///
    /// A candidate state's post-state is known as soon as a checkpoint has folded it; it is not the
    /// node's current root until the block that carries it is accepted. This is the checkpoint half
    /// of the split; [`Self::publish_current_root`] is the acceptance half, and
    /// [`Self::record_root`] stays the atomic pair for genesis and for the lazy empty root.
    pub async fn record_known_root(&self, key: Blake2b256Hash) -> Result<(), String> {
        self.put(key.to_byte_array().to_vec(), ROOT_TAG.to_vec())
            .await
    }

    /// Move `CURRENT_ROOT` to `key` **if the store already knows it** (law 80, B40).
    ///
    /// Returns whether the pointer moved. An unknown root is refused rather than written: a publish
    /// may only name a state a checkpoint recorded, and the two writes are separate transactions
    /// here because they belong to different events — the checkpoint and the block's acceptance.
    pub async fn publish_current_root(&self, key: Blake2b256Hash) -> Result<bool, String> {
        let bytes = key.to_byte_array().to_vec();
        if !self.is_known_root(key).await? {
            return Ok(false);
        }
        self.put(CURRENT_ROOT.to_vec(), bytes).await?;
        Ok(true)
    }

    /// Record `key` as a known root and set it as current (port of `recordRoot`).
    ///
    /// **One call, not two** (the programme's L2). The two writes are one fact — "this root exists, and
    /// it is the current one" — and splitting them across two `put`s gave the store the chance to commit
    /// the second without the first: a crash in between leaves `CURRENT_ROOT` naming a root the store
    /// does not know, and every read that resolves through it then fails with no explanation. The store
    /// opens one transaction per call (`shared/src/lmdb.rs`), so batching the pairs is what makes the
    /// pair atomic rather than merely adjacent.
    pub async fn record_root(&self, key: Blake2b256Hash) -> Result<(), String> {
        let bytes = key.to_byte_array().to_vec();
        self.store.lock().await.put(vec![
            (bytes.clone(), ROOT_TAG.to_vec()),
            (CURRENT_ROOT.to_vec(), bytes),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_shared::store::InMemoryKeyValueStore;
    use std::sync::Arc;

    fn store() -> RootsStore {
        RootsStore::new(Arc::new(tokio::sync::Mutex::new(
            Box::new(InMemoryKeyValueStore::default())
                as Box<dyn rchain_shared::store::KeyValueStore + Send + Sync>,
        )))
    }

    #[tokio::test]
    async fn current_root_is_none_before_anything_is_recorded() {
        assert_eq!(store().current_root().await.expect("read"), None);
    }

    /// **A `current-root` that is not 32 bytes is an error, not a panic** (AUDIT C164).
    ///
    /// `Blake2b256Hash::from_byte_array` asserts its length, and this read used to hand it whatever
    /// the store returned — so a truncated or corrupted value panicked the node on the path every
    /// state read goes through, instead of saying what was wrong.
    ///
    /// Two arms, because which way it fails is the point: the wrong length is refused **naming both
    /// lengths**, as `Blake2b256HashCodec::decode` does, and a well-formed value still reads — so
    /// this is a length check and not a read that refuses everything.
    #[tokio::test]
    async fn a_truncated_current_root_is_refused_rather_than_panicking() {
        let roots = store();
        // Written through the same store the port writes through: the node's own writes are always
        // 32 bytes, so this is the corrupt-or-truncated case, which is the one that panicked.
        roots
            .put(CURRENT_ROOT.to_vec(), vec![0u8; 31])
            .await
            .expect("a corrupt store");

        let err = roots
            .current_root()
            .await
            .expect_err("31 bytes is not a Blake2b256Hash");
        assert!(
            err.contains("31") && err.contains("32"),
            "the refusal names both lengths, as the codec's does: {err}"
        );

        let good = Blake2b256Hash::from_bytes([0x11; 32]);
        roots
            .put(CURRENT_ROOT.to_vec(), good.to_byte_array().to_vec())
            .await
            .expect("a good store");
        assert_eq!(roots.current_root().await.expect("read"), Some(good));
    }

    /// The guard: a root the store has never seen is **refused**, and refusing it leaves the current
    /// root untouched. If this returned the root anyway, a node could adopt a root it cannot walk —
    /// a state pointing at history it does not have.
    #[tokio::test]
    async fn validate_and_set_refuses_an_unknown_root() {
        let roots = store();
        let unknown = Blake2b256Hash::from_bytes([0xAB; 32]);

        assert_eq!(
            roots
                .validate_and_set_current_root(unknown)
                .await
                .expect("read"),
            None,
            "an unknown root must not be adopted"
        );
        assert_eq!(
            roots.current_root().await.expect("read"),
            None,
            "a refused root must not become current"
        );
    }

    /// Recording a root makes it known *and* current, and afterwards the validated setter accepts it
    /// — the two paths agree on what "known" means.
    #[tokio::test]
    async fn a_recorded_root_round_trips_and_is_then_acceptable() {
        let roots = store();
        let root = Blake2b256Hash::from_bytes([0x11; 32]);

        roots.record_root(root).await.expect("record");
        assert_eq!(roots.current_root().await.expect("read"), Some(root));

        // Recording a *later* root must not forget the earlier one: the store keeps every recorded
        // root as known and moves only the current pointer, so a validation that walks back to a
        // previous root still succeeds.
        let later = Blake2b256Hash::from_bytes([0x22; 32]);
        roots.record_root(later).await.expect("record later");
        assert_eq!(roots.current_root().await.expect("read"), Some(later));

        assert_eq!(
            roots
                .validate_and_set_current_root(root)
                .await
                .expect("read"),
            Some(root),
            "the earlier root is still known"
        );
        assert_eq!(
            roots.current_root().await.expect("read"),
            Some(root),
            "and becomes current again"
        );
    }

    /// **Recording is not publishing** (law 80, B40, HAZOP F-U17-10). A checkpoint records a
    /// candidate post-state as known; the acceptance step is what moves `CURRENT_ROOT`, and it moves
    /// only to a root the store already knows.
    #[tokio::test]
    async fn a_deferred_commit_records_the_root_without_moving_current() {
        let roots = store();
        let accepted = Blake2b256Hash::from_bytes([0x11; 32]);
        let candidate = Blake2b256Hash::from_bytes([0x22; 32]);
        roots
            .record_root(accepted)
            .await
            .expect("the accepted root");

        roots
            .record_known_root(candidate)
            .await
            .expect("the candidate root");
        assert_eq!(
            roots.current_root().await.expect("read"),
            Some(accepted),
            "a known root is not current until it is published"
        );

        assert!(roots
            .publish_current_root(candidate)
            .await
            .expect("publish"));
        assert_eq!(roots.current_root().await.expect("read"), Some(candidate));

        // An unknown root is refused rather than written: a publish may only name a state a
        // checkpoint recorded.
        assert!(!roots
            .publish_current_root(Blake2b256Hash::from_bytes([0xAB; 32]))
            .await
            .expect("refused"));
        assert_eq!(roots.current_root().await.expect("read"), Some(candidate));
    }
}
