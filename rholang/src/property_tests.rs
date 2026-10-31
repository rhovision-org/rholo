//! Property tests for the rholang layer's laws.
//!
//! Law 3 (capture-avoiding de Bruijn substitution, total on `Closed`, and `sort(subst t) =
//! subst(sort t)`) and Law 5 (spatial matching binds a free variable **at most once**). Randomized
//! evidence alongside the Lean statements (`Rchain/Subst.lean`, `Match.lean`) and the unit tests in
//! `substitute.rs`/`matcher/`.

use proptest::prelude::*;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestCaseError;

use rchain_models::ast::{AlwaysEqual, EList, ETuple, Expr, Par, ProcSort, Send, Var};
use rchain_models::par_ops::{from_expr, par_concat};
use rchain_models::sorted::SortedProc;
use rchain_models::sorter::sort_par_term;
use rchain_models::types::is_closed;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::env::Env;
use crate::matcher::{spatial_match, FreeMap};
use crate::scheduler::EffectMode;

/// An arbitrary **closed** process: grounds, bound vars, wildcards, and collections of them.
/// **Law 46 and the absence rule as properties, over the space the Lean statements quantify over.**
///
/// Issue #150's close condition asks for the chosen formula to be "held by a property test against the
/// Lean statement", and until this module there was none for the reward: the laws' witnesses are unit
/// tests, and the two property-test modules in the tree cover laws 3/4/5/21/25 (here) and 26/27/29
/// (`casper/src/property_tests.rs`). A unit test fixes an instance; these two theorems are about *every*
/// instance, and the difference is what this module exists to close.
///
/// Both are tested through the functions the Lean names — `epoch_reward` (`reward`) and `apply_weight`
/// (`absenceAdjusted` over `participationWeight`) — rather than through `close_block`, so the property
/// is about the arithmetic itself and not about the boundary plumbing that calls it. The plumbing has
/// its own tests.
mod reward_laws {
    use super::*;
    use crate::native_state::{apply_weight, epoch_reward};
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, NonNegI64};
    use std::collections::BTreeMap;

    fn validator(i: usize) -> Validator {
        let byte = u8::try_from(i).expect("a test index");
        Validator::new([byte; 65])
    }

    fn nn(v: i64) -> NonNegI64 {
        NonNegI64::try_from(v).expect("a non-negative test amount")
    }

    proptest! {
        /// **Law 46 — the shares never exceed the pot, and none is negative.**
        ///
        /// The Lean hypotheses are the contract's own and are reproduced here rather than assumed: the
        /// active total **is** the sum of the members' bonds (`hactive`, which is why the generator sums
        /// them rather than taking an independent draw), and the normaliser is positive (`hD`). With
        /// those two, `sum_rewards_le_pot` holds for every list of stakes — and the ranges here are wide
        /// enough that a formula which dropped either division, or multiplied the wrong factor, would
        /// break it on the first few cases.
        #[test]
        fn law46_the_shares_never_exceed_the_pot(
            pot in 0i64..1_000_000_000,
            minimum_bond in 1i64..1_000,
            bonds in prop::collection::vec(0i64..100_000, 1..6),
        ) {
            let active_bonds: i64 = bonds.iter().sum();
            prop_assume!(active_bonds / minimum_bond > 0);
            let shares: Vec<i64> = bonds
                .iter()
                .map(|b| epoch_reward(pot, minimum_bond, active_bonds, *b).expect("in range"))
                .collect();
            let total: i128 = shares.iter().map(|s| i128::from(*s)).sum();
            prop_assert!(
                total <= i128::from(pot),
                "shares {:?} sum to {total}, above the pot {pot}",
                shares
            );
            prop_assert!(shares.iter().all(|s| *s >= 0), "a share cannot be negative: {:?}", shares);
        }

        /// **The rule never raises a reward, and never invents or drops a validator**: every key that
        /// went in comes out with a value no larger than it had.
        ///
        /// **This replaced a sub-map statement, and the replacement is not cosmetic.** The rule used to
        /// *filter* — so "same values, or keys gone" was exactly its shape — and it now *scales*, which
        /// keeps every key and lowers some values. A test still asserting the sub-map would fail against
        /// the real rule, which is the useful kind of failure but not the property the register needs.
        /// What survives is `absence_never_raises`, entry by entry, plus
        /// `the_absence_rule_moves_no_stake` — a bond is still not an argument to it.
        #[test]
        fn law44_the_absence_rule_never_raises_a_reward(
            grace in 0usize..8,
            knee in 0usize..8,
            heights in prop::collection::vec(0i64..20, 1..5),
            boundary in 0i64..20,
            reward in 0i64..1_000_000,
        ) {
            let rewards: BTreeMap<Validator, NonNegI64> =
                (0..heights.len()).map(|i| (validator(i), nn(reward))).collect();
            let participation: BTreeMap<Validator, BlockHeight> = heights
                .iter()
                .enumerate()
                .map(|(i, h)| (validator(i), BlockHeight::try_from(*h).expect("a test height")))
                .collect();
            let out = apply_weight(
                rewards.clone(),
                &participation,
                boundary,
                i64::try_from(grace).expect("a small grace"),
                i64::try_from(knee).expect("a small knee"),
                &BTreeSet::new(),
            );

            prop_assert_eq!(out.len(), rewards.len(), "a scaling keeps every key");
            for (v, paid) in &out {
                prop_assert!(
                    i64::from(*paid) <= i64::from(rewards[v]),
                    "a validator is paid at most what it was owed, whatever its lag"
                );
            }
        }

        /// **The rule withholds.** This is the direction the property above does **not** pin, and that
        /// is not a hypothetical: the identity function — a weight that is always the whole share —
        /// satisfies `law44_the_absence_rule_never_raises_a_reward`,
        /// `law44_a_validator_inside_the_grace_is_kept` and the Lean implications, because those are all
        /// of the form "if it withholds, it withholds correctly". A rule that withholds nothing was,
        /// until this pair of tests, indistinguishable from the real one by every check in the tree.
        ///
        /// A validator silent *past the knee*, under a rule that is on, is paid **nothing** — and the
        /// reward it does not receive is not moved anywhere, it stays in the vault for a later epoch.
        #[test]
        fn law44_a_validator_outside_the_knee_is_not_paid(
            slack in 1usize..5,
            extra in 1i64..3,
            reward in 1i64..1_000_000,
            boundary in 10i64..40,
        ) {
            let slack = i64::try_from(slack).expect("a small slack");
            let v = validator(1);
            let rewards = BTreeMap::from([(v, nn(reward))]);
            // The ranges are chosen so this cannot go negative — a `prop_assume!` would be the other
            // way, and the first version of this test got it wrong and panicked in the generator
            // rather than in the assertion.
            let spoken_at = boundary - slack - extra;
            let participation = BTreeMap::from([(
                v,
                BlockHeight::try_from(spoken_at).expect("a height at or above zero"),
            )]);
            // `grace == knee` is the binary shape: the cliff the tree shipped before this was graded.
            let out = apply_weight(rewards, &participation, boundary, slack, slack, &BTreeSet::new());
            prop_assert_eq!(
                out.get(&v).map(|r| i64::from(*r)),
                Some(0),
                "a validator silent past the knee is paid nothing, whatever it was owed"
            );
        }

        /// **And one that has never spoken at all is as absent as it gets** — the missing-record
        /// branch, which is a separate arm of the rule and the one a validator that never bonded in
        /// would take.
        #[test]
        fn law44_a_validator_that_never_spoke_is_not_paid(slack in 1usize..8, boundary in 0i64..40) {
            let slack = i64::try_from(slack).expect("a small slack");
            let v = validator(1);
            let rewards = BTreeMap::from([(v, nn(100))]);
            let out = apply_weight(rewards, &BTreeMap::new(), boundary, slack, slack, &BTreeSet::new());
            prop_assert_eq!(
                out.get(&v).map(|r| i64::from(*r)),
                Some(0),
                "a validator with no entry has not spoken, so it is not paid"
            );
        }

        /// **And a knee of zero withholds nothing** — `Pos.rhox`'s behaviour, and the shipped default:
        /// the rule is off, so the oldest possible silence costs nothing. This is the opposite direction
        /// from the two tests above, and the three together are what make the rule's *switch* a checked
        /// thing rather than a claim.
        #[test]
        fn law44_a_zero_knee_withholds_nothing(
            heights in prop::collection::vec(0i64..20, 1..5),
            boundary in 0i64..40,
        ) {
            let rewards: BTreeMap<Validator, NonNegI64> =
                (0..heights.len()).map(|i| (validator(i), nn(100))).collect();
            let participation: BTreeMap<Validator, BlockHeight> = heights
                .iter()
                .enumerate()
                .map(|(i, h)| (validator(i), BlockHeight::try_from(*h).expect("a test height")))
                .collect();
            prop_assert_eq!(apply_weight(rewards.clone(), &participation, boundary, 0, 0, &BTreeSet::new()), rewards);
        }

        /// **And a validator inside the grace is paid in full** —
        /// `a_returning_validator_is_paid_in_full` as a property: a lag within the **grace** is
        /// indistinguishable from having just spoken, so the rule has nothing to recover from and an
        /// honest validator that was briefly away is made whole. The hypothesis is over the grace and
        /// not the knee, which is the whole reason the weight has two parameters.
        #[test]
        fn law44_a_validator_inside_the_grace_is_kept(
            grace in 1usize..8,
            offset in 0i64..8,
            boundary in 8i64..40,
        ) {
            let grace = i64::try_from(grace).expect("a small grace");
            prop_assume!(offset <= grace);
            let v = validator(1);
            let rewards = BTreeMap::from([(v, nn(100))]);
            let participation = BTreeMap::from([
                (v, BlockHeight::try_from(boundary - offset).expect("a height")),
            ]);
            prop_assert_eq!(
                apply_weight(rewards.clone(), &participation, boundary, grace, grace, &BTreeSet::new()),
                rewards
            );
        }

        /// **Between the grace and the knee the weight is partial** — the direction the binary rule
        /// could not express at all, and the reason the weight has two parameters rather than one.
        ///
        /// This is the Rust twin of the model's `the_ramp_is_a_ramp`, and it is the arm that fails if
        /// the ramp is dropped: a binary reading in place of it pays such a validator either everything
        /// or nothing, and never something strictly in between. `paid > 0` as well as `paid < reward`,
        /// because a rule that simply zeroed this band would satisfy the upper half alone.
        #[test]
        fn law44_between_the_grace_and_the_knee_a_validator_is_paid_in_part(
            grace in 0i64..5,
            spread in 2i64..6,
            offset in 1i64..5,
            reward in 100i64..1_000_000,
            boundary in 20i64..60,
        ) {
            let knee = grace + spread;
            let behind = grace + offset;
            prop_assume!(behind < knee);
            let v = validator(1);
            let rewards = BTreeMap::from([(v, nn(reward))]);
            let participation = BTreeMap::from([
                (v, BlockHeight::try_from(boundary - behind).expect("a height")),
            ]);
            let out = apply_weight(rewards, &participation, boundary, grace, knee, &BTreeSet::new());
            let paid = i64::from(*out.get(&v).expect("a scaling keeps every key"));
            prop_assert!(paid < reward, "inside the ramp the share is cut");
            prop_assert!(paid > 0, "and it is not cut to nothing");
        }
    }
}

