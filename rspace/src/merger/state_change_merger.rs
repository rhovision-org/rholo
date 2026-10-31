//! Turning a [`StateChange`] into hash-addressed trie actions (Law 9: deterministic merge).
//!
//! Mirrors `rspace/src/main/scala/coop/rchain/rspace/merger/StateChangeMerger.scala`.

use std::collections::BTreeMap;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;

use crate::hashing::stable_hash_provider::hash_hashes;
use crate::history::history_reader::HistoryReaderBinary;
use crate::hot_store_trie_action::HotStoreTrieAction;
use crate::merger::channel_change::ChannelChange;
use crate::merger::event_log_index::NumberChannelsDiff;
use crate::merger::seq_diff;
use crate::merger::state_change::StateChange;

/// A join to add or remove on merge (port of `StateChangeMerger.JoinAction`).
enum JoinAction {
    AddJoin(Vec<Blake2b256Hash>),
    RemoveJoin(Vec<Blake2b256Hash>),
}

impl JoinAction {
    fn channels(&self) -> &[Blake2b256Hash] {
        match self {
            JoinAction::AddJoin(channels) | JoinAction::RemoveJoin(channels) => channels,
        }
    }
}

/// Produce a single trie action from a channel change against the base reader (port of
/// `StateChangeMerger.mkTrieAction`).
async fn mk_trie_action<C, P, A, K, R, U>(
    history_pointer: Blake2b256Hash,
    init_value: Vec<Vec<u8>>,
    changes: &ChannelChange<Vec<u8>>,
    remove_action: R,
    update_action: U,
) -> Result<HotStoreTrieAction<C, P, A, K>, String>
where
    R: FnOnce(Blake2b256Hash) -> HotStoreTrieAction<C, P, A, K>,
    U: FnOnce(Blake2b256Hash, Vec<Vec<u8>>) -> HotStoreTrieAction<C, P, A, K>,
{
    let new_val = {
        let mut v = seq_diff(&init_value, &changes.removed);
        v.extend(changes.added.clone());
        v
    };
    if new_val.is_empty() && !init_value.is_empty() {
        Ok(remove_action(history_pointer))
    } else if init_value != new_val {
        Ok(update_action(history_pointer, new_val))
    } else {
        Err(
            "Merging logic error: empty channel change for produce or join when computing trie action."
                .to_string(),
        )
    }
}

/// Cancel matching byte-identical datum additions and removals across
/// accepted causal chains before replaying the net change against the base.
/// A producer in a seeded block, later consumed by the accepted descendant,
/// must not be reinserted when both chains enter the same merge.
fn net_datum_change(changes: &ChannelChange<Vec<u8>>) -> ChannelChange<Vec<u8>> {
    let mut added = changes.added.clone();
    let mut removed = Vec::new();
    for datum in &changes.removed {
        if let Some(index) = added.iter().position(|candidate| candidate == datum) {
            // Multiset subtraction removes exactly one occurrence.
            added.remove(index);
        } else {
            removed.push(datum.clone());
        }
    }
    ChannelChange { added, removed }
}

