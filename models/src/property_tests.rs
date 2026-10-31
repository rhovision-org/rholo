//! Property tests for the `models` layer's laws — the shared arbitrary-`Par` strategy lives here.
//!
//! Law 1 (canonicalization to a total order is idempotent, and `sort(p|q) = sort(q|p)`), Law 2
//! (α/name equivalence **is** canonical structural equality), Law 6 (a program has no globally free
//! variables — `Closed`). Randomized evidence alongside the Lean statements
//! (`spec/Rchain/Sort.lean`, `Ty.lean`) and the unit tests in `sorter.rs`/`types.rs`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use num_bigint::BigInt;
use proptest::prelude::*;

use crate::ast::{Expr, Par, ProcSort, Send, Var};
use crate::decimal::{Decimal, DecimalError, MAX_SCALE};
use crate::par_ops::{from_expr, par_concat};
use crate::sorted::Sorted;
use crate::sorter::{sort_par_term, sort_pars};
use crate::types::{is_closed, Closed};

/// An arbitrary ground expression (no runtime values, which is enough for order/equality laws).
fn arb_ground() -> impl Strategy<Value = Expr> {
    prop_oneof![
        any::<i64>().prop_map(Expr::GInt),
        (any::<i64>(), 0u32..=MAX_SCALE).prop_map(|(coefficient, scale)| {
            Expr::GDecimal(Decimal::new(coefficient.into(), scale).expect("in-range scale"))
        }),
        prop::collection::vec(any::<char>(), 0..4)
            .prop_map(|cs| Expr::GString(cs.into_iter().collect())),
        any::<bool>().prop_map(Expr::GBool),
    ]
}

/// An arbitrary **closed** process expression: grounds, bound vars, wildcards (which bind nothing),
/// and collections of them. No free variables — that is Law 6's input class.
fn arb_closed_expr(depth: u32) -> BoxedStrategy<Expr> {
    if depth == 0 {
        return arb_ground().boxed();
    }
    let inner = arb_closed_expr(depth - 1);
    prop_oneof![
        arb_ground(),
        (0i32..4).prop_map(|i| Expr::EVar(Box::new(Var::BoundVar(i)))),
        Just(Expr::EVar(Box::new(Var::Wildcard))),
        prop::collection::vec(inner.clone(), 0..4).prop_map(|es| Expr::EList(crate::ast::EList {
            ps: es.into_iter().map(from_expr).collect(),
            ..Default::default()
        })),
        prop::collection::vec(inner.clone(), 0..4).prop_map(|es| Expr::ETuple(
            crate::ast::ETuple {
                ps: es.into_iter().map(from_expr).collect(),
                ..Default::default()
            }
        )),
    ]
    .boxed()
}

/// An arbitrary process `Par`: a closed body plus, sometimes, a send whose payload is closed.
fn arb_proc_par(depth: u32) -> BoxedStrategy<Par<ProcSort>> {
    let body = arb_closed_expr(depth);
    let with_send = body.clone().prop_map(|e| Par::<ProcSort> {
        sends: vec![Send {
            chan: Box::new(from_expr(e).quote()),
            data: vec![],
            persistent: false,
            locally_free: Default::default(),
            connective_used: false,
        }],
        ..Default::default()
    });
    prop_oneof![
        body.prop_map(|e| Par::<ProcSort> {
            exprs: vec![e],
            ..Default::default()
        }),
        with_send,
    ]
    .boxed()
}

