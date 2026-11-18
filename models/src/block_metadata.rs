//! Block metadata.
//!
//! Mirrors `models/src/main/scala/coop/rchain/models/BlockMetadata.scala`. `Hash` is overridden to
//! hash only `block_hash` (mirrors the Scala). Wire `from_proto`/`to_proto` are deferred to the
//! prost layer.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use prost::Message as _;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use crate::block::state_hash::StateHash;
use crate::block_hash::BlockHash;
use crate::casper::protocol::casper_message::{BlockHeader, BlockMessage};
use crate::proto::casper::{BlockMetadataProto, BondProto};
use crate::validator::Validator;

/// **The key of a fringe** — the hash over its blocks, which is what a block records as its
/// [`BlockMetadata::member_of_fringe`] and what `BlockDagKeyValueStorage` used to key its retired
/// `fringe-data` store by (Law 18: fringe identity is order-independent — the hash is over the
/// `BTreeSet`, so the input order cannot move it).
///
/// This lived on `FringeData` until the store retired (Law 66/68; C250's residue, C270): the claim a
/// fringe held is now a per-block fact (`BlockMetadata.fringe_state_hash`, `BlockMetadata.fringe`, the
/// block's `rejected_deploys`, and `member_of_fringe`), so the *type* goes while this one computation
/// — which names the fringe a block belongs to — stays, because the readers that replaced the store
/// still need to name a fringe to *find* its blocks.
pub fn fringe_hash_of(fringe: &BTreeSet<BlockHash>) -> Blake2b256Hash {
    let parts: Vec<&[u8]> = fringe.iter().map(|h| h.as_bytes() as &[u8]).collect();
    Blake2b256Hash::create_many(&parts)
}

/// **Why** a block was marked failed, which decides whether a restoring rule may clear the record and
/// whether the failure is the block's own fault (AUDIT C173).
///
/// The three cases are the ones the register's row separates, and they drive two decisions that were
/// previously the same bit: whether to set [`BlockMetadata::slashable`], and whether a later child's
/// arrival may re-validate the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureCause {
    /// A property of the block itself: a structural fault, a missing/forged signature, a deploy the
    /// block may not carry. Permanent and attributable — the only case a proposer may slash for.
    Attributable,
    /// **This node's replay disagreed**: a state-hash mismatch, a rejected-deploy set, a bonds cache
    /// that did not recompute. The block may be perfectly valid elsewhere, so the failure is not
    /// attributable — and it is the one cause a restoring rule may clear, because the divergence can
    /// be repaired by moving this node's view rather than by changing the block.
    Divergence,
    /// Refused **because a justification failed** (`NeglectedInvalidBlock`, and the `InvalidBlockNumber`
    /// that follows from a failed parent). Not this block's fault: without this case, one transient
    /// failure fabricates slash evidence against every validator above it.
    Cascade,
}

/// **How much of what a validator holds a slash may take**, by the character of the offence
/// (AUDIT C199).
///
/// The bond is the stake at risk, and a rule that always took *all* of it made an operator's worst
/// case total: one refusal cost the whole bond while going offline cost nothing. The tiers put the
/// punishment next to the fault — forgery is punished in full, a rule the author's own block breaks
/// costs a quarter, and crossing a bound a stale pool or a clock skew explains costs a tenth.
///
/// A tier is a fraction of **everything the validator holds in the PoS system** — its bond, its
/// accrued and unwithdrawn rewards, and an escrowed withdrawal claim — and the remainder is
/// **returned to its own vault**, so the loss is exactly the tier. That is why the tiers are stated as
/// a share rather than an amount: the bound is the thing an operator needs to know before bonding.
///
/// **`Unspecified` is not a tier; it is the pre-tier rule.** A record written before the tiers existed
/// carries code `0` and must slash in full, which is what it would have done. No new offence writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlashSeverity {
    /// Deliberate: the block asserts something forged. Full confiscation.
    Malicious,
    /// A rule the author's own block breaks with something to gain — a DAG position it is not entitled
    /// to, a deploy it may not carry. A quarter.
    Misdemeanour,
    /// A bound crossed where the block is otherwise self-consistent — the class a stale deploy pool, a
    /// clock skew or a local budget error explains. A tenth.
    HonestMistake,
    /// A record written before the tiers existed. Slashes in full, as it would have.
    Unspecified,
}

