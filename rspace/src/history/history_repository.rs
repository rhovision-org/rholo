//! The top-level history repository: checkpoint, reset, and reader construction.
//!
//! Mirrors `HistoryRepository.scala` + `HistoryRepositoryImpl.scala`.

use std::marker::PhantomData;
use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_shared::serialize::Serialize;

use crate::hashing::stable_hash_provider::{hash_channel, hash_channels};
use crate::history::cold_store::{ColdKeyValueStore, PersistedData};
use crate::history::history::History;
use crate::history::history_action::HistoryAction;
use crate::history::history_reader::HistoryReader;
use crate::history::instances::rspace_history_reader_impl::RSpaceHistoryReaderImpl;
use crate::history::key_segment::KeySegment;
use crate::history::root_repository::RootRepository;
use crate::hot_store_action::HotStoreAction;
use crate::hot_store_trie_action::HotStoreTrieAction;
use crate::native_store::{NativeHistoryReader, NativeStoreAction};
use crate::serializers::scodec_serialize::{
    encode_continuations, encode_continuations_binary, encode_datums, encode_datums_binary,
    encode_joins, encode_joins_binary,
};

const PREFIX_DATUM: u8 = 0x00;
const PREFIX_KONT: u8 = 0x01;
const PREFIX_JOINS: u8 = 0x02;

/// The history repository (port of `HistoryRepository` / `HistoryRepositoryImpl`).
pub struct HistoryRepository<C, P, A, K> {
    current_history: Arc<dyn History>,
    roots_repository: Arc<RootRepository>,
    leaf_store: ColdKeyValueStore,
    marker: PhantomData<(C, P, A, K)>,
}

/// A cold-store write: leaf hash + leaf content (port of `ColdAction`).
type ColdAction = (Blake2b256Hash, Option<PersistedData>);

fn key_segment(prefix: u8, hash: Blake2b256Hash) -> KeySegment {
    let mut bytes = vec![prefix];
    bytes.extend_from_slice(hash.as_bytes());
    // One prefix byte plus a 32-byte hash is 33 bytes, well inside the 127-byte invariant, so the
    // constructor cannot refuse and this stays total (`expect` states the measurement the deleted
    // `new` used to take on trust).
    KeySegment::try_from(bytes).expect("1 + 32 = 33 bytes is at most 127 bytes")
}

fn calculate_storage_action<C, P, A, K>(
    action: &HotStoreTrieAction<C, P, A, K>,
) -> (ColdAction, HistoryAction)
where
    A: Serialize<A>,
    P: Serialize<P>,
    K: Serialize<K>,
    C: Serialize<C>,
{
    match action {
        HotStoreTrieAction::TrieInsertProduce(hash, data) => {
            let leaf = PersistedData::DataLeaf(encode_datums(data));
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(PREFIX_DATUM, *hash),
                    hash: leaf_hash,
                },
            )
        }
        HotStoreTrieAction::TrieInsertConsume(hash, conts) => {
            let leaf = PersistedData::ContinuationsLeaf(encode_continuations(conts));
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(PREFIX_KONT, *hash),
                    hash: leaf_hash,
                },
            )
        }
        HotStoreTrieAction::TrieInsertJoins(hash, joins) => {
            let leaf = PersistedData::JoinsLeaf(encode_joins(joins));
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(PREFIX_JOINS, *hash),
                    hash: leaf_hash,
                },
            )
        }
        HotStoreTrieAction::TrieInsertBinaryProduce(hash, data) => {
            let leaf = PersistedData::DataLeaf(encode_datums_binary(data));
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(PREFIX_DATUM, *hash),
                    hash: leaf_hash,
                },
            )
        }
        HotStoreTrieAction::TrieInsertBinaryConsume(hash, conts) => {
            let leaf = PersistedData::ContinuationsLeaf(encode_continuations_binary(conts));
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(PREFIX_KONT, *hash),
                    hash: leaf_hash,
                },
            )
        }
        HotStoreTrieAction::TrieInsertBinaryJoins(hash, joins) => {
            let leaf = PersistedData::JoinsLeaf(encode_joins_binary(joins));
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(PREFIX_JOINS, *hash),
                    hash: leaf_hash,
                },
            )
        }
        HotStoreTrieAction::TrieDeleteProduce(hash) => (
            (*hash, None),
            HistoryAction::Delete {
                key: key_segment(PREFIX_DATUM, *hash),
            },
        ),
        HotStoreTrieAction::TrieDeleteConsume(hash) => (
            (*hash, None),
            HistoryAction::Delete {
                key: key_segment(PREFIX_KONT, *hash),
            },
        ),
        HotStoreTrieAction::TrieDeleteJoins(hash) => (
            (*hash, None),
            HistoryAction::Delete {
                key: key_segment(PREFIX_JOINS, *hash),
            },
        ),
    }
}