fn arb_closed(depth: u32) -> BoxedStrategy<Par<ProcSort>> {
    if depth == 0 {
        return any::<i64>().prop_map(|i| from_expr(Expr::GInt(i))).boxed();
    }
    let inner = arb_closed(depth - 1);
    let expr = prop_oneof![
        any::<i64>().prop_map(Expr::GInt),
        prop::collection::vec(any::<char>(), 0..3)
            .prop_map(|cs| Expr::GString(cs.into_iter().collect())),
        (0i32..3).prop_map(|i| Expr::EVar(Box::new(Var::BoundVar(i)))),
        prop::collection::vec(inner.clone(), 0..3).prop_map(|ps| Expr::EList(EList {
            ps,
            ..Default::default()
        })),
        prop::collection::vec(inner.clone(), 0..3).prop_map(|ps| Expr::ETuple(ETuple {
            ps,
            ..Default::default()
        })),
    ];
    let with_send = (inner.clone(), inner.clone()).prop_map(|(chan, payload)| Par::<ProcSort> {
        sends: vec![Send {
            chan: Box::new(chan.quote()),
            data: vec![payload.quote()],
            persistent: false,
            locally_free: AlwaysEqual(vec![]),
            connective_used: false,
        }],
        ..Default::default()
    });
    prop_oneof![
        expr.prop_map(|e| from_expr(e)),
        with_send,
        (inner.clone(), inner.clone()).prop_map(|(a, b)| par_concat(&a, &b)),
    ]
    .boxed()
}