impl SlashSeverity {
    /// The share of everything at risk this tier confiscates, in basis points of the total.
    pub fn basis_points(self) -> i64 {
        match self {
            SlashSeverity::Malicious => 10_000,
            SlashSeverity::Misdemeanour => 2_500,
            SlashSeverity::HonestMistake => 1_000,
            // The pre-tier rule was the whole bond, and a legacy record has to replay to the state it
            // was written in.
            SlashSeverity::Unspecified => 10_000,
        }
    }

    /// The protobuf code for this tier (see `BlockMetadataProto.slashSeverity`).
    pub fn to_code(self) -> i32 {
        match self {
            SlashSeverity::Malicious => 1,
            SlashSeverity::Misdemeanour => 2,
            SlashSeverity::HonestMistake => 3,
            SlashSeverity::Unspecified => 0,
        }
    }

    /// Read a tier back from its protobuf code. `0` — a record predating the tiers, or one that never
    /// carried an offence — is `Unspecified`, which slashes in full.
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => SlashSeverity::Malicious,
            2 => SlashSeverity::Misdemeanour,
            3 => SlashSeverity::HonestMistake,
            _ => SlashSeverity::Unspecified,
        }
    }

    /// The harsher of two tiers, for a validator that offended more than once. `Unspecified` is the
    /// weakest — it is the absence of a recorded tier, not a punishment.
    pub fn harsher(self, other: SlashSeverity) -> SlashSeverity {
        if other.basis_points() > self.basis_points() {
            other
        } else {
            self
        }
    }
}

impl FailureCause {
    /// The protobuf code for this cause (see `BlockMetadataProto.failureCause`).
    pub fn to_code(self) -> i32 {
        match self {
            FailureCause::Attributable => 1,
            FailureCause::Divergence => 2,
            FailureCause::Cascade => 3,
        }
    }

    /// Read a cause back from its protobuf code. `0` (and anything unrecognised) is `None`.
    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            1 => Some(FailureCause::Attributable),
            2 => Some(FailureCause::Divergence),
            3 => Some(FailureCause::Cascade),
            _ => None,
        }
    }
}