/// Map a native-state mutation to a cold-store leaf + radix-history action.
fn calculate_native_storage_action(action: &NativeStoreAction) -> (ColdAction, HistoryAction) {
    match action {
        NativeStoreAction::Put { prefix, key, value } => {
            let leaf = PersistedData::NativeLeaf(value.clone());
            let leaf_hash =
                Blake2b256Hash::create(&crate::history::cold_store::encode_persisted_data(&leaf));
            (
                (leaf_hash, Some(leaf)),
                HistoryAction::Insert {
                    key: key_segment(*prefix, *key),
                    hash: leaf_hash,
                },
            )
        }
        NativeStoreAction::Delete { prefix, key } => (
            (*key, None),
            HistoryAction::Delete {
                key: key_segment(*prefix, *key),
            },
        ),
    }
}

fn transform<C, P, A, K>(action: &HotStoreAction<C, P, A, K>) -> HotStoreTrieAction<C, P, A, K>
where
    C: Serialize<C> + Clone,
    P: Clone,
    A: Clone,
    K: Clone,
{
    match action {
        HotStoreAction::InsertData(channel, data) => {
            HotStoreTrieAction::TrieInsertProduce(hash_channel(channel), data.to_vec())
        }
        HotStoreAction::InsertContinuations(channels, conts) => {
            HotStoreTrieAction::TrieInsertConsume(hash_channels(channels), conts.to_vec())
        }
        HotStoreAction::InsertJoins(channel, joins) => {
            HotStoreTrieAction::TrieInsertJoins(hash_channel(channel), joins.to_vec())
        }
        HotStoreAction::DeleteData(channel) => {
            HotStoreTrieAction::TrieDeleteProduce(hash_channel(channel))
        }
        HotStoreAction::DeleteContinuations(channels) => {
            HotStoreTrieAction::TrieDeleteConsume(hash_channels(channels))
        }
        HotStoreAction::DeleteJoins(channel) => {
            HotStoreTrieAction::TrieDeleteJoins(hash_channel(channel))
        }
    }
}

impl<C, P, A, K> HistoryRepository<C, P, A, K> {
    pub fn new(
        current_history: Arc<dyn History>,
        roots_repository: Arc<RootRepository>,
        leaf_store: ColdKeyValueStore,
    ) -> Self {
        HistoryRepository {
            current_history,
            roots_repository,
            leaf_store,
            marker: PhantomData,
        }
    }

    pub fn root(&self) -> Blake2b256Hash {
        self.current_history.root()
    }

    pub fn history(&self) -> Arc<dyn History> {
        self.current_history.clone()
    }

    pub async fn checkpoint(
        &self,
        actions: &[HotStoreAction<C, P, A, K>],
    ) -> Result<Arc<Self>, String>
    where
        C: Serialize<C> + Clone,
        P: Serialize<P> + Clone,
        A: Serialize<A> + Clone,
        K: Serialize<K> + Clone,
    {
        self.checkpoint_with_native(actions, &[]).await
    }

    /// Checkpoint both hot-store changes and native system-contract mutations into one new history
    /// root.
    pub async fn checkpoint_with_native(
        &self,
        actions: &[HotStoreAction<C, P, A, K>],
        native_actions: &[NativeStoreAction],
    ) -> Result<Arc<Self>, String>
    where
        C: Serialize<C> + Clone,
        P: Serialize<P> + Clone,
        A: Serialize<A> + Clone,
        K: Serialize<K> + Clone,
    {
        let trie_actions: Vec<HotStoreTrieAction<C, P, A, K>> =
            actions.iter().map(transform).collect();
        self.do_checkpoint_with_native(&trie_actions, native_actions)
            .await
    }

