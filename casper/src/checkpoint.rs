//! **Checkpoint digests and the finality-quorum certificate** (sync/verification separation).
//!
//! Sync no longer replays history. A joiner accepts the finalized state by verifying a **quorum of
//! signed checkpoint attestations**: a validator signs the checkpoint digest with a domain tag of its
//! own, and a certificate is quorate when the signers hold strictly more than two thirds of the
//! bonded stake (Law 14's exact integer rule, `is_super_majority`).
//!
//! The digest is the statement. It binds the shard, the finalized anchor block, the anchor's
//! **post-state** (the checkpoint root — not the ordinary fringe root, which is a member's claim),
//! the previous checkpoint's digest, the epoch and the finalized fringe. A signature over one
//! checkpoint is therefore not a signature over another, and a certificate cannot cross shards.
//!
//! **An absent parent is encoded as 32 zero bytes, and an explicit zero parent is refused.** The
//! sentinel is deliberate: without it `parent_digest: None` and `Some("00…00")` would be two
//! distinct wire statements with one digest. A zero digest is never a real parent, so the generic
//! verifier rejects it as malformed before hashing; the genesis-only wrapper additionally refuses
//! any named parent on the sync path.
//!
//! **What the certificate does and does not establish.** It attributes the root to a supermajority of
//! the bonded set: a quorum that all computed the same wrong root still certifies it (Law 85's
//! residual is unchanged). The historical audit — `rnode audit` — is the independent counterweight;
//! this module is the stake rule, not a state proof.

use std::collections::{BTreeMap, BTreeSet};

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockHeader;
use rchain_models::casper::protocol::checkpoint::{
    Checkpoint, CheckpointAttestation, CheckpointCertificate,
};
use rchain_models::validator::Validator;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;

use crate::validator_identity::checkpoint_attestation_prehash;

/// Domain tag of the checkpoint digest (distinct from the finality attestation's tag).
pub const CHECKPOINT_TAG: &[u8] = b"rchain-checkpoint-v1";

/// Why a checkpoint or a certificate was refused. Every arm names the fact; none is a generic
/// "invalid".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    /// A field does not decode, has the wrong length, or the fringe is empty/duplicated.
    Malformed(String),
    /// The certificate names a different parent checkpoint than the verifier expected.
    WrongParent {
        expected: Option<Blake2b256Hash>,
        found: Option<Blake2b256Hash>,
    },
    /// The anchor is not a member of the finalized fringe.
    AnchorNotInFringe,
    /// The signed stake did not reach the strict two-thirds rule.
    NotQuorate { supporting: i128, total: i128 },
    /// No bonded stake to measure against.
    NoBonds,
    /// An attestation's digest or signature does not verify.
    BadSignature(String),
    /// **The certificate commits to a different historical metadata set than the one the walk
    /// holds.** The statement's root is what a joiner recomputes; a mismatch means the headers it
    /// was given are not the headers the quorum signed over.
    MetadataRootMismatch { claimed: String, derived: String },
    /// **The certificate has no metadata commitment.** The sync path requires one; a legacy
    /// statement is accepted only by the audit, which reports the absence.
    MissingMetadataRoot,
    /// **The certificate's validation-window commitment does not match the walked headers.** A
    /// post-anchor block would be validated against roots the quorum did not sign over.
    ValidationWindowMismatch { claimed: String, derived: String },
    /// **The certificate has no validation-window commitment.** The sync path requires one; a legacy
    /// statement is accepted only by the audit, which reports the absence.
    MissingValidationWindow,
    /// The anchor state's active bonds differ from the bond set the verifier trusts. Weighing the
    /// signatures against the old set would count the wrong stake, so the certificate is refused.
    /// A bond-set transition needs the checkpoint chain (each certificate verified against its
    /// parent's state); until that rule exists this node refuses rather than guessing.
    BondSetDrifted { genesis: usize, anchor: usize },
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointError::Malformed(why) => write!(f, "malformed checkpoint: {why}"),
            CheckpointError::WrongParent { expected, found } => write!(
                f,
                "checkpoint parent mismatch: expected {}, found {}",
                fmt_hash(*expected),
                fmt_hash(*found)
            ),
            CheckpointError::AnchorNotInFringe => {
                write!(f, "the checkpoint anchor is not a member of its fringe")
            }
            CheckpointError::NotQuorate { supporting, total } => write!(
                f,
                "the certificate is not quorate: {supporting} of {total} bonded stake signed \
                 (strictly more than two thirds is required)"
            ),
            CheckpointError::NoBonds => write!(f, "there is no bonded stake to verify against"),
            CheckpointError::BadSignature(why) => write!(f, "bad checkpoint attestation: {why}"),
            CheckpointError::MetadataRootMismatch { claimed, derived } => write!(
                f,
                "the checkpoint commits to metadata root {claimed}, but the walked headers derive \
                 {derived}"
            ),
            CheckpointError::MissingMetadataRoot => write!(
                f,
                "the checkpoint does not commit to its historical metadata; this build refuses an \
                 uncommitted metadata set on the sync path"
            ),
            CheckpointError::ValidationWindowMismatch { claimed, derived } => write!(
                f,
                "the checkpoint commits to validation window {claimed}, but the walked headers \
                 derive {derived}"
            ),
            CheckpointError::MissingValidationWindow => write!(
                f,
                "the checkpoint does not commit to a validation window; this build refuses to \
                 validate against roots the quorum did not sign"
            ),
            CheckpointError::BondSetDrifted { genesis, anchor } => write!(
                f,
                "the anchor state's active bond set ({anchor} validator(s)) differs from the \
                 genesis trust anchor's ({genesis}); a certificate signed over one partition \
                 cannot be weighed under the other"
            ),
        }
    }
}

impl std::error::Error for CheckpointError {}

fn fmt_hash(hash: Option<Blake2b256Hash>) -> String {
    hash.map(|h| h.to_hex())
        .unwrap_or_else(|| "none".to_string())
}

