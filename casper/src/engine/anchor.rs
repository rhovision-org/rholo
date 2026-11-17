//! **Trust anchors and the accepted state** (Law 108, sync/verification separation).
//!
//! A node runs from exactly one of two anchors: the operator's **pinned genesis** (a genesis-only
//! network, or a node starting from its own ceremony) or a **verified quorum checkpoint**. There is no
//! third source, and there is no "best available root" state.
//!
//! [`AcceptedState`]'s fields are private and its only constructors are [`AcceptedState::from_genesis`]
//! and [`AcceptedState::from_certificate`]. The root is derived from the anchor inside the
//! constructor, so a running node's root cannot be one its anchor does not name — the unsafe state the
//! prior launch path could reach by republishing `CURRENT_ROOT` from a DAG/tip heuristic.

use std::collections::BTreeSet;
use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::base16;

use crate::checkpoint::VerifiedCheckpoint;

/// **The operator's genesis pin.** The genesis block hash and the post-state its descriptors derive;
/// the genesis anchor accepts only this block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenesisPin {
    pub block: BlockHash,
    pub post_state: StateHash,
}

/// The trust anchor.
#[derive(Clone, Debug)]
pub enum TrustAnchor {
    /// A genesis-only network (or a node starting from its own genesis ceremony).
    Genesis(GenesisPin),
    /// A verified quorum checkpoint certificate and the checkpoint root it names.
    Checkpoint {
        verified: VerifiedCheckpoint,
        root: Blake2b256Hash,
        /// The checkpoint's anchor block, parsed once from the verified certificate.
        block: BlockHash,
        /// The certificate's finalized fringe, parsed once. Its members are the justifications a
        /// first post-checkpoint block may name; they are committed history, not a store read.
        fringe: BTreeSet<BlockHash>,
    },
}

impl TrustAnchor {
    /// The state root the node runs from.
    pub fn root(&self) -> Blake2b256Hash {
        match self {
            TrustAnchor::Genesis(pin) => Blake2b256Hash::from_byte_array(pin.post_state.as_bytes()),
            TrustAnchor::Checkpoint { root, .. } => *root,
        }
    }

    /// The block the anchor names: the pinned genesis, or the checkpoint's anchor block.
    pub fn block(&self) -> BlockHash {
        match self {
            TrustAnchor::Genesis(pin) => pin.block,
            TrustAnchor::Checkpoint { block, .. } => *block,
        }
    }
}

/// **The accepted state a running node carries.** Private fields; the constructors below are the only
/// ways to make one, and each derives the root from its anchor.
#[derive(Clone, Debug)]
pub struct AcceptedState {
    anchor: TrustAnchor,
    root: Blake2b256Hash,
    /// The certified validation window (Law 109): the historical roots a post-anchor block may read.
    window: BTreeSet<Blake2b256Hash>,
}

impl AcceptedState {
    /// **The genesis anchor.** The block must be the pinned genesis — height zero, the pinned hash and
    /// the pinned post-state. A bootstrap's unverified genesis is not enough.
    pub fn from_genesis(pin: GenesisPin, block: &BlockMessage) -> Result<Self, String> {
        if i64::from(block.block_number) != 0 {
            return Err(format!(
                "a genesis anchor requires a height-zero block; {} is at height {}",
                block.block_hash.to_hex(),
                block.block_number
            ));
        }
        if block.block_hash != pin.block {
            return Err(format!(
                "the genesis block {} is not the pinned genesis {}",
                block.block_hash.to_hex(),
                pin.block.to_hex()
            ));
        }
        if block.post_state_hash != pin.post_state {
            return Err(format!(
                "the genesis block {} carries post-state {}, not the pinned {}",
                block.block_hash.to_hex(),
                base16::encode(block.post_state_hash.as_bytes()),
                base16::encode(pin.post_state.as_bytes())
            ));
        }
        let root = Blake2b256Hash::from_byte_array(block.post_state_hash.as_bytes());
        Ok(AcceptedState {
            anchor: TrustAnchor::Genesis(pin),
            root,
            window: BTreeSet::new(),
        })
    }

    /// **The checkpoint anchor.** The root is the certificate's anchor post-state; the caller supplies
    /// the certified validation window.
    pub fn from_certificate(
        verified: VerifiedCheckpoint,
        window: BTreeSet<Blake2b256Hash>,
    ) -> Result<Self, String> {
        let root = checkpoint_root(&verified)?;
        let block = checkpoint_anchor_hash(&verified)?;
        let fringe = checkpoint_fringe(&verified)?;
        Ok(AcceptedState {
            anchor: TrustAnchor::Checkpoint {
                verified,
                root,
                block,
                fringe,
            },
            root,
            window,
        })
    }