/// An arbitrary **ground** par: grounds, collections of grounds, and sends of grounds — **no
/// `EVar` at all**.
///
/// This is the class a *datum* may come from. A par holding a `BoundVar` is `is_closed` by the
/// free-variable rule (a bound var is not free) but is not a valid datum: the reference points at a
/// binder outside the term, so the matcher refuses to bind it to a pattern variable. The first
/// version of the property below used `arb_closed`, and proptest's counterexample was exactly such a
/// dangling `BoundVar` — the generator, not the matcher, was wrong.
fn arb_ground_par(depth: u32) -> BoxedStrategy<Par<ProcSort>> {
    if depth == 0 {
        return prop_oneof![
            any::<i64>().prop_map(|i| from_expr(Expr::GInt(i))),
            prop::collection::vec(any::<char>(), 0..3)
                .prop_map(|cs| from_expr(Expr::GString(cs.into_iter().collect()))),
        ]
        .boxed();
    }
    let inner = arb_ground_par(depth - 1);
    prop_oneof![
        any::<i64>().prop_map(|i| from_expr(Expr::GInt(i))),
        prop::collection::vec(inner.clone(), 0..3).prop_map(|ps| from_expr(Expr::EList(EList {
            ps,
            ..Default::default()
        }))),
        prop::collection::vec(inner.clone(), 0..3).prop_map(|ps| from_expr(Expr::ETuple(ETuple {
            ps,
            ..Default::default()
        }))),
        (inner.clone(), inner.clone()).prop_map(|(chan, payload)| Par::<ProcSort> {
            sends: vec![Send {
                chan: Box::new(chan.quote()),
                data: vec![payload.quote()],
                persistent: false,
                locally_free: AlwaysEqual(vec![]),
                connective_used: false,
            }],
            ..Default::default()
        }),
    ]
    .boxed()
}