fn decode_32(hex: &str, what: &str) -> Result<[u8; 32], CheckpointError> {
    let bytes = base16::decode(hex)
        .ok_or_else(|| CheckpointError::Malformed(format!("{what} is not base16")))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| CheckpointError::Malformed(format!("{what} is not 32 bytes")))
}

fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// **The checkpoint statement, as signed.** One implementation, called by the producer, the verifier
/// and the offline checker.
pub fn checkpoint_digest(checkpoint: &Checkpoint) -> Result<Blake2b256Hash, CheckpointError> {
    if checkpoint.fringe_hashes.is_empty() {
        return Err(CheckpointError::Malformed(
            "the finalized fringe is empty".to_string(),
        ));
    }
    if checkpoint.anchor_height < 0 {
        return Err(CheckpointError::Malformed(
            "the anchor height is negative".to_string(),
        ));
    }
    // **The epoch is a nonnegative shape field.** The producer derives it from a nonnegative
    // anchor height and a positive epoch length; a signed statement that carries a negative epoch
    // is not a statement this build can have produced, and accepting it would put an impossible
    // value inside a digest every verifier agrees on.
    if checkpoint.epoch < 0 {
        return Err(CheckpointError::Malformed(
            "the checkpoint epoch is negative".to_string(),
        ));
    }
    let anchor = decode_32(&checkpoint.anchor, "anchor")?;
    let post = decode_32(&checkpoint.anchor_post_state, "anchorPostState")?;
    let parent = match &checkpoint.parent_digest {
        Some(hex) => {
            let bytes = decode_32(hex, "parentDigest")?;
            // **A zero parent is invalid, not absent.** An absent parent is encoded as 32 zero
            // bytes below; if an explicit zero digest were hashed as well, `None` and
            // `Some(00…00)` would be two distinct wire statements with one digest. The sentinel
            // makes the explicit zero unrepresentable, so the two cannot alias.
            if bytes == [0u8; 32] {
                return Err(CheckpointError::Malformed(
                    "parentDigest is the zero digest".to_string(),
                ));
            }
            bytes
        }
        None => [0u8; 32],
    };
    let fringe_hex = &checkpoint.fringe_hashes;
    if !fringe_hex
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&checkpoint.anchor))
    {
        return Err(CheckpointError::AnchorNotInFringe);
    }
    let mut fringe: Vec<[u8; 32]> = fringe_hex
        .iter()
        .map(|h| decode_32(h, "fringe hash"))
        .collect::<Result<_, _>>()?;
    fringe.sort();
    let before = fringe.len();
    fringe.dedup();
    if fringe.len() != before {
        return Err(CheckpointError::Malformed(
            "the fringe names a block twice".to_string(),
        ));
    }

    let mut buf = Vec::with_capacity(64 + fringe.len() * 32);
    lp(&mut buf, CHECKPOINT_TAG);
    lp(&mut buf, checkpoint.shard_id.as_bytes());
    buf.extend_from_slice(&anchor);
    buf.extend_from_slice(&checkpoint.anchor_height.to_be_bytes());
    buf.extend_from_slice(&post);
    buf.extend_from_slice(&parent);
    buf.extend_from_slice(&checkpoint.epoch.to_be_bytes());
    buf.extend_from_slice(&(fringe.len() as u32).to_be_bytes());
    for h in &fringe {
        buf.extend_from_slice(h);
    }
    // **The optional commitments are encoded unambiguously.** A statement with neither keeps the
    // exact v1 preimage (and digest) it had before these fields existed. A statement with at least
    // one appends a single **presence bitmap** (bit0 metadataRoot, bit1 historyRoot) followed by each
    // present root under its own domain tag. Two `0x01 || root` appendages would alias: a
    // metadata-only statement and a history-only statement with the same root would share a signed
    // digest. The bitmap separates those shapes, and the tags keep the two slots distinguishable when
    // both are present.
    let metadata_root = checkpoint
        .metadata_root
        .as_ref()
        .map(|root| decode_32(root, "metadataRoot"))
        .transpose()?;
    let history_root = checkpoint
        .history_root
        .as_ref()
        .map(|root| decode_32(root, "historyRoot"))
        .transpose()?;
    let presence = u8::from(metadata_root.is_some()) | (u8::from(history_root.is_some()) << 1);
    if presence != 0 {
        buf.push(presence);
        if let Some(root) = metadata_root {
            lp(
                &mut buf,
                rchain_models::casper::protocol::checkpoint::METADATA_ROOT_TAG,
            );
            buf.extend_from_slice(&root);
        }
        if let Some(root) = history_root {
            lp(
                &mut buf,
                rchain_models::casper::protocol::checkpoint::HISTORY_ROOT_TAG,
            );
            buf.extend_from_slice(&root);
        }
    }
    Ok(Blake2b256Hash::create(&buf))
}