    pub fn anchor(&self) -> &TrustAnchor {
        &self.anchor
    }

    pub fn root(&self) -> Blake2b256Hash {
        self.root
    }

    pub fn window(&self) -> &BTreeSet<Blake2b256Hash> {
        &self.window
    }
}

/// **The runtime's admissible pre-state roots** (review R2).
///
/// The accepted state names the checkpoint root and the certificate's validation window. As the
/// chain advances, this node derives further roots by validating post-anchor blocks. A block whose
/// pre-state is none of those is refused — **the state store's contents are not consulted**, so an
/// older historical root left on disk (from before the join, or from a restart) is not permission.
#[derive(Clone, Debug)]
pub struct AcceptedWindow {
    /// The anchor's root: the state a post-anchor block may build on directly.
    anchor_root: Blake2b256Hash,
    /// The anchor block: a valid justification even before it is in `validated`.
    anchor_block: BlockHash,
    /// The anchor's height. A checkpoint joiner's accepted state begins here: blocks at or below it
    /// are committed history it holds no state for and must not re-validate.
    anchor_height: i64,
    /// **The merge base the certificate signs**. A post-anchor block's merge starts from the
    /// state at the fringe its parents finalised; the checkpoint signs that state (the anchor's own
    /// `fringe_state_hash`), and a witness test asserts the signed root is the anchor header's claim.
    merge_base: Option<TrustedMergeBase>,
    /// The certificate's validation window (Law 109).
    certified: BTreeSet<Blake2b256Hash>,
    /// Roots this node derived by accepting a validated post-anchor block.
    derived: BTreeSet<Blake2b256Hash>,
    /// Blocks this node validated and did not mark failed. A block whose justifications are all
    /// here (or the anchor) descends from the anchor; the gate admits its merged pre-state to
    /// replay, which recomputes that pre-state from the same justifications.
    validated: BTreeSet<BlockHash>,
}

/// **The state a checkpoint signs as the merge base**.
///
/// The merge of a block's parents starts from `prev_fringe_state`: the state at the fringe its
/// parents finalised. A joiner has that root signed by the quorum (the certificate's `fringeState`,
/// which must equal the anchor header's committed `fringe_state_hash`), so replay can use it instead
/// of the fringe-state claim of a sibling the joiner received but never validated — whose metadata
/// carries no derived fringe root at all. `anchor` names the block whose metadata decides which
/// fringe the root belongs to: the substitution applies exactly when the parent set's own fringe is
/// the anchor's.
#[derive(Clone, Debug)]
pub struct TrustedMergeBase {
    /// The anchor block: its metadata names the fringe whose state is `base`.
    pub anchor: BlockHash,
    /// The state at the fringe the anchor finalised (the certificate's signed `fringeState`).
    pub base: Blake2b256Hash,
}

impl AcceptedWindow {
    /// The window an accepted state starts with: the anchor root, the certified set and the
    /// certificate's committed fringe. The fringe members are the justifications a first
    /// post-checkpoint block may name, so a joiner does not refuse its first multi-parent round.
    pub fn from_accepted(state: &AcceptedState) -> Self {
        let (anchor_block, anchor_height, validated, merge_base) = match state.anchor() {
            TrustAnchor::Genesis(pin) => (pin.block, 0, BTreeSet::new(), None),
            TrustAnchor::Checkpoint {
                block,
                verified,
                fringe,
                ..
            } => (
                *block,
                verified.checkpoint.anchor_height,
                fringe.clone(),
                Some(TrustedMergeBase {
                    anchor: *block,
                    base: state.root(),
                }),
            ),
        };
        AcceptedWindow {
            anchor_root: state.root(),
            anchor_block,
            anchor_height,
            merge_base,
            certified: state.window().clone(),
            derived: BTreeSet::new(),
            validated,
        }
    }

    /// **Note the roots a validated post-anchor block derived.** Called beside the insert that
    /// records the block; a rejected or failed block derives nothing this node may build on.
    pub fn note_roots(&mut self, metadata: &BlockMetadata) {
        if !metadata.validated || metadata.validation_failed {
            return;
        }
        self.validated.insert(metadata.block_hash);
        self.derived.insert(Blake2b256Hash::from_byte_array(
            metadata.post_state_hash.as_bytes(),
        ));
        self.derived.insert(Blake2b256Hash::from_byte_array(
            metadata.fringe_state_hash.as_bytes(),
        ));
    }