/// One variable in *pattern* position — the `@x` of `for (@[x, y] <- c)`, which the normalizer
/// builds as a par holding the free var with **`connective_used` set at every level**. That flag is
/// what makes the matcher match rather than compare for equality (`spatial_match`'s first branch,
/// faithful to Scala's `matchPar`), so a hand-built pattern that leaves it false silently becomes an
/// equality test — which is how the first version of these properties quietly failed.
fn var_pattern(level: i32) -> Par<ProcSort> {
    let mut p = from_expr(Expr::EVar(Box::new(Var::FreeVar(level))));
    p.connective_used = true;
    p
}

/// A pattern `[x, y]` over the two given levels, with the outer par, the **collection** and the
/// elements all marked connective.
///
/// The collection's own flag is the one the trap above bites at: `from_expr` derives the par's flag
/// from the expression's, `Expr::EList`'s is its own `connective_used` field (`par_ops.rs:87`), and
/// that field is `false` by default — so a fixture that sets only the outer par and the elements
/// leaves the *collection* concrete, and the matcher compares `[x, y]` against the datum for
/// equality instead of matching it. `var_pattern`'s comment records exactly this failure one level
/// down; this is the same failure one level up, and it made the law-5 property below unable to fail
/// (2026-09-24).
fn list_pattern(levels: (i32, i32)) -> Par<ProcSort> {
    let mut p = from_expr(Expr::EList(EList {
        ps: vec![var_pattern(levels.0), var_pattern(levels.1)],
        connective_used: true,
        ..Default::default()
    }));
    p.connective_used = true;
    p
}

/// The same collection with *distinct* free variables — the case that must match.
fn distinct_var_pattern(level: i32) -> impl Strategy<Value = Par<ProcSort>> {
    Just(level).prop_map(move |level| list_pattern((level, level + 1)))
}

/// The same collection with the *same* free variable twice — the case that must not match.
fn arb_repeated_var_pattern(level: i32) -> impl Strategy<Value = Par<ProcSort>> {
    Just(level).prop_map(move |level| list_pattern((level, level)))
}

proptest! {
    /// **Law 3, the refinement half.** Substituting a closed value into a closed term yields a closed
    /// term: the substitution is total on `Closed`, so the interpreter never has to invent a value
    /// for a variable that escaped a binder.
    #[test]
    fn law3_substituting_a_closed_value_keeps_the_term_closed(p in arb_closed(3), v in arb_closed(2)) {
        prop_assert!(is_closed(&p));
        let env = Env::make_env([v]);
        let out = crate::substitute::substitute_par(&p, 0, &env).expect("substitution");
        prop_assert!(is_closed(&out), "substitution must not free a variable");
    }

    /// **Law 3, the commutative square.** `sort(subst t) = subst(sort t)`: canonicalizing before or
    /// after substitution gives the same term, which is what lets the sorter and the substituter be
    /// applied in either order on the way to a state hash.
    #[test]
    fn law3_substitution_and_sorting_commute(p in arb_closed(3), v in arb_closed(2)) {
        let env = Env::make_env([v]);
        let sorted_first = crate::substitute::substitute_par(&sort_par_term(&p), 0, &env)
            .expect("substitution");
        let substituted_first =
            sort_par_term(&crate::substitute::substitute_par(&p, 0, &env).expect("substitution"));
        prop_assert_eq!(sorted_first, substituted_first);
    }

    /// **Law 5.** A free variable is bound **at most once**: a pattern that mentions the same
    /// variable twice never matches, even when the datum would make the two bindings *consistent*
    /// (`[x, x]` against `[1, 1]`). Without the rule that test would match and a pattern could pin
    /// two positions together, which the language does not allow.
    #[test]
    fn law5_a_pattern_that_binds_a_variable_twice_never_matches(pattern in arb_repeated_var_pattern(0)) {
        // The discriminating datum: both positions equal, so the naive binding is consistent.
        let data = from_expr(Expr::EList(EList {
            ps: vec![from_expr(Expr::GInt(1)), from_expr(Expr::GInt(1))],
            ..Default::default()
        }));
        let matches = spatial_match(&sort_par_term(&data), &sort_par_term(&pattern), &FreeMap::new())
            .expect("no error");
        prop_assert!(matches.is_empty(), "a doubly-bound pattern must not match: {matches:?}");
    }

    /// **The control the test above cannot fail without.** A pattern binds each name at most once
    /// *and* a pattern of distinct names does match: without this half, an assertion that a match is
    /// empty is satisfied equally well by a matcher that never matches anything — which is what the
    /// unfixed `list_pattern` fixture was, one flag away from the equality fast path.
    #[test]
    fn law5_a_pattern_that_binds_distinct_variables_matches(pattern in distinct_var_pattern(0)) {
        let data = from_expr(Expr::EList(EList {
            ps: vec![from_expr(Expr::GInt(1)), from_expr(Expr::GInt(2))],
            ..Default::default()
        }));
        let matches = spatial_match(&sort_par_term(&data), &sort_par_term(&pattern), &FreeMap::new())
            .expect("no error");
        prop_assert_eq!(matches.len(), 1, "a pattern of distinct variables must match once");
        let fm = &matches[0];
        prop_assert_eq!(fm.get(&0), Some(&from_expr(Expr::GInt(1))));
        prop_assert_eq!(fm.get(&1), Some(&from_expr(Expr::GInt(2))));
    }

    /// The other side of the same rule, through the **receiver's own entry point**: a `BindPattern`
    /// holding one bare variable matches any datum, and the value it binds is closed when the datum
    /// is (`RhoMatch::get` is what the store calls; `spatial_match` on a bare-var *par* is a lower
    /// layer whose contract is the element list, not the whole term).
    #[test]
    fn law5_a_variable_bind_pattern_matches_any_datum(target in arb_ground_par(2)) {
        use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
        use rchain_rspace::match_::Match;
        use rchain_models::runtime::{BindPattern, ListParWithRandom};

        use crate::storage::RhoMatch;

        // Built exactly as the `rho_match_binds_free_vars` unit test builds it — a par holding the
        // free var with `connective_used`, and *no* `locally_free` bookkeeping (`from_expr`'s
        // bookkeeping is what the sorter expects, not what the matcher does).
        let pattern = BindPattern {
            patterns: vec![SortedProc::new(Par {
                exprs: vec![Expr::EVar(Box::new(Var::FreeVar(0)))],
                connective_used: true,
                ..Default::default()
            })],
            remainder: None,
            free_count: 1,
        };
        let data = ListParWithRandom {
            pars: vec![SortedProc::new(target.clone())],
            random_state: Blake2b512Random::from_init(&[0u8; 32]),
        };

        let matched = RhoMatch
            .get(&pattern, &data)
            .expect("the matcher decided")
            .unwrap_or_else(|| panic!("a bare variable must match any datum, but did not match {target:?}"));
        for par in &matched.pars {
            prop_assert!(
                is_closed(par.as_par()),
                "the binding must be closed when the datum is: {:?}",
                par.as_par()
            );
        }
    }

    /// A ground pattern matches only an equal datum (`spatial_match` with no connectives is
    /// equality), and a *non-ground* closed pattern does not match an unrelated term — the matcher
    /// does not fall back to "anything goes".
    #[test]
    fn law5_a_ground_pattern_matches_only_itself(i in any::<i64>(), j in any::<i64>()) {
        let pattern = from_expr(Expr::GInt(i));
        let target = from_expr(Expr::GInt(j));
        let matches = spatial_match(&target, &pattern, &FreeMap::new()).expect("no error");
        prop_assert_eq!(!matches.is_empty(), i == j);
    }
}