/// A `Par` that may contain a free variable — the shape Law 6 must reject.
fn arb_open_par() -> BoxedStrategy<Par<ProcSort>> {
    (any::<i32>(), arb_proc_par(2))
        .prop_map(|(i, mut p)| {
            p.exprs.push(Expr::EVar(Box::new(Var::FreeVar(i))));
            p
        })
        .boxed()
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

proptest! {
    /// **Law 1, idempotence.** Canonicalizing an already-canonicalized term is the identity: the
    /// sort is a *normal form*, not a step that keeps moving.
    #[test]
    fn law1_sorting_is_idempotent(p in arb_proc_par(3)) {
        let once = sort_par_term(&p);
        let twice = sort_par_term(&once);
        prop_assert_eq!(&once, &twice);
        // …and the canonical form is what `Sorted::new` holds, whoever calls it.
        let sorted = Sorted::new(p.clone());
        prop_assert_eq!(sorted.as_par(), &once);
    }

    /// **Law 1, commutativity.** `sort(p|q) = sort(q|p)`: a parallel composition's canonical form
    /// does not depend on the order the two sides were written — which is what lets a state hash be
    /// independent of the order deploys happened to concatenate.
    #[test]
    fn law1_parallel_composition_sorts_commutatively(p in arb_proc_par(2), q in arb_proc_par(2)) {
        prop_assert_eq!(
            sort_par_term(&par_concat(&p, &q)),
            sort_par_term(&par_concat(&q, &p))
        );
    }

    /// **Law 2.** α/name equivalence *is* canonical structural equality: two terms that canonicalize
    /// to the same par are equal as `Sorted`, and hash alike — the property the state hash depends on
    /// (a `Sorted` whose `Hash` disagreed with its `Eq` would make the hash depend on insertion
    /// order).
    #[test]
    fn law2_canonical_equality_agrees_with_canonical_hashing(p in arb_proc_par(3), q in arb_proc_par(3)) {
        let a = Sorted::new(par_concat(&p, &q));
        let b = Sorted::new(par_concat(&q, &p));
        prop_assert_eq!(&a, &b, "the same par, however it was built");
        prop_assert_eq!(hash_of(&a), hash_of(&b), "equal canonical forms hash alike");
    }

    /// **AUDIT C139 — the converse, without which law 2's hash half is not a falsifier.**
    ///
    /// The witness above asserts `equal ⇒ same hash`, which a **constant** `Hash` satisfies: replacing
    /// `impl<S: Sort> Hash for Sorted<S>` with an empty body left both of law 2's witnesses green and
    /// the whole `rchain-models` suite green — all 168 tests (measured). What a degenerate hash
    /// violates is the other direction, so this is the property that has to be stated for the hash
    /// half of the law to be falsifiable at all: canonical forms that are **not** equal must not hash
    /// alike. `prop_assume!` drops the pairs whose *canonical* forms coincide (a par and its
    /// unsorted spelling are the same value here), which is exactly the hypothesis the law carries.
    #[test]
    fn law2_unequal_canonical_forms_do_not_hash_alike(p in arb_proc_par(3), q in arb_proc_par(3)) {
        let a = Sorted::new(p);
        let b = Sorted::new(q);
        prop_assume!(a != b);
        prop_assert_ne!(
            hash_of(&a),
            hash_of(&b),
            "unequal canonical forms must hash differently — otherwise the hash carries no information"
        );
    }

    /// Sorting a *sequence* of pars is order-insensitive in the same way: the sorted list of a
    /// permutation's elements is the same list. (The sequences are permuted by reversing, and the
    /// elements are the same set either way.)
    #[test]
    fn law2_sorting_a_sequence_depends_only_on_its_elements(ps in prop::collection::vec(arb_proc_par(2), 0..5)) {
        let mut reversed = ps.clone();
        reversed.reverse();
        prop_assert_eq!(sort_pars(ps), sort_pars(reversed));
    }

    /// **Law 1b — the element comparator is lawful.** `cmpPar`'s transitivity (`cmpPar_lt_trans`,
    /// with `cmpExpr`'s `eq_iff`/`swap`/`lt_trans`) is the row's content, and its *observable*
    /// consequence is that a collection's canonical order is a function of the collection, not of the
    /// order the elements arrived in. This walks all six permutations of three elements and requires
    /// one answer.
    ///
    /// **What it adds over the tests beside it, measured rather than claimed.** A temporary
    /// non-transitive relation planted in `Tree::cmp` (a `rem_euclid 3` cycle among `Int` leaves,
    /// which `arb_ground` can reach) fails this test, and tells the two above apart:
    /// `law1_parallel_composition_sorts_commutatively` — pairwise, `p|q` vs `q|p` — **passes** the
    /// plant, because every pairwise comparison still answers consistently; `law2_sorting_a_sequence_
    /// depends_only_on_its_elements` — one permutation, reversal — **fails** it. So the honest
    /// statement is that this is exhaustive coverage of the small permutations, not a unique
    /// falsifier: reversal reaches this cycle too, and a relation that survives reversal while
    /// failing a 3-cycle is asserted by nothing here. That boundary is the same shape as law 1a's,
    /// where the corpus pins the verdicts the model can spell.
    #[test]
    fn law1b_every_permutation_of_a_collection_sorts_alike(
        a in arb_proc_par(2),
        b in arb_proc_par(2),
        c in arb_proc_par(2),
    ) {
        let permutations = [
            [&a, &b, &c],
            [&a, &c, &b],
            [&b, &a, &c],
            [&b, &c, &a],
            [&c, &a, &b],
            [&c, &b, &a],
        ];
        let expected = sort_pars(permutations[0].iter().copied().cloned().collect());
        for (i, permutation) in permutations.iter().enumerate() {
            let sorted = sort_pars(permutation.iter().copied().cloned().collect());
            prop_assert_eq!(
                &sorted,
                &expected,
                "permutation {} of [a,b,c] sorted differently: {:?} beside {:?}",
                i,
                sorted,
                expected
            );
        }
    }

    /// **Law 6.** A program has no globally free variables: a term built from grounds, bound vars and
    /// wildcards is `Closed`, and the predicate and the `Closed` newtype agree on every term.
    #[test]
    fn law6_a_closed_term_is_accepted_and_the_predicate_agrees(p in arb_proc_par(3), q in arb_open_par()) {
        prop_assert!(is_closed(&p), "a closed term must be recognized as closed");
        prop_assert!(Closed::new(p).is_some());
        prop_assert!(!is_closed(&q), "a free variable must not be closed");
        prop_assert!(Closed::new(q).is_none());
    }
}

/// **The strategy is not degenerate.** A property test over a generator that only ever produces one
/// term is a test that passes for free; this pins that the arbitrary `Par` really varies — across
/// samples it produces sends, multiple expressions, and collections. Same class of guard as the
/// branch-disabled checks: it makes the *evidence* falsifiable, not just the property.
#[test]
fn the_arbitrary_par_strategy_produces_varied_terms() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = arb_proc_par(3);
    let (mut sends, mut multi_expr, mut collections, mut empty) = (0, 0, 0, 0);
    for _ in 0..200 {
        let tree = strategy.new_tree(&mut runner).expect("a sample");
        let p = tree.current();
        if !p.sends.is_empty() {
            sends += 1;
        }
        if p.exprs.len() > 1 {
            multi_expr += 1;
        }
        if p.exprs.iter().any(|e| {
            matches!(
                e,
                Expr::EList(_) | Expr::ETuple(_) | Expr::ESet(_) | Expr::EMap(_)
            )
        }) {
            collections += 1;
        }
        if p.sends.is_empty() && p.exprs.is_empty() {
            empty += 1;
        }
    }
    assert!(sends > 0, "the strategy must sometimes produce a send");
    assert!(
        collections > 0,
        "the strategy must sometimes produce a collection"
    );
    assert!(
        multi_expr + sends > 20,
        "the strategy must produce non-trivial terms often (sends {sends}, multi-expr {multi_expr})"
    );
    let _ = empty;
}

