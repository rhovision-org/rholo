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

/// Domain tag of one header's canonical projection.
pub const HEADER_COMMITMENT_TAG: &[u8] = b"rchain-header-v1";

fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// **The canonical projection of a header the metadata commitment binds.**
///
/// It covers exactly the fields a body carries — hash, height, sender, sequence number,
/// justifications, bonds, the two state roots, the signature and its algorithm. It deliberately
/// **excludes** `fringe`, `fringe_state_hash` and `body_available`: the first two are derived from
/// the DAG and are absent from `BlockMessage` (so an offline audit could not recompute a commitment
/// that included them), and the last is a per-peer transport detail. The walk's shape check bounds
/// the fringe; the audit's replay bounds the state. The exclusion is recorded, not implied.
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
    pub anchor_post_state: String,
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