/// **Law 3's *hypothesis*, pinned on the code side.** The closedness law holds because the
/// substituted *value* is closed, not because substitution is closed-preserving in general: put an
/// open value at the variable and a free variable necessarily ends up in the result. That is why
/// `Rchain/Subst.lean`'s `subst_closed` carries `∀ v, Closed (σ v)` — the statement without it is
/// false of this very function, and the property test below it would be false too.
///
/// Not a `proptest!` property: it is a fixed boundary case, and it is the boundary the model's axiom
/// got wrong (the register's law-3 note records the finding).
#[test]
fn an_open_value_at_the_variable_leaves_a_free_variable() {
    let term = from_expr(Expr::EVar(Box::new(Var::BoundVar(0))));
    let open = from_expr(Expr::EVar(Box::new(Var::FreeVar(0))));
    let out =
        crate::substitute::substitute_par(&term, 0, &Env::make_env([open])).expect("substitution");
    assert!(
        !is_closed(&out),
        "an open value at the variable must leave a free variable behind"
    );
}

/// **A wildcard (or free variable) in *term* position is an illegal substitution**, not a silent
/// pass-through: `substitute_par` returns `SubstituteError`, which is what Scala's
/// `maybeSubstitute(Var)` does (`SubstituteError(s"Illegal Substitution [$term]")` for anything that
/// is not a `BoundVar`). The distinction matters: a wildcard is a *pattern* construct (a receive
/// pattern keeps one, and substitution leaves it alone), while a wildcard standing as a term has no
/// value to substitute and no meaning to preserve.
#[test]
fn substituting_a_term_level_wildcard_is_an_error() {
    let term = from_expr(Expr::EVar(Box::new(Var::Wildcard)));
    let env = Env::make_env([from_expr(Expr::GInt(1))]);
    assert!(
        crate::substitute::substitute_par(&term, 0, &env).is_err(),
        "a term-level wildcard has no substitution"
    );
}

// --- Laws 21/25: the schedulers refine the sequential reference -------------------------------