/// A block's metadata (the block-storage DAG index entry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMetadata {
    pub block_hash: BlockHash,
    pub block_num: BlockHeight,
    pub sender: Validator,
    pub seq_num: SeqNum,
    pub justifications: BTreeSet<BlockHash>,
    pub bonds_map: BTreeMap<Validator, NonNegI64>,
    pub validated: bool,
    pub validation_failed: bool,

    /// Whether this failure is a property of the *block* rather than of this node's ability to replay it.
    ///
    /// Deliberately separate from [`Self::validation_failed`], which means "unusable here" and is set for
    /// both cases. A replay that cannot run at all - an unreadable pre-state, a store error, a sidecar that
    /// cannot be regenerated - says nothing about the sender, and treating it as the sender's fault costs an
    /// honest validator its stake for a local problem (#70). Only a *completed* validation that disagrees
    /// (a state mismatch, or a rejectable status) is attributable to the block.
    ///
    /// **Carried in the protobuf as `slashable = 22` (AUDIT C122) — and it was not, in a way worse than
    /// "a restart forgets it".** This field is the input to C110's slash rule (`validate::slashable_senders`
    /// over the metadata `dag.lookup` returns), and until C122 that rule was unreachable: `from_proto`
    /// hard-coded `false`, and every read of a stored metadata goes back through this codec, so the flag was
    /// false for *every* metadata any caller could see — not merely after a restart, as this comment used to
    /// claim, and not only for the round trip the in-memory writers took. The writers
    /// (`validate_block_checkpoint`, `mark_failed`) set it before `dag.insert` and nothing ever
    /// read back a `true`, so a proposer's `to_slash` was always empty and `slash_is_unjustified` treated
    /// every `Slash` as unjustified. The comment was also wrong that `validation_failed` is not carried: it
    /// is, at `casper.proto:201`.
    pub slashable: bool,

    /// **Why** the block failed, not merely that it did (AUDIT C173).
    ///
    /// `validation_failed` says "unusable here" and `slashable` says "the block's own fault"; neither
    /// says whether the failure is *this node's* view of the world or the block's own property. A
    /// restoring rule has to be keyed on exactly that difference, or it is an unbounded re-fetch
    /// (AUDIT C180's class).
    ///
    /// `None` means **no cause was recorded** — a record written before this field existed, or one
    /// that never carried a failure. It is not an assertion that the block did not fail, so a reader
    /// must consult `validation_failed` for that; the restoring rule treats an absent cause as
    /// ineligible, which is the safe direction.
    pub failure_cause: Option<FailureCause>,

    /// **How much of what the sender holds a slash for this failure may take** (AUDIT C199).
    ///
    /// The tier for the *offence*, recorded here because the slash is built from this metadata: a
    /// proposer reads its own records to decide what to slash, and a receiving node re-derives the same
    /// tier from its own copy before accepting the slash. It is node-local, like `slashable` beside it
    /// — the *tier itself* travels to the state through the block's own `SystemDeployData::Slash`.
    ///
    /// [`SlashSeverity::Unspecified`] is the absence of a recorded tier (a record written before this
    /// existed), and it slashes in full, which is what such a record would have done.
    pub slash_severity: SlashSeverity,

    /// How many times this node has re-validated the record in an attempt to clear it. Persisted so
    /// [`FailureCause::Divergence`]'s attempt cap survives a restart rather than resetting to zero.
    pub restore_attempts: u32,

    pub fringe: BTreeSet<BlockHash>,
    pub fringe_state_hash: StateHash,

    /// **The block's own state roots** (sync/verification separation). `fringe_state_hash` is the
    /// state after the block's *fringe*; these are the pre- and post-state of the block itself, and
    /// on a metadata-only node they are the only copy — it holds no body to read them from. The
    /// boot guards read the post-state. Zero for a record written before they were carried, which
    /// is distinguishable because a real state root is never all-zero.
    pub pre_state_hash: StateHash,
    pub post_state_hash: StateHash,

    /// The signature over the block hash and the algorithm that produced it. `metadata` alone is
    /// what a metadata-only node holds, and an equivocation's evidence is checkable from these
    /// three values without the body (`EquivocationEvidence::is_signed_by`).
    pub sig: Vec<u8>,
    pub sig_algorithm: String,

    pub member_of_fringe: Option<Blake2b256Hash>,
}

/// **The zero record**, for fixtures and `..Default::default()` in a constructor that predates a
/// field. It is not a valid block: every hash and root is zero. A production writer names every
/// field it means, and the store's readers refuse a zero state root where one must be real.
impl Default for BlockMetadata {
    fn default() -> Self {
        BlockMetadata {
            block_hash: BlockHash::new([0u8; 32]),
            block_num: BlockHeight::zero(),
            sender: Validator::new([0u8; 65]),
            seq_num: SeqNum::zero(),
            justifications: BTreeSet::new(),
            bonds_map: BTreeMap::new(),
            validated: false,
            validation_failed: false,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: StateHash::new([0u8; 32]),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            sig: Vec::new(),
            sig_algorithm: String::new(),
            member_of_fringe: None,
        }
    }
}

// BlockMetadata is uniquely identified by its block hash (per the Scala).
impl Hash for BlockMetadata {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.block_hash.hash(state);
    }
}

impl BlockMetadata {
    /// **The header view of this record.** Every committed field comes from the metadata;
    /// `body_available` is a per-peer transport detail the caller supplies, and it is the one field
    /// the metadata commitment excludes.
    pub fn to_header(&self, body_available: bool) -> BlockHeader {
        BlockHeader {
            block_hash: self.block_hash,
            block_num: self.block_num,
            sender: self.sender,
            seq_num: self.seq_num,
            justifications: self.justifications.clone(),
            bonds_map: self.bonds_map.clone(),
            fringe: self.fringe.clone(),
            fringe_state_hash: self.fringe_state_hash,
            pre_state_hash: self.pre_state_hash,
            post_state_hash: self.post_state_hash,
            body_available,
            sig: self.sig.clone(),
            sig_algorithm: self.sig_algorithm.clone(),
        }
    }

