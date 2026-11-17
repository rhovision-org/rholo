//! **The checkpoint statement** (sync/verification separation).
//!
//! A checkpoint names the state a node may start from without replaying history: the finalized
//! anchor block, its post-state, the finalized fringe it belongs to, the checkpoint chain's parent,
//! and the epoch. Its digest is the statement validators sign; the certificate is the digest plus
//! enough signatures to meet the finality quorum.
//!
//! Hex strings are used on this wire type so `models` stays free of the crypto crates. The digest and
//! every verification rule live in `rchain-casper::checkpoint`, which decodes and length-checks these
//! fields before hashing anything.

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use serde::{Deserialize, Serialize};

use crate::block_hash::BlockHash;
use crate::casper::protocol::casper_message::BlockHeader;

/// Domain tag of the metadata-set commitment.
pub const METADATA_SET_TAG: &[u8] = b"rchain-metadata-set-v1";

/// Domain tag of the optional metadata-root slot in the statement digest. The two optional slots are
/// tagged separately so a statement cannot be reinterpreted with its commitments transposed; see
/// `casper/src/checkpoint.rs::checkpoint_digest`.
pub const METADATA_ROOT_TAG: &[u8] = b"rchain-checkpoint-metadata-root-v1";

/// Domain tag of the optional history-root slot in the statement digest.
pub const HISTORY_ROOT_TAG: &[u8] = b"rchain-checkpoint-history-root-v1";

/// Domain tag of the **signed fringe state** slot in a v2 statement digest. The slot is mandatory in
/// v2 (the statement's run root); the tag keeps it distinguishable from the anchor post-state, which
/// is not the merge when the finalized fringe has more than one member (C405).
pub const FRINGE_STATE_TAG: &[u8] = b"rchain-checkpoint-fringe-state-v1";

/// Domain tag of one header's canonical projection.
pub const HEADER_COMMITMENT_TAG: &[u8] = b"rchain-header-v1";

/// **How many recent headers' state roots a post-anchor block may read.** A post-anchor block runs
/// the PoS close for the block finalized a few heights behind it; the state roots it needs are the
/// last `VALIDATION_WINDOW_BLOCKS` headers' pre/post/fringe roots. The window is a protocol constant:
/// the producer commits to it (`historyRoot`), the joiner hydrates exactly it, and a validator needing
/// a root outside it is refused rather than widening the window.
pub const VALIDATION_WINDOW_BLOCKS: usize = 8;

/// Domain tag of the validation-window commitment.
pub const VALIDATION_WINDOW_TAG: &[u8] = b"rchain-validation-window-v1";

fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// **The canonical projection of a header the metadata commitment binds.**
///
/// It covers every consensus field the DAG reads — hash, height, sender, sequence number,
/// justifications, bonds, the **fringe membership**, the **fringe state root**, the two state roots,
/// the signature and its algorithm. `fringe` and `fringe_state_hash` were once excluded because a
/// `BlockMessage` does not carry them; that let a peer change consensus-relevant metadata under the
/// same certificate (review R1). They are now committed, and the audit reads them from the node's
/// header route before checking the commitment. The only field still excluded is `body_available`, a
/// per-peer transport detail.
pub fn header_commitment_bytes(header: &BlockHeader) -> Vec<u8> {
    let mut out = Vec::new();
    lp(&mut out, HEADER_COMMITMENT_TAG);
    out.extend_from_slice(header.block_hash.as_bytes());
    out.extend_from_slice(&i64::from(header.block_num).to_be_bytes());
    out.extend_from_slice(header.sender.as_bytes());
    out.extend_from_slice(&i64::from(header.seq_num).to_be_bytes());

    let mut justifications: Vec<BlockHash> = header.justifications.iter().copied().collect();
    justifications.sort();
    out.extend_from_slice(&(justifications.len() as u32).to_be_bytes());
    for hash in &justifications {
        out.extend_from_slice(hash.as_bytes());
    }

    let mut fringe: Vec<BlockHash> = header.fringe.iter().copied().collect();
    fringe.sort();
    out.extend_from_slice(&(fringe.len() as u32).to_be_bytes());
    for hash in &fringe {
        out.extend_from_slice(hash.as_bytes());
    }
    out.extend_from_slice(header.fringe_state_hash.as_bytes());

    out.extend_from_slice(&(header.bonds_map.len() as u32).to_be_bytes());
    for (validator, stake) in &header.bonds_map {
        out.extend_from_slice(validator.as_bytes());
        out.extend_from_slice(&i64::from(*stake).to_be_bytes());
    }

    out.extend_from_slice(header.pre_state_hash.as_bytes());
    out.extend_from_slice(header.post_state_hash.as_bytes());
    lp(&mut out, &header.sig);
    lp(&mut out, header.sig_algorithm.as_bytes());
    out
}