/// A small, always-valid rholang program built from a template: a fresh channel, a few sends on it,
/// a receive that consumes them, and an optional second channel with a matching pair. Generated as
/// *source* because the schedulers are driven through `evaluate`.
fn arb_program() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(any::<i64>(), 1..4),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(values, persist, nested)| {
            let sends: String = values
                .iter()
                .map(|v| {
                    if persist {
                        format!(r#"c!!({v}) | "#)
                    } else {
                        format!(r#"c!({v}) | "#)
                    }
                })
                .collect();
            if nested {
                format!(
                    // The receive's body produces on a second channel: the shape that gives the
                    // scheduler a *chain* of effects (a receive whose continuation sends) rather than
                    // one flat round of COMMs. The payload is a literal — sending the bound variable
                    // itself would need quoting (`@x` binds a name in this position), which is a
                    // rholang-context subtlety the property is not about.
                    r#"new c, d in {{ {sends}for (@x <- c) {{ d!(0) }} | for (@y <- d) {{ Nil }} }}"#
                )
            } else {
                format!(r#"new c in {{ {sends}for (@x <- c) {{ Nil }} }}"#)
            }
        })
}

/// Build a runtime in the given effect mode, over a fresh store.
async fn runtime_in_mode(mode: EffectMode) -> crate::runtime::RhoRuntime {
    use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
    use rchain_models::sorted::SortedProc;
    use rchain_rspace::factory::create_history_repository;
    use rchain_rspace::hot_store::InMemHotStore;
    use rchain_rspace::rspace::RSpace;
    use rchain_shared::store_manager::InMemoryStoreManager;

    let manager = InMemoryStoreManager::default();
    let history = create_history_repository::<
        SortedProc,
        BindPattern,
        ListParWithRandom,
        TaggedContinuation,
    >(&manager, "scheduler-property")
    .await
    .expect("history repository");
    let reader = history.get_history_reader(history.root()).await;
    let hot = Arc::new(InMemHotStore::new(reader.base()));
    let (play, _replay) =
        RSpace::create_with_replay(history.clone(), hot, Arc::new(crate::storage::RhoMatch));
    crate::runtime::RhoRuntime::create_with_effect_mode(
        play,
        history,
        SortedProc::default(),
        true,
        mode,
    )
    .await
    .expect("rho runtime")
}

/// One randomized comparison of a scheduler mode against the sequential reference. Runs on its own
/// runtime and returns `(state hash, event-log length)` for each side.
fn compare_mode(program: &str, mode: EffectMode, cases: u32) -> Result<(), TestCaseError> {
    use rchain_crypto::hash::blake2b512_random::Blake2b512Random;

    // **Multi-threaded on purpose, and it is load-bearing (AUDIT C150).** The gated and the
    // validated-relaxed schedulers are compared against the sequential fold here, and on a
    // `new_current_thread` runtime that comparison *cannot fail*: an ungated set of `tokio::spawn`ed
    // tasks is still polled in spawn order, which is DFS order, so deleting the gate's
    // predecessor-await chain outright left `law21_...` green (observed). Measured with four worker
    // threads: the same deletion turns it red, 3 of 3 runs, while the runtime change alone — gate
    // intact — passes, so what the gate guarantees is real and was merely unobservable. Four rather
    // than the default because one worker still serialises; the point is two effects in flight at once.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        for _ in 0..cases {
            let rand = Blake2b512Random::from_init(&[0u8; 32]);
            let reference = runtime_in_mode(EffectMode::Sequential).await;
            let candidate = runtime_in_mode(mode).await;

            let rr = reference.evaluate(program, &rand).await.expect("reference");
            let cr = candidate.evaluate(program, &rand).await.expect("candidate");
            prop_assert!(rr.succeeded() || !rr.errors.is_empty());
            prop_assert!(
                cr.succeeded() || !cr.errors.is_empty(),
                "the mode must not fail differently: {:?} vs {:?}",
                cr.errors,
                rr.errors
            );

            let rc = reference.create_checkpoint().await.expect("checkpoint");
            let cc = candidate.create_checkpoint().await.expect("checkpoint");
            let (root, log_len) = (rc.root, rc.log.len());
            prop_assert_eq!(
                cc.root,
                root,
                "a scheduler mode must reach the sequential state for {}",
                program
            );
            prop_assert_eq!(
                cc.log.len(),
                log_len,
                "…and emit the same number of events for {}",
                program
            );
        }
        Ok(())
    })
}

/// **Law 21 (gate refines sequential).** The gate scheduler runs the effect at DFS path `i` only
/// after `0..i−1` complete, and must therefore reach the sequential reducer's state and event log —
/// on a *randomized* program, not only on the fixed corpus the integration test uses.
#[test]
fn law21_the_gate_scheduler_refines_the_sequential_reference() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = arb_program();
    for _ in 0..16 {
        // A low case count: each case builds two runtimes and evaluates twice.
        let program = strategy.new_tree(&mut runner).expect("a sample").current();
        compare_mode(&program, EffectMode::Gate, 1).expect("the gate mode refines sequential");
    }
}