/// **The shape and self-consistency of the headers the metadata commitment covers.**
///
/// The commitment now includes each header's fringe membership and fringe state root, so the set
/// must be checked for the claims those fields make: one parentless genesis with an empty fringe and
/// `fringe_state_hash == pre_state_hash` (the `insert_genesis` invariant); every non-genesis header
/// has a committed fringe state root (zero is the absence marker); every fringe member is an older
/// header in the set; and two entries for one hash agree in every projected field. A violation is a
/// refusal before the commitment is computed, not a root over an inconsistent set.
pub fn check_committed_headers<'a>(
    headers: impl IntoIterator<Item = &'a BlockHeader>,
) -> Result<(), CheckpointError> {
    let mut by_hash: BTreeMap<BlockHash, &BlockHeader> = BTreeMap::new();
    let mut genesis: Option<&BlockHeader> = None;
    for header in headers {
        if let Some(previous) = by_hash.get(&header.block_hash) {
            if rchain_models::casper::protocol::checkpoint::header_commitment_bytes(previous)
                != rchain_models::casper::protocol::checkpoint::header_commitment_bytes(header)
            {
                return Err(CheckpointError::Malformed(format!(
                    "two headers disagree for block {}",
                    header.block_hash.to_hex()
                )));
            }
            continue;
        }
        if i64::from(header.block_num) == 0 {
            if genesis.replace(header).is_some() {
                return Err(CheckpointError::Malformed(
                    "the committed header set has more than one height-zero block".to_string(),
                ));
            }
        }
        by_hash.insert(header.block_hash, header);
    }
    let Some(genesis) = genesis else {
        return Err(CheckpointError::Malformed(
            "the committed header set has no height-zero block".to_string(),
        ));
    };
    if !genesis.justifications.is_empty()
        || !genesis.fringe.is_empty()
        || genesis.fringe_state_hash != genesis.pre_state_hash
    {
        return Err(CheckpointError::Malformed(format!(
            "the genesis header {} is not the parentless, empty-fringe, pre-state-rooted block \
             this node's genesis rule requires",
            genesis.block_hash.to_hex()
        )));
    }
    let zero = rchain_models::block::state_hash::StateHash::new([0u8; 32]);
    for header in by_hash.values() {
        if i64::from(header.block_num) == 0 {
            continue;
        }
        if header.fringe_state_hash == zero {
            return Err(CheckpointError::Malformed(format!(
                "block {} commits no fringe state root (zero is the absence marker)",
                header.block_hash.to_hex()
            )));
        }
        for member in &header.fringe {
            match by_hash.get(member) {
                Some(member_header) if member_header.block_num < header.block_num => {}
                Some(_) => {
                    return Err(CheckpointError::Malformed(format!(
                        "block {} names fringe member {} at a height that is not older",
                        header.block_hash.to_hex(),
                        member.to_hex()
                    )))
                }
                None => {
                    return Err(CheckpointError::Malformed(format!(
                        "block {} names fringe member {}, which the committed set does not hold",
                        header.block_hash.to_hex(),
                        member.to_hex()
                    )))
                }
            }
        }
    }
    Ok(())
}

/// **The genesis header must agree with the genesis body.** The metadata set excludes genesis, so
/// its fringe fields are not covered by `metadataRoot`; this is the separate check. Every
/// body-derived field is compared, and the `insert_genesis` invariant (empty fringe,
/// `fringe_state_hash == pre_state_hash`) is required.
pub fn verify_genesis_header(
    header: &BlockHeader,
    body: &rchain_models::casper::protocol::casper_message::BlockMessage,
) -> Result<(), String> {
    if header.block_hash != body.block_hash
        || i64::from(header.block_num) != 0
        || header.sender != body.sender
        || header.seq_num != body.seq_num
        || header.justifications != body.justifications.iter().copied().collect()
        || header.bonds_map != body.bonds
        || header.pre_state_hash != body.pre_state_hash
        || header.post_state_hash != body.post_state_hash
        || header.sig != body.sig
        || header.sig_algorithm != body.sig_algorithm
    {
        return Err(format!(
            "the genesis header {} does not agree with the downloaded genesis body {}",
            header.block_hash.to_hex(),
            body.block_hash.to_hex()
        ));
    }
    if !header.fringe.is_empty() {
        return Err(format!(
            "the genesis header {} names a fringe",
            header.block_hash.to_hex()
        ));
    }
    if header.fringe_state_hash != body.pre_state_hash {
        return Err(format!(
            "the genesis header's fringe state root is not its pre-state root (block {})",
            header.block_hash.to_hex()
        ));
    }
    Ok(())
}

/// **Recompute the validation-window commitment from the headers a joiner walked.** The statement
/// names the roots a post-anchor block may read; the joiner checks that the set is the one the quorum
/// signed and hydrates exactly it. A missing commitment or a mismatch is a refusal on the sync path;
/// the audit is the only caller allowed to accept a legacy `None`, and it reports the absence.
pub fn verify_validation_window<'a>(
    checkpoint: &Checkpoint,
    headers: impl IntoIterator<Item = &'a BlockHeader>,
) -> Result<(), CheckpointError> {
    let claimed = checkpoint
        .history_root
        .as_ref()
        .ok_or(CheckpointError::MissingValidationWindow)?;
    let claimed_bytes = decode_32(claimed, "historyRoot")?;
    let derived = rchain_models::casper::protocol::checkpoint::validation_window_root(headers);
    if derived.as_bytes() != &claimed_bytes {
        return Err(CheckpointError::ValidationWindowMismatch {
            claimed: claimed.clone(),
            derived: derived.to_hex(),
        });
    }
    Ok(())
}

/// **Recompute the metadata commitment from the headers a joiner walked.**
///
/// The certificate's root is the quorum's commitment to the anchor's ancestor headers; the joiner
/// has the headers, so it can check the statement it is about to trust. Genesis is excluded by
/// [`metadata_root`] itself, and a missing commitment is a refusal on the sync path (the audit is
/// the only caller allowed to accept a legacy `None`, and it reports the absence).
pub fn verify_metadata_root<'a>(
    checkpoint: &Checkpoint,
    headers: impl IntoIterator<Item = &'a BlockHeader>,
) -> Result<(), CheckpointError> {
    let claimed = checkpoint
        .metadata_root
        .as_ref()
        .ok_or(CheckpointError::MissingMetadataRoot)?;
    let claimed_bytes = decode_32(claimed, "metadataRoot")?;
    let derived = rchain_models::casper::protocol::checkpoint::metadata_root(headers);
    if derived.as_bytes() != &claimed_bytes {
        return Err(CheckpointError::MetadataRootMismatch {
            claimed: claimed.clone(),
            derived: derived.to_hex(),
        });
    }
    Ok(())
}

/// A verified certificate: the statement, its digest, and the stake that signed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCheckpoint {
    pub checkpoint: Checkpoint,
    pub digest: Blake2b256Hash,
    pub supporting_stake: i128,
    pub total_stake: i128,
}