    /// **The membership rule, not a height rule.**
    pub fn permits(&self, pre_state: StateHash) -> bool {
        let root = Blake2b256Hash::from_byte_array(pre_state.as_bytes());
        root == self.anchor_root || self.certified.contains(&root) || self.derived.contains(&root)
    }

    /// **The ancestry rule.** A multi-parent block's pre-state is a fresh merge of its parents, so
    /// it is not any validated block's recorded root. The block's justifications *are* the merge
    /// input: when every one is the anchor or a block this node validated, the block descends from
    /// the anchor, and replay recomputes the declared pre-state from the same set. An empty set is
    /// never an exemption (the genesis shape is not a received post-anchor block).
    pub fn permits_justifications(&self, justifications: &[BlockHash]) -> bool {
        !justifications.is_empty()
            && justifications
                .iter()
                .all(|h| *h == self.anchor_block || self.validated.contains(h))
    }

    /// **The replay base must be a root whose state this node holds.** The anchor root and the
    /// roots this node derived by accepting validated post-anchor blocks are held (the anchor's
    /// state was downloaded, a derived root came from a replay). A **certified** window root is
    /// *named* by the certificate but its state is not downloaded by a checkpoint joiner: the
    /// certificate commits roots, not states. Using a certified root as a replay base therefore
    /// reaches the store's "unknown root" — which is a fact about this node's accepted history, not
    /// an internal fault, so it must be refused before replay.
    ///
    /// The case that found this: a block at the anchor's own height, a sibling of the anchor, has
    /// the anchor's **pre-state** as its pre-state. That root is certified (it is the anchor
    /// header's pre-state) but not held; the block was admitted, the replay failed "unknown root",
    /// and the fault latched readiness so the rejoined validator never proposed.
    pub fn permits_replay_root(&self, pre_state: StateHash) -> bool {
        let root = Blake2b256Hash::from_byte_array(pre_state.as_bytes());
        root == self.anchor_root || self.derived.contains(&root)
    }

    /// The pre-replay gate's rule: a **held** accepted root, **or** an anchored justification set.
    /// The second arm does not make the state store permission — replay still recomputes the
    /// pre-state from the justifications and refuses a mismatch with `InvalidPreStateHash`.
    pub fn permits_block(&self, pre_state: StateHash, justifications: &[BlockHash]) -> bool {
        self.permits_replay_root(pre_state) || self.permits_justifications(justifications)
    }

    /// The block this window is anchored to.
    pub fn anchor_block(&self) -> BlockHash {
        self.anchor_block
    }

    /// The anchor's height.
    pub fn anchor_height(&self) -> i64 {
        self.anchor_height
    }

    /// **The signed merge base**: none for a genesis anchor, the certificate's signed
    /// `fringeState` and its anchor for a checkpoint.
    pub fn merge_base(&self) -> Option<&TrustedMergeBase> {
        self.merge_base.as_ref()
    }

    /// **The accepted state begins at the anchor.** A block at or below the anchor height that is
    /// not the anchor itself is committed history this node holds no pre-state for (a sibling at the
    /// anchor's height, or an ancestor): re-validating it reaches a certified-but-unheld root. It is
    /// refused as an uncertified pre-state, not faulted as an internal error.
    pub fn is_committed_at_or_below_anchor(
        &self,
        block_number: i64,
        block_hash: BlockHash,
    ) -> bool {
        block_number <= self.anchor_height && block_hash != self.anchor_block
    }
}

/// The shared runtime window. `None` before the boot installs it: a block that arrives early is a
/// retryable internal fault, not an acceptance and not a verdict about the block.
pub type SharedAcceptedWindow = Arc<tokio::sync::RwLock<Option<AcceptedWindow>>>;

/// **The checkpoint's run root: the signed merged fringe state (v2), or the member's post-state for
/// a v1 singleton.** It is not the anchor block's post-state in general: a multi-member fringe's
/// merge is the state the first post-anchor block builds on (Laws 104/108).
fn checkpoint_root(verified: &VerifiedCheckpoint) -> Result<Blake2b256Hash, String> {
    crate::checkpoint::checkpoint_run_root(&verified.checkpoint).map_err(|e| e.to_string())
}