/// **Law 25 (validated speculation refines sequential).** The validated-relaxed mode may commit in
/// any order *iff* each commit validates Law 24; on the block path it must publish exactly the
/// sequential fold's state. Same randomized comparison, one mode over.
#[test]
fn law25_the_validated_relaxed_scheduler_refines_sequential() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = arb_program();
    for _ in 0..16 {
        let program = strategy.new_tree(&mut runner).expect("a sample").current();
        compare_mode(&program, EffectMode::RelaxedValidated, 1)
            .expect("the validated-relaxed mode refines sequential");
    }
}

/// **Law 4 (COMM determinism).** Reducing the same program twice — fresh runtimes, same seed — must
/// reach the same state hash *and* the same event log. A COMM whose outcome depended on a hash map's
/// iteration order, a wall clock, or the effect interleaving would break this, and the state hash is
/// what consensus compares.
#[test]
fn law4_the_same_program_reduces_to_the_same_state() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = arb_program();
    for _ in 0..16 {
        let program = strategy.new_tree(&mut runner).expect("a sample").current();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        rt.block_on(async {
            let (first, second) = (
                runtime_in_mode(EffectMode::Sequential).await,
                runtime_in_mode(EffectMode::Sequential).await,
            );
            let rand =
                rchain_crypto::hash::blake2b512_random::Blake2b512Random::from_init(&[0u8; 32]);
            let r1 = first.evaluate(&program, &rand).await.expect("first");
            let r2 = second.evaluate(&program, &rand).await.expect("second");
            assert_eq!(
                r1.errors.len(),
                r2.errors.len(),
                "same errors for {program}"
            );

            let c1 = first.create_checkpoint().await.expect("checkpoint");
            let c2 = second.create_checkpoint().await.expect("checkpoint");
            assert_eq!(
                c1.root, c2.root,
                "the same program must reduce to the same state: {program}"
            );
            assert_eq!(
                c1.log.len(),
                c2.log.len(),
                "…and the same events: {program}"
            );
        });
    }
}

/// **AUDIT C151's search, recorded: the relaxed scheduler reaches the sequential fold on every shape
/// tried, certificate or not.**
///
/// C151 owes "a program whose relaxed commits diverge without validation", because
/// `law25_the_validated_relaxed_scheduler_refines_sequential` would also pass with the certificate
/// deleted — `arb_program`'s shapes are confluent under reordering, so there is nothing for the gate
/// to reject. This is the search for a program that is not, written down rather than left as a note.
///
/// Each shape below runs under `EffectMode::Relaxed` — **the relaxed scheduler with no certificate at
/// all** — and must still reach the sequential reference's state hash and event count. Seventeen
/// shapes were tried across two rounds; the ones kept are the nearest misses, including the ordering
/// law 24 exists for: a produce `o`, a consume `p` and a second produce `q` with `o < p < q` in DFS
/// order, where `q` writes `p`'s channel — the DFS-later write the certificate refuses at the *queue*
/// level (`rspace`'s `validation_fails_on_later_write_and_does_not_retry`). None diverged, at 40
/// repetitions each.
///
/// **So the missing witness is not merely unfound by `arb_program`; it does not exist for these
/// shapes, for a structural reason.** `EffectMode::Relaxed` preserves *per-channel* DFS op order in
/// the claim queue (Law 20), and the interleaving it frees is cross-channel — which changes neither
/// how many events commit nor what the space holds for any program here. The certificate's observable
/// effect is at the acquire, not on the resulting state.
///
/// Kept as a test rather than a comment because it is falsifiable in the direction that matters:
/// break per-channel ordering and these shapes diverge.
#[test]
fn c151_the_unvalidated_relaxed_scheduler_reaches_sequential_on_every_shape_tried() {
    let cases = 4;
    let shapes = [
        // o < p < q: an early consume on a channel a later effect also writes.
        ("produce consume produce", "new c, r in { c!(1) | for (x <- c) { r!(*x) } | c!(2) }"),
        ("later write to consumed chan", "new c, r, s in { c!(1) | for (x <- c) { r!(*x) } | c!(2) | for (z <- c) { s!(*z) } }"),
        // Competing consumers on one channel: only one can take the datum.
        ("competing consumes", "new a, r1, r2 in { for (x <- a) { r1!(*x) } | for (y <- a) { r2!(*y) } | a!(7) }"),
        // Joins acquire two channels, so their claims interleave across channels.
        ("join then compete", "new a, b, r1, r2 in { for (x <- a; y <- b) { r1!((*x, *y)) } | a!(1) | b!(2) | for (z <- a) { r2!(*z) } }"),
        ("two joins one datum", "new a, b, r1, r2 in { for (x <- a; y <- b) { r1!((*x, *y)) } | for (u <- a; v <- b) { r2!((*u, *v)) } | a!(1) | b!(2) }"),
        ("diamond", "new a, b, c, r in { a!(1) | for (x <- a) { b!(*x) } | for (y <- a) { c!(*y) } | for (u <- b; v <- c) { r!((*u, *v)) } }"),
        ("cross feed", "new a, b, r in { a!(1) | b!(2) | for (x <- a) { b!(*x) } | for (y <- b) { r!(*y) } }"),
        ("consume two from one chan", "new a, r in { a!(1) | a!(2) | for (x <- a) { for (y <- a) { r!((*x, *y)) } } }"),
    ];

    for (name, program) in shapes {
        compare_mode(program, EffectMode::Relaxed, cases)
            .unwrap_or_else(|e| panic!("{name}: the unvalidated relaxed scheduler diverged: {e}"));
        compare_mode(program, EffectMode::RelaxedValidated, cases)
            .unwrap_or_else(|e| panic!("{name}: the validated relaxed scheduler diverged: {e}"));
    }
}