/// **Verify a checkpoint certificate against the bond map that finalised it.**
///
/// `bonds` is the parent checkpoint's bond map (the genesis bonds for the first checkpoint, or an
/// operator pin's state); `expected_parent` is the digest the joiner already holds, if any.
///
/// The checks are ordered so a refusal names the earliest fact that failed: shape and anchor first,
/// then the digest, then the parent, then each signature, then the stake rule. An unbonded signer is
/// ignored (weight zero) — the producer's admission gate is not the verifier's authority.
pub fn verify_checkpoint_certificate(
    certificate: &CheckpointCertificate,
    bonds: &BTreeMap<Validator, NonNegI64>,
    expected_parent: Option<Blake2b256Hash>,
) -> Result<VerifiedCheckpoint, CheckpointError> {
    let checkpoint = &certificate.checkpoint;
    let digest = checkpoint_digest(checkpoint)?;

    // The parent the joiner expects, if it names one.
    if expected_parent.is_some() {
        let found = match &checkpoint.parent_digest {
            Some(hex) => Some(Blake2b256Hash::from_byte_array(&decode_32(
                hex,
                "parentDigest",
            )?)),
            None => None,
        };
        if found != expected_parent {
            return Err(CheckpointError::WrongParent {
                expected: expected_parent,
                found,
            });
        }
    }

    let total: i128 = bonds.values().map(|s| i128::from(i64::from(*s))).sum();
    if total <= 0 || bonds.is_empty() {
        return Err(CheckpointError::NoBonds);
    }

    let digest_hex = digest.to_hex();
    let prehash = checkpoint_attestation_prehash(&digest);
    let mut signers: BTreeSet<Validator> = BTreeSet::new();
    for attestation in &certificate.attestations {
        if !attestation
            .checkpoint_digest
            .eq_ignore_ascii_case(&digest_hex)
        {
            return Err(CheckpointError::BadSignature(format!(
                "an attestation names digest {}, not {digest_hex}",
                attestation.checkpoint_digest
            )));
        }
        let sender_bytes = base16::decode(&attestation.sender)
            .ok_or_else(|| CheckpointError::BadSignature("sender is not base16".to_string()))?;
        let sender = Validator::try_from(sender_bytes.as_slice())
            .map_err(|e| CheckpointError::BadSignature(format!("sender is not a key: {e}")))?;
        // Unbonded signers carry no weight; they are logged by the caller's counter, not counted.
        if !bonds.contains_key(&sender) {
            continue;
        }
        if !signers.insert(sender) {
            continue; // one validator, one vote
        }
        let sig = base16::decode(&attestation.sig)
            .ok_or_else(|| CheckpointError::BadSignature("sig is not base16".to_string()))?;
        let alg =
            rchain_crypto::signatures::signatures_alg::from_algorithm(&attestation.sig_algorithm)
                .ok_or_else(|| {
                CheckpointError::BadSignature(format!(
                    "unknown signature algorithm {}",
                    attestation.sig_algorithm
                ))
            })?;
        if !alg.verify(&prehash, &sig, sender.as_bytes()) {
            return Err(CheckpointError::BadSignature(format!(
                "the attestation from {} does not verify over the checkpoint digest",
                attestation.sender
            )));
        }
    }

    let supporting: i128 = signers
        .iter()
        .map(|signer| i128::from(i64::from(*bonds.get(signer).unwrap_or(&NonNegI64::zero()))))
        .sum();
    if !rchain_sdk::consensus::is_super_majority(supporting, total) {
        return Err(CheckpointError::NotQuorate { supporting, total });
    }

    Ok(VerifiedCheckpoint {
        checkpoint: checkpoint.clone(),
        digest,
        supporting_stake: supporting,
        total_stake: total,
    })
}

/// **Verify a certificate whose parent must be absent.** Every checkpoint this build produces is
/// genesis-rooted (`parent_digest: None`), and the trusted bond set is the genesis active set. A
/// certificate that names a parent is one this verifier cannot chain, so it is refused by name
/// rather than verified as if it were the root.
pub fn verify_genesis_rooted_certificate(
    certificate: &CheckpointCertificate,
    bonds: &BTreeMap<Validator, NonNegI64>,
) -> Result<VerifiedCheckpoint, CheckpointError> {
    // **The statement's shape is validated before the parent rule.** `named_parent` decodes a
    // parent but does not hash; without this, an explicit zero parent would be reported
    // `WrongParent` here and `Malformed` by the generic verifier. One statement, one refusal.
    checkpoint_digest(&certificate.checkpoint)?;
    if let Some(parent) = named_parent(certificate) {
        return Err(CheckpointError::WrongParent {
            expected: None,
            found: Some(parent),
        });
    }
    verify_checkpoint_certificate(certificate, bonds, None)
}

fn named_parent(certificate: &CheckpointCertificate) -> Option<Blake2b256Hash> {
    certificate
        .checkpoint
        .parent_digest
        .as_ref()
        .and_then(|hex| base16::decode(hex))
        .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
        .map(|bytes| Blake2b256Hash::from_byte_array(&bytes))
}

/// **Refuse a certificate whose anchor state is not governed by the trusted bond set.** The
/// certificate is weighed against `trusted`; if the anchor post-state's active bonds differ from it,
/// those signatures were cast under a different partition and cannot be counted here.
pub fn check_bond_set_unchanged(
    trusted: &BTreeMap<Validator, NonNegI64>,
    at_anchor: &BTreeMap<Validator, NonNegI64>,
) -> Result<(), CheckpointError> {
    if trusted != at_anchor {
        return Err(CheckpointError::BondSetDrifted {
            genesis: trusted.len(),
            anchor: at_anchor.len(),
        });
    }
    Ok(())
}

