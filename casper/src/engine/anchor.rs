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
        Ok(AcceptedState {
            anchor: TrustAnchor::Checkpoint { verified, root },
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
    /// The certificate's validation window (Law 109).
    certified: BTreeSet<Blake2b256Hash>,
    /// Roots this node derived by accepting a validated post-anchor block.
    derived: BTreeSet<Blake2b256Hash>,
}

impl AcceptedWindow {
    /// The window an accepted state starts with: the anchor root and the certified set.
    pub fn from_accepted(state: &AcceptedState) -> Self {
        AcceptedWindow {
            anchor_root: state.root(),
            certified: state.window().clone(),
            derived: BTreeSet::new(),
        }
    }

    /// **Note the roots a validated post-anchor block derived.** Called beside the insert that
    /// records the block; a rejected or failed block derives nothing this node may build on.
    pub fn note_roots(&mut self, metadata: &BlockMetadata) {
        if !metadata.validated || metadata.validation_failed {
            return;
        }
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
}

/// The shared runtime window. `None` before the boot installs it: a block that arrives early is a
/// retryable internal fault, not an acceptance and not a verdict about the block.
pub type SharedAcceptedWindow = Arc<tokio::sync::RwLock<Option<AcceptedWindow>>>;

fn checkpoint_root(verified: &VerifiedCheckpoint) -> Result<Blake2b256Hash, String> {
    let bytes = base16::decode(&verified.checkpoint.anchor_post_state)
        .ok_or_else(|| "the checkpoint anchor post-state is not base16".to_string())?;
    if bytes.len() != 32 {
        return Err(format!(
            "the checkpoint anchor post-state is {} bytes, not 32",
            bytes.len()
        ));
    }
    Ok(Blake2b256Hash::from_byte_array(&bytes))
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
}