/// **Law 57's split as a property, over the space its Lean statement quantifies over.**
///
/// `split_sums_to_the_reward` is about *every* reward, *every* operator stake and *every* list of
/// delegated principals — and the invariant it carries is the one the whole primitive rests on: the
/// delegators' shares plus the operator's remainder are the reward the key was paid, so a validator
/// with delegators commits exactly what it would have committed alone and law 46 survives the split
/// rather than needing to be restated. A unit test fixes one instance of that (the split's own
/// `the_split_is_not_the_identity`, which is a fixed triple) and cannot see the cases where the floor
/// bites differently.
///
/// Tested through the functions the Lean names — `split_reward`/`pro_rata` (`Rchain.proRata`) — rather
/// than through `close_block`, so the property is about the arithmetic and not about the boundary
/// plumbing that calls it. The plumbing has its own tests.
mod delegation_laws {
    use super::*;
    use crate::native_state::{pro_rata, split_reward};

    // A doc comment cannot sit on the `proptest!` invocation — rustdoc does not generate
    // documentation for macro invocations, and the warning is a CI failure under `-D warnings` — so
    // each property's reasoning is a plain comment above it rather than a `///`.
    proptest! {
        // **The split is exact.** The shares and the operator's remainder add up to the reward, for
        // every reward, every operator stake and every list of principals — which is stronger than law
        // 46's inequality and is what makes the split safe to sit in front of it.
        //
        // Red under a `split_reward` that floors the remainder *without* giving it back (the sum is
        // then short by the dust) or that hands the whole reward to the operator (the shares are then
        // empty and each delegator is paid nothing).
        #[test]
        fn law57_the_split_sums_to_the_reward(
            reward in 0i64..1_000_000_000,
            own in 0i64..1_000_000,
            amounts in prop::collection::vec(0i64..1_000_000, 0..6),
        ) {
            let (shares, remainder) = split_reward(reward, own, &amounts).expect("in range");
            let paid: i64 = shares.iter().sum();
            prop_assert_eq!(
                paid + remainder,
                reward,
                "shares {:?} plus remainder {} is not the reward {}",
                shares,
                remainder,
                reward
            );
            prop_assert!(remainder >= 0, "the operator's remainder cannot be negative");
            prop_assert!(
                shares.iter().all(|s| *s >= 0),
                "a share cannot be negative: {:?}",
                shares
            );
        }

        // **The delegators never take more than the reward between them** — the property
        // `pro_rata_sum_le` states, and what stops a share from being paid out of the operator's own
        // stake in the key.
        //
        // **This is also the slash's refund, and deliberately not a second property.** The refund is
        // `pro_rata(returned, offender_part, delegators)` — the same function with the base being the
        // offender's own holdings instead of the operator's stake — and `own` here is an independent
        // draw rather than a value derived from `amounts`, so the base is *already* arbitrary over the
        // same space. A third property asserting the same bound under different variable names would
        // be padding dressed as coverage, which is the failure this register keeps finding; what the
        // refund does need and has is its own **instance** (`the_refund_is_split_pro_rata` in the model,
        // and its Rust twin in `delegation_tests`), because an instance is where a wrong *shape* shows.
        #[test]
        fn law57_the_delegators_never_take_more_than_the_reward(
            reward in 0i64..1_000_000_000,
            own in 0i64..1_000_000,
            amounts in prop::collection::vec(0i64..1_000_000, 0..6),
        ) {
            let shares = pro_rata(reward, own, &amounts).expect("in range");
            let paid: i64 = shares.iter().sum();
            prop_assert!(paid <= reward, "shares {shares:?} sum to {paid}, above {reward}");
        }

        // **With no delegations the split is the identity**: no shares, and the operator's remainder
        // is the whole reward. The arithmetic half of the port's dormancy requirement; the store half
        // is `delegation_tests::the_delegation_leaves_are_absent_until_the_first_delegation`.
        //
        // Red under a split that emits a zero-valued share per delegator, and under one that hands a
        // slice of the reward to a party that is not there.
        #[test]
        fn law57_the_split_with_no_delegators_is_the_identity(reward in 0i64..1_000_000_000, own in 0i64..1_000_000) {
            let (shares, remainder) = split_reward(reward, own, &[]).expect("in range");
            prop_assert!(shares.is_empty(), "no delegators, no shares: {shares:?}");
            prop_assert_eq!(remainder, reward);
        }
    }
}