    pub fn from_proto(b: &BlockMetadataProto) -> Result<Self, crate::errors::ModelsError> {
        Ok(BlockMetadata {
            block_hash: BlockHash::try_from(b.block_hash.as_slice())?,
            block_num: BlockHeight::try_from(b.block_num)
                .map_err(|_| crate::errors::ModelsError::Malformed("negative block number"))?,
            sender: Validator::try_from(b.sender.as_slice())?,
            seq_num: SeqNum::try_from(b.seq_num)
                .map_err(|_| crate::errors::ModelsError::Malformed("negative sequence number"))?,
            justifications: b
                .justifications
                .iter()
                .map(|j| BlockHash::try_from(j.as_slice()))
                .collect::<Result<BTreeSet<BlockHash>, crate::errors::ModelsError>>()?,
            bonds_map: b
                .bonds
                .iter()
                .map(|bond| {
                    let stake = NonNegI64::try_from(bond.stake).map_err(|_| {
                        crate::errors::ModelsError::Malformed("negative bond stake")
                    })?;
                    Ok((Validator::try_from(bond.validator.as_slice())?, stake))
                })
                .collect::<Result<_, crate::errors::ModelsError>>()?,
            validated: b.validated,
            validation_failed: b.validation_failed,
            slashable: b.slashable,
            failure_cause: FailureCause::from_code(b.failure_cause),
            slash_severity: SlashSeverity::from_code(b.slash_severity),
            restore_attempts: b.restore_attempts,
            fringe: b
                .fringe
                .iter()
                .map(|f| BlockHash::try_from(f.as_slice()))
                .collect::<Result<BTreeSet<BlockHash>, crate::errors::ModelsError>>()?,
            // An empty/absent field decodes to the zero root: a record from an earlier build. It is
            // not a valid state root, and saying so is the reader's job, not this codec's.
            pre_state_hash: state_hash_or_zero(&b.pre_state_hash)?,
            post_state_hash: state_hash_or_zero(&b.post_state_hash)?,
            sig: b.sig.clone(),
            sig_algorithm: b.sig_algorithm.clone(),
            // The Scala `StateHash` is a `ByteString`, so `fromBlock` can produce an empty
            // `fringeStateHash`; the Rust fixed-width `StateHash` maps that to zero-fill (the empty
            // value is a transient pre-validation placeholder, never a real hash).
            fringe_state_hash: if b.fringe_state_hash.is_empty() {
                StateHash::new([0u8; 32])
            } else {
                StateHash::try_from(b.fringe_state_hash.as_slice())?
            },
            member_of_fringe: if b.member_of_fringe.is_empty() {
                None
            } else {
                Some(Blake2b256Hash::try_from(b.member_of_fringe.as_slice())?)
            },
        })
    }

