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

use serde::{Deserialize, Serialize};

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
