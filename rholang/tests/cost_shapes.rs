//! **The charge shapes, measured through the reducer** — the second half of the cost gate.
//!
//! `tools/check-cost-register.py` reads the *declaration*: every arm of the reducer has a row in
//! `spec/cost-register.tsv`, and the charge it names occurs where the row says. That check cannot see
//! whether the charge still *grows*. This one can: for each shape family it evaluates the same
//! operation at two sizes and reads the **charged phlo** — deterministic, unlike wall time — so a
//! charge that regresses to a constant while its constructor is still called fails here.
//!
//! The isolating idiom is the register's own: the same expression without the operation is the
//! baseline, so the literal's evaluation and the method dispatch cancel, and the difference of
//! differences is the operation's marginal charge.
//!
//! **Not env-gated**, unlike `gas_amplification.rs`: it measures charge, not time, and runs in
//! well under a second, so it belongs in `cargo test --workspace`.
//!
//! **What it does not claim.** It does not prove the work matches the charge — the charge is linear
//! for `O(n log n)` work, which is the recorded log-factor residual (Law 91a's note). It says the
//! charge is not flat, and that it grows at the declared rate for the operations it samples.

mod common;

use common::build_runtime;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[7u8; 32])
}

/// The phlo one term charges to evaluate.
async fn charged(term: &str) -> i64 {
    let rt = build_runtime(false).await;
    let res = rt
        .evaluate(term, &fixed_rand())
        .await
        .expect("no node fault");
    assert!(
        res.errors.is_empty(),
        "the shape term must evaluate cleanly: {term}: {:?}",
        res.errors
    );
    res.cost.value
}

/// The operation's own charge at size `n`: the same expression without it is the baseline.
async fn marginal(term: String, baseline: String) -> i64 {
    charged(&term).await - charged(&baseline).await
}

fn ints(n: usize) -> String {
    let items: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    items.join(",")
}

/// **A constant operation does not grow with its operand.** `nth` indexes one element: the charge
/// must be the same at 8 elements and at 64.
#[tokio::test]
async fn constant_operations_do_not_grow_with_the_operand() {
    let list = |n: usize| format!("[{}]", ints(n));
    let small = marginal(format!("{}.nth(0)", list(8)), list(8)).await;
    let large = marginal(format!("{}.nth(0)", list(64)), list(64)).await;
    assert_eq!(
        small, large,
        "`nth` must charge the same at 8 and at 64 elements ({small} vs {large})"
    );
}

/// **A linear charge grows with the operand, element for element.** `toSet` canonicalises its output
/// and charges 3 phlo per element, so 48 more elements must cost 144 more phlo.
#[tokio::test]
async fn linear_operations_grow_with_their_operand() {
    let small = marginal(format!("[{}].toSet()", ints(16)), format!("[{}]", ints(16))).await;
    let large = marginal(format!("[{}].toSet()", ints(64)), format!("[{}]", ints(64))).await;
    assert_eq!(
        large - small,
        3 * (64 - 16),
        "`toSet` must charge 3 phlo per element it canonicalises ({small} → {large})"
    );
}

/// **A product charge grows with both operands.** `indexOf` scans a haystack a window at a time, so
/// its worst case is `n · k`: growing the haystack by 48 at a 4-character needle must add
/// `48 · 4` phlo, and a flat or haystack-only charge fails.
#[tokio::test]
async fn product_operations_grow_with_both_operands() {
    let hay = |n: usize| "a".repeat(n);
    let small = marginal(
        format!("\"{}\".indexOf(\"bbbb\")", hay(16)),
        format!("\"{}\"", hay(16)),
    )
    .await;
    let large = marginal(
        format!("\"{}\".indexOf(\"bbbb\")", hay(64)),
        format!("\"{}\"", hay(64)),
    )
    .await;
    assert!(
        large - small >= (64 - 16) * 4,
        "`indexOf` must charge the product of haystack and needle ({small} → {large}); a charge that \
         tracks only the haystack adds {}",
        (64 - 16)
    );
}

/// **A product of two growing operands grows quadratically.** `diff` on two `n`-element sets charges
/// `3 · n · n`; going from 16 to 32 elements must add at least `9 · 16²` phlo, which no linear charge
/// can reach.
#[tokio::test]
async fn quadratic_operations_grow_with_the_square() {
    // Both arms produce the **same** output — the `diff` term's result is empty every time — so the
    // baseline is a constant empty set rather than a set of the operand's size, whose own storage
    // charge would otherwise swamp the difference.
    let set = |n: usize| format!("[{}].toSet()", ints(n));
    let small = marginal(
        format!("{}.diff({})", set(16), set(16)),
        "Set()".to_string(),
    )
    .await;
    let large = marginal(
        format!("{}.diff({})", set(32), set(32)),
        "Set()".to_string(),
    )
    .await;
    assert!(
        large - small >= 9 * 16 * 16,
        "`diff` must charge the product of its operands ({small} → {large})"
    );
}
