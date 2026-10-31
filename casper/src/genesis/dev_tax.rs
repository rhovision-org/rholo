//! The developer allocation — derived from the total the wallets file allocates.
//!
//! **A deliberate, registered deviation, not a port.** The Scala ceremony installs exactly the vaults
//! its wallets file names. This module derives one allocation that no wallets file contains, so it is
//! recorded as a hard-fork deviation in `spec/audit/passes.md` and carries Law 78 in
//! `spec/Rchain/Genesis.lean`: the rule below is stated and proved there, and this file is the Rust the
//! law is tied to. **The allocation is not a vault**: it is credited to a *vesting account* and
//! released on a schedule, which is Law 79 (`spec/Rchain/Vesting.lean`, and the arithmetic in
//! `rholang/src/native_state.rs::dev_tax_vested`). This module is *how much*; that one is *when*.
//!
//! **Why derived rather than a data line.** REV has no mint — supply *is* the genesis wallets
//! file (`rholang/src/system_processes.rs`, "REV is minted at genesis only") — and the vault balances
//! are installed as native state outside the genesis block's deploys, so they are content-addressed
//! into the genesis post-state hash (AUDIT C46). A number fixed in a file the operator supplies would
//! therefore be a number a node replaying the genesis can disagree about; deriving it from the same
//! file every node already reads makes the two compute the same hash by construction.
//!
//! **What "the same file" has to mean, and the seam where the claim stops.** The total this module
//! reads is the total the genesis will *install*, and `dev_allocation` refuses the wallets files for
//! which that is not the same as the sum of the rows. And the agreement it buys is between a node that
//! **replays** the genesis and one that created it: a node that instead **restores** the chain over LFS
//! takes the bootstrap's genesis as given (`insert_genesis` marks it validated without replaying it).
//! The restore-completion guard (`engine::node_launch::check_running_dev_vesting`) enforces a local
//! claim when the joiner has one — bonds + wallets + a non-empty `dev-tax-address` — and trusts the
//! bootstrap when it has none; an absent claim is not a claim of no allocation. That boundary is stated
//! here rather than implied away, because an earlier draft of this module's prose claimed more than the
//! restore path delivers.
//!
//! The rule is `max(20 000 000 REV, 2% of the allocated total)`. The arms cross at exactly one billion
//! REV (`DEV_TAX_CROSSOVER`), so below that allocation the floor governs and above it the percentage
//! does.

use std::collections::BTreeSet;

use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::NonNegI64;

use super::contracts::Vault;

/// Drops per REV. The tree has no other code constant for this (it is a docs/tests convention —
/// `node/src/api/faucet.rs` prices 0.3 REV at 30 000 000 drops, the same 1e8); introducing it here
/// names the unit the allocation is stated in rather than leaving it implicit in a literal.
pub const DROPS_PER_REV: i128 = 100_000_000;

/// **The production genesis recipient, hard-coded in the repository.** The maintainer's custody
/// decision, kept here rather than shipped as a config default: an empty `dev-tax-address` resolves to
/// this address on a production node and to no allocation inside dev mode, so every production genesis
/// (and every node replaying one) uses the same recipient while devnets and tests stay allocation-free.
pub const PRODUCTION_DEV_TAX_ADDRESS: &str =
    "11112aRbXppXw33soKBN3B7tPHpZ18foZvyBqr1tKS2vx3V7aVHsQq";

/// **The address a genesis uses.** An explicit configuration wins (another network can name its own
/// recipient); an empty one is [`PRODUCTION_DEV_TAX_ADDRESS`] outside dev mode and `""` — no
/// allocation — inside it. Resolved once, before genesis creation and the replay descriptors read it.
pub fn effective_address(configured: &str, dev_mode: bool) -> String {
    if !configured.is_empty() {
        configured.to_string()
    } else if dev_mode {
        String::new()
    } else {
        PRODUCTION_DEV_TAX_ADDRESS.to_string()
    }
}

/// The floor of the allocation, in REV.
pub const DEV_TAX_FLOOR_REV: i128 = 20_000_000;

/// The floor, in drops.
pub const DEV_TAX_FLOOR: i128 = DEV_TAX_FLOOR_REV * DROPS_PER_REV;

/// The rate, in basis points (200 = 2%), on the existing `executor_share` convention.
pub const DEV_TAX_BPS: i128 = 200;

