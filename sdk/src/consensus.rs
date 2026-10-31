//! Consensus stake helpers.
//!
//! Mirrors `sdk/src/main/scala/coop/rchain/sdk/consensus/Stake.scala`.
//! Law 14: finality requires strictly more than 2/3 of bonded stake.

use std::collections::BTreeMap;

use rchain_shared::refined::{BlockHeight, NonNegI64};

/// Check if `stake` prevails the 2/3 supermajority threshold of `total_stake`.
///
/// Law 14: strictly more than 2/3 of bonded stake. This is the *exact* integer comparison
/// `3 * stake > 2 * total_stake` (computed in `i128`, so it cannot overflow); the Scala
/// `stake.toDouble / totalStake > 2d / 3` loses precision for stakes ≥ 2⁵³ and is
/// rounding-dependent at the exact 2/3 boundary.
///
/// Takes `i128` so callers accumulate bonded-stake sums in `i128` (a validator set with large
/// stakes can overflow `i64` before reaching this comparison).
pub fn is_super_majority(stake: i128, total_stake: i128) -> bool {
    stake * 3 > total_stake * 2
}

/// **The height a supermajority of the bonded stake has reached.** The finalizer's liveness clock:
/// take each sender's latest height, highest first, and return the first height at which the stake
/// behind it is a strict supermajority of `bonds`. `0` means no supermajority has spoken.
///
/// The rule is here rather than duplicated in `block-storage` and the PoS liveness rule: a
/// second copy of "which height has a quorum reached" is a second consensus rule waiting to drift.
pub fn quorum_supported_height<S>(
    bonds: &BTreeMap<S, NonNegI64>,
    latest: &BTreeMap<S, BlockHeight>,
) -> BlockHeight
where
    S: Ord,
{
    let total: i128 = bonds
        .values()
        .map(|stake| i128::from(i64::from(*stake)))
        .sum();
    let mut at_height: Vec<(BlockHeight, i128)> = bonds
        .iter()
        .filter_map(|(sender, stake)| {
            latest
                .get(sender)
                .map(|height| (*height, i128::from(i64::from(*stake))))
        })
        .collect();
    at_height.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut supporting = 0i128;
    for (height, stake) in at_height {
        supporting += stake;
        if is_super_majority(supporting, total) {
            return height;
        }
    }
    BlockHeight::zero()
}

#[cfg(test)]
mod tests {
    use super::is_super_majority;

    #[test]
    fn two_thirds_is_not_supermajority() {
        // 2/3 exactly is NOT > 2/3.
        assert!(!is_super_majority(2, 3));
    }

    #[test]
    fn above_two_thirds_is_supermajority() {
        assert!(is_super_majority(3, 4));
    }

    #[test]
    fn below_two_thirds_is_not_supermajority() {
        assert!(!is_super_majority(1, 3));
    }

    #[test]
    fn large_stake_just_above_two_thirds_is_exact() {
        // stake = 2·2^53 + 1, total = 3·2^53: the ratio is just above 2/3. The old f64 form
        // cannot represent 2·2^53+1 (it rounds to 2·2^53) and misclassifies this as "not a
        // supermajority"; the exact integer form is correct.
        let stake = 2 * (1i128 << 53) + 1;
        let total = 3 * (1i128 << 53);
        assert!(is_super_majority(stake, total));
    }

    #[test]
    fn i64_overflowing_stakes_do_not_wrap() {
        // A validator set whose total bonded stake exceeds i64::MAX must still compare exactly.
        // Two validators at i64::MAX each sum past i64::MAX in i64 arithmetic but fit in i128.
        let total = i128::from(i64::MAX) * 3;
        let two_thirds = i128::from(i64::MAX) * 2;
        assert!(!is_super_majority(two_thirds, total));
        assert!(is_super_majority(two_thirds + 1, total));
    }
}