/// Serialize an attestation for the HTTP surface.
pub fn attestation_json(attestation: &CheckpointAttestation) -> String {
    serde_json::to_string(attestation).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_crypto::private_key::PrivateKey;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
    use rchain_models::casper::protocol::checkpoint::Checkpoint;
    use rchain_shared::refined::NonNegI64;

    fn hash(byte: u8) -> String {
        base16::encode(&[byte; 32])
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            shard_id: "root".to_string(),
            anchor: hash(1),
            anchor_height: 42,
            anchor_post_state: hash(2),
            fringe_hashes: vec![hash(1)],
            parent_digest: None,
            epoch: 4,
            metadata_root: None,
            history_root: None,
        }
    }

    fn validator(seed: u8) -> (Validator, PrivateKey) {
        let key = PrivateKey::new(vec![seed; 32]);
        let public = rchain_crypto::signatures::secp256k1::Secp256k1
            .to_public(&key)
            .expect("public key");
        let public_bytes = public.bytes().to_vec();
        (
            Validator::try_from(public_bytes.as_slice()).expect("a validator key"),
            key,
        )
    }

    fn header(height: i64, hash_byte: u8) -> BlockHeader {
        let pre = rchain_models::block::state_hash::StateHash::new([4u8; 32]);
        BlockHeader {
            block_hash: rchain_models::block_hash::BlockHash::new([hash_byte; 32]),
            block_num: rchain_shared::refined::BlockHeight::try_from(height).expect("height"),
            sender: Validator::new([0u8; 65]),
            seq_num: rchain_shared::refined::SeqNum::try_from(0).expect("seq"),
            justifications: BTreeSet::new(),
            bonds_map: BTreeMap::new(),
            fringe: BTreeSet::new(),
            // Genesis carries the insert-genesis invariant; every other header commits a fringe
            // state root distinct from its pre-state.
            fringe_state_hash: if height == 0 {
                pre
            } else {
                rchain_models::block::state_hash::StateHash::new([3u8; 32])
            },
            pre_state_hash: pre,
            post_state_hash: rchain_models::block::state_hash::StateHash::new([5u8; 32]),
            body_available: false,
            sig: vec![1, 2, 3],
            sig_algorithm: "secp256k1".to_string(),
        }
    }

    fn attest(checkpoint: &Checkpoint, key: &PrivateKey) -> CheckpointAttestation {
        let digest = checkpoint_digest(checkpoint).expect("digest");
        let signature = rchain_crypto::signatures::secp256k1::Secp256k1
            .sign(&checkpoint_attestation_prehash(&digest), key.bytes())
            .expect("sign");
        let public = rchain_crypto::signatures::secp256k1::Secp256k1
            .to_public(key)
            .expect("public");
        CheckpointAttestation {
            checkpoint_digest: digest.to_hex(),
            sender: base16::encode(public.bytes()),
            sig_algorithm: "secp256k1".to_string(),
            sig: base16::encode(&signature),
        }
    }

    #[test]
    fn the_digest_binds_every_field() {
        let base = checkpoint();
        let base_digest = checkpoint_digest(&base).expect("digest");
        for mutate in [
            |c: &mut Checkpoint| c.shard_id = "other".to_string(),
            |c: &mut Checkpoint| {
                c.anchor = hash(9);
                c.fringe_hashes = vec![hash(9)];
            },
            |c: &mut Checkpoint| c.anchor_height = 43,
            |c: &mut Checkpoint| c.anchor_post_state = hash(9),
            |c: &mut Checkpoint| c.epoch = 5,
            |c: &mut Checkpoint| c.parent_digest = Some(hash(7)),
            |c: &mut Checkpoint| c.metadata_root = Some(hash(7)),
            |c: &mut Checkpoint| c.history_root = Some(hash(7)),
            |c: &mut Checkpoint| {
                c.fringe_hashes = vec![hash(1), hash(9)];
            },
        ] {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(
                checkpoint_digest(&changed).expect("digest"),
                base_digest,
                "a changed field must change the digest"
            );
        }
    }

    #[test]
    fn the_fringe_order_does_not_change_the_digest_and_a_duplicate_is_refused() {
        let mut a = checkpoint();
        a.fringe_hashes = vec![hash(1), hash(2)];
        let mut b = a.clone();
        b.fringe_hashes.reverse();
        assert_eq!(
            checkpoint_digest(&a).expect("a"),
            checkpoint_digest(&b).expect("b")
        );
        b.fringe_hashes.push(hash(1));
        assert!(matches!(
            checkpoint_digest(&b),
            Err(CheckpointError::Malformed(_))
        ));
    }

    #[test]
    fn the_anchor_must_be_in_the_fringe() {
        let mut c = checkpoint();
        c.anchor = hash(9);
        assert_eq!(
            checkpoint_digest(&c),
            Err(CheckpointError::AnchorNotInFringe)
        );
    }

    #[test]
    fn a_certificate_at_exactly_two_thirds_is_refused() {
        // 3 validators, equal stake: two of three is exactly 2/3 and must not pass.
        let (v1, k1) = validator(1);
        let (v2, k2) = validator(2);
        let (v3, _k3) = validator(3);
        let bonds: BTreeMap<Validator, NonNegI64> = [v1, v2, v3]
            .into_iter()
            .map(|v| (v, NonNegI64::try_from(100).unwrap()))
            .collect();
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1), attest(&c, &k2)],
        };
        assert_eq!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::NotQuorate {
                supporting: 200,
                total: 300
            })
        );
    }

    #[test]
    fn a_certificate_above_two_thirds_is_accepted() {
        let (v1, k1) = validator(1);
        let (v2, k2) = validator(2);
        let (v3, k3) = validator(3);
        let bonds: BTreeMap<Validator, NonNegI64> = [v1, v2, v3]
            .into_iter()
            .map(|v| (v, NonNegI64::try_from(100).unwrap()))
            .collect();
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1), attest(&c, &k2), attest(&c, &k3)],
        };
        let verified = verify_checkpoint_certificate(&cert, &bonds, None).expect("quorate");
        assert_eq!(verified.supporting_stake, 300);
        assert_eq!(verified.total_stake, 300);
    }

    #[test]
    fn an_unbonded_signer_carries_no_weight() {
        let (v1, k1) = validator(1);
        let (_v2, k2) = validator(2);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1), attest(&c, &k2)],
        };
        let verified = verify_checkpoint_certificate(&cert, &bonds, None).expect("quorate");
        assert_eq!(verified.supporting_stake, 100);
    }

    #[test]
    fn a_signature_over_another_digest_is_refused() {
        let (v1, k1) = validator(1);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let mut other = checkpoint();
        other.anchor_post_state = hash(9);
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&other, &k1)],
        };
        assert!(matches!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::BadSignature(_))
        ));
    }

    #[test]
    fn a_genesis_rooted_verifier_refuses_a_checkpoint_that_names_a_parent() {
        let (v1, k1) = validator(1);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let mut c = checkpoint();
        c.parent_digest = Some(hash(7));
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1)],
        };
        // `verify_checkpoint_certificate` would accept it: its signer is every bonded validator.
        assert!(verify_checkpoint_certificate(&cert, &bonds, None).is_ok());
        assert!(matches!(
            verify_genesis_rooted_certificate(&cert, &bonds),
            Err(CheckpointError::WrongParent {
                expected: None,
                found: Some(_)
            })
        ));
    }

    #[test]
    fn a_bond_set_that_drifted_from_the_trust_anchor_is_refused() {
        let (v1, _k1) = validator(1);
        let (v2, _k2) = validator(2);
        let one: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let two: BTreeMap<Validator, NonNegI64> = [
            (v1, NonNegI64::try_from(100).unwrap()),
            (v2, NonNegI64::try_from(100).unwrap()),
        ]
        .into_iter()
        .collect();
        assert!(check_bond_set_unchanged(&one, &one).is_ok());
        assert!(matches!(
            check_bond_set_unchanged(&one, &two),
            Err(CheckpointError::BondSetDrifted {
                genesis: 1,
                anchor: 2
            })
        ));
    }

    #[test]
    fn two_conflicting_checkpoints_share_a_signer_under_a_split() {
        // Two quorums must intersect in >1/3 stake; with 3 equal validators, 2-signer quorums share
        // at least one signer. (The theorem is in the Lean model; this pins the Rust rule's shape.)
        let (v1, _k1) = validator(1);
        let (v2, _k2) = validator(2);
        let (v3, _k3) = validator(3);
        let a: BTreeSet<Validator> = [v1, v2].into_iter().collect();
        let b: BTreeSet<Validator> = [v2, v3].into_iter().collect();
        assert!(!a.is_disjoint(&b));
    }

    /// **The absent-parent encoding is unchanged.** The sentinel fix rejects an explicit zero
    /// parent without moving any digest a `None`-parent statement already had; this value is the
    /// pre-fix fixture's digest.
    #[test]
    fn a_none_parent_statement_keeps_its_v1_digest() {
        assert_eq!(
            checkpoint_digest(&checkpoint()).expect("digest").to_hex(),
            "688779f55fb88078a2a5939ca49ee62e726131d3dfbe2fdf4ce6d6c8d0ed803d"
        );
    }

    #[derive(serde::Deserialize)]
    struct DigestCase {
        name: String,
        checkpoint: Checkpoint,
        digest: String,
    }

    #[derive(serde::Deserialize)]
    struct DigestFixture {
        cases: Vec<DigestCase>,
    }

    fn digest_fixture() -> DigestFixture {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/checkpoint-digest-goldens.json"
        )))
        .expect("the digest fixture parses")
    }

    /// **Every optional-commitment shape has its own digest.** The legacy neither-present shape keeps
    /// the v1 digest; a metadata-only and a history-only statement with the same root do not alias;
    /// the both-present shape and its transposition differ. The same fixture is read by the Python
    /// benchmark encoder, so the two implementations cannot drift silently.
    #[test]
    fn the_presence_bitmap_distinguishes_every_optional_combination() {
        let fixture = digest_fixture();
        let mut seen = BTreeSet::new();
        for case in &fixture.cases {
            let digest = checkpoint_digest(&case.checkpoint)
                .expect("a fixture statement is well shaped")
                .to_hex();
            assert_eq!(digest, case.digest, "case {}", case.name);
            assert!(
                seen.insert(digest.clone()),
                "case {} repeats a digest; the optional slots alias",
                case.name
            );
        }
        assert_eq!(seen.len(), fixture.cases.len());
    }

    /// The two single-slot shapes are not interchangeable: swapping which slot carries the root
    /// changes the digest, so a signature over one cannot authenticate the other.
    #[test]
    fn the_optional_slots_cannot_be_transposed() {
        let fixture = digest_fixture();
        let digest = |name: &str| {
            fixture
                .cases
                .iter()
                .find(|case| case.name == name)
                .expect("a case")
                .digest
                .clone()
        };
        assert_ne!(digest("metadata_only"), digest("history_only"));
        assert_ne!(digest("both"), digest("transposed"));
    }

    /// **A signature over a metadata-only statement does not verify a history-only one.** The
    /// attestation names the first statement's digest; the verifier recomputes the second's and
    /// refuses before any bond arithmetic.
    #[test]
    fn a_signature_over_a_metadata_only_statement_does_not_verify_a_history_only_statement() {
        let (v1, k1) = validator(1);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let mut metadata_only = checkpoint();
        metadata_only.metadata_root = Some(hash(7));
        let mut history_only = checkpoint();
        history_only.history_root = Some(hash(8));
        let certificate = CheckpointCertificate {
            checkpoint: history_only,
            attestations: vec![attest(&metadata_only, &k1)],
        };
        assert!(matches!(
            verify_checkpoint_certificate(&certificate, &bonds, None),
            Err(CheckpointError::BadSignature(_))
        ));
    }

    /// **A zero parent is invalid, not absent.** `None` hashes as 32 zero bytes; an explicit
    /// `Some(00…00)` must not reach the hash, or two distinct statements would share a digest.
    #[test]
    fn an_explicit_zero_parent_is_malformed() {
        let mut c = checkpoint();
        c.parent_digest = Some(hash(0));
        assert!(matches!(
            checkpoint_digest(&c),
            Err(CheckpointError::Malformed(_))
        ));
    }

    /// The mutation the audit named: change an absent parent to an explicit zero and keep the
    /// existing signature. The digest cannot be recomputed, so the attestation cannot verify.
    #[test]
    fn changing_an_absent_parent_to_zero_without_resigning_is_refused() {
        let (v1, k1) = validator(1);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let c = checkpoint();
        let mut mutated = c.clone();
        mutated.parent_digest = Some(hash(0));
        let cert = CheckpointCertificate {
            checkpoint: mutated,
            attestations: vec![attest(&c, &k1)],
        };
        assert!(matches!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::Malformed(_))
        ));
        // The genesis-only wrapper validates the shape before `named_parent`, so both entry
        // points name the same fact.
        assert!(matches!(
            verify_genesis_rooted_certificate(&cert, &bonds),
            Err(CheckpointError::Malformed(_))
        ));
    }

    #[test]
    fn an_epoch_below_zero_is_malformed() {
        let mut c = checkpoint();
        c.epoch = -1;
        assert!(matches!(
            checkpoint_digest(&c),
            Err(CheckpointError::Malformed(_))
        ));
    }

    /// **A forged signature over the correct digest must fail in the cryptographic check, not in
    /// the declared-digest comparison.** This is the test the old suite lacked: disabling
    /// `alg.verify` left every other case green.
    #[test]
    fn a_corrupted_signature_over_the_correct_digest_is_refused() {
        let (v1, k1) = validator(1);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let c = checkpoint();
        let mut attestation = attest(&c, &k1);
        let mut sig = base16::decode(&attestation.sig).expect("sig is hex");
        sig[0] ^= 0xff;
        attestation.sig = base16::encode(&sig);
        let cert = CheckpointCertificate {
            checkpoint: c,
            attestations: vec![attestation],
        };
        assert!(matches!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::BadSignature(_))
        ));
    }

    /// A signature over the correct prehash but made by another key, declared under a bonded
    /// sender, must fail in `alg.verify`.
    #[test]
    fn a_signature_from_another_key_is_refused() {
        let (v1, _k1) = validator(1);
        let (_v2, k2) = validator(2);
        let bonds: BTreeMap<Validator, NonNegI64> =
            [(v1.clone(), NonNegI64::try_from(100).unwrap())]
                .into_iter()
                .collect();
        let c = checkpoint();
        let mut attestation = attest(&c, &k2);
        attestation.sender = base16::encode(v1.as_bytes());
        let cert = CheckpointCertificate {
            checkpoint: c,
            attestations: vec![attestation],
        };
        assert!(matches!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::BadSignature(_))
        ));
    }

    /// **The stateless half of the conflict witness.** Two actually signed 3-of-4 certificates at
    /// one parent both verify; a verifier alone cannot reject the second, which is why the store
    /// must detect the conflict (U2). The signer sets intersect, as Law 104 says.
    #[test]
    fn two_signed_conflicting_certificates_both_verify() {
        let keys = [validator(1), validator(2), validator(3), validator(4)];
        let bonds: BTreeMap<Validator, NonNegI64> = keys
            .iter()
            .map(|(v, _)| (v.clone(), NonNegI64::try_from(100).unwrap()))
            .collect();
        let a = checkpoint();
        let mut b = checkpoint();
        b.anchor_post_state = hash(9);
        let cert_a = CheckpointCertificate {
            checkpoint: a.clone(),
            attestations: vec![
                attest(&a, &keys[0].1),
                attest(&a, &keys[1].1),
                attest(&a, &keys[2].1),
            ],
        };
        let cert_b = CheckpointCertificate {
            checkpoint: b.clone(),
            attestations: vec![
                attest(&b, &keys[1].1),
                attest(&b, &keys[2].1),
                attest(&b, &keys[3].1),
            ],
        };
        let verified_a =
            verify_checkpoint_certificate(&cert_a, &bonds, None).expect("certificate a");
        let verified_b =
            verify_checkpoint_certificate(&cert_b, &bonds, None).expect("certificate b");
        assert_eq!(verified_a.supporting_stake, 300);
        assert_eq!(verified_b.supporting_stake, 300);
        assert_ne!(verified_a.digest, verified_b.digest);
        let senders = |cert: &CheckpointCertificate| -> BTreeSet<Validator> {
            cert.attestations
                .iter()
                .map(|attestation| {
                    let bytes = base16::decode(&attestation.sender).expect("sender is hex");
                    Validator::try_from(bytes.as_slice()).expect("sender is a key")
                })
                .collect()
        };
        assert!(!senders(&cert_a).is_disjoint(&senders(&cert_b)));
    }

    /// **The metadata commitment is a set commitment over the walked headers.** Genesis is
    /// excluded; `body_available` is a transport detail and is not hashed; a changed committed
    /// field changes the root; a missing commitment is refused on the sync path.
    #[test]
    fn a_metadata_commitment_is_bound_and_verifies() {
        use rchain_models::casper::protocol::checkpoint::metadata_root;

        let genesis = header(0, 1);
        let child = header(1, 2);
        let root = metadata_root([&genesis, &child]);
        assert_eq!(
            root,
            metadata_root([&child]),
            "genesis is excluded from the commitment"
        );

        let mut c = checkpoint();
        let without = checkpoint_digest(&c).expect("digest");
        c.metadata_root = Some(root.to_hex());
        assert_ne!(
            checkpoint_digest(&c).expect("digest"),
            without,
            "the commitment is bound by the signed digest"
        );
        assert!(verify_metadata_root(&c, [&genesis, &child]).is_ok());

        let mut other = child.clone();
        other.body_available = true;
        assert!(
            verify_metadata_root(&c, [&genesis, &other]).is_ok(),
            "body availability is a per-peer transport detail, not a committed field"
        );

        let mut tampered = child.clone();
        tampered.pre_state_hash = rchain_models::block::state_hash::StateHash::new([9u8; 32]);
        assert!(matches!(
            verify_metadata_root(&c, [&genesis, &tampered]),
            Err(CheckpointError::MetadataRootMismatch { .. })
        ));

        let legacy = checkpoint();
        assert!(matches!(
            verify_metadata_root(&legacy, [&genesis, &child]),
            Err(CheckpointError::MissingMetadataRoot)
        ));
    }

    /// **Review R1 — the per-header fringe claim is committed.** A changed recent fringe member, a
    /// changed older fringe state root and a swap of two recent fringe states each change the
    /// metadata root. The window is a *set*, so the swap can leave it unchanged; the per-header
    /// commitment is what detects it. `body_available` stays a transport detail.
    #[test]
    fn the_fringe_claim_is_committed_per_header() {
        use rchain_models::casper::protocol::checkpoint::{metadata_root, validation_window_roots};

        let genesis = header(0, 1);
        let mut recent = header(1, 2);
        recent.fringe = BTreeSet::from([genesis.block_hash]);
        recent.fringe_state_hash = rchain_models::block::state_hash::StateHash::new([7u8; 32]);
        let mut older = header(2, 3);
        older.fringe_state_hash = rchain_models::block::state_hash::StateHash::new([8u8; 32]);
        let base = metadata_root([&genesis, &recent, &older]);

        let mut changed_member = recent.clone();
        changed_member.fringe.insert(BlockHash::new([9u8; 32]));
        assert_ne!(
            base,
            metadata_root([&genesis, &changed_member, &older]),
            "a fringe member is committed"
        );

        let mut changed_older = older.clone();
        changed_older.fringe_state_hash =
            rchain_models::block::state_hash::StateHash::new([9u8; 32]);
        assert_ne!(
            base,
            metadata_root([&genesis, &recent, &changed_older]),
            "an older fringe state root is committed"
        );

        let mut swapped_recent = recent.clone();
        swapped_recent.fringe_state_hash = older.fringe_state_hash;
        let mut swapped_older = older.clone();
        swapped_older.fringe_state_hash = recent.fringe_state_hash;
        assert_ne!(
            base,
            metadata_root([&genesis, &swapped_recent, &swapped_older]),
            "a transposition of two fringe states is committed"
        );
        assert_eq!(
            validation_window_roots([&genesis, &recent, &older]),
            validation_window_roots([&genesis, &swapped_recent, &swapped_older]),
            "the window is a set; the per-header commitment is what detects the swap"
        );

        let mut transport = recent.clone();
        transport.body_available = true;
        assert_eq!(
            base,
            metadata_root([&genesis, &transport, &older]),
            "body availability is a per-peer transport detail"
        );
    }

    #[test]
    fn a_committed_header_set_with_an_absent_fringe_member_is_refused() {
        let genesis = header(0, 1);
        let mut child = header(1, 2);
        child.fringe = BTreeSet::from([BlockHash::new([9u8; 32])]);
        assert!(check_committed_headers([&genesis, &child]).is_err());
    }

    #[test]
    fn a_genesis_header_that_breaks_the_insert_genesis_invariant_is_refused() {
        let mut genesis = header(0, 1);
        genesis.fringe_state_hash = rchain_models::block::state_hash::StateHash::new([9u8; 32]);
        assert!(check_committed_headers([&genesis]).is_err());
    }

    #[test]
    fn the_genesis_header_must_agree_with_its_body() {
        use rchain_models::casper::protocol::casper_message::{BlockMessage, RholangState};

        let genesis = header(0, 1);
        let body = BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: genesis.block_hash,
            block_number: genesis.block_num,
            sender: genesis.sender,
            seq_num: genesis.seq_num,
            pre_state_hash: genesis.pre_state_hash,
            post_state_hash: genesis.post_state_hash,
            justifications: Vec::new(),
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: genesis.sig_algorithm.clone(),
            sig: genesis.sig.clone(),
            timestamp: 0,
        };
        assert!(verify_genesis_header(&genesis, &body).is_ok());
        let mut fringe = genesis.clone();
        fringe.fringe.insert(BlockHash::new([9u8; 32]));
        assert!(verify_genesis_header(&fringe, &body).is_err());
        let mut state = genesis.clone();
        state.fringe_state_hash = rchain_models::block::state_hash::StateHash::new([9u8; 32]);
        assert!(verify_genesis_header(&state, &body).is_err());
    }

    /// **The validation window is committed, and only the committed window validates.** The
    /// commitment binds the selected roots; a dropped header or a changed field derives another root
    /// and is refused. A legacy statement without the commitment is refused by the sync check.
    #[test]
    fn a_validation_window_is_bound_and_verifies() {
        use rchain_models::casper::protocol::checkpoint::{
            validation_window_root, validation_window_roots,
        };

        let genesis = header(0, 1);
        let child = header(1, 2);
        let mut grandchild = header(2, 3);
        grandchild.pre_state_hash = rchain_models::block::state_hash::StateHash::new([6u8; 32]);
        let headers = [&genesis, &child, &grandchild];

        let root = validation_window_root(headers);
        assert!(!validation_window_roots(headers).is_empty());
        let mut c = checkpoint();
        let without = checkpoint_digest(&c).expect("digest");
        c.history_root = Some(root.to_hex());
        assert_ne!(
            checkpoint_digest(&c).expect("digest"),
            without,
            "the window commitment is bound by the signed digest"
        );
        assert!(verify_validation_window(&c, headers).is_ok());

        let shortened = [&genesis, &child];
        assert!(matches!(
            verify_validation_window(&c, shortened),
            Err(CheckpointError::ValidationWindowMismatch { .. })
        ));

        let mut tampered = child.clone();
        tampered.post_state_hash = rchain_models::block::state_hash::StateHash::new([9u8; 32]);
        assert!(matches!(
            verify_validation_window(&c, [&genesis, &tampered, &grandchild]),
            Err(CheckpointError::ValidationWindowMismatch { .. })
        ));

        let legacy = checkpoint();
        assert!(matches!(
            verify_validation_window(&legacy, headers),
            Err(CheckpointError::MissingValidationWindow)
        ));
    }
}