fn checkpoint_anchor_hash(verified: &VerifiedCheckpoint) -> Result<BlockHash, String> {
    BlockHash::try_from_hex(&verified.checkpoint.anchor)
        .map_err(|e| format!("the checkpoint anchor is not a block hash: {e}"))
}

fn checkpoint_fringe(verified: &VerifiedCheckpoint) -> Result<BTreeSet<BlockHash>, String> {
    verified
        .checkpoint
        .fringe_hashes
        .iter()
        .map(|member| {
            BlockHash::try_from_hex(member).map_err(|e| {
                format!("the checkpoint fringe member {member} is not a block hash: {e}")
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::casper_message::RholangState;
    use rchain_models::casper::protocol::checkpoint::Checkpoint;
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, SeqNum};
    use std::collections::BTreeMap;

    fn block(height: i64, hash_byte: u8, post_byte: u8) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "/root".to_string(),
            block_hash: BlockHash::new([hash_byte; 32]),
            block_number: BlockHeight::try_from(height).expect("height"),
            sender: Validator::new([0u8; 65]),
            seq_num: SeqNum::zero(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([post_byte; 32]),
            justifications: Vec::new(),
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: Vec::new(),
            timestamp: 0,
        }
    }

    fn verified(post_byte: u8) -> VerifiedCheckpoint {
        VerifiedCheckpoint {
            checkpoint: Checkpoint {
                shard_id: "/root".to_string(),
                anchor: base16::encode(&[1u8; 32]),
                anchor_height: 9,
                anchor_post_state: base16::encode(&[post_byte; 32]),
                fringe_state: None,
                fringe_hashes: vec![base16::encode(&[1u8; 32])],
                parent_digest: None,
                epoch: 0,
                metadata_root: None,
                history_root: None,
            },
            digest: Blake2b256Hash::from_byte_array(&[7u8; 32]),
            supporting_stake: 3,
            total_stake: 3,
        }
    }

    /// **Law 108a's Rust witness.** The accepted state's root is the certificate's checkpoint root,
    /// not a caller-supplied one.
    #[test]
    fn an_accepted_state_carries_its_anchors_root() {
        let state =
            AcceptedState::from_certificate(verified(0xAB), BTreeSet::new()).expect("anchor");
        assert_eq!(
            state.root(),
            state.anchor().root(),
            "the root is the anchor's, by construction"
        );
        assert_eq!(
            state.root(),
            Blake2b256Hash::from_byte_array(&[0xAB; 32]),
            "the root is the certificate's anchor post-state"
        );
    }

    /// **The checkpoint's run root is the certified fringe state**, not the anchor block's
    /// post-state: the multi-member merge is the state the first post-anchor block builds on.
    #[test]
    fn a_checkpoint_root_is_the_certified_fringe_state() {
        let mut v = verified(0xAB);
        v.checkpoint.fringe_state = Some(base16::encode(&[0xCD; 32]));
        let state = AcceptedState::from_certificate(v, BTreeSet::new()).expect("anchor");
        assert_eq!(
            state.root(),
            Blake2b256Hash::from_byte_array(&[0xCD; 32]),
            "the root is the signed fringe merge"
        );
        assert_ne!(
            state.root(),
            Blake2b256Hash::from_byte_array(&[0xAB; 32]),
            "the anchor block's post-state is not the merge"
        );
    }

    /// **The genesis anchor is the pin.** A block with another hash, another post-state, or a nonzero
    /// height is refused.
    #[test]
    fn a_genesis_anchor_must_match_the_pin() {
        let genesis = block(0, 1, 2);
        let pin = GenesisPin {
            block: genesis.block_hash,
            post_state: genesis.post_state_hash,
        };
        let state = AcceptedState::from_genesis(pin, &genesis).expect("the pinned genesis");
        assert_eq!(
            state.root(),
            Blake2b256Hash::from_byte_array(genesis.post_state_hash.as_bytes())
        );

        let other_block = GenesisPin {
            block: BlockHash::new([9u8; 32]),
            post_state: genesis.post_state_hash,
        };
        assert!(AcceptedState::from_genesis(other_block, &genesis).is_err());

        let other_post = GenesisPin {
            block: genesis.block_hash,
            post_state: StateHash::new([9u8; 32]),
        };
        assert!(AcceptedState::from_genesis(other_post, &genesis).is_err());

        let not_genesis = block(1, 1, 2);
        assert!(AcceptedState::from_genesis(pin, &not_genesis).is_err());
    }

    /// **Review R2 — the runtime window is membership, not height.** Only the anchor root, a
    /// certified window root or a root this node derived by accepting a validated block is
    /// permitted; a root that merely exists in the store is not.
    #[test]
    fn an_accepted_window_permits_only_the_anchor_certified_and_derived_roots() {
        let certified = Blake2b256Hash::from_byte_array(&[0xCC; 32]);
        let state = AcceptedState::from_certificate(verified(0xAB), BTreeSet::from([certified]))
            .expect("anchor");
        let mut window = AcceptedWindow::from_accepted(&state);

        assert!(
            window.permits(StateHash::new([0xAB; 32])),
            "the anchor root"
        );
        assert!(
            window.permits(StateHash::new([0xCC; 32])),
            "a certified window root"
        );
        assert!(
            !window.permits(StateHash::new([0xDD; 32])),
            "an uncertified historical root is refused even if a store holds it"
        );

        let mut meta = BlockMetadata::from_block(&block(10, 3, 0xEE));
        meta.validated = true;
        meta.validation_failed = false;
        window.note_roots(&meta);
        assert!(
            window.permits(meta.post_state_hash),
            "a root derived by a validated post-anchor block"
        );
    }

    /// **a certified root is not a replay base.** The certificate names its validation
    /// window's roots; a checkpoint joiner holds only the anchor state (and what it derived). A
    /// sibling of the anchor has the anchor's *pre-state* as its pre-state — certified, but not
    /// held — and must be refused before replay as an uncertified pre-state verdict, not faulted as
    /// an internal "unknown root" that latches readiness for ever.
    #[test]
    fn a_certified_root_that_is_not_the_anchor_is_not_a_replay_base() {
        let certified = Blake2b256Hash::from_byte_array(&[0xCC; 32]);
        let state = AcceptedState::from_certificate(verified(0xAB), BTreeSet::from([certified]))
            .expect("anchor");
        let window = AcceptedWindow::from_accepted(&state);

        assert!(
            window.permits(StateHash::new([0xCC; 32])),
            "the read rule still admits a certified window root"
        );
        assert!(
            !window.permits_replay_root(StateHash::new([0xCC; 32])),
            "a certified root is not a state this node holds"
        );
        assert!(
            !window.permits_block(StateHash::new([0xCC; 32]), &[BlockHash::new([9u8; 32])]),
            "an anchor sibling is refused before replay"
        );
        assert!(
            window.permits_block(StateHash::new([0xAB; 32]), &[]),
            "the anchor root is held"
        );
    }

    /// **the accepted state begins at the anchor.** A checkpoint joiner must not re-validate a
    /// block at or below the anchor height: a sibling of the anchor at the same height has a
    /// committed-but-unheld pre-state, and replaying it is what faulted the pipeline and kept the
    /// rejoined validator from proposing. The anchor itself is the exception; blocks above are the
    /// catch-up.
    #[test]
    fn a_checkpoint_window_refuses_a_block_at_or_below_the_anchor() {
        let state =
            AcceptedState::from_certificate(verified(0xAB), BTreeSet::new()).expect("anchor");
        let window = AcceptedWindow::from_accepted(&state);
        let anchor_block = BlockHash::new([1u8; 32]);
        assert_eq!(window.anchor_height(), 9, "the fixture's checkpoint height");
        assert!(
            window.is_committed_at_or_below_anchor(9, BlockHash::new([9u8; 32])),
            "a sibling at the anchor's height is committed history"
        );
        assert!(
            window.is_committed_at_or_below_anchor(8, BlockHash::new([8u8; 32])),
            "an ancestor below the anchor is committed history"
        );
        assert!(
            !window.is_committed_at_or_below_anchor(9, anchor_block),
            "the anchor itself is not re-validated"
        );
        assert!(
            !window.is_committed_at_or_below_anchor(10, anchor_block),
            "a post-anchor block is the catch-up, not committed history"
        );
    }

    /// A rejected block derives nothing: its roots are not admitted for later blocks.
    #[test]
    fn a_derived_root_is_permitted_only_after_a_validated_block_is_noted() {
        let state =
            AcceptedState::from_certificate(verified(0xAB), BTreeSet::new()).expect("anchor");
        let mut window = AcceptedWindow::from_accepted(&state);
        let mut meta = BlockMetadata::from_block(&block(10, 3, 0xEE));
        meta.validated = true;
        meta.validation_failed = true;
        window.note_roots(&meta);
        assert!(
            !window.permits(meta.post_state_hash),
            "a failed block's root is not an input"
        );
    }

    /// **C402 — a multi-parent child's merged pre-state is admitted on its justifications.** The
    /// fresh merge is not any parent's post/fringe root; the ancestry rule is what admits it.
    #[test]
    fn a_multi_parent_child_is_permitted_when_its_justifications_are_validated() {
        let genesis = block(0, 1, 2);
        let pin = GenesisPin {
            block: genesis.block_hash,
            post_state: genesis.post_state_hash,
        };
        let state = AcceptedState::from_genesis(pin, &genesis).expect("the pinned genesis");
        let mut window = AcceptedWindow::from_accepted(&state);

        let mut parent_a = BlockMetadata::from_block(&block(1, 3, 0xEE));
        parent_a.validated = true;
        parent_a.validation_failed = false;
        window.note_roots(&parent_a);
        let mut parent_b = BlockMetadata::from_block(&block(1, 4, 0xEF));
        parent_b.validated = true;
        parent_b.validation_failed = false;
        window.note_roots(&parent_b);

        let merged = StateHash::new([0x77; 32]);
        assert!(
            !window.permits(merged),
            "the merged pre-state is not a recorded root"
        );
        assert!(
            window.permits_justifications(&[
                genesis.block_hash,
                parent_a.block_hash,
                parent_b.block_hash
            ]),
            "the anchor and both validated parents are anchored"
        );
        assert!(
            window.permits_block(
                merged,
                &[genesis.block_hash, parent_a.block_hash, parent_b.block_hash]
            ),
            "the merged pre-state is admitted over anchored justifications"
        );
    }

    /// A justification that is not the anchor and not a validated block gets no exemption.
    #[test]
    fn a_child_of_an_unvalidated_justification_is_refused() {
        let genesis = block(0, 1, 2);
        let pin = GenesisPin {
            block: genesis.block_hash,
            post_state: genesis.post_state_hash,
        };
        let state = AcceptedState::from_genesis(pin, &genesis).expect("the pinned genesis");
        let mut window = AcceptedWindow::from_accepted(&state);

        let mut parent = BlockMetadata::from_block(&block(1, 3, 0xEE));
        parent.validated = false;
        parent.validation_failed = false;
        window.note_roots(&parent);

        assert!(!window.permits_justifications(&[parent.block_hash]));
        assert!(!window.permits_block(
            StateHash::new([0x77; 32]),
            &[genesis.block_hash, parent.block_hash]
        ));
    }

    /// The genesis shape is not an ancestry exemption.
    #[test]
    fn an_empty_justification_set_gets_no_ancestry_exemption() {
        let genesis = block(0, 1, 2);
        let pin = GenesisPin {
            block: genesis.block_hash,
            post_state: genesis.post_state_hash,
        };
        let state = AcceptedState::from_genesis(pin, &genesis).expect("the pinned genesis");
        let window = AcceptedWindow::from_accepted(&state);

        assert!(!window.permits_justifications(&[]));
        assert!(!window.permits_block(StateHash::new([0x77; 32]), &[]));
    }

    /// A failed record is not a validated justification, even though its hash was seen.
    #[test]
    fn a_failed_block_hash_is_not_a_validated_justification() {
        let state =
            AcceptedState::from_certificate(verified(0xAB), BTreeSet::new()).expect("anchor");
        let mut window = AcceptedWindow::from_accepted(&state);
        let mut meta = BlockMetadata::from_block(&block(10, 3, 0xEE));
        meta.validated = true;
        meta.validation_failed = true;
        window.note_roots(&meta);

        assert!(!window.permits_justifications(&[meta.block_hash]));
    }

    /// **A checkpoint joiner starts with its committed fringe as validated justifications.** Without
    /// this seed, the first post-checkpoint multi-parent block would be refused like block #2.
    #[test]
    fn a_checkpoint_anchor_admits_its_committed_fringe_as_justifications() {
        let mut checkpoint = verified(0xAB);
        checkpoint
            .checkpoint
            .fringe_hashes
            .push(base16::encode(&[2u8; 32]));
        let state = AcceptedState::from_certificate(checkpoint, BTreeSet::new()).expect("anchor");
        let window = AcceptedWindow::from_accepted(&state);

        assert_eq!(window.anchor_block(), BlockHash::new([1u8; 32]));
        assert!(
            window.permits_justifications(&[BlockHash::new([1u8; 32]), BlockHash::new([2u8; 32])]),
            "the anchor block and its committed fringe sibling are admissible justifications"
        );
        assert!(!window.permits_justifications(&[BlockHash::new([3u8; 32])]));
    }
}