/// Compute the trie actions that apply a [`StateChange`] on top of a base state (port of
/// `StateChangeMerger.computeTrieActions`).
///
/// `handle_channel_change` is the mergeable-channel override hook; in the Scala it is effectful
/// (`=> F[Option[HotStoreTrieAction]]`), here it is a plain `Option`-returning closure.
pub async fn compute_trie_actions<C, P, A, K, F>(
    changes: &StateChange,
    base_reader: &(dyn HistoryReaderBinary<C, P, A, K> + Sync),
    mergeable_chs: NumberChannelsDiff,
    handle_channel_change: F,
) -> Result<Vec<HotStoreTrieAction<C, P, A, K>>, String>
where
    C: Send + Sync,
    P: Send + Sync,
    A: Send + Sync,
    K: Send + Sync,
    F: Fn(
        &Blake2b256Hash,
        &ChannelChange<Vec<u8>>,
        &NumberChannelsDiff,
    ) -> Option<HotStoreTrieAction<C, P, A, K>>,
{
    // Consume trie actions and the joins they add/remove.
    let mut consume_trie_actions: Vec<HotStoreTrieAction<C, P, A, K>> = Vec::new();
    let mut join_actions: Vec<JoinAction> = Vec::new();

    for (consume_channels, channel_change) in &changes.kont_changes {
        let history_pointer = hash_hashes(consume_channels);
        let init: Vec<Vec<u8>> = base_reader
            .get_continuations(history_pointer)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|wc| wc.raw)
            .collect();
        let new_val = {
            let mut v = seq_diff(&init, &channel_change.removed);
            v.extend(channel_change.added.clone());
            v
        };
        if init == new_val {
            return Err(
                "Merging logic error: empty consume change when computing trie action.".to_string(),
            );
        }
        if new_val.is_empty() && !init.is_empty() {
            consume_trie_actions.push(HotStoreTrieAction::TrieDeleteConsume(history_pointer));
            join_actions.push(JoinAction::RemoveJoin(consume_channels.clone()));
        } else if init.is_empty() {
            consume_trie_actions.push(HotStoreTrieAction::TrieInsertBinaryConsume(
                history_pointer,
                new_val,
            ));
            join_actions.push(JoinAction::AddJoin(consume_channels.clone()));
        } else {
            consume_trie_actions.push(HotStoreTrieAction::TrieInsertBinaryConsume(
                history_pointer,
                new_val,
            ));
        }
    }

    // Produce trie actions.
    let mut produce_trie_actions: Vec<HotStoreTrieAction<C, P, A, K>> = Vec::new();
    for (history_pointer, channel_change) in &changes.datums_changes {
        match handle_channel_change(history_pointer, channel_change, &mergeable_chs) {
            Some(action) => produce_trie_actions.push(action),
            None => {
                let net = net_datum_change(channel_change);
                if net.added.is_empty() && net.removed.is_empty() {
                    // Consumed data already produced by an accepted causal
                    // ancestor has no remaining effect on the base trie.
                    continue;
                }
                let init: Vec<Vec<u8>> = base_reader
                    .get_data(*history_pointer)
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|d| d.raw)
                    .collect();
                let action = mk_trie_action(
                    *history_pointer,
                    init,
                    &net,
                    HotStoreTrieAction::TrieDeleteProduce,
                    HotStoreTrieAction::TrieInsertBinaryProduce,
                )
                .await?;
                produce_trie_actions.push(action);
            }
        }
    }

    // Join trie actions: spread each join add/remove over its member channels.
    let mut joins_changes: BTreeMap<Blake2b256Hash, ChannelChange<Vec<u8>>> = BTreeMap::new();
    for join_action in &join_actions {
        let join_channels = join_action.channels();
        let join = changes
            .consume_channels_to_join_serialized_map
            .get(join_channels)
            .ok_or_else(|| {
                "No ByteVector value for join found when merging when computing trie action."
                    .to_string()
            })?;
        for c in join_channels {
            let cur = joins_changes.remove(c).unwrap_or_default();
            let new_val = match join_action {
                JoinAction::AddJoin(_) => ChannelChange {
                    added: [vec![join.clone()], cur.added].concat(),
                    removed: cur.removed,
                },
                JoinAction::RemoveJoin(_) => ChannelChange {
                    added: cur.added,
                    removed: [vec![join.clone()], cur.removed].concat(),
                },
            };
            joins_changes.insert(*c, new_val);
        }
    }

    let mut joins_trie_actions: Vec<HotStoreTrieAction<C, P, A, K>> = Vec::new();
    for (history_pointer, channel_change) in &joins_changes {
        let init: Vec<Vec<u8>> = base_reader
            .get_joins(*history_pointer)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|j| j.raw)
            .collect();
        let action = mk_trie_action(
            *history_pointer,
            init,
            channel_change,
            HotStoreTrieAction::TrieDeleteJoins,
            HotStoreTrieAction::TrieInsertBinaryJoins,
        )
        .await?;
        joins_trie_actions.push(action);
    }

    let mut result = produce_trie_actions;
    result.extend(consume_trie_actions);
    result.extend(joins_trie_actions);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::internal::WaitingContinuation;
    use crate::serializers::scodec_serialize::{DatumB, JoinsB, WaitingContinuationB};
    use crate::trace::event::Consume;

    fn hash(n: u8) -> Blake2b256Hash {
        Blake2b256Hash::from_bytes([n; 32])
    }

    fn change(added: Vec<Vec<u8>>, removed: Vec<Vec<u8>>) -> ChannelChange<Vec<u8>> {
        ChannelChange { added, removed }
    }

    /// `JoinAction`'s two variants answer the same channel list — the merge reads the channels off
    /// whichever action it built, so a variant that carried them differently would send the trie
    /// action to the wrong place.
    #[test]
    fn join_actions_report_their_channels() {
        let channels = vec![hash(1), hash(2)];
        assert_eq!(JoinAction::AddJoin(channels.clone()).channels(), channels);
        assert_eq!(
            JoinAction::RemoveJoin(channels.clone()).channels(),
            channels
        );
    }

    /// The three outcomes of a channel change: everything removed ⇒ a removal action; some content
    /// left ⇒ an update action carrying exactly the new value. The value is `init − removed + added`,
    /// so a removal actually takes effect and an addition lands (Law 9's deterministic merge).
    #[tokio::test]
    async fn a_channel_change_produces_a_remove_or_an_update_action() {
        type Action = HotStoreTrieAction<String, String, String, String>;

        // Everything removed and nothing added: the channel is emptied.
        let removed = mk_trie_action::<String, String, String, String, _, _>(
            hash(9),
            vec![b"a".to_vec(), b"b".to_vec()],
            &change(vec![], vec![b"a".to_vec(), b"b".to_vec()]),
            |pointer| Action::TrieDeleteProduce(pointer),
            |_, _| panic!("an emptied channel must be removed, not updated"),
        )
        .await
        .expect("removal");
        assert!(
            matches!(removed, Action::TrieDeleteProduce(..)),
            "{removed:?}"
        );

        // A partial change: the update action carries the survivors plus the addition.
        let updated = mk_trie_action::<String, String, String, String, _, _>(
            hash(9),
            vec![b"a".to_vec(), b"b".to_vec()],
            &change(vec![b"c".to_vec()], vec![b"a".to_vec()]),
            |_| panic!("a non-empty channel must be updated, not removed"),
            |_, value| Action::TrieInsertBinaryProduce(Blake2b256Hash::from_bytes([0; 32]), value),
        )
        .await
        .expect("update");
        match updated {
            Action::TrieInsertBinaryProduce(_, value) => {
                assert_eq!(value, vec![b"b".to_vec(), b"c".to_vec()]);
            }
            other => panic!("expected an update, got {other:?}"),
        }
    }

    /// A reader over three maps, with a switch to make every read fail — the same double
    /// `merger/state_change.rs`'s tests use, copied here because a `#[cfg(test)]` item in a sibling
    /// module is not reachable from this one and widening it for a test would be a production change.
    ///
    /// The failure arm is the point of having it: `compute_trie_actions` reads the base state through
    /// this trait, and a read error that were flattened into "empty" would build a *wrong* trie action
    /// (an insert where nothing was there, a delete where something was) rather than refusing.
    #[derive(Default)]
    struct MockReader {
        data: BTreeMap<Blake2b256Hash, Vec<DatumB<String>>>,
        konts: BTreeMap<Blake2b256Hash, Vec<WaitingContinuationB<String, String>>>,
        joins: BTreeMap<Blake2b256Hash, Vec<JoinsB<String>>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl HistoryReaderBinary<String, String, String, String> for MockReader {
        async fn get_data(
            &self,
            key: Blake2b256Hash,
        ) -> Result<Vec<DatumB<String>>, crate::errors::RSpaceError> {
            if self.fail {
                return Err(crate::errors::RSpaceError::Codec("test reader failure"));
            }
            Ok(self.data.get(&key).cloned().unwrap_or_default())
        }
        async fn get_continuations(
            &self,
            key: Blake2b256Hash,
        ) -> Result<Vec<WaitingContinuationB<String, String>>, crate::errors::RSpaceError> {
            if self.fail {
                return Err(crate::errors::RSpaceError::Codec("test reader failure"));
            }
            Ok(self.konts.get(&key).cloned().unwrap_or_default())
        }
        async fn get_joins(
            &self,
            key: Blake2b256Hash,
        ) -> Result<Vec<JoinsB<String>>, crate::errors::RSpaceError> {
            if self.fail {
                return Err(crate::errors::RSpaceError::Codec("test reader failure"));
            }
            Ok(self.joins.get(&key).cloned().unwrap_or_default())
        }
    }

    fn raw(byte: u8) -> Vec<u8> {
        vec![byte]
    }

    /// A byte-view continuation. The merger reads only `.raw`; the `decoded` half must still be a
    /// real value, so it is built the way the sibling module's tests build theirs.
    fn continuation(raw: Vec<u8>) -> WaitingContinuationB<String, String> {
        WaitingContinuationB {
            decoded: WaitingContinuation {
                patterns: Vec::new(),
                continuation: String::new(),
                persist: false,
                peeks: std::collections::BTreeSet::new(),
                source: Consume::apply(
                    &["c".to_string()],
                    &["p".to_string()],
                    &"k".to_string(),
                    false,
                ),
            },
            raw,
        }
    }

    fn no_hook(
    ) -> impl Fn(&Blake2b256Hash, &ChannelChange<Vec<u8>>, &NumberChannelsDiff) -> Option<Action>
    {
        |_, _, _| None
    }

    type Action = HotStoreTrieAction<String, String, String, String>;

    /// Matching exact serialized bytes cancel one occurrence, not an
    /// unrelated datum or another copy of the same serialized datum.
    #[test]
    fn net_datum_changes_preserve_multiset_cardinality() {
        let source = change(vec![raw(0), raw(2), raw(0)], vec![raw(0), raw(3)]);
        let net = net_datum_change(&source);
        assert_eq!(net.added, vec![raw(2), raw(0)]);
        assert_eq!(net.removed, vec![raw(3)]);
        assert_eq!(
            net_datum_change(&change(vec![raw(0)], vec![raw(0)])),
            change(vec![], vec![]),
        );
    }

    /// A seed that is fully consumed by a later accepted chain leaves no
    /// action at all when the old base did not hold that seed datum.
    #[tokio::test]
    async fn fully_consumed_causal_seed_has_no_net_trie_action() {
        let channel = hash(9);
        let seed = StateChange {
            datums_changes: BTreeMap::from([(channel, change(vec![raw(0)], vec![]))]),
            ..Default::default()
        };
        let consumer = StateChange {
            datums_changes: BTreeMap::from([(channel, change(vec![], vec![raw(0)]))]),
            ..Default::default()
        };
        let combined = StateChange::combine(&seed, &consumer);
        let actions = compute_trie_actions(
            &combined,
            &MockReader::default(),
            NumberChannelsDiff::new(),
            no_hook(),
        )
        .await
        .expect("exactly cancelled datum changes are the identity");
        assert!(actions.is_empty(), "consumed seed must not reappear");
    }

    /// A seed datum was produced, then consumed by the accepted branch.
    /// Replaying both chains from the old base must not resurrect the seed.
    #[tokio::test]
    async fn a_seed_then_consumed_linear_cell_has_only_the_winner_after_merge() {
        let channel = hash(9);
        let seed = StateChange {
            datums_changes: BTreeMap::from([(channel, change(vec![raw(0)], vec![]))]),
            ..Default::default()
        };
        let winner = StateChange {
            datums_changes: BTreeMap::from([(channel, change(vec![raw(2)], vec![raw(0)]))]),
            ..Default::default()
        };
        let combined = StateChange::combine(&seed, &winner);
        let actions = compute_trie_actions(
            &combined,
            &MockReader::default(),
            NumberChannelsDiff::new(),
            no_hook(),
        )
        .await
        .expect("accepted seed plus winner must be a valid transition");
        assert_eq!(actions.len(), 1, "one changed linear cell");
        match &actions[0] {
            Action::TrieInsertBinaryProduce(pointer, values) => {
                assert_eq!(*pointer, channel);
                assert_eq!(
                    values,
                    &vec![raw(2)],
                    "a consumed seed datum was resurrected next to the accepted update"
                );
            }
            other => panic!("unexpected merge action: {other:?}"),
        }
    }

    /// **`compute_trie_actions` is the merge's entry point, and only its per-channel helper had been
    /// tested.** The file's three tests all drive `mk_trie_action`, which takes the base value as an
    /// argument — so the *public* function, the one `casper/src/merging.rs` actually calls, had never
    /// run: the read of the base state, the three outcomes of a consume change (insert / delete /
    /// update), the join spread over the action's member channels, and the action ordering were all
    /// unexercised. Its refusals matter most: the trie actions it returns are applied to the store.
    #[tokio::test]
    async fn the_merge_entry_point_builds_actions_from_the_base_state() {
        let channels = vec![hash(1), hash(2)];

        // A consume change against an empty base: the continuation is inserted, and its join is
        // *added* — spread over the channels the join names, which is what the join trie is keyed by.
        let add = StateChange {
            datums_changes: BTreeMap::new(),
            kont_changes: BTreeMap::from([(channels.clone(), change(vec![raw(7)], vec![]))]),
            consume_channels_to_join_serialized_map: BTreeMap::from([(channels.clone(), raw(9))]),
        };
        let actions = compute_trie_actions(
            &add,
            &MockReader::default(),
            NumberChannelsDiff::new(),
            no_hook(),
        )
        .await
        .expect("an inserted continuation");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::TrieInsertBinaryConsume(..))),
            "the continuation is inserted: {actions:?}"
        );
        let join_inserts = actions
            .iter()
            .filter(|a| matches!(a, Action::TrieInsertBinaryJoins(..)))
            .count();
        assert_eq!(
            join_inserts, 2,
            "the join is added to each of the two channels it names: {actions:?}"
        );

        // The reverse: the base holds the continuation and the change removes it — the actions are the
        // delete pair, and the join is *removed* from both channels.
        let pointer = hash_hashes(&channels);
        let remove = StateChange {
            datums_changes: BTreeMap::new(),
            kont_changes: BTreeMap::from([(channels.clone(), change(vec![], vec![raw(7)]))]),
            consume_channels_to_join_serialized_map: BTreeMap::from([(channels.clone(), raw(9))]),
        };
        // The join trie is keyed by *each* channel the join names (the merger reads `get_joins` at the
        // channel's own hash), so a removal must find the join under both channels. An empty join map
        // here is not a smaller fixture but a different claim — the merger reports it as an
        // inconsistency, which is what the refusals test pins.
        let reader = MockReader {
            konts: BTreeMap::from([(pointer, vec![continuation(raw(7))])]),
            joins: channels
                .iter()
                .map(|c| {
                    (
                        *c,
                        vec![JoinsB {
                            decoded: Vec::new(),
                            raw: raw(9),
                        }],
                    )
                })
                .collect(),
            ..Default::default()
        };
        let actions = compute_trie_actions(&remove, &reader, NumberChannelsDiff::new(), no_hook())
            .await
            .expect("a removed continuation");
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::TrieDeleteConsume(..))),
            "the continuation is deleted: {actions:?}"
        );
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::TrieDeleteJoins(..)))
                .count(),
            2,
            "and the join is removed from both channels: {actions:?}"
        );
    }

    /// **The three refusals of the entry point.** Each is a state the merge must not paper over:
    ///
    ///   * a consume change that changes nothing means the caller and the merger disagree about the
    ///     channel — the same error `mk_trie_action` raises, but reached through the *public* function;
    ///   * a join action whose channels have no serialized join is a caller bug, and the message says
    ///     which lookup failed;
    ///   * a base-state read that **fails** must be reported, not read as an empty channel — an empty
    ///     base turns a delete into an insert, which is a wrong action applied to the store.
    #[tokio::test]
    async fn the_merge_entry_point_reports_what_it_cannot_merge() {
        let channels = vec![hash(1), hash(2)];

        // (a) `added` empty and `removed` empty against an empty base: nothing changed.
        let no_change = StateChange {
            datums_changes: BTreeMap::new(),
            kont_changes: BTreeMap::from([(channels.clone(), change(vec![], vec![]))]),
            consume_channels_to_join_serialized_map: BTreeMap::from([(channels.clone(), raw(9))]),
        };
        let err = compute_trie_actions(
            &no_change,
            &MockReader::default(),
            NumberChannelsDiff::new(),
            no_hook(),
        )
        .await
        .expect_err("a consume change that changes nothing is a disagreement");
        assert!(
            err.contains("Merging logic error") && err.contains("empty consume change"),
            "{err}"
        );

        // (b) a join action with no serialized join for its channels.
        let missing_join = StateChange {
            datums_changes: BTreeMap::new(),
            kont_changes: BTreeMap::from([(channels.clone(), change(vec![raw(7)], vec![]))]),
            consume_channels_to_join_serialized_map: BTreeMap::new(),
        };
        let err = compute_trie_actions(
            &missing_join,
            &MockReader::default(),
            NumberChannelsDiff::new(),
            no_hook(),
        )
        .await
        .expect_err("a join with no serialized value is a caller bug");
        assert!(err.contains("No ByteVector value for join"), "{err}");

        // (c) the reader fails: the error is propagated, in both the consume and the produce paths.
        let down = MockReader {
            fail: true,
            ..Default::default()
        };
        let err = compute_trie_actions(&missing_join, &down, NumberChannelsDiff::new(), no_hook())
            .await
            .expect_err("a base-state read that fails is not an empty channel");
        assert!(err.contains("test reader failure"), "{err}");
    }

    /// **A change that changes nothing is an error, not a no-op.** The merger is called only when
    /// there is a difference to merge, so `init == new` means the caller and the merger disagree
    /// about the channel's state — silently returning a no-op action would hide that and leave the
    /// trie action queue inconsistent with the values it was computed from.
    #[tokio::test]
    async fn an_empty_channel_change_is_reported_as_an_error() {
        type Action = HotStoreTrieAction<String, String, String, String>;

        let err = mk_trie_action::<String, String, String, String, _, _>(
            hash(9),
            vec![b"a".to_vec()],
            &change(vec![], vec![]),
            |_| Action::TrieDeleteProduce(Blake2b256Hash::from_bytes([0; 32])),
            |_, _| panic!("a no-op change must not reach an action"),
        )
        .await
        .expect_err("a change that changes nothing must be reported");
        assert!(err.contains("Merging logic error"), "{err}");

        // The degenerate all-empty case is the same error, not a removal of nothing.
        let err = mk_trie_action::<String, String, String, String, _, _>(
            hash(9),
            Vec::new(),
            &change(vec![], vec![]),
            |_| Action::TrieDeleteProduce(Blake2b256Hash::from_bytes([0; 32])),
            |_, _| panic!("a no-op change must not reach an action"),
        )
        .await
        .expect_err("an empty change against an empty channel is reported too");
        assert!(err.contains("Merging logic error"), "{err}");
    }
}
