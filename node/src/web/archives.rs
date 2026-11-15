//! **The history tier's discovery and challenge sampling** (sync/verification separation).
//!
//! Sync from a checkpoint makes a node correct with no history below its anchor. History is then a
//! **service**, not a consensus requirement: an archive keeps bodies, advertises that it does, and
//! answers challenges that prove possession. Validators and observers can both archive; an observer
//! stakes hardware rather than tokens and is paid a service fee (the funding decision is an external
//! row — see the register).
//!
//! **A challenge proves possession at challenge time.** The sample is deterministic from the
//! caller's nonce, so the archive cannot choose the easy heights, and the caller verifies each body
//! with `hash_block`. A missing body is a gap the archive failed to keep, not a proof about any
//! other height.

use std::collections::BTreeSet;

/// The operator's history settings (from `api-server.archive*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryConfig {
    /// Advertise and serve the archive tier.
    pub archive: bool,
    /// Heights of history retained when archiving (`0` = all).
    pub history_retention: i64,
    /// Archive endpoints this node was configured with (`--archive-peers`).
    pub archive_peers: Vec<String>,
    /// `receipt` (default), `allowance`, or `open`.
    pub credit_policy: String,
    /// The archive's settlement public key (hex); required by a metered policy.
    pub settlement_key: Option<String>,
    /// `payer-key = bytes` allowances (`--archive-allow`).
    pub allowances: Vec<String>,
    /// The settlement period's length in seconds.
    pub period_seconds: u64,
    /// Per-payer serving rate.
    pub rate_bytes_per_sec: u64,
    /// The most bytes one payer may be served in a period.
    pub max_bytes_per_period: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            archive: false,
            history_retention: 0,
            archive_peers: Vec::new(),
            // **A metered archive by default.** A node that advertises the tier without a settlement
            // key refuses to start rather than silently serving history unmetered; `--archive-open`
            // is the explicit opt-out.
            credit_policy: "receipt".to_string(),
            settlement_key: None,
            allowances: Vec::new(),
            period_seconds: 3_600,
            rate_bytes_per_sec: 1 << 20,
            max_bytes_per_period: 1 << 30,
        }
    }
}

impl HistoryConfig {
    /// The policy this configuration names, with `--archive-open` taking precedence.
    pub fn policy(&self) -> Result<crate::web::history_service::CreditPolicy, String> {
        crate::web::history_service::CreditPolicy::parse(&self.credit_policy)
    }
}

/// The most heights one challenge may sample. A bounded response, independent of the chain length.
pub const MAX_CHALLENGE_COUNT: usize = 256;

/// The default sample size: 64 detects a 10% gap with ≈99.9% probability.
pub const DEFAULT_CHALLENGE_COUNT: usize = 64;

/// **Sample `count` distinct heights in `[0, max_height]` deterministically from `nonce`.**
///
/// SplitMix64 seeded by `blake2b256(nonce)`; rejection sampling keeps the distinctness without
/// hashing every height, so a challenge over a two-million-height chain costs O(count). The order is
/// ascending so the response is canonical and two archives answering the same nonce can be compared.
pub fn sample_heights(nonce: &str, count: usize, max_height: i64) -> Vec<i64> {
    if max_height < 0 || count == 0 {
        return Vec::new();
    }
    let total = (max_height as u128) + 1;
    let take = count
        .min(MAX_CHALLENGE_COUNT)
        .min(usize::try_from(total).unwrap_or(usize::MAX));
    let digest = rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::create(nonce.as_bytes());
    // The seed is the first eight bytes of the digest, big-endian. Built byte by byte: the slice is
    // a fixed 32-byte array, and an explicit fold keeps the construction total without a conversion
    // that could fail.
    let mut state: u64 = 0;
    for byte in &digest.as_bytes()[..8] {
        state = (state << 8) | u64::from(*byte);
    }
    let mut chosen: BTreeSet<i64> = BTreeSet::new();
    while chosen.len() < take {
        state = splitmix64(state);
        chosen.insert((u128::from(state) % total) as i64);
    }
    chosen.into_iter().collect()
}

fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_sample_is_deterministic_distinct_and_in_range() {
        let first = sample_heights("abc", 64, 1_000);
        assert_eq!(
            first,
            sample_heights("abc", 64, 1_000),
            "same nonce, same sample"
        );
        assert_eq!(first.len(), 64);
        let unique: BTreeSet<i64> = first.iter().copied().collect();
        assert_eq!(unique.len(), 64, "heights are distinct");
        assert!(first.iter().all(|h| (0..=1_000).contains(h)));
        assert_ne!(
            first,
            sample_heights("abd", 64, 1_000),
            "another nonce samples elsewhere"
        );
    }

    #[test]
    fn the_sample_is_bounded_by_the_count_cap_and_the_height_range() {
        assert_eq!(
            sample_heights("x", 10_000, 10).len(),
            11,
            "only the heights that exist"
        );
        assert_eq!(
            sample_heights("x", 10_000, 10_000_000).len(),
            MAX_CHALLENGE_COUNT,
            "the per-response cap"
        );
        assert_eq!(sample_heights("x", 10, 3).len(), 4);
        assert!(sample_heights("x", 10, -1).is_empty());
        assert!(sample_heights("x", 0, 10).is_empty());
    }
}