    pub async fn do_checkpoint(
        &self,
        trie_actions: &[HotStoreTrieAction<C, P, A, K>],
    ) -> Result<Arc<Self>, String>
    where
        C: Serialize<C> + Clone,
        P: Serialize<P> + Clone,
        A: Serialize<A> + Clone,
        K: Serialize<K> + Clone,
    {
        self.do_checkpoint_with_native(trie_actions, &[]).await
    }

    /// **The candidate-state checkpoint**: the same fold and the same cold writes as
    /// [`Self::checkpoint_with_native`], but the new root is recorded as **known** and `CURRENT_ROOT`
    /// is not moved (law 80, B40, HAZOP F-U17-10). A replay or a play that has not yet been accepted
    /// must not publish the post-state it computed; the acceptance step calls
    /// [`Self::publish_if_known`] once the block is in the DAG.
    pub async fn checkpoint_with_native_deferred(
        &self,
        actions: &[HotStoreAction<C, P, A, K>],
        native_actions: &[NativeStoreAction],
    ) -> Result<Arc<Self>, String>
    where
        C: Serialize<C> + Clone,
        P: Serialize<P> + Clone,
        A: Serialize<A> + Clone,
        K: Serialize<K> + Clone,
    {
        let trie_actions: Vec<HotStoreTrieAction<C, P, A, K>> =
            actions.iter().map(transform).collect();
        self.do_checkpoint_native_impl(&trie_actions, native_actions, false)
            .await
    }

    /// Move `CURRENT_ROOT` to `root` if the store already knows it (law 80, B40). Returns whether the
    /// pointer moved; `Ok(false)` means no checkpoint recorded that state, so it is not publishable.
    pub async fn publish_if_known(&self, root: Blake2b256Hash) -> Result<bool, String> {
        self.roots_repository.publish_if_known(root).await
    }

    /// The durable `CURRENT_ROOT`, read without recording the empty root — the pointer law 80 is
    /// about, as distinct from [`Self::root`] (the in-memory history's root).
    pub async fn published_root(&self) -> Result<Option<Blake2b256Hash>, String> {
        self.roots_repository.published_root().await
    }

    pub async fn do_checkpoint_with_native(
        &self,
        trie_actions: &[HotStoreTrieAction<C, P, A, K>],
        native_actions: &[NativeStoreAction],
    ) -> Result<Arc<Self>, String>
    where
        C: Serialize<C> + Clone,
        P: Serialize<P> + Clone,
        A: Serialize<A> + Clone,
        K: Serialize<K> + Clone,
    {
        self.do_checkpoint_native_impl(trie_actions, native_actions, true)
            .await
    }

    async fn do_checkpoint_native_impl(
        &self,
        trie_actions: &[HotStoreTrieAction<C, P, A, K>],
        native_actions: &[NativeStoreAction],
        publish: bool,
    ) -> Result<Arc<Self>, String>
    where
        C: Serialize<C> + Clone,
        P: Serialize<P> + Clone,
        A: Serialize<A> + Clone,
        K: Serialize<K> + Clone,
    {
        let mut cold_actions: Vec<(Blake2b256Hash, PersistedData)> = Vec::new();
        let mut history_actions: Vec<HistoryAction> = Vec::new();
        for action in trie_actions {
            let (cold, history) = calculate_storage_action(action);
            if let Some(leaf) = cold.1 {
                cold_actions.push((cold.0, leaf));
            }
            history_actions.push(history);
        }
        for action in native_actions {
            let (cold, history) = calculate_native_storage_action(action);
            if let Some(leaf) = cold.1 {
                cold_actions.push((cold.0, leaf));
            }
            history_actions.push(history);
        }

        // Write cold leaves (put-if-absent).
        if !cold_actions.is_empty() {
            let keys: Vec<Blake2b256Hash> = cold_actions.iter().map(|(k, _)| *k).collect();
            let present = self.leaf_store.contains(&keys).await?;
            let absent: Vec<(Blake2b256Hash, PersistedData)> = cold_actions
                .iter()
                .zip(present.iter())
                .filter(|(_, &p)| !p)
                .map(|((k, v), _)| (*k, v.clone()))
                .collect();
            self.leaf_store.put(&absent).await?;
        }

        // Apply radix-history actions and record the new root. A candidate checkpoint records it as
        // known only; the acceptance step publishes it (law 80, B40).
        let new_history = self.current_history.process(&history_actions).await?;
        if publish {
            self.roots_repository.commit(new_history.root()).await?;
        } else {
            self.roots_repository
                .commit_deferred(new_history.root())
                .await?;
        }

        Ok(Arc::new(HistoryRepository {
            current_history: new_history,
            roots_repository: self.roots_repository.clone(),
            leaf_store: self.leaf_store.clone(),
            marker: PhantomData,
        }))
    }