/// The basis-point denominator, on the existing `executor_share` convention.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// The allocated total at which the percentage arm first reaches the floor — **one billion REV**
/// (1e17 drops). Below it the floor governs; at or above it, the percentage.
pub const DEV_TAX_CROSSOVER: i128 = DEV_TAX_FLOOR * BPS_DENOMINATOR / DEV_TAX_BPS;

/// The allocation owed on an allocated total of `total_drops`: `max(20M REV, 2% of the total)`.
///
/// Integer division truncates, so the rounding dust stays with the network rather than the
/// allocation. Overflow is refused rather than clamped, per this crate's rule for a genesis value
/// that decides an amount (`contracts::ProofOfStake` does the same to a negative bond bound).
pub fn dev_tax_amount(total_drops: i128) -> Result<NonNegI64, String> {
    if total_drops < 0 {
        return Err(format!(
            "the allocated total cannot be negative, got {total_drops} drops"
        ));
    }
    let percentage = total_drops
        .checked_mul(DEV_TAX_BPS)
        .ok_or_else(|| format!("the allocated total {total_drops} drops overflows the 2% arm"))?
        / BPS_DENOMINATOR;
    let amount = DEV_TAX_FLOOR.max(percentage);
    let amount = i64::try_from(amount).map_err(|_| {
        format!("the developer allocation of {amount} drops does not fit a vault balance")
    })?;
    NonNegI64::try_from(amount).map_err(|e| e.to_string())
}

/// Parse a configured `dev-tax-address`, for the startup check and for the derivation — one statement
/// of what a valid address is, so the two cannot drift.
pub fn parse_address(address: &str) -> Result<RevAddress, String> {
    RevAddress::parse(address).map_err(|e| {
        format!("casper.genesis-block-data.dev-tax-address `{address}` is not a REV address: {e}")
    })
}

/// The developer allocation for a parsed wallets file: the recipient and the amount, or `None` when no
/// address is configured.
///
/// **The amount is derived from a file whose addresses are unique, and both of the ways it could be
/// otherwise are refused rather than absorbed.**
///
/// * *A repeated holder address* is refused. `set_vault_balance` keys a vault on its base58 address and
///   **overwrites**, so a wallets file that names an address twice installs only its last row — the
///   rows are not a total the chain holds, and deriving from their sum allocates on REV that never
///   exists. Above the crossover the difference is real: two rows of `A,1e17` sum to `2e17` and would
///   take the percentage arm for 40M, while the genesis installs one billion REV for `A` and owes 20M.
///   Enforcing uniqueness here rather than canonicalising silently is deliberate — a repeated address is
///   a data error, and last-wins is the kind of quiet behaviour this tree rejects elsewhere. **Only the
///   allocation boundary refuses it**: an empty recipient returns before any of this, so a chain with no
///   allocation keeps taking exactly the wallets files it took before.
/// * *An entry for the recipient itself* is refused, because the allocation is credited to a separate
///   vesting account rather than to a vault — a wallets entry for the same address would be a second,
///   unvested credit to it.
///
/// The total is the sum of the balances the genesis will install, which — with uniqueness enforced — is
/// now the same number as the sum of the rows.
pub fn dev_allocation(
    vaults: &[Vault],
    address: &str,
) -> Result<Option<(RevAddress, NonNegI64)>, String> {
    if address.is_empty() {
        return Ok(None);
    }
    let rev_address = parse_address(address)?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total = 0i128;
    for v in vaults {
        if v.rev_address == rev_address {
            return Err(format!(
                "the wallets file already names the developer address {address}: the allocation is a \
                 separate vesting account, and an entry for the same address would be a second, \
                 unvested credit to it"
            ));
        }
        let key = v.rev_address.to_base58();
        if !seen.insert(key.clone()) {
            return Err(format!(
                "the wallets file names the address {key} more than once: the genesis installs one \
                 balance per address, so the repeated rows are not a total the chain holds, and the \
                 allocation is derived from the balance that is installed"
            ));
        }
        total = total
            .checked_add(i128::from(i64::from(v.initial_balance)))
            .ok_or_else(|| "the allocated total overflows".to_string())?;
    }
    Ok(Some((rev_address, dev_tax_amount(total)?)))
}