    pub fn to_proto(&self) -> BlockMetadataProto {
        BlockMetadataProto {
            block_hash: self.block_hash.as_bytes().to_vec(),
            block_num: i64::from(self.block_num),
            sender: self.sender.as_bytes().to_vec(),
            seq_num: i64::from(self.seq_num),
            justifications: self
                .justifications
                .iter()
                .map(|j| j.as_bytes().to_vec())
                .collect(),
            bonds: self
                .bonds_map
                .iter()
                .map(|(validator, stake)| BondProto {
                    validator: validator.as_bytes().to_vec(),
                    stake: i64::from(*stake),
                })
                .collect(),
            validated: self.validated,
            validation_failed: self.validation_failed,
            slashable: self.slashable,
            failure_cause: self.failure_cause.map(|c| c.to_code()).unwrap_or(0),
            slash_severity: self.slash_severity.to_code(),
            restore_attempts: self.restore_attempts,
            fringe: self.fringe.iter().map(|f| f.as_bytes().to_vec()).collect(),
            fringe_state_hash: self.fringe_state_hash.as_bytes().to_vec(),
            pre_state_hash: self.pre_state_hash.as_bytes().to_vec(),
            post_state_hash: self.post_state_hash.as_bytes().to_vec(),
            sig: self.sig.clone(),
            sig_algorithm: self.sig_algorithm.clone(),
            member_of_fringe: self
                .member_of_fringe
                .as_ref()
                .map(|h| h.as_bytes().to_vec())
                .unwrap_or_default(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_proto().encode_to_vec()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::errors::ModelsError> {
        let proto = BlockMetadataProto::decode(bytes)
            .map_err(|e| crate::errors::ModelsError::Decode(e.to_string()))?;
        BlockMetadata::from_proto(&proto)
    }

    /// Build metadata from a block message (port of `BlockMetadata.fromBlock`).
    pub fn from_block(b: &BlockMessage) -> Self {
        BlockMetadata {
            block_hash: b.block_hash,
            block_num: b.block_number,
            sender: b.sender,
            seq_num: b.seq_num,
            justifications: b.justifications.iter().copied().collect(),
            bonds_map: b.bonds.clone(),
            validated: false,
            validation_failed: false,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: StateHash::new([0u8; 32]),
            // The body's own roots: the only copy a metadata-only node will have.
            pre_state_hash: b.pre_state_hash,
            post_state_hash: b.post_state_hash,
            sig: b.sig.clone(),
            sig_algorithm: b.sig_algorithm.clone(),
            member_of_fringe: None,
        }
    }
}

/// Decode a state-root field, treating an absent one as the zero root (a record from a build that
/// did not carry it). A non-empty field that is not 32 bytes is malformed, not a zero.
fn state_hash_or_zero(bytes: &[u8]) -> Result<StateHash, crate::errors::ModelsError> {
    match bytes.len() {
        0 => Ok(StateHash::new([0u8; 32])),
        32 => Ok(StateHash::from_slice(bytes)),
        other => Err(crate::errors::ModelsError::Length {
            got: other,
            expected: 32,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }
    fn validator(byte: u8) -> Validator {
        Validator::new([byte; 65])
    }

    #[test]
    fn law18_block_metadata_round_trips() {
        let meta = BlockMetadata {
            block_hash: block_hash(1),
            block_num: 3.try_into().unwrap(),
            sender: validator(2),
            seq_num: 1.try_into().unwrap(),
            justifications: [block_hash(2), block_hash(1)].into_iter().collect(),
            bonds_map: BTreeMap::from([(validator(2), 100.try_into().unwrap())]),
            validated: true,
            validation_failed: true,
            slashable: false,
            failure_cause: Some(FailureCause::Divergence),
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 2,
            fringe: [block_hash(5)].into_iter().collect(),
            fringe_state_hash: StateHash::new([9u8; 32]),
            pre_state_hash: StateHash::new([6u8; 32]),
            post_state_hash: StateHash::new([7u8; 32]),
            sig: vec![4u8; 64],
            sig_algorithm: "secp256k1".to_string(),
            member_of_fringe: Some(Blake2b256Hash::from_bytes([8u8; 32])),
        };
        let decoded = BlockMetadata::from_bytes(&meta.to_bytes()).unwrap();
        assert_eq!(decoded, meta);
    }

    /// The `slashable` flag is the input to the slash rule (C110/C122); a codec that drops it
    /// leaves every stored record unrestorable. The law-18 fixture uses `slashable: false`, so it
    /// cannot see the field move; this one sets it true.
    #[test]
    fn a_slashable_flag_survives_the_store_round_trip() {
        let meta = BlockMetadata {
            block_hash: block_hash(1),
            block_num: 3.try_into().unwrap(),
            sender: validator(2),
            seq_num: 1.try_into().unwrap(),
            justifications: [block_hash(2), block_hash(1)].into_iter().collect(),
            bonds_map: BTreeMap::from([(validator(2), 100.try_into().unwrap())]),
            validated: true,
            validation_failed: false,
            slashable: true,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: [block_hash(5)].into_iter().collect(),
            fringe_state_hash: StateHash::new([9u8; 32]),
            pre_state_hash: StateHash::new([6u8; 32]),
            post_state_hash: StateHash::new([7u8; 32]),
            sig: vec![4u8; 64],
            sig_algorithm: "secp256k1".to_string(),
            member_of_fringe: Some(Blake2b256Hash::from_bytes([8u8; 32])),
        };
        let decoded = BlockMetadata::from_bytes(&meta.to_bytes()).unwrap();
        assert!(decoded.slashable, "the slashable flag was not carried");
        assert_eq!(decoded, meta);
    }

    /// **The failure cause and the attempt count survive the codec, and an absent cause decodes as
    /// `None`.** The restoring rule (AUDIT C173) is keyed on the cause, so a codec that dropped it —
    /// the way `slashable` was once dropped (C122) — would leave every stored record unrestorable
    /// after a restart while the in-memory writers looked correct.
    #[test]
    fn the_failure_cause_survives_the_store_round_trip() {
        let base = crate::casper::protocol::casper_message::BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: block_hash(1),
            block_number: 1.try_into().unwrap(),
            sender: validator(2),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: Default::default(),
            rejected_blocks: Default::default(),
            rejected_senders: Default::default(),
            state: Default::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        };

        for cause in [
            FailureCause::Attributable,
            FailureCause::Divergence,
            FailureCause::Cascade,
        ] {
            let meta = BlockMetadata {
                validation_failed: true,
                failure_cause: Some(cause),
                slash_severity: SlashSeverity::Unspecified,
                restore_attempts: 1,
                ..BlockMetadata::from_block(&base)
            };
            let decoded = BlockMetadata::from_bytes(&meta.to_bytes()).unwrap();
            assert_eq!(decoded.failure_cause, Some(cause), "{cause:?} was lost");
            assert_eq!(decoded.restore_attempts, 1);
        }

        // Zero — the protobuf default, and what every writer that knows no cause emits — is `None`.
        let no_cause = BlockMetadata::from_block(&base);
        let decoded = BlockMetadata::from_bytes(&no_cause.to_bytes()).unwrap();
        assert_eq!(decoded.failure_cause, None);
        assert_eq!(decoded.restore_attempts, 0);
    }

    #[test]
    fn from_block_extracts_identity_fields() {
        let block = crate::casper::protocol::casper_message::BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: block_hash(1),
            block_number: 4.try_into().unwrap(),
            sender: validator(2),
            seq_num: 3.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications: vec![block_hash(2)],
            bonds: BTreeMap::from([(validator(2), 100.try_into().unwrap())]),
            rejected_deploys: Default::default(),
            rejected_blocks: Default::default(),
            rejected_senders: Default::default(),
            state: Default::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        };
        let meta = BlockMetadata::from_block(&block);
        assert_eq!(meta.block_hash, block_hash(1));
        assert_eq!(meta.block_num, 4.try_into().unwrap());
        assert_eq!(meta.sender, validator(2));
        assert_eq!(meta.seq_num, 3.try_into().unwrap());
        assert_eq!(meta.justifications, [block_hash(2)].into_iter().collect());
        assert_eq!(
            meta.bonds_map,
            BTreeMap::from([(validator(2), 100.try_into().unwrap())])
        );
        assert!(!meta.validated);
    }

    /// **Law 18's witness, moved here with `fringe_hash_of` — and the move was forced, not chosen.**
    ///
    /// This test lived in `fringe_data.rs`, which the record redesign deleted (Law 66: the store, the
    /// type and the join retired, because the claim travels with its block). Deleting the module took the
    /// witness with it, and the register's own check refused the anchor that then pointed at nothing —
    /// which is the check earning its keep, since nothing else in the tree would have noticed a law
    /// losing its only test.
    ///
    /// The property: a fringe's identity is its *sorted* members, so two sets built in different
    /// insertion orders are one key. `fringe_hash_of` iterates its `BTreeSet` input, so this is also the
    /// statement that the key does not depend on how the set was assembled.
    #[test]
    fn law18_fringe_hash_is_order_independent() {
        let mut one = BTreeSet::new();
        one.insert(block_hash(1));
        one.insert(block_hash(2));
        let mut two = BTreeSet::new();
        two.insert(block_hash(2));
        two.insert(block_hash(1));
        assert_eq!(
            fringe_hash_of(&one),
            fringe_hash_of(&two),
            "one fringe, one key, whatever order it was built in"
        );
    }
}