    pub async fn reset(&self, root: Blake2b256Hash) -> Result<Arc<Self>, String>
    where
        C: Serialize<C>,
        P: Serialize<P>,
        A: Serialize<A>,
        K: Serialize<K>,
    {
        self.roots_repository
            .validate_and_set_current_root(root)
            .await?;
        let next = self.current_history.reset(root).await?;
        Ok(Arc::new(HistoryRepository {
            current_history: next,
            roots_repository: self.roots_repository.clone(),
            leaf_store: self.leaf_store.clone(),
            marker: PhantomData,
        }))
    }

    /// **Reset without publishing** — the programme's L2, and the merge's half of the split.
    ///
    /// [`Self::reset`] advances `CURRENT_ROOT` as it resets, which is right for a caller whose new root
    /// *is* the base: a rewind, a rejoin, a replay. It is wrong for the **merge**, which resets to its
    /// base and then does real work before the merged trie exists — `do_checkpoint_with_native` is
    /// where the merge's root appears. With `reset`, a crash between the two leaves `CURRENT_ROOT`
    /// naming an **ancestor**: on restart the node resumes from neither the pre-merge nor the
    /// post-merge state, and the two commit points are the reason.
    ///
    /// The **validation is kept** (`validate_known_root`, the same refusal `reset` gives an unknown
    /// root); what is dropped is the write. The merge therefore has one commit point, and it is the one
    /// where the merged state actually exists.
    pub async fn reset_volatile(&self, root: Blake2b256Hash) -> Result<Arc<Self>, String>
    where
        C: Serialize<C>,
        P: Serialize<P>,
        A: Serialize<A>,
        K: Serialize<K>,
    {
        self.roots_repository.validate_known_root(root).await?;
        let next = self.current_history.reset(root).await?;
        Ok(Arc::new(HistoryRepository {
            current_history: next,
            roots_repository: self.roots_repository.clone(),
            leaf_store: self.leaf_store.clone(),
            marker: PhantomData,
        }))
    }

    pub async fn get_history_reader(
        &self,
        state_hash: Blake2b256Hash,
    ) -> Arc<dyn HistoryReader<C, P, A, K>>
    where
        C: Serialize<C> + Send + Sync + 'static,
        P: Serialize<P> + Send + Sync + 'static,
        A: Serialize<A> + Send + Sync + 'static,
        K: Serialize<K> + Send + Sync + 'static,
    {
        // The oracle builds this in `F` (`getHistoryReader: F[HistoryReader]`); the port's signature
        // is total and its callers are `casper`/`node`, so an unreadable root is carried *into* the
        // reader and answered on the first read rather than becoming an empty-rooted reader that
        // reports "no data" for every key (AUDIT C53).
        Arc::new(match self.current_history.reset(state_hash).await {
            Ok(history) => RSpaceHistoryReaderImpl::new(history, self.leaf_store.clone()),
            Err(e) => RSpaceHistoryReaderImpl::unreadable(e, state_hash, self.leaf_store.clone()),
        })
    }

