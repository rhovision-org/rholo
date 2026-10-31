//! **Liveness enforcement's pure records and predicates** (RCHIP #55, Law 89).
//!
//! The state machine itself lives in [`crate::native_state`] because it writes the PoS leaves at an
//! epoch boundary; this module holds the pieces that must be identical on play and replay and that
//! are worth testing without a store: the fixed-stride codecs, the quorum-supported tip, the
//! absence predicate, and the vote threshold (delegating to the one `is_super_majority` rule).
//!
//! **The bond is not in any of these types.** Jail and vote records carry counters, windows and
//! ballots; a bond is only reachable through the existing `NativeSystemState::slash`, which the
//! boundary calls when a vote passes. That is the structural half of "jail first; only a vote can
//! slash".

use std::collections::{BTreeMap, BTreeSet};

use rchain_models::validator::Validator;
use rchain_shared::refined::{BlockHeight, NonNegI64};

/// A validator public key's byte length (the `Validator` refinement).
pub const VALIDATOR_LEN: usize = 65;
const I64_LEN: usize = 8;
const ANCHOR_LEN: usize = 32 + I64_LEN;

/// One validator's consecutive-absence and jail state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AbsenceRecord {
    /// Advancing boundaries in a row on which the validator was absent. Reset by one present
    /// boundary; a jail resets it when imposed.
    pub absent_boundaries: i64,
    /// Advancing boundaries left in the current jail; `0` means not jailed. The bond is never here.
    pub jail_remaining: i64,
}

impl AbsenceRecord {
    pub fn is_jailed(&self) -> bool {
        self.jail_remaining > 0
    }
}

/// The last boundary at which finality actually advanced: the fringe state hash and the
/// quorum-supported tip it carried. Both are derived by every node; the anchor makes "advance"
/// a comparison rather than a wall-clock decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub fringe_state_hash: [u8; 32],
    pub quorum_tip: i64,
}

/// One open slash vote against one jailed validator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoteWindow {
    /// Boundary at which the jail (and this window) opened.
    pub opened_at: i64,
    /// Advancing boundaries left in the window; `0` means the window has closed.
    pub remaining: i64,
}

/// A ballot's key: the target first, then the voter, so the canonical `BTreeMap` order groups all
/// ballots for one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BallotKey {
    pub target: Validator,
    pub voter: Validator,
}

const ABSENCE_ENTRY_LEN: usize = VALIDATOR_LEN + 2 * I64_LEN;
const VOTE_WINDOW_ENTRY_LEN: usize = VALIDATOR_LEN + 2 * I64_LEN;
const BALLOT_ENTRY_LEN: usize = 2 * VALIDATOR_LEN + 1;

fn validator_at(bytes: &[u8], at: usize) -> Validator {
    Validator::from_slice(&bytes[at..at + VALIDATOR_LEN])
}

fn read_i64(bytes: &[u8], at: usize) -> i64 {
    let mut arr = [0u8; I64_LEN];
    arr.copy_from_slice(&bytes[at..at + I64_LEN]);
    i64::from_le_bytes(arr)
}

fn stride_error(what: &str, len: usize, stride: usize) -> String {
    format!("{what} encoding has {len} bytes, not a multiple of {stride}")
}

pub fn encode_absence(records: &BTreeMap<Validator, AbsenceRecord>) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.len() * ABSENCE_ENTRY_LEN);
    for (validator, record) in records {
        out.extend_from_slice(validator.as_bytes());
        out.extend_from_slice(&record.absent_boundaries.to_le_bytes());
        out.extend_from_slice(&record.jail_remaining.to_le_bytes());
    }
    out
}

