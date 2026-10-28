//! Approved (finalized-fringe) store.
//!
//! Mirrors `block-storage/src/main/scala/coop/rchain/blockstorage/ApprovedStore.scala`. The store is
//! keyed by a single byte (the finalized-fringe key) and holds a `FinalizedFringe` protobuf.

use std::sync::Arc;

use rchain_models::casper::protocol::casper_message::FinalizedFringe;
use rchain_shared::store_manager::KeyValueStoreManager;
use rchain_shared::typed_store::{KeyValueTypedStore, KeyValueTypedStoreCodec};

use crate::dag::codecs::{ByteCodec, FringeCodec};

/// A typed store from a single-byte key to a finalized fringe (port of `ApprovedStore[F]`).
pub type ApprovedStore = Arc<dyn KeyValueTypedStore<u8, FinalizedFringe>>;

/// The finalized-fringe store key (the single key written by the approved store).
pub const FINALIZED_FRINGE_KEY: u8 = 42;

/// Open the approved store from a store manager (port of `approvedStore.create[F](kvm)`).
pub async fn create(kvm: &dyn KeyValueStoreManager) -> Result<ApprovedStore, String> {
    let store = kvm.store("finalized-store").await?;
    Ok(Arc::new(KeyValueTypedStoreCodec::new(
        store,
        Arc::new(ByteCodec),
        Arc::new(FringeCodec),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_shared::store_manager::InMemoryStoreManager;

    use crate::test_support::fringe;

    /// The key is the Scala's `approvedBlockKey = 42`, not an arbitrary constant: a node that wrote
    /// its fringe under an earlier build (or the Scala) must find it under this one.
    #[test]
    fn the_store_key_is_the_scala_approved_block_key() {
        assert_eq!(FINALIZED_FRINGE_KEY, 42);
    }

    /// `create` opens the store by *name*, so two opens over the same manager are the same store —
    /// which is what makes the fringe survive a restart — and a second manager is a different one.
    /// The values go through `FringeCodec` (protobuf), so the round trip also pins that the codec is
    /// the one wired in here and not, say, the block-message codec.
    #[tokio::test]
    async fn opening_the_store_again_finds_what_was_written() {
        let kvm = InMemoryStoreManager::default();
        let first = create(&kvm).await.expect("open");
        assert_eq!(
            first.get(&[FINALIZED_FRINGE_KEY]).await.expect("get")[0],
            None,
            "an empty store has no fringe"
        );

        let stored = fringe();
        first
            .put(&[(FINALIZED_FRINGE_KEY, stored.clone())])
            .await
            .expect("put");

        let reopened = create(&kvm).await.expect("reopen");
        assert_eq!(
            reopened.get(&[FINALIZED_FRINGE_KEY]).await.expect("get")[0],
            Some(stored.clone()),
            "the same manager hands back the same store"
        );

        // A different manager starts empty: the item lives in the named store, not in a static.
        let other = create(&InMemoryStoreManager::default())
            .await
            .expect("open");
        assert_eq!(
            other.get(&[FINALIZED_FRINGE_KEY]).await.expect("get")[0],
            None
        );
    }

    /// `get` is a **batch** call: the slice is a list of keys, and the result is positionally
    /// aligned with it — absent keys come back as `None` in their own slot rather than being dropped,
    /// which is what lets a caller zip keys against results.
    #[tokio::test]
    async fn a_batch_lookup_is_positionally_aligned_and_absent_keys_stay_in_their_slot() {
        let store = create(&InMemoryStoreManager::default())
            .await
            .expect("open");

        // Two distinct keys: the store's own key, and any other byte (only 42 is ever written).
        assert_eq!(
            store.get(&[FINALIZED_FRINGE_KEY, 7]).await.expect("get"),
            vec![None, None],
            "an empty store answers one `None` per key"
        );

        let stored = fringe();
        store
            .put(&[(FINALIZED_FRINGE_KEY, stored.clone())])
            .await
            .expect("put");
        assert_eq!(
            store.get(&[7, FINALIZED_FRINGE_KEY]).await.expect("get"),
            vec![None, Some(stored)],
            "the hit is in the second slot, matching the second key"
        );
    }

    /// A manager that cannot open the store makes `create` return that error rather than panicking
    /// or handing back a store that would fail on first use — the node's startup path depends on
    /// being able to report "the database would not open".
    #[tokio::test]
    async fn a_manager_that_cannot_open_the_store_surfaces_the_error() {
        struct Broken;
        #[async_trait::async_trait]
        impl rchain_shared::store_manager::KeyValueStoreManager for Broken {
            async fn store(
                &self,
                _name: &str,
            ) -> Result<rchain_shared::typed_store::SharedStore, String> {
                Err("the database would not open".to_string())
            }
            async fn shutdown(&self) {}
        }

        // `map(|_| ())` so the `Ok` side is `Debug`: the store type is a trait object, which is not.
        let err = create(&Broken)
            .await
            .map(|_| ())
            .expect_err("broken manager");
        assert_eq!(err, "the database would not open");
    }
}