    /// A reader of native system-contract state rooted at `state_hash`.
    pub async fn get_native_reader(
        &self,
        state_hash: Blake2b256Hash,
    ) -> Arc<dyn NativeHistoryReader>
    where
        C: Serialize<C> + Send + Sync + 'static,
        P: Serialize<P> + Send + Sync + 'static,
        A: Serialize<A> + Send + Sync + 'static,
        K: Serialize<K> + Send + Sync + 'static,
    {
        Arc::new(match self.current_history.reset(state_hash).await {
            Ok(history) => {
                RSpaceHistoryReaderImpl::<C, P, A, K>::new(history, self.leaf_store.clone())
            }
            Err(e) => RSpaceHistoryReaderImpl::<C, P, A, K>::unreadable(
                e,
                state_hash,
                self.leaf_store.clone(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::create_history_repository;
    use rchain_shared::store_manager::InMemoryStoreManager;

    // The repository is generic over the channel/pattern/leaf/continuation types; the root machinery
    // under test is independent of them, so the concrete choice is arbitrary (as in `hot_store`'s
    // tests).
    type Repo = HistoryRepository<String, String, String, String>;

    async fn repository(manager: &InMemoryStoreManager) -> Arc<Repo> {
        create_history_repository::<String, String, String, String>(manager, "rspace")
            .await
            .expect("history repository")
    }

    /// **The root is read from the store, not defaulted per process.** Two repositories built over
    /// the same stores report the same root, so a node that reopens its data directory continues the
    /// chain it was on rather than starting a fresh trie beside the old one. That is the property the
    /// state hash of every subsequent block depends on.
    #[tokio::test]
    async fn two_repositories_over_the_same_stores_agree_on_the_root() {
        let manager = InMemoryStoreManager::default();
        let first = repository(&manager).await;
        let second = repository(&manager).await;
        assert_eq!(first.root(), second.root());

        // And a root the first one resets to is visible to a repository built afterwards (the reset
        // went through the shared roots store).
        let empty = crate::history::history::empty_root_hash_value();
        let moved = first.reset(empty).await.expect("reset to the empty root");
        assert_eq!(moved.root(), empty);
        let third = repository(&manager).await;
        assert_eq!(third.root(), empty);
    }

    /// **The merge's reset still validates, and it still moves the in-memory view** (the L2 gap).
    ///
    /// The *publication* half of this split is pinned where it can be observed —
    /// `root_repository::tests::validate_known_root_refuses_without_publishing`, which is the level
    /// that holds two known roots. What is pinned here is that `reset_volatile` is a reset and not a
    /// no-op: an unknown root is refused exactly as `reset` refuses it, and a known one produces a
    /// repository rooted at it.
    #[tokio::test]
    async fn a_volatile_reset_still_validates_and_still_moves_the_view() {
        let manager = InMemoryStoreManager::default();
        let repo = repository(&manager).await;
        let empty = crate::history::history::empty_root_hash_value();

        let unknown = Blake2b256Hash::from_bytes([0xCD; 32]);
        assert!(
            repo.reset_volatile(unknown).await.is_err(),
            "a root the store does not hold is still refused — the validation is not the half \
             that was dropped"
        );

        let volatile = repo.reset_volatile(empty).await.expect("a known root");
        assert_eq!(volatile.root(), empty, "the in-memory view moved");

        // The publishing reset is unchanged, which is what a rewind or a rejoin uses.
        let moved = repo.reset(empty).await.expect("reset to the empty root");
        assert_eq!(moved.root(), empty);
        assert_eq!(
            repository(&manager).await.root(),
            empty,
            "the publishing reset survives a restart, as it must"
        );
    }

    /// Resetting to a root the store has never recorded is an error, and the repository keeps its
    /// current root — a node cannot be pointed at history it does not hold, and cannot be left
    /// believing it moved.
    #[tokio::test]
    async fn reset_to_an_unknown_root_is_an_error() {
        let manager = InMemoryStoreManager::default();
        let repo = repository(&manager).await;
        let before = repo.root();

        let unknown = Blake2b256Hash::from_bytes([0xAB; 32]);
        // `Result::expect_err` needs `Debug` on the Ok type, which the repository does not derive —
        // match instead of demanding a `Debug` impl for a test's convenience.
        let err = match repo.reset(unknown).await {
            Ok(_) => panic!("an unknown root must be refused"),
            Err(err) => err,
        };
        assert!(err.contains("unknown root"), "{err}");
        assert_eq!(
            repo.root(),
            before,
            "a refused reset must not move the root"
        );
    }
}