pub fn decode_absence(bytes: &[u8]) -> Result<BTreeMap<Validator, AbsenceRecord>, String> {
    if bytes.len() % ABSENCE_ENTRY_LEN != 0 {
        return Err(stride_error("liveness", bytes.len(), ABSENCE_ENTRY_LEN));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(ABSENCE_ENTRY_LEN) {
        out.insert(
            validator_at(chunk, 0),
            AbsenceRecord {
                absent_boundaries: read_i64(chunk, VALIDATOR_LEN),
                jail_remaining: read_i64(chunk, VALIDATOR_LEN + I64_LEN),
            },
        );
    }
    Ok(out)
}

pub fn encode_anchor(anchor: &Anchor) -> Vec<u8> {
    let mut out = Vec::with_capacity(ANCHOR_LEN);
    out.extend_from_slice(&anchor.fringe_state_hash);
    out.extend_from_slice(&anchor.quorum_tip.to_le_bytes());
    out
}

pub fn decode_anchor(bytes: &[u8]) -> Result<Anchor, String> {
    if bytes.len() != ANCHOR_LEN {
        return Err(format!(
            "liveness anchor has {} bytes, not {ANCHOR_LEN}",
            bytes.len()
        ));
    }
    let mut fringe = [0u8; 32];
    fringe.copy_from_slice(&bytes[..32]);
    Ok(Anchor {
        fringe_state_hash: fringe,
        quorum_tip: read_i64(bytes, 32),
    })
}

pub fn encode_vote_windows(windows: &BTreeMap<Validator, VoteWindow>) -> Vec<u8> {
    let mut out = Vec::with_capacity(windows.len() * VOTE_WINDOW_ENTRY_LEN);
    for (validator, window) in windows {
        out.extend_from_slice(validator.as_bytes());
        out.extend_from_slice(&window.opened_at.to_le_bytes());
        out.extend_from_slice(&window.remaining.to_le_bytes());
    }
    out
}

pub fn decode_vote_windows(bytes: &[u8]) -> Result<BTreeMap<Validator, VoteWindow>, String> {
    if bytes.len() % VOTE_WINDOW_ENTRY_LEN != 0 {
        return Err(stride_error(
            "liveness vote-windows",
            bytes.len(),
            VOTE_WINDOW_ENTRY_LEN,
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(VOTE_WINDOW_ENTRY_LEN) {
        out.insert(
            validator_at(chunk, 0),
            VoteWindow {
                opened_at: read_i64(chunk, VALIDATOR_LEN),
                remaining: read_i64(chunk, VALIDATOR_LEN + I64_LEN),
            },
        );
    }
    Ok(out)
}

pub fn encode_ballots(ballots: &BTreeMap<BallotKey, bool>) -> Vec<u8> {
    let mut out = Vec::with_capacity(ballots.len() * BALLOT_ENTRY_LEN);
    for (key, in_favor) in ballots {
        out.extend_from_slice(key.target.as_bytes());
        out.extend_from_slice(key.voter.as_bytes());
        out.push(u8::from(*in_favor));
    }
    out
}

pub fn decode_ballots(bytes: &[u8]) -> Result<BTreeMap<BallotKey, bool>, String> {
    if bytes.len() % BALLOT_ENTRY_LEN != 0 {
        return Err(stride_error(
            "liveness ballots",
            bytes.len(),
            BALLOT_ENTRY_LEN,
        ));
    }
    let mut out = BTreeMap::new();
    for chunk in bytes.chunks_exact(BALLOT_ENTRY_LEN) {
        let flag = chunk[2 * VALIDATOR_LEN];
        let in_favor = match flag {
            0 => false,
            1 => true,
            other => return Err(format!("ballot flag is {other}, not 0 or 1")),
        };
        out.insert(
            BallotKey {
                target: validator_at(chunk, 0),
                voter: validator_at(chunk, VALIDATOR_LEN),
            },
            in_favor,
        );
    }
    Ok(out)
}

pub fn encode_validator_set(set: &BTreeSet<Validator>) -> Vec<u8> {
    let mut out = Vec::with_capacity(set.len() * VALIDATOR_LEN);
    for validator in set {
        out.extend_from_slice(validator.as_bytes());
    }
    out
}

pub fn decode_validator_set(bytes: &[u8]) -> Result<BTreeSet<Validator>, String> {
    if bytes.len() % VALIDATOR_LEN != 0 {
        return Err(stride_error("banished", bytes.len(), VALIDATOR_LEN));
    }
    Ok(bytes
        .chunks_exact(VALIDATOR_LEN)
        .map(|chunk| validator_at(chunk, 0))
        .collect())
}

/// **The height a supermajority of the active stake has reached.** The same derivation the finalizer
/// uses (`rchain_sdk::consensus::quorum_supported_height`), over the active map and the boundary's
/// receiver-derived participation; `0` means no supermajority has spoken (an empty or young fringe).
pub fn quorum_tip(
    active: &BTreeMap<Validator, NonNegI64>,
    participation: &BTreeMap<Validator, BlockHeight>,
) -> i64 {
    let mut rows: Vec<(i64, i128)> = active
        .iter()
        .filter_map(|(validator, stake)| {
            participation
                .get(validator)
                .map(|height| (i64::from(*height), i128::from(i64::from(*stake))))
        })
        .collect();
    let total: i128 = active
        .values()
        .map(|stake| i128::from(i64::from(*stake)))
        .sum();
    rows.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut supporting = 0i128;
    for (height, stake) in rows {
        supporting += stake;
        if rchain_sdk::consensus::is_super_majority(supporting, total) {
            return height;
        }
    }
    0
}

/// **88/89's absence predicate.** A validator with no message in the finalised fringe, or one whose
/// latest message is more than `threshold` heights behind the quorum-supported tip, is absent for
/// this boundary. The caller must not apply this when the tip is `0` (no evidence).
pub fn is_absent(
    participation: &BTreeMap<Validator, BlockHeight>,
    tip: i64,
    threshold: i64,
    validator: &Validator,
) -> bool {
    match participation.get(validator) {
        None => true,
        Some(height) => tip.saturating_sub(i64::from(*height)) > threshold,
    }
}

/// **A vote passes only above two thirds of the active stake.** Delegates to the one supermajority
/// rule (Law 14); exact integer arithmetic, no float.
pub fn passes_vote(for_stake: i128, total_stake: i128) -> bool {
    rchain_sdk::consensus::is_super_majority(for_stake, total_stake)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validator(byte: u8) -> Validator {
        Validator::new([byte; 65])
    }

    fn height(n: i64) -> BlockHeight {
        BlockHeight::try_from(n).expect("height")
    }

    fn stake(n: i64) -> NonNegI64 {
        NonNegI64::try_from(n).expect("stake")
    }

    #[test]
    fn the_absence_codec_round_trips() {
        let records = BTreeMap::from([
            (
                validator(1),
                AbsenceRecord {
                    absent_boundaries: 2,
                    jail_remaining: 5,
                },
            ),
            (
                validator(2),
                AbsenceRecord {
                    absent_boundaries: 0,
                    jail_remaining: 0,
                },
            ),
        ]);
        assert_eq!(
            decode_absence(&encode_absence(&records)).expect("decode"),
            records
        );
        assert!(decode_absence(&[0u8; 3]).is_err());
    }

    #[test]
    fn the_anchor_and_vote_codecs_round_trip() {
        let anchor = Anchor {
            fringe_state_hash: [7u8; 32],
            quorum_tip: 42,
        };
        assert_eq!(
            decode_anchor(&encode_anchor(&anchor)).expect("anchor"),
            anchor
        );
        assert!(decode_anchor(&[0u8; 39]).is_err());

        let windows = BTreeMap::from([(
            validator(1),
            VoteWindow {
                opened_at: 100,
                remaining: 3,
            },
        )]);
        assert_eq!(
            decode_vote_windows(&encode_vote_windows(&windows)).expect("windows"),
            windows
        );

        let ballots = BTreeMap::from([(
            BallotKey {
                target: validator(1),
                voter: validator(2),
            },
            true,
        )]);
        assert_eq!(
            decode_ballots(&encode_ballots(&ballots)).expect("ballots"),
            ballots
        );
        let mut bad = encode_ballots(&ballots);
        *bad.last_mut().expect("flag") = 2;
        assert!(decode_ballots(&bad).is_err());
    }

    #[test]
    fn the_quorum_tip_is_the_height_a_supermajority_reached() {
        let active = BTreeMap::from([
            (validator(1), stake(10)),
            (validator(2), stake(10)),
            (validator(3), stake(10)),
        ]);
        // Two of three is exactly 2/3, not a supermajority; the third's height is the tip.
        let participation = BTreeMap::from([
            (validator(1), height(50)),
            (validator(2), height(40)),
            (validator(3), height(30)),
        ]);
        assert_eq!(quorum_tip(&active, &participation), 30);
        // With the third absent, no supermajority has spoken.
        let two = BTreeMap::from([(validator(1), height(50)), (validator(2), height(40))]);
        assert_eq!(quorum_tip(&active, &two), 0);
        // A larger active set where 3 of 4 is a supermajority.
        let active4 = BTreeMap::from([
            (validator(1), stake(10)),
            (validator(2), stake(10)),
            (validator(3), stake(10)),
            (validator(4), stake(10)),
        ]);
        let three = BTreeMap::from([
            (validator(1), height(50)),
            (validator(2), height(40)),
            (validator(3), height(30)),
        ]);
        assert_eq!(quorum_tip(&active4, &three), 30);
    }

    #[test]
    fn absence_is_missing_or_behind_the_threshold() {
        let participation = BTreeMap::from([(validator(1), height(90))]);
        assert!(
            !is_absent(&participation, 100, 10, &validator(1)),
            "exactly at the threshold is present"
        );
        assert!(
            is_absent(&participation, 100, 9, &validator(1)),
            "one past the threshold is absent"
        );
        assert!(
            is_absent(&participation, 100, 10, &validator(2)),
            "no message is absent"
        );
    }

    #[test]
    fn a_vote_needs_strictly_more_than_two_thirds() {
        assert!(!passes_vote(2, 3));
        assert!(passes_vote(3, 4));
        assert!(passes_vote(7, 10));
        assert!(!passes_vote(6, 10));
    }
}