/// **The gate at genesis creation** (Law 78): a production genesis must decide the allocation.
///
/// `Err` means "refuse". Kept beside the rule so the condition and its sentence are one thing a test
/// can drive without standing up a node; the caller supplies the context that this is the branch that
/// creates a genesis.
///
/// `dev_mode` is the whole of the predicate, and both of the other candidates are wrong:
/// `standalone` alone would refuse a standalone node **restarting** onto a chain it already built (the
/// caller guards that with `dag_set.is_empty()`), and "a genesis is being created" alone would refuse
/// every devnet, which creates one too.
pub fn refuse_a_production_genesis_without_an_address(
    dev_mode: bool,
    address: &str,
) -> Result<(), String> {
    if !dev_mode && address.is_empty() {
        return Err(
            "casper.genesis-block-data.dev-tax-address is empty and this node is creating a genesis \
             block outside --dev-mode: the developer allocation is fixed by the genesis and cannot be \
             added afterwards without a hard fork, so it is not a value to leave unset. Set it, or \
             run with --dev-mode for a network that allocates none"
                .to_string(),
        );
    }
    Ok(())
}

/// **The second half of the production genesis gate** (Law 79): an allocation with no schedule is
/// not a production term.
///
/// `0` is a real rule — the whole allocation is released at the first epoch boundary — and it is the
/// right default for a chain with no allocation, where the address is empty and this gate does not
/// apply. When an address **is** set outside `--dev-mode`, a zero term makes the entire allocation
/// spendable at once, so a production ceremony has to state the term it intends. A negative term is
/// already refused where the config becomes a number (`proof_of_stake_from_config`).
pub fn refuse_a_production_genesis_with_a_zero_vesting_term(
    dev_mode: bool,
    address: &str,
    vesting_blocks: i64,
) -> Result<(), String> {
    if !dev_mode && !address.is_empty() && vesting_blocks == 0 {
        return Err(
            "casper.genesis-block-data.dev-tax-vesting-blocks is 0 and this node is creating a \
             genesis block outside --dev-mode: a zero term releases the whole developer allocation at \
             the first epoch boundary. Set the term (the shipped value is 36 months at 5 s/block), or \
             run with --dev-mode for a network whose allocation need not vest"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REV as drops, for readable expectations.
    fn rev(n: i128) -> i128 {
        n * DROPS_PER_REV
    }

    fn amount(total_rev: i128) -> i64 {
        i64::from(dev_tax_amount(rev(total_rev)).expect("a valid total"))
    }

    /// **The law's Rust witness.** The floor governs below one billion REV and the percentage at or
    /// above it, and the two arms meet exactly at the crossover.
    #[test]
    fn the_floor_governs_below_a_billion_rev_and_two_percent_above() {
        // Just below the crossover: 2% is under the floor, so the floor governs.
        let just_below = DEV_TAX_CROSSOVER - 1;
        assert!(
            just_below * DEV_TAX_BPS / BPS_DENOMINATOR < DEV_TAX_FLOOR,
            "the crossover must be the *first* total at which 2% reaches the floor"
        );
        assert_eq!(
            i64::from(dev_tax_amount(just_below).unwrap()),
            DEV_TAX_FLOOR as i64
        );

        // At the crossover both arms are equal: one billion REV allocates 20M REV either way.
        assert_eq!(DEV_TAX_CROSSOVER, rev(1_000_000_000));
        assert_eq!(
            DEV_TAX_CROSSOVER * DEV_TAX_BPS / BPS_DENOMINATOR,
            DEV_TAX_FLOOR
        );
        assert_eq!(amount(1_000_000_000), DEV_TAX_FLOOR as i64);

        // Above it the percentage governs and exceeds the floor.
        assert_eq!(amount(2_000_000_000), rev(40_000_000) as i64);
    }

    /// The floor is never undershot, at any total.
    #[test]
    fn the_allocation_is_never_below_the_floor() {
        for total in [0i128, 1, rev(1), rev(999_999), rev(1_000_000_000)] {
            assert!(i64::from(dev_tax_amount(total).unwrap()) >= DEV_TAX_FLOOR as i64);
        }
    }

    /// Monotone: a larger allocation never owes less.
    #[test]
    fn the_allocation_is_monotone_in_the_total() {
        let mut last = 0i64;
        for total in [
            0i128,
            rev(1),
            rev(500_000_000),
            rev(1_000_000_000),
            rev(5_000_000_000),
        ] {
            let now = i64::from(dev_tax_amount(total).unwrap());
            assert!(now >= last, "the allocation fell going up to {total} drops");
            last = now;
        }
    }

    /// The allocation on the allocation this network actually has: 908 962 714 REV, dated 13 May 2026
    /// (`docs/src/node/history-chain.md`). 2% of it is 18 179 254 REV, under the floor, so the
    /// allocation is 20 000 000 REV and the genesis total becomes 928 962 714 REV.
    #[test]
    fn the_published_allocation_takes_the_floor() {
        let published = 908_962_714i128;
        assert_eq!(amount(published), DEV_TAX_FLOOR as i64);
        // 2% of the published total is 18 179 254.28 REV — under the floor, so it is not taken.
        assert_eq!(
            rev(published) * DEV_TAX_BPS / BPS_DENOMINATOR,
            1_817_925_428_000_000
        );
        assert_eq!(
            published + amount(published) as i128 / DROPS_PER_REV,
            928_962_714
        );
    }

    /// The allocation is a whole number of drops and truncates toward the network.
    #[test]
    fn the_percentage_arm_truncates_the_dust() {
        // Above the crossover, and /50 leaves a remainder: the exact quotient is
        // 20 000 000 000 000 000.66, kept as the integer.
        let total = 1_000_000_000_000_000_033i128;
        assert!(total > DEV_TAX_CROSSOVER);
        assert_eq!(
            i64::from(dev_tax_amount(total).unwrap()),
            20_000_000_000_000_000
        );
        let dust = (total * DEV_TAX_BPS) % BPS_DENOMINATOR;
        assert!(
            dust > 0 && dust < BPS_DENOMINATOR,
            "expected the arm to leave dust, got {dust}"
        );
    }

    /// An implausible total is refused, not clamped.
    #[test]
    fn an_implausible_total_is_refused() {
        assert!(dev_tax_amount(-1).is_err());
        assert!(dev_tax_amount(i128::MAX).is_err());
    }

    /// A distinct address per byte, so a test can name its parties.
    fn addr(second: u8) -> RevAddress {
        let mut key = vec![0u8; 65];
        key[0] = 0x04;
        key[1] = second;
        RevAddress::from_public_key(&rchain_crypto::public_key::PublicKey::new(key)).unwrap()
    }

    /// One wallets row.
    fn row(address: &RevAddress, balance: i64) -> Vault {
        Vault {
            rev_address: address.clone(),
            initial_balance: NonNegI64::try_from(balance).unwrap(),
        }
    }

    /// One billion REV, in drops — the amount at which the two arms of the rule meet.
    const BILLION_REV: i64 = 100_000_000_000_000_000;

    /// **An empty recipient allocates nothing — and checks nothing.** The legacy path returns before
    /// the allocation boundary, so a chain with no allocation keeps taking exactly the wallets files it
    /// took before, including one the boundary would refuse.
    #[test]
    fn no_address_allocates_nothing_and_checks_nothing() {
        assert_eq!(dev_allocation(&[], "").unwrap(), None);
        let a = addr(0x00);
        let duplicated = vec![row(&a, 1), row(&a, 1)];
        assert_eq!(
            dev_allocation(&duplicated, "").unwrap(),
            None,
            "the empty path must not acquire the uniqueness rule"
        );
    }

    /// The allocation is derived from the file's total, and the recipient comes back with it.
    #[test]
    fn the_allocation_is_derived_from_the_file_total() {
        let holder = addr(0x00);
        let dev = addr(0x01);
        let vaults = vec![row(&holder, rev(908_962_714) as i64)];
        let (address, amount) = dev_allocation(&vaults, &dev.to_base58()).unwrap().unwrap();
        assert_eq!(address, dev, "the recipient is returned with the amount");
        assert_eq!(i64::from(amount), rev(20_000_000) as i64);
    }

    /// **A repeated holder address is refused, for identical and for different balances** — the
    /// regression the review asked for. Above the crossover, where the difference the refusal prevents
    /// is visible: two rows of `A,1e17` sum to `2e17` and would take the percentage arm for 40M REV,
    /// while the genesis installs one billion REV for `A` and owes 20M. The distinct-holder control is
    /// what keeps the test from passing by refusing everything.
    #[test]
    fn a_repeated_holder_address_is_refused() {
        let a = addr(0x00);
        let b = addr(0x01);
        let dev = addr(0x02);

        for (label, second) in [("identical", BILLION_REV), ("different", 1)] {
            let vaults = vec![row(&a, BILLION_REV), row(&a, second)];
            let err = dev_allocation(&vaults, &dev.to_base58())
                .expect_err("a repeated holder address must not reach the total");
            assert!(
                err.contains("more than once"),
                "{label} balances: the refusal names the repeat: {err}"
            );
        }

        // The control: two *distinct* holders really do sum to 2e17, and 2% of that is 40M.
        let distinct = vec![row(&a, BILLION_REV), row(&b, BILLION_REV)];
        let (_, amount) = dev_allocation(&distinct, &dev.to_base58())
            .unwrap()
            .unwrap();
        assert_eq!(
            i64::from(amount),
            rev(40_000_000) as i64,
            "the refusal is about the repeat, not about the amount"
        );
    }

    /// **A wallets entry for the recipient itself is refused.** The allocation is credited to a
    /// separate vesting account, so an entry for the same address would be a second, unvested credit.
    #[test]
    fn a_wallets_entry_for_the_recipient_is_refused() {
        let a = addr(0x00);
        let dev = addr(0x01);
        let vaults = vec![row(&a, 1), row(&dev, 1)];
        let err = dev_allocation(&vaults, &dev.to_base58())
            .expect_err("the recipient must not also be a holder");
        assert!(err.contains("developer address"), "{err}");
    }

    /// **The gate at genesis creation**: a production genesis must decide, a dev network need not, and
    /// a set address is the operator's decision either way.
    #[test]
    fn a_production_genesis_without_an_address_is_refused() {
        assert!(refuse_a_production_genesis_without_an_address(false, "").is_err());
        assert!(refuse_a_production_genesis_without_an_address(false, "any-address").is_ok());
        assert!(refuse_a_production_genesis_without_an_address(true, "").is_ok());
        assert!(refuse_a_production_genesis_without_an_address(true, "any-address").is_ok());
        let err = refuse_a_production_genesis_without_an_address(false, "").unwrap_err();
        assert!(
            err.contains("dev-tax-address") && err.contains("--dev-mode"),
            "the refusal names the key and the escape: {err}"
        );
    }

    /// **A production allocation must state its term.** Zero is a real rule (release everything at
    /// the first boundary) but it is not a production term; it stays legal for a devnet and for a
    /// chain with no allocation, where the address is empty and this gate does not apply.
    #[test]
    fn a_production_genesis_with_a_zero_vesting_term_is_refused() {
        let address = "any-address";
        assert!(refuse_a_production_genesis_with_a_zero_vesting_term(false, address, 0).is_err());
        assert!(refuse_a_production_genesis_with_a_zero_vesting_term(false, address, 1).is_ok());
        assert!(refuse_a_production_genesis_with_a_zero_vesting_term(true, address, 0).is_ok());
        assert!(refuse_a_production_genesis_with_a_zero_vesting_term(false, "", 0).is_ok());
        let err =
            refuse_a_production_genesis_with_a_zero_vesting_term(false, address, 0).unwrap_err();
        assert!(
            err.contains("dev-tax-vesting-blocks") && err.contains("--dev-mode"),
            "the refusal names the key and the escape: {err}"
        );
    }

    /// **The repository carries the production recipient; dev mode never inherits it.** An empty
    /// configured address resolves to the constant outside dev mode and to no allocation inside it;
    /// an explicit value wins in either mode (another network can name its own recipient).
    #[test]
    fn an_empty_address_resolves_to_the_production_recipient_outside_dev_mode_only() {
        assert_eq!(
            effective_address("", false),
            PRODUCTION_DEV_TAX_ADDRESS,
            "a production genesis whose config leaves the address empty uses the repo constant"
        );
        assert_eq!(
            effective_address("", true),
            "",
            "a devnet must not inherit a real allocation"
        );
        let explicit = "11112VYAt8rUGNRRZX3eJdgagaAhtWTK8Js7F7X5iqddMVqyDTtYau";
        assert_eq!(effective_address(explicit, false), explicit);
        assert_eq!(effective_address(explicit, true), explicit);
    }
}