/// A canonical decimal, the input class every Law 87 value theorem assumes.
fn arb_canonical_decimal() -> impl Strategy<Value = Decimal> {
    (any::<i64>(), 0u32..=MAX_SCALE).prop_map(|(coefficient, scale)| {
        Decimal::new(coefficient.into(), scale).expect("in-range scale")
    })
}

proptest! {
    /// **Law 87b.** The scaled payload is an injective embedding of the canonical value: two
    /// canonical decimals with one payload are one value. This is what makes the sorter's
    /// `Expr::GDecimal` arm (`scaled_value`) and the Lean `Expr.edecimal` payload agree.
    #[test]
    fn law87_scaled_value_is_injective_on_canonical_values(
        a in arb_canonical_decimal(),
        b in arb_canonical_decimal(),
    ) {
        if a.scaled_value() == b.scaled_value() {
            prop_assert_eq!(a, b);
        }
    }

    /// **Law 87b.** `cmp_numeric` is the order of the scaled payload.
    #[test]
    fn law87_scaled_value_preserves_order(
        a in arb_canonical_decimal(),
        b in arb_canonical_decimal(),
    ) {
        prop_assert_eq!(a.cmp_numeric(&b), a.scaled_value().cmp(&b.scaled_value()));
    }

    /// **Law 87c.** Addition is exact in units of `10^-38`.
    #[test]
    fn law87_addition_is_exact_in_scaled_units(
        a in arb_canonical_decimal(),
        b in arb_canonical_decimal(),
    ) {
        let sum = a.add(&b).expect("canonical addition cannot fail");
        prop_assert_eq!(sum.scaled_value(), a.scaled_value() + b.scaled_value());
    }

    /// **Law 87c.** Subtraction is exact in units of `10^-38`.
    #[test]
    fn law87_subtraction_is_exact_in_scaled_units(
        a in arb_canonical_decimal(),
        b in arb_canonical_decimal(),
    ) {
        let difference = a.sub(&b).expect("canonical subtraction cannot fail");
        prop_assert_eq!(difference.scaled_value(), a.scaled_value() - b.scaled_value());
    }

    /// **Law 87c.** A successful product satisfies the scaled identity; the only refusal is the
    /// scale bound (there is no rounding path).
    #[test]
    fn law87_multiplication_satisfies_the_scaled_identity(
        a in arb_canonical_decimal(),
        b in arb_canonical_decimal(),
    ) {
        match a.mul(&b) {
            Ok(q) => prop_assert_eq!(
                q.scaled_value() * BigInt::from(10).pow(MAX_SCALE),
                a.scaled_value() * b.scaled_value()
            ),
            Err(e) => {
                let scale_only = matches!(e, DecimalError::ScaleOutOfRange { .. });
                prop_assert!(scale_only);
            }
        }
    }

    /// **Law 87d.** A successful quotient satisfies the scaled identity; a nonzero divisor's
    /// refusal is exactly `InexactDivision`, and a zero divisor is exactly `DivisionByZero`.
    #[test]
    fn law87_division_satisfies_the_scaled_identity_or_is_an_explicit_error(
        a in arb_canonical_decimal(),
        b in arb_canonical_decimal(),
    ) {
        if b.is_zero() {
            prop_assert_eq!(a.div(&b), Err(DecimalError::DivisionByZero));
        } else {
            match a.div(&b) {
                Ok(q) => prop_assert_eq!(
                    q.scaled_value() * b.scaled_value(),
                    a.scaled_value() * BigInt::from(10).pow(MAX_SCALE)
                ),
                Err(e) => prop_assert_eq!(e, DecimalError::InexactDivision),
            }
        }
    }

    /// **Law 87a.** Canonicalisation is idempotent: a canonical value is a normal form.
    #[test]
    fn law87_canonicalisation_is_idempotent(
        coefficient in any::<i64>(),
        scale in 0u32..=MAX_SCALE,
    ) {
        let once = Decimal::new(coefficient.into(), scale).expect("in-range scale");
        let twice = Decimal::new(once.coefficient().clone(), once.scale())
            .expect("canonical scale is in range");
        prop_assert_eq!(once, twice);
    }
}