/// **The commitment over a certified header set**: the anchor's transitive ancestor closure,
/// genesis excluded, ordered by block hash so no peer's traversal order changes the root. A
/// duplicate hash is one entry: the walk and the audit key headers by hash.
pub fn metadata_root<'a>(headers: impl IntoIterator<Item = &'a BlockHeader>) -> Blake2b256Hash {
    let mut entries: Vec<&BlockHeader> = headers
        .into_iter()
        .filter(|header| i64::from(header.block_num) > 0)
        .collect();
    entries.sort_by_key(|header| header.block_hash);
    entries.dedup_by_key(|header| header.block_hash);

    let mut out = Vec::new();
    lp(&mut out, METADATA_SET_TAG);
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for header in entries {
        out.extend_from_slice(header.block_hash.as_bytes());
        let bytes = header_commitment_bytes(header);
        lp(&mut out, &bytes);
    }
    Blake2b256Hash::create(&out)
}

/// **The distinct state roots of the last [`VALIDATION_WINDOW_BLOCKS`] headers**, ordered by height
/// and then by block hash, with each header contributing its pre-state, post-state and fringe-state
/// roots (zero roots are the absence marker and are skipped). The selection is deterministic from the
/// walked header set, so the producer and the joiner derive the same window.
pub fn validation_window_roots<'a>(
    headers: impl IntoIterator<Item = &'a BlockHeader>,
) -> std::collections::BTreeSet<[u8; 32]> {
    let mut ordered: Vec<&BlockHeader> = headers.into_iter().collect();
    ordered.sort_by_key(|header| (i64::from(header.block_num), header.block_hash));
    let mut roots = std::collections::BTreeSet::new();
    for header in ordered.iter().rev().take(VALIDATION_WINDOW_BLOCKS) {
        for root in [
            header.pre_state_hash,
            header.post_state_hash,
            header.fringe_state_hash,
        ] {
            if root != crate::block::state_hash::StateHash::new([0u8; 32]) {
                roots.insert(*root.as_bytes());
            }
        }
    }
    roots
}

/// **The commitment over the validation window.** A sorted, lp-framed set commitment; the statement
/// binds it with `historyRoot`.
pub fn validation_window_root<'a>(
    headers: impl IntoIterator<Item = &'a BlockHeader>,
) -> Blake2b256Hash {
    let roots = validation_window_roots(headers);
    let mut out = Vec::new();
    lp(&mut out, VALIDATION_WINDOW_TAG);
    out.extend_from_slice(&(roots.len() as u32).to_be_bytes());
    for root in roots {
        out.extend_from_slice(&root);
    }
    Blake2b256Hash::create(&out)
}

/// The checkpoint statement, as signed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// The shard this checkpoint belongs to; a certificate cannot cross shards.
    pub shard_id: String,
    /// The finalized anchor block: the max `(height, hash)` member of the finalized fringe.
    pub anchor: String,
    /// The anchor's block number.
    pub anchor_height: i64,
    /// The anchor's **post-state** root (not the ordinary fringe root, which is a member's claim).
    /// Identity/audit only since v2; the run root is [`Self::fringe_state`].
    pub anchor_post_state: String,
    /// **The merged state of the finalized fringe** — the state the first post-anchor block builds
    /// on, and the checkpoint's run root (Laws 104/108, C405). It is not any member's post-state when
    /// the fringe has more than one member. `None` is a v1 legacy statement: accepted only when the
    /// fringe has a single member, where the merge is that member's post-state.
    #[serde(default)]
    pub fringe_state: Option<String>,
    /// The finalized fringe, sorted and unique. The anchor is one of its members.
    pub fringe_hashes: Vec<String>,
    /// The previous checkpoint's digest; `None` for a genesis-rooted checkpoint.
    pub parent_digest: Option<String>,
    /// `block_number / epoch_length`, used for the bond-set selection and freshness rule.
    pub epoch: i64,
    /// **The commitment to the historical metadata the anchor's state was derived from**
    /// ([`metadata_root`] over the anchor's ancestor closure). The digest binds it when present.
    /// `None` is a legacy statement; the sync path refuses it and only the audit accepts it, so a
    /// reader is never told that an uncommitted metadata set was authenticated.
    #[serde(default)]
    pub metadata_root: Option<String>,
    /// **The commitment to the validation window** ([`validation_window_root`]): the state roots a
    /// post-anchor block may read. The sync hydrates exactly this set; a missing root is a refusal.
    /// `None` is a legacy statement; only the audit accepts it, and it reports the absence.
    #[serde(default)]
    pub history_root: Option<String>,
}

/// One validator's signature over a checkpoint digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointAttestation {
    /// The digest this signature is over, hex.
    pub checkpoint_digest: String,
    /// The signing validator's public key, hex.
    pub sender: String,
    pub sig_algorithm: String,
    /// The signature, hex.
    pub sig: String,
}

/// A checkpoint and the attestations a node holds for it. Verification (the quorum rule) lives in
/// `rchain-casper::checkpoint`; this type only carries the statement and the signatures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointCertificate {
    pub checkpoint: Checkpoint,
    pub attestations: Vec<CheckpointAttestation>,
}
