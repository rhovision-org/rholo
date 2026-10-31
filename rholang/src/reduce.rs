//! The pure expression-evaluation surface of the reducer (Law 4).
//!
//! Mirrors `Reduce.scala` (`evalExpr` / `evalExprToExpr` / `evalSingleExpr` / `evalToBool` and the
//! arithmetic/comparison/boolean/string/method helpers), the effectful term dispatch
//! (`eval(Send/Receive/New/Match/Bundle)`, `produce`/`consume`, `new` allocation), and the
//! collection methods (`union`/`diff`/`add`/`delete`/`contains`/`slice`/`keys`).

// Only the wasm32 reducer spawns through this door; the native one has no spawn shim to route.
#[cfg(target_arch = "wasm32")]
use rchain_shared::chan;
use rchain_shared::lock::Unpoison;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use num_bigint::BigInt;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::{
    AlwaysEqual, Bundle, EList, ETuple, Expr, GPrivate, GUnforgeable, Match, Name, New, Par,
    ParMap, ParSet, Receive, ReceiveBind, Send, Sort, SortJoin, Var,
};
use rchain_models::decimal::Decimal;
use rchain_models::par_ops::{from_expr, par_concat, single_bundle, single_expr, typ};
use rchain_models::runtime::{BindPattern, ListParWithRandom, ParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::sorter::{par_map, par_set};
use rchain_rspace::concurrent::channel_queue::{
    AcquireError, ChannelClaimQueue, ClaimGuard, HeadLease,
};
use rchain_rspace::scheduled_space::ReleaseToken;

use crate::accounting::{Cost, CostAccounting, Costs};
use crate::env::Env;
use crate::errors::RholangError;
use crate::matcher::spatial_match_result;
use crate::scheduler::{DfsPath, EffectMode};
use crate::substitute::substitute_par_and_charge;

fn union_free(a: Vec<i32>, b: Vec<i32>) -> Vec<i32> {
    let mut set: BTreeSet<i32> = a.into_iter().collect();
    set.extend(b);
    set.into_iter().collect()
}

/// Split the reduction RNG for the `i`-th of `n` sibling terms. This is the sequential
/// interpreter's index-based split, preserved exactly so `new`-name freshness is unchanged.
fn split_rand(
    rand: &Blake2b512Random,
    i: usize,
    n: usize,
) -> Result<Blake2b512Random, RholangError> {
    if n == 1 {
        Ok((*rand).clone())
    } else if n > 256 {
        Ok(rand
            .split_short(u16::try_from(i).map_err(|e| RholangError::ReduceError(e.to_string()))?))
    } else {
        Ok(rand.split_byte(u8::try_from(i).map_err(|e| RholangError::ReduceError(e.to_string()))?))
    }
}

/// Recompute a `Par`'s cached `locallyFree` from its sub-terms (port of `updateLocallyFree`).
pub fn update_locally_free(par: &Par) -> Par {
    let mut free = Vec::new();
    for s in &par.sends {
        free = union_free(free, s.locally_free.0.clone());
    }
    for r in &par.receives {
        free = union_free(free, r.locally_free.0.clone());
    }
    for n in &par.news {
        free = union_free(free, n.locally_free.0.clone());
    }
    for e in &par.exprs {
        free = union_free(free, rchain_models::par_ops::locally_free_of_expr(e, 0));
    }
    for m in &par.matches {
        free = union_free(free, m.locally_free.0.clone());
    }
    for b in &par.bundles {
        free = union_free(free, b.body.locally_free.0.clone());
    }
    Par {
        locally_free: AlwaysEqual(free),
        ..par.clone()
    }
}

fn eval_var(v: &Var, env: &Env<Par>, cost: &CostAccounting) -> Result<Par, RholangError> {
    cost.charge(Costs::var_eval_cost())?;
    match v {
        Var::BoundVar(level) => env
            .get(*level)
            .ok_or_else(|| RholangError::ReduceError(format!("Unbound variable: {level}"))),
        Var::Wildcard | Var::FreeVar(_) => Err(RholangError::ReduceError(
            "Unbound variable: attempting to evaluate a pattern".to_string(),
        )),
        Var::Empty => Err(RholangError::ReduceError(
            "Impossible var instance EMPTY".to_string(),
        )),
    }
}

fn eval_to_bool(par: &Par, env: &Env<Par>, cost: &CostAccounting) -> Result<bool, RholangError> {
    let v = eval_single_expr(par, env, cost)?;
    expect_bool(&v)
}

fn eval_to_long(par: &Par, env: &Env<Par>, cost: &CostAccounting) -> Result<i64, RholangError> {
    match eval_single_expr(par, env, cost)? {
        Expr::GInt(v) => Ok(v),
        // RCHIP #51: a `BigInt` that fits in `i64` is a valid index/count.
        Expr::GBigInt(v) => v
            .to_string()
            .parse::<i64>()
            .map_err(|_| RholangError::ReduceError(format!("Error: value out of range: {v}"))),
        other => Err(RholangError::ReduceError(format!(
            "Error: expected Int, got {}",
            typ(&other)
        ))),
    }
}

fn restrict_to_int(n: i64) -> Result<usize, RholangError> {
    if n > i32::MAX as i64 || n < i32::MIN as i64 {
        Err(RholangError::ReduceError(format!(
            "Error: value out of range: {n}"
        )))
    } else {
        Ok(n as usize)
    }
}

pub fn eval_single_expr<S: Sort>(
    par: &Par<S>,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    match single_expr(par) {
        Some(expr) => eval_expr_to_expr(expr, env, cost),
        None => Err(RholangError::ReduceError(
            "Expected a single expression".to_string(),
        )),
    }
}

/// The integer value of an `Int`/`BigInt` expression (RCHIP #51: one integer domain).
fn as_integer(e: &Expr) -> Option<BigInt> {
    match e {
        Expr::GInt(v) => Some(BigInt::from(*v)),
        Expr::GBigInt(v) => Some(v.clone()),
        _ => None,
    }
}

fn is_zero(e: &Expr) -> bool {
    match e {
        Expr::GInt(v) => *v == 0,
        Expr::GBigInt(v) => *v == BigInt::from(0),
        _ => false,
    }
}

/// The numeric value of an `Int`, `BigInt` or `Decimal`, exact. `None` for any other expression.
fn as_decimal(e: &Expr) -> Option<Decimal> {
    match e {
        Expr::GInt(v) => Decimal::new(BigInt::from(*v), 0).ok(),
        Expr::GBigInt(v) => Decimal::new(v.clone(), 0).ok(),
        Expr::GDecimal(d) => Some(d.clone()),
        _ => None,
    }
}

fn has_decimal(a: &Expr, b: &Expr) -> bool {
    matches!(a, Expr::GDecimal(_)) || matches!(b, Expr::GDecimal(_))
}

/// Numeric equality spanning `Int`/`BigInt`/`Decimal` (so `2 == 2n == 2d`); `None` when not both
/// are numeric, which falls back to structural equality in `EEq`/`ENeq`.
fn numeric_eq(a: &Par, b: &Par) -> Option<bool> {
    let x = as_decimal(single_expr(a)?)?;
    let y = as_decimal(single_expr(b)?)?;
    Some(x.cmp_numeric(&y) == CmpOrdering::Equal)
}

/// The operations `decimal_binop` implements. An enum rather than a string, so the dispatch is
/// exhaustive: the type-system gate forbids a partial arm (`unreachable!`) in production code.
#[derive(Clone, Copy)]
enum DecimalOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl DecimalOp {
    fn symbol(self) -> &'static str {
        match self {
            DecimalOp::Add => "+",
            DecimalOp::Sub => "-",
            DecimalOp::Mul => "*",
            DecimalOp::Div => "/",
        }
    }
}

/// The decimal path for `+ - * /`: mixed `Int`/`BigInt`/`Decimal` operands promote to a decimal
/// result, and an integral result stays a decimal (`1.5d - 0.5d` is `1d`, not `Int(1)`).
fn decimal_binop(
    op: DecimalOp,
    v1: &Expr,
    v2: &Expr,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    let (Some(l), Some(r)) = (as_decimal(v1), as_decimal(v2)) else {
        return Err(if as_decimal(v1).is_some() {
            RholangError::OperatorExpectedError {
                op: op.symbol().to_string(),
                expected: "Decimal".to_string(),
                other_type: typ(v2).to_string(),
            }
        } else {
            RholangError::OperatorNotDefined {
                op: op.symbol().to_string(),
                other_type: typ(v1).to_string(),
            }
        });
    };
    let result = match op {
        DecimalOp::Add => {
            cost.charge(Costs::decimal_sum(&l, &r))?;
            l.add(&r)
        }
        DecimalOp::Sub => {
            cost.charge(Costs::decimal_subtraction(&l, &r))?;
            l.sub(&r)
        }
        DecimalOp::Mul => {
            cost.charge(Costs::decimal_multiplication(&l, &r))?;
            l.mul(&r)
        }
        DecimalOp::Div => {
            if r.is_zero() {
                return Err(RholangError::ReduceError("/ by zero".to_string()));
            }
            cost.charge(Costs::decimal_division(&l, &r))?;
            l.div(&r)
        }
    };
    result
        .map(Expr::GDecimal)
        .map_err(|e| RholangError::ReduceError(e.to_string()))
}

/// A binary integer operation: an `i64` fast path (`checked`) that **never wraps** — on overflow it
/// promotes both operands and computes exactly in `BigInt` (RCHIP #51). Mixed `Int`/`BigInt`
/// operands promote too. `big` must never be called with a zero divisor.
fn int_binop(
    op: &str,
    v1: &Expr,
    v2: &Expr,
    cost: &CostAccounting,
    checked: fn(i64, i64) -> Option<i64>,
    big: fn(&BigInt, &BigInt) -> Result<BigInt, RholangError>,
    int_cost: Cost,
    big_cost: fn(&BigInt, &BigInt) -> Cost,
) -> Result<Expr, RholangError> {
    match (v1, v2) {
        (Expr::GInt(l), Expr::GInt(r)) => match checked(*l, *r) {
            Some(value) => {
                cost.charge(int_cost)?;
                Ok(Expr::GInt(value))
            }
            None => int_bigop(&BigInt::from(*l), &BigInt::from(*r), cost, big, big_cost),
        },
        (Expr::GInt(l), Expr::GBigInt(r)) => int_bigop(&BigInt::from(*l), r, cost, big, big_cost),
        (Expr::GBigInt(l), Expr::GInt(r)) => int_bigop(l, &BigInt::from(*r), cost, big, big_cost),
        (Expr::GBigInt(l), Expr::GBigInt(r)) => int_bigop(l, r, cost, big, big_cost),
        (Expr::GInt(_) | Expr::GBigInt(_), o) => Err(RholangError::OperatorExpectedError {
            op: op.to_string(),
            expected: "Int".to_string(),
            other_type: typ(o).to_string(),
        }),
        (o, _) => Err(RholangError::OperatorNotDefined {
            op: op.to_string(),
            other_type: typ(o).to_string(),
        }),
    }
}

fn int_bigop(
    l: &BigInt,
    r: &BigInt,
    cost: &CostAccounting,
    big: fn(&BigInt, &BigInt) -> Result<BigInt, RholangError>,
    big_cost: fn(&BigInt, &BigInt) -> Cost,
) -> Result<Expr, RholangError> {
    cost.charge(big_cost(l, r))?;
    Ok(Expr::GBigInt(big(l, r)?))
}

/// `set + x` — insert `x` into the set, re-canonicalised. This is the `add` **method**'s body,
/// shared with the `+` operator exactly as Scala shares them (`EPlusBody`'s
/// `case (lhs: ESetBody, rhs) => add(lhs, List[Par](rhs))` calls the same `add` the `.add(x)`
/// method does), so the two cannot drift apart.
///
/// **The charge is the set it rebuilds, not one addition** (C353). It clones every element and then
/// re-canonicalises the whole set through `par_set`, so the work is the output's canonicalisation —
/// the same shape `set` charges on a map (C345). The flat `add_cost` this used to charge let a fold
/// that inserts N elements buy `O(N² log N)` work for `O(N)` phlo: one member of the class the
/// October re-audit closed, in the arm the arm-by-arm reading did not reach, and found by the cost
/// register's first census.
fn set_add(b: &ParSet, element: Par, cost: &CostAccounting) -> Result<Expr, RholangError> {
    cost.charge(Costs::canonicalization_cost(
        (b.ps.len() as i64).saturating_add(1),
    ))?;
    let element_conn = element.connective_used;
    let element_lf = element.locally_free.0.clone();
    let mut ps = b.ps.clone();
    ps.push(element);
    let mut s = par_set(ps);
    s.connective_used = b.connective_used || element_conn;
    s.locally_free = AlwaysEqual(union_free(b.locally_free.0.clone(), element_lf));
    s.remainder = None;
    Ok(Expr::ESet(s))
}

/// `collection - x` — `delete` for sets and maps (Scala's `EMinusBody` has an arm for each, both
/// calling `delete`), shared with the `.delete(x)` method for the same reason as [`set_add`].
fn collection_delete(
    base: &Expr,
    element: &Par,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    match base {
        Expr::ESet(b) => {
            cost.charge(Costs::remove_cost().mul(b.ps.len() as i64))?;
            let ps: Vec<Par> = b.ps.iter().filter(|p| *p != element).cloned().collect();
            Ok(Expr::ESet(par_set(ps)))
        }
        Expr::EMap(b) => {
            cost.charge(Costs::remove_cost().mul(b.kvs.len() as i64))?;
            let kvs: Vec<(Par, Par)> = b
                .kvs
                .iter()
                .filter(|(k, _)| k != element)
                .cloned()
                .collect();
            Ok(Expr::EMap(par_map(kvs)))
        }
        other => Err(method_not_defined("delete", other)),
    }
}

fn relop(
    p1: &Par,
    p2: &Par,
    relopb: fn(bool, bool) -> bool,
    relopi: fn(i64, i64) -> bool,
    relopbi: fn(&BigInt, &BigInt) -> bool,
    relops: fn(&str, &str) -> bool,
    relopd: fn(CmpOrdering) -> bool,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    let v1 = eval_single_expr(p1, env, cost)?;
    let v2 = eval_single_expr(p2, env, cost)?;
    Ok(match (&v1, &v2) {
        (Expr::GBool(b1), Expr::GBool(b2)) => {
            cost.charge(Costs::comparison_cost())?;
            Expr::GBool(relopb(*b1, *b2))
        }
        (Expr::GInt(i1), Expr::GInt(i2)) => {
            cost.charge(Costs::comparison_cost())?;
            Expr::GBool(relopi(*i1, *i2))
        }
        (Expr::GBigInt(b1), Expr::GBigInt(b2)) => {
            cost.charge(Costs::big_int_comparison(b1, b2))?;
            Expr::GBool(relopbi(b1, b2))
        }
        // Mixed `Int`/`BigInt` comparisons promote (RCHIP #51).
        (Expr::GInt(i1), Expr::GBigInt(b2)) => {
            let b1 = BigInt::from(*i1);
            cost.charge(Costs::big_int_comparison(&b1, b2))?;
            Expr::GBool(relopbi(&b1, b2))
        }
        (Expr::GBigInt(b1), Expr::GInt(i2)) => {
            let b2 = BigInt::from(*i2);
            cost.charge(Costs::big_int_comparison(b1, &b2))?;
            Expr::GBool(relopbi(b1, &b2))
        }
        (Expr::GString(s1), Expr::GString(s2)) => {
            cost.charge(Costs::comparison_cost())?;
            Expr::GBool(relops(s1, s2))
        }
        // A decimal compares numerically against any numeric operand, exactly (issue #380).
        (Expr::GDecimal(_), _) | (_, Expr::GDecimal(_)) => {
            let (Some(l), Some(r)) = (as_decimal(&v1), as_decimal(&v2)) else {
                return Err(RholangError::ReduceError(format!(
                    "Unexpected compare: {v1:?} vs. {v2:?}"
                )));
            };
            cost.charge(Costs::decimal_comparison(&l, &r))?;
            Expr::GBool(relopd(l.cmp_numeric(&r)))
        }
        _ => {
            return Err(RholangError::ReduceError(format!(
                "Unexpected compare: {v1:?} vs. {v2:?}"
            )))
        }
    })
}

fn eval_to_string_pair(key: &Expr, value: &Expr) -> Result<(String, String), RholangError> {
    match (key, value) {
        (Expr::GString(k), Expr::GString(v)) => Ok((k.clone(), v.clone())),
        (Expr::GString(k), Expr::GInt(v)) => Ok((k.clone(), v.to_string())),
        (Expr::GString(k), Expr::GBigInt(v)) => Ok((k.clone(), v.to_string())),
        (Expr::GString(k), Expr::GDecimal(v)) => Ok((k.clone(), v.to_plain_string())),
        (Expr::GString(k), Expr::GBool(v)) => Ok((k.clone(), v.to_string())),
        (Expr::GString(k), Expr::GUri(v)) => Ok((k.clone(), v.clone())),
        (Expr::GString(_), value) => Err(RholangError::ReduceError(format!(
            "Error: interpolation doesn't support {}",
            typ(value)
        ))),
        _ => Err(RholangError::ReduceError(
            "Error: interpolation Map should only contain String keys".to_string(),
        )),
    }
}

fn interpolate(string: &str, pairs: &[(String, String)]) -> String {
    let mut result = String::new();
    let mut current = string;
    while !current.is_empty() {
        match pairs
            .iter()
            .find(|(k, _)| current.starts_with(&format!("${{{k}}}")))
        {
            Some((k, v)) => {
                result.push_str(v);
                current = &current[k.len() + 3..];
            }
            None => {
                let mut chars = current.chars();
                let c = match chars.next() {
                    Some(c) => c,
                    // Unreachable: `current` is non-empty by the loop guard.
                    None => break,
                };
                result.push(c);
                current = chars.as_str();
            }
        }
    }
    result
}

fn eval_expr_to_par<S: Sort>(
    expr: &Expr,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Par<S>, RholangError> {
    match expr {
        Expr::EVar(v) => {
            let p = eval_var(v, env, cost)?;
            eval_expr(&p, env, cost).map(|r| r.re_sort())
        }
        Expr::EMethod(em) => {
            cost.charge(Costs::method_call_cost())?;
            let evaled_target = eval_expr(&em.target, env, cost)?;
            let evaled_args: Vec<Par> = em
                .arguments
                .iter()
                .map(|a| eval_expr(a, env, cost))
                .collect::<Result<_, _>>()?;
            eval_method(&em.method_name, &evaled_target, &evaled_args, env, cost)
                .map(|r| r.re_sort())
        }
        _ => Ok(from_expr(eval_expr_to_expr(expr, env, cost)?).re_sort()),
    }
}

/// The left and right operands of a binary operator node, for the ops whose **chains** are not bounded
/// by the parser's nesting guard — the ones `split_left_spine` folds.
///
/// `None` for everything else: the unary ops (`not`, unary `-`) nest only through parentheses or
/// repeated keywords, both of which `MAX_PARSE_DEPTH` bounds at 128, and the non-operator shapes
/// (collections, `EMethod`, `EVar`, `==`, `matches`) are bounded by the parser's guard and by C100's
/// value bound. **A left-associative chain is the one shape whose depth is not a nesting depth**:
/// `1 + 1 + … + 1` is a single nesting level with a spine as long as the chain, which is AUDIT C101.
fn binary_operands(e: &Expr) -> Option<(&Par, &Par)> {
    match e {
        Expr::EMult(a, b)
        | Expr::EDiv(a, b)
        | Expr::EMod(a, b)
        | Expr::EPlus(a, b)
        | Expr::EMinus(a, b)
        | Expr::ELt(a, b)
        | Expr::ELte(a, b)
        | Expr::EGt(a, b)
        | Expr::EGte(a, b)
        | Expr::EAnd(a, b)
        | Expr::EOr(a, b)
        | Expr::EShortAnd(a, b)
        | Expr::EShortOr(a, b)
        | Expr::EPercentPercent(a, b)
        | Expr::EPlusPlus(a, b)
        | Expr::EMinusMinus(a, b) => Some((a, b)),
        _ => None,
    }
}

/// The bottom of a left spine: whatever the chain's first operand turned out to be.
enum SpineBase<'a> {
    /// A single expression — handed to `eval_arm` directly.
    Expr(&'a Expr),
    /// A `Par` that is not one expression — handed to `eval_single_expr`, which produces the very error
    /// the arm would have ("Expected a single expression"), so the two paths cannot diverge on it.
    Par(&'a Par),
}

/// A left-nested chain of binary operators, innermost operator first, with the operand beneath it.
///
/// Walks the *syntax* — no evaluation, so no stack — which is the whole point: the chain's length is
/// not a nesting depth the parser can bound, so it must not become a recursion depth either.
fn split_left_spine(expr: &Expr) -> Option<(Vec<(&Expr, &Par)>, SpineBase<'_>)> {
    let mut chain: Vec<(&Expr, &Par)> = Vec::new();
    let mut cur = expr;
    loop {
        let Some((left, right)) = binary_operands(cur) else {
            break;
        };
        chain.push((cur, right));
        match single_expr(left) {
            Some(next) => cur = next,
            None => {
                // The left operand is not a single expression, so the chain stops *at* this operator
                // and the operand beneath it is the `Par` itself.
                chain.reverse();
                return Some((chain, SpineBase::Par(left)));
            }
        }
    }
    if chain.is_empty() {
        return None;
    }
    chain.reverse();
    Some((chain, SpineBase::Expr(cur)))
}

/// `a && b` / `a || b`, with the right operand as a **thunk**: it must not be evaluated — and so must
/// not be charged, and must not be able to raise — when the left operand decides the result. One
/// implementation, called by the arm and by the spine walk, so the two cannot drift.
fn bool_short_circuit(
    kind: ShortKind,
    left: bool,
    right: impl FnOnce() -> Result<bool, RholangError>,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    let decided = match kind {
        ShortKind::And => !left,
        ShortKind::Or => left,
    };
    let b2 = match (kind, decided) {
        (ShortKind::And, true) => false,
        (ShortKind::Or, true) => true,
        (_, false) => right()?,
    };
    cost.charge(match kind {
        ShortKind::And => Costs::boolean_and_cost(),
        ShortKind::Or => Costs::boolean_or_cost(),
    })?;
    Ok(Expr::GBool(match kind {
        ShortKind::And => left && b2,
        ShortKind::Or => left || b2,
    }))
}

#[derive(Clone, Copy)]
enum ShortKind {
    And,
    Or,
}

/// The `Bool` an already-evaluated value has to be — the tail of `eval_to_bool`, shared with it so the
/// message cannot differ between the recursive path and the spine's.
fn expect_bool(v: &Expr) -> Result<bool, RholangError> {
    match v {
        Expr::GBool(b) => Ok(*b),
        other => Err(RholangError::ReduceError(format!(
            "Error: expected Bool, got {}",
            typ(other)
        ))),
    }
}

/// One spine level: apply `op` to an already-evaluated left value and the operator's right operand.
///
/// For every op but the short-circuiting pair this **rebuilds a one-level node and runs the same arm**
/// (`eval_arm`) the recursive path would have run. That is deliberate: the operator's cost, its
/// dispatch (`Set + x`, the `++` unions, `%%` interpolation) and its errors stay in one place instead
/// of being transcribed. It is also *gas-exact* — an arm's result is a value, and evaluating a value
/// charges nothing (`eval_expr_to_par`'s fallthrough is the leaf arm, a clone) — so a chain's cost is
/// the same as before to the charge.
fn apply_spine_level(
    op: &Expr,
    acc: Expr,
    right: &Par,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    match op {
        Expr::EShortAnd(..) => {
            let b1 = expect_bool(&acc)?;
            bool_short_circuit(ShortKind::And, b1, || eval_to_bool(right, env, cost), cost)
        }
        Expr::EShortOr(..) => {
            let b1 = expect_bool(&acc)?;
            bool_short_circuit(ShortKind::Or, b1, || eval_to_bool(right, env, cost), cost)
        }
        _ => eval_arm(&rebuild_binary(op, acc, right)?, env, cost),
    }
}

/// A one-level copy of `op` with `acc` as its left operand: the shape `eval_arm` expects.
fn rebuild_binary(op: &Expr, acc: Expr, right: &Par) -> Result<Expr, RholangError> {
    let l = Box::new(from_expr(acc));
    let r = Box::new(right.clone());
    Ok(match op {
        Expr::EMult(..) => Expr::EMult(l, r),
        Expr::EDiv(..) => Expr::EDiv(l, r),
        Expr::EMod(..) => Expr::EMod(l, r),
        Expr::EPlus(..) => Expr::EPlus(l, r),
        Expr::EMinus(..) => Expr::EMinus(l, r),
        Expr::ELt(..) => Expr::ELt(l, r),
        Expr::ELte(..) => Expr::ELte(l, r),
        Expr::EGt(..) => Expr::EGt(l, r),
        Expr::EGte(..) => Expr::EGte(l, r),
        Expr::EEq(..) => Expr::EEq(l, r),
        Expr::ENeq(..) => Expr::ENeq(l, r),
        Expr::EAnd(..) => Expr::EAnd(l, r),
        Expr::EOr(..) => Expr::EOr(l, r),
        Expr::EPercentPercent(..) => Expr::EPercentPercent(l, r),
        Expr::EPlusPlus(..) => Expr::EPlusPlus(l, r),
        Expr::EMinusMinus(..) => Expr::EMinusMinus(l, r),
        other => {
            return Err(RholangError::BugFoundError(format!(
                "a spine level was rebuilt for a non-binary operator: {other:?}"
            )))
        }
    })
}

/// Evaluate an expression to its value.
///
/// **A left-nested operator chain is evaluated iteratively** (AUDIT C101). `1 + 1 + … + 1` is one
/// parser nesting level with an arbitrarily long spine, and the recursive reading of it cost one Rust
/// frame per link: a legal ~350-link chain as a send's datum **aborted the process** on the node's
/// 32 MiB worker (`thread 'tokio-rt-worker' has overflowed its stack`), inside this function's
/// recursion, before C100's guard could see the datum. The spine is walked here and its levels are
/// applied in a loop, so the chain's length costs no stack — and its cost in *gas* is unchanged,
/// because each level still runs the arm (see `apply_spine_level`).
fn eval_expr_to_expr(
    expr: &Expr,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Expr, RholangError> {
    let Some((chain, base)) = split_left_spine(expr) else {
        return eval_arm(expr, env, cost);
    };
    let mut acc = match base {
        SpineBase::Expr(e) => eval_arm(e, env, cost)?,
        SpineBase::Par(p) => eval_single_expr(p, env, cost)?,
    };
    for (op, right) in chain {
        acc = apply_spine_level(op, acc, right, env, cost)?;
    }
    Ok(acc)
}

/// One expression, the recursive way — every arm as it was, reached for a shape the spine walk does not
/// fold (a leaf, a collection, a method call, `==`, `matches`, a unary operator).
fn eval_arm(expr: &Expr, env: &Env<Par>, cost: &CostAccounting) -> Result<Expr, RholangError> {
    match expr {
        Expr::GBool(_)
        | Expr::GInt(_)
        | Expr::GBigInt(_)
        | Expr::GDecimal(_)
        | Expr::GString(_)
        | Expr::GUri(_)
        | Expr::GByteArray(_) => Ok(expr.clone()),
        Expr::ENot(p) => Ok(Expr::GBool(!eval_to_bool(p, env, cost)?)),
        Expr::ENeg(p) => {
            let v = eval_single_expr(p, env, cost)?;
            match v {
                // `-i64::MIN` overflows `i64`: promote instead of wrapping (RCHIP #51).
                Expr::GInt(hs) => match hs.checked_neg() {
                    Some(value) => Ok(Expr::GInt(value)),
                    None => {
                        let r = -BigInt::from(hs);
                        cost.charge(Costs::big_int_negation(&r))?;
                        Ok(Expr::GBigInt(r))
                    }
                },
                Expr::GBigInt(hs) => {
                    let r = -hs;
                    cost.charge(Costs::big_int_negation(&r))?;
                    Ok(Expr::GBigInt(r))
                }
                Expr::GDecimal(d) => {
                    cost.charge(Costs::decimal_negation(&d))?;
                    Ok(Expr::GDecimal(d.neg()))
                }
                other => Err(RholangError::OperatorNotDefined {
                    op: "Negation".to_string(),
                    other_type: typ(&other).to_string(),
                }),
            }
        }
        Expr::EMult(p1, p2) => {
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            if has_decimal(&v1, &v2) {
                return decimal_binop(DecimalOp::Mul, &v1, &v2, cost);
            }
            int_binop(
                "*",
                &v1,
                &v2,
                cost,
                i64::checked_mul,
                |l, r| Ok(l * r),
                Costs::multiplication_cost(),
                Costs::big_int_multiplication,
            )
        }
        Expr::EDiv(p1, p2) => {
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            if has_decimal(&v1, &v2) {
                return decimal_binop(DecimalOp::Div, &v1, &v2, cost);
            }
            if is_zero(&v2) {
                return Err(RholangError::ReduceError("/ by zero".to_string()));
            }
            int_binop(
                "/",
                &v1,
                &v2,
                cost,
                i64::checked_div,
                |l, r| Ok(l / r),
                Costs::division_cost(),
                Costs::big_int_division,
            )
        }
        Expr::EMod(p1, p2) => {
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            // `%` is not defined on decimals, so the refusal comes before the zero-divisor guard:
            // `1 % 0.0d` is a type error, not `/ by zero`.
            if has_decimal(&v1, &v2) {
                return Err(if matches!(v1, Expr::GDecimal(_)) {
                    RholangError::OperatorNotDefined {
                        op: "%".to_string(),
                        other_type: typ(&v1).to_string(),
                    }
                } else {
                    RholangError::OperatorExpectedError {
                        op: "%".to_string(),
                        expected: "Int".to_string(),
                        other_type: typ(&v2).to_string(),
                    }
                });
            }
            if is_zero(&v2) {
                return Err(RholangError::ReduceError("/ by zero".to_string()));
            }
            int_binop(
                "%",
                &v1,
                &v2,
                cost,
                i64::checked_rem,
                |l, r| Ok(l % r),
                Costs::modulo_cost(),
                Costs::big_int_modulo,
            )
        }
        Expr::EPlus(p1, p2) => {
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            // `Set + x` inserts (Scala: `case (lhs: ESetBody, rhs) => add(lhs, …)`), charged
            // `OP_CALL_COST` for the dispatch before `add`'s own cost. The arm was missing, so
            // `Set(1) + 2` — valid rholang, and what `convenience_methods_test.rho` exercises —
            // errored as `OperatorNotDefined`.
            if let Expr::ESet(b) = &v1 {
                cost.charge(Costs::op_call_cost())?;
                return set_add(b, from_expr(v2), cost);
            }
            if has_decimal(&v1, &v2) {
                return decimal_binop(DecimalOp::Add, &v1, &v2, cost);
            }
            int_binop(
                "+",
                &v1,
                &v2,
                cost,
                i64::checked_add,
                |l, r| Ok(l + r),
                Costs::sum_cost(),
                Costs::big_int_sum,
            )
        }
        Expr::EMinus(p1, p2) => {
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            // `Set - x` / `Map - x` remove (Scala's `EMinusBody` calls `delete` for each).
            if matches!(v1, Expr::ESet(_) | Expr::EMap(_)) {
                cost.charge(Costs::op_call_cost())?;
                return collection_delete(&v1, &from_expr(v2), cost);
            }
            if has_decimal(&v1, &v2) {
                return decimal_binop(DecimalOp::Sub, &v1, &v2, cost);
            }
            int_binop(
                "-",
                &v1,
                &v2,
                cost,
                i64::checked_sub,
                |l, r| Ok(l - r),
                Costs::subtraction_cost(),
                Costs::big_int_subtraction,
            )
        }
        Expr::ELt(p1, p2) => relop(
            p1,
            p2,
            |a, b| a < b,
            |a, b| a < b,
            |a, b| a < b,
            |a, b| a < b,
            |o| o == CmpOrdering::Less,
            env,
            cost,
        ),
        Expr::ELte(p1, p2) => relop(
            p1,
            p2,
            |a, b| a <= b,
            |a, b| a <= b,
            |a, b| a <= b,
            |a, b| a <= b,
            |o| o != CmpOrdering::Greater,
            env,
            cost,
        ),
        Expr::EGt(p1, p2) => relop(
            p1,
            p2,
            |a, b| a > b,
            |a, b| a > b,
            |a, b| a > b,
            |a, b| a > b,
            |o| o == CmpOrdering::Greater,
            env,
            cost,
        ),
        Expr::EGte(p1, p2) => relop(
            p1,
            p2,
            |a, b| a >= b,
            |a, b| a >= b,
            |a, b| a >= b,
            |a, b| a >= b,
            |o| o != CmpOrdering::Less,
            env,
            cost,
        ),
        Expr::EEq(p1, p2) => {
            let v1 = eval_expr(p1, env, cost)?;
            let v2 = eval_expr(p2, env, cost)?;
            let sv1 = substitute_par_and_charge(&v1, 0, env, cost)?;
            let sv2 = substitute_par_and_charge(&v2, 0, env, cost)?;
            cost.charge(Costs::equality_check_cost(&sv1, &sv2))?;
            // RCHIP #51: `Int`/`BigInt` with the same value are equal (`2 == 2n`).
            let equal = match numeric_eq(&sv1, &sv2) {
                Some(equal) => equal,
                None => sv1 == sv2,
            };
            Ok(Expr::GBool(equal))
        }
        Expr::ENeq(p1, p2) => {
            let v1 = eval_expr(p1, env, cost)?;
            let v2 = eval_expr(p2, env, cost)?;
            let sv1 = substitute_par_and_charge(&v1, 0, env, cost)?;
            let sv2 = substitute_par_and_charge(&v2, 0, env, cost)?;
            cost.charge(Costs::equality_check_cost(&sv1, &sv2))?;
            let equal = match numeric_eq(&sv1, &sv2) {
                Some(equal) => equal,
                None => sv1 == sv2,
            };
            Ok(Expr::GBool(!equal))
        }
        Expr::EAnd(p1, p2) => {
            let b1 = eval_to_bool(p1, env, cost)?;
            let b2 = eval_to_bool(p2, env, cost)?;
            cost.charge(Costs::boolean_and_cost())?;
            Ok(Expr::GBool(b1 && b2))
        }
        Expr::EOr(p1, p2) => {
            let b1 = eval_to_bool(p1, env, cost)?;
            let b2 = eval_to_bool(p2, env, cost)?;
            cost.charge(Costs::boolean_or_cost())?;
            Ok(Expr::GBool(b1 || b2))
        }
        Expr::EShortAnd(p1, p2) => {
            let b1 = eval_to_bool(p1, env, cost)?;
            // Shared with the spine walk (`apply_spine_level`), so the short-circuit rule — and the
            // fact that the right operand is neither charged nor able to raise when the left decides —
            // lives in exactly one place.
            bool_short_circuit(ShortKind::And, b1, || eval_to_bool(p2, env, cost), cost)
        }
        Expr::EShortOr(p1, p2) => {
            let b1 = eval_to_bool(p1, env, cost)?;
            bool_short_circuit(ShortKind::Or, b1, || eval_to_bool(p2, env, cost), cost)
        }
        Expr::EMatches(target, pattern) => {
            let evaled_target = eval_expr(target, env, cost)?;
            let subst_target = substitute_par_and_charge(&evaled_target, 0, env, cost)?;
            let subst_pattern = substitute_par_and_charge(pattern, 1, env, cost)?;
            let m = spatial_match_result(&subst_target, &subst_pattern)?;
            Ok(Expr::GBool(m.is_some()))
        }
        Expr::EPercentPercent(p1, p2) => {
            cost.charge(Costs::op_call_cost())?;
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            match (&v1, &v2) {
                (Expr::GString(lhs), Expr::EMap(ParMap { kvs, .. })) => {
                    if lhs.is_empty() && kvs.is_empty() {
                        Ok(Expr::GString(lhs.clone()))
                    } else {
                        let mut pairs = Vec::new();
                        for (k, v) in kvs {
                            let key_expr = eval_single_expr(k, env, cost)?;
                            let value_expr = eval_single_expr(v, env, cost)?;
                            pairs.push(eval_to_string_pair(&key_expr, &value_expr)?);
                        }
                        cost.charge(Costs::interpolate_cost(lhs.len() as i64, kvs.len() as i64))?;
                        Ok(Expr::GString(interpolate(lhs, &pairs)))
                    }
                }
                (Expr::GString(_), o) => Err(RholangError::OperatorExpectedError {
                    op: "%%".to_string(),
                    expected: "Map".to_string(),
                    other_type: typ(o).to_string(),
                }),
                (o, _) => Err(RholangError::OperatorNotDefined {
                    op: "%%".to_string(),
                    other_type: typ(o).to_string(),
                }),
            }
        }
        Expr::EPlusPlus(p1, p2) => {
            cost.charge(Costs::op_call_cost())?;
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            match (&v1, &v2) {
                (Expr::GString(l), Expr::GString(r)) => {
                    cost.charge(Costs::string_append_cost(l.len() as i64, r.len() as i64))?;
                    Ok(Expr::GString(format!("{l}{r}")))
                }
                (Expr::GByteArray(l), Expr::GByteArray(r)) => {
                    cost.charge(Costs::string_append_cost(l.len() as i64, r.len() as i64))?;
                    let mut out = l.clone();
                    out.extend(r);
                    Ok(Expr::GByteArray(out))
                }
                (Expr::EList(l), Expr::EList(r)) => {
                    cost.charge(Costs::list_append_cost((l.ps.len() + r.ps.len()) as i64))?;
                    let mut ps = l.ps.clone();
                    ps.extend(r.ps.clone());
                    Ok(Expr::EList(EList {
                        ps,
                        locally_free: AlwaysEqual(union_free(
                            l.locally_free.0.clone(),
                            r.locally_free.0.clone(),
                        )),
                        connective_used: l.connective_used || r.connective_used,
                        ..Default::default()
                    }))
                }
                (Expr::ESet(l), Expr::ESet(r)) => {
                    // Set union (`ParSet.union`): both operands are canonicalised into the result, so
                    // both are charged — the old charge was the right operand only, which made
                    // `bigSet ++ {}` free while it still rebuilt `bigSet` (C345).
                    cost.charge(Costs::union_cost(l.ps.len() as i64, r.ps.len() as i64))?;
                    let mut ps = l.ps.clone();
                    ps.extend(r.ps.clone());
                    let mut s = par_set(ps);
                    s.connective_used = l.connective_used || r.connective_used;
                    s.locally_free = AlwaysEqual(union_free(
                        l.locally_free.0.clone(),
                        r.locally_free.0.clone(),
                    ));
                    s.remainder = None;
                    Ok(Expr::ESet(s))
                }
                (Expr::EMap(l), Expr::EMap(r)) => {
                    // Map union (`baseMap ++ otherMap`, right-biased: the other map's value wins on
                    // a colliding key — which is what `par_map`'s canonicalisation gives, since it
                    // keeps the last of each key).
                    cost.charge(Costs::union_cost(l.kvs.len() as i64, r.kvs.len() as i64))?;
                    let mut kvs = l.kvs.clone();
                    kvs.extend(r.kvs.clone());
                    let mut m = par_map(kvs);
                    m.connective_used = l.connective_used || r.connective_used;
                    m.locally_free = AlwaysEqual(union_free(
                        l.locally_free.0.clone(),
                        r.locally_free.0.clone(),
                    ));
                    m.remainder = None;
                    Ok(Expr::EMap(m))
                }
                (Expr::GString(_), o) => Err(RholangError::OperatorExpectedError {
                    op: "++".to_string(),
                    expected: "String".to_string(),
                    other_type: typ(o).to_string(),
                }),
                (Expr::EList(_), o) => Err(RholangError::OperatorExpectedError {
                    op: "++".to_string(),
                    expected: "List".to_string(),
                    other_type: typ(o).to_string(),
                }),
                (Expr::EMap(_), o) => Err(RholangError::OperatorExpectedError {
                    op: "++".to_string(),
                    expected: "Map".to_string(),
                    other_type: typ(o).to_string(),
                }),
                (Expr::ESet(_), o) => Err(RholangError::OperatorExpectedError {
                    op: "++".to_string(),
                    expected: "Set".to_string(),
                    other_type: typ(o).to_string(),
                }),
                (o, _) => Err(RholangError::OperatorNotDefined {
                    op: "++".to_string(),
                    other_type: typ(o).to_string(),
                }),
            }
        }
        Expr::EMinusMinus(p1, p2) => {
            cost.charge(Costs::op_call_cost())?;
            let v1 = eval_single_expr(p1, env, cost)?;
            let v2 = eval_single_expr(p2, env, cost)?;
            match (&v1, &v2) {
                (Expr::ESet(b), Expr::ESet(o)) => {
                    cost.charge(Costs::diff_cost(b.ps.len() as i64, o.ps.len() as i64))?;
                    let ps: Vec<Par> = b.ps.iter().filter(|p| !o.ps.contains(p)).cloned().collect();
                    Ok(Expr::ESet(par_set(ps)))
                }
                (Expr::ESet(_), o) => Err(RholangError::OperatorExpectedError {
                    op: "--".to_string(),
                    expected: "Set".to_string(),
                    other_type: typ(o).to_string(),
                }),
                (o, _) => Err(RholangError::OperatorNotDefined {
                    op: "--".to_string(),
                    other_type: typ(o).to_string(),
                }),
            }
        }
        Expr::EVar(v) => {
            let p = eval_var(v, env, cost)?;
            eval_single_expr(&p, env, cost)
        }
        Expr::EList(el) => {
            let evaled: Vec<Par> = el
                .ps
                .iter()
                .map(|p| eval_expr(p, env, cost).map(|p| update_locally_free(&p)))
                .collect::<Result<_, _>>()?;
            Ok(Expr::EList(EList {
                ps: evaled,
                locally_free: el.locally_free.clone(),
                connective_used: el.connective_used,
                ..Default::default()
            }))
        }
        Expr::ETuple(el) => {
            let evaled: Vec<Par> = el
                .ps
                .iter()
                .map(|p| eval_expr(p, env, cost).map(|p| update_locally_free(&p)))
                .collect::<Result<_, _>>()?;
            Ok(Expr::ETuple(ETuple {
                ps: evaled,
                locally_free: el.locally_free.clone(),
                connective_used: el.connective_used,
            }))
        }
        Expr::ESet(set) => {
            let evaled: Vec<Par> = set
                .ps
                .iter()
                .map(|p| eval_expr(p, env, cost).map(|p| update_locally_free(&p)))
                .collect::<Result<_, _>>()?;
            let mut s = par_set(evaled);
            s.connective_used = set.connective_used;
            s.locally_free = set.locally_free.clone();
            s.remainder = set.remainder.clone();
            Ok(Expr::ESet(s))
        }
        Expr::EMap(map) => {
            let evaled: Vec<(Par, Par)> = map
                .kvs
                .iter()
                .map(|(k, v)| {
                    Ok((
                        update_locally_free(&eval_expr(k, env, cost)?),
                        update_locally_free(&eval_expr(v, env, cost)?),
                    ))
                })
                .collect::<Result<_, RholangError>>()?;
            let mut m = par_map(evaled);
            m.connective_used = map.connective_used;
            m.locally_free = map.locally_free.clone();
            m.remainder = map.remainder.clone();
            Ok(Expr::EMap(m))
        }
        Expr::EMethod(em) => {
            cost.charge(Costs::method_call_cost())?;
            let evaled_target = eval_expr(&em.target, env, cost)?;
            let evaled_args: Vec<Par> = em
                .arguments
                .iter()
                .map(|a| eval_expr(a, env, cost))
                .collect::<Result<_, _>>()?;
            let result_par = eval_method(&em.method_name, &evaled_target, &evaled_args, env, cost)?;
            eval_single_expr(&result_par, env, cost)
        }
    }
}

/// Evaluate the top-level expressions of a `Par` (port of `evalExpr`).
pub fn eval_expr<S: Sort + SortJoin<S>>(
    par: &Par<S>,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Par<S>, RholangError> {
    let mut result = Par {
        exprs: Vec::new(),
        ..par.clone()
    };
    for e in &par.exprs {
        let evaled = eval_expr_to_par(e, env, cost)?;
        result = par_concat(&result, &evaled).re_sort();
    }
    Ok(result)
}

fn check_arity(method: &str, expected: usize, actual: usize) -> Result<(), RholangError> {
    if actual != expected {
        Err(RholangError::MethodArgumentNumberMismatch {
            method: method.to_string(),
            expected: expected as i32,
            actual: actual as i32,
        })
    } else {
        Ok(())
    }
}

/// Arity check for the methods that accept a range of arguments (e.g. `substring(i[, j])`).
/// The error reports the minimum accepted count.
fn check_arity_between(
    method: &str,
    min: usize,
    max: usize,
    actual: usize,
) -> Result<(), RholangError> {
    if actual < min || actual > max {
        Err(RholangError::MethodArgumentNumberMismatch {
            method: method.to_string(),
            expected: min as i32,
            actual: actual as i32,
        })
    } else {
        Ok(())
    }
}

/// Require an evaluated `Expr` to be a string.
fn expect_string(method: &str, e: Expr) -> Result<String, RholangError> {
    match e {
        Expr::GString(s) => Ok(s),
        other => Err(method_not_defined(method, &other)),
    }
}

/// Evaluate `p` and require a string.
fn string_arg(
    method: &str,
    p: &Par,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<String, RholangError> {
    expect_string(method, eval_single_expr(p, env, cost)?)
}

/// Require an already-evaluated `Par` to hold a string (used where the argument was evaluated as a
/// `Par`, e.g. `contains`, so it is not evaluated — and charged — twice).
fn expect_string_par(method: &str, p: &Par) -> Result<String, RholangError> {
    match single_expr(p) {
        Some(Expr::GString(s)) => Ok(s.clone()),
        Some(other) => Err(method_not_defined(method, other)),
        None => Err(RholangError::ReduceError(format!(
            "Error: {method} expects a string argument"
        ))),
    }
}

/// The string form of a value, for `toString`/`format` (the inverse of `toInt`/`toBigInt`).
fn expr_to_string(method: &str, e: &Expr) -> Result<String, RholangError> {
    match e {
        Expr::GString(s) => Ok(s.clone()),
        Expr::GInt(v) => Ok(v.to_string()),
        Expr::GBigInt(v) => Ok(v.to_string()),
        Expr::GDecimal(d) => Ok(d.to_plain_string()),
        Expr::GBool(b) => Ok(if *b { "true" } else { "false" }.to_string()),
        Expr::GUri(u) => Ok(u.clone()),
        other => Err(method_not_defined(method, other)),
    }
}

/// `bitAnd`/`bitOr`/`bitXor` (RCHIP #12). On `Int`s the result is an `Int` (these never leave
/// the range); a `BigInt` operand promotes, in two's complement, as RCHIP #51's one integer domain
/// does; two `ByteArray`s of equal length combine byte by byte.
fn bitwise_binop(
    method: &str,
    l: Expr,
    r: Expr,
    cost: &CostAccounting,
    word: fn(i64, i64) -> i64,
    big: fn(&BigInt, &BigInt) -> BigInt,
    byte: fn(u8, u8) -> u8,
) -> Result<Expr, RholangError> {
    match (l, r) {
        (Expr::GInt(a), Expr::GInt(b)) => {
            cost.charge(Costs::bitwise_int_cost(method))?;
            Ok(Expr::GInt(word(a, b)))
        }
        (Expr::GByteArray(a), Expr::GByteArray(b)) => {
            if a.len() != b.len() {
                return Err(RholangError::ReduceError(format!(
                    "Error: {method} on byte arrays of different lengths ({} and {})",
                    a.len(),
                    b.len()
                )));
            }
            cost.charge(Costs::bitwise_cost(a.len() as i64, method))?;
            Ok(Expr::GByteArray(
                a.iter().zip(b.iter()).map(|(x, y)| byte(*x, *y)).collect(),
            ))
        }
        (l, r) => match (as_integer(&l), as_integer(&r)) {
            (Some(a), Some(b)) => {
                cost.charge(Costs::bitwise_cost(
                    Costs::big_int_size(&a).max(Costs::big_int_size(&b)),
                    method,
                ))?;
                Ok(Expr::GBigInt(big(&a, &b)))
            }
            // The target is a type the method is defined on, so the argument is the misfit.
            (Some(_), None) => Err(method_not_defined(method, &r)),
            (None, _) if matches!(l, Expr::GByteArray(_)) => Err(method_not_defined(method, &r)),
            (None, _) => Err(method_not_defined(method, &l)),
        },
    }
}

/// The shift count of `shiftLeft`/`shiftRight`/`shiftRightUnsigned`: a non-negative integer.
fn shift_count(
    method: &str,
    p: &Par,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<u64, RholangError> {
    let n = eval_to_long(p, env, cost)?;
    u64::try_from(n)
        .map_err(|_| RholangError::ReduceError(format!("Error: {method} by a negative count: {n}")))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// **Bytes, never a character index** (AUDIT C252).
///
/// The old form took `&s[i..i + 2]` and handed each window to `from_str_radix`. `s.len()` counts
/// *bytes*, so the window's arithmetic is right — but a slice of a `&str` must land on character
/// boundaries, and a two-byte window over a string containing a multi-byte character need not. So a
/// **deploy** could do this:
///
/// - `"aéa"` is four bytes (1 + 2 + 1), so the even-length check passes; the window `2..4` starts
///   inside `é`, and the slice **panics** — a stranger kills the node with a deploy, which is what
///   the audit observed;
/// - the neighbouring window that *does* land on boundaries hands `from_str_radix` bytes that are not
///   a hex digit any more, and the hex it parses is not the character the term contains.
///
/// `as_bytes` removes the class rather than the site: there is no slice to leave, because an index
/// cannot step off a boundary it never asks about. `to_digit(16)` is the per-byte counterpart of
/// `from_str_radix`, and it differs in **exactly one** place, stated rather than discovered — the
/// radix parser accepts a leading `+` (`"+1"` parses as 1) and `to_digit` does not, so `"+1"` becomes
/// `None`. `"+1"` is not hex, and the stricter answer is the right one.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let hi = char::from(pair[0]).to_digit(16)?;
            let lo = char::from(pair[1]).to_digit(16)?;
            u8::try_from((hi << 4) | lo).ok()
        })
        .collect()
}

fn make_tuple(k: Par, v: Par) -> Par {
    from_expr(Expr::ETuple(ETuple {
        ps: vec![k, v],
        locally_free: AlwaysEqual(vec![]),
        connective_used: false,
    }))
}

fn unapply_tuple2(p: &Par) -> Option<(Par, Par)> {
    match single_expr(p) {
        Some(Expr::ETuple(ETuple { ps, .. })) if ps.len() == 2 => {
            Some((ps[0].clone(), ps[1].clone()))
        }
        _ => None,
    }
}

fn method_not_defined(method: &str, expr: &Expr) -> RholangError {
    RholangError::MethodNotDefined {
        method: method.to_string(),
        other_type: typ(expr).to_string(),
    }
}

fn eval_method(
    method: &str,
    target: &Par,
    args: &[Par],
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Par, RholangError> {
    match method {
        "nth" => {
            check_arity("nth", 1, args.len())?;
            cost.charge(Costs::nth_method_call_cost())?;
            let nth = restrict_to_int(eval_to_long(&args[0], env, cost)?)?;
            let v = eval_single_expr(target, env, cost)?;
            match v {
                Expr::EList(EList { ps, .. }) | Expr::ETuple(ETuple { ps, .. }) => {
                    ps.get(nth).cloned().ok_or_else(|| {
                        RholangError::ReduceError(format!("Error: index out of bound: {nth}"))
                    })
                }
                Expr::GByteArray(bs) => {
                    if nth < bs.len() {
                        Ok(from_expr(Expr::GInt(bs[nth] as i64)))
                    } else {
                        Err(RholangError::ReduceError(format!(
                            "Error: index out of bound: {nth}"
                        )))
                    }
                }
                other => Err(RholangError::ReduceError(format!(
                    "Error: nth applied to something that wasn't a list or tuple. ({})",
                    typ(&other)
                ))),
            }
        }
        "toInt" => {
            check_arity("toInt", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            match base {
                Expr::GInt(v) => Ok(from_expr(Expr::GInt(v))),
                Expr::GBigInt(bi) => {
                    cost.charge(Costs::to_int_cost_bigint(&bi))?;
                    let v = bi.to_string().parse::<i64>().map_err(|_| {
                        RholangError::ReduceError(format!(
                            "Method toInt(): input BigInt value {bi} out of range"
                        ))
                    })?;
                    Ok(from_expr(Expr::GInt(v)))
                }
                Expr::GString(s) => {
                    cost.charge(Costs::to_int_cost_string(&s))?;
                    let v = s.parse::<i64>().map_err(|_| {
                        RholangError::ReduceError(format!(
                            "Method toInt(): input string \"{s}\" cannot be converted to Int"
                        ))
                    })?;
                    Ok(from_expr(Expr::GInt(v)))
                }
                Expr::GDecimal(d) => {
                    cost.charge(Costs::to_int_cost_bigint(d.coefficient()))?;
                    match d.to_i64() {
                        Some(v) => Ok(from_expr(Expr::GInt(v))),
                        None if d.scale() == 0 => Err(RholangError::ReduceError(format!(
                            "Method toInt(): input Decimal value {d} out of range"
                        ))),
                        None => Err(RholangError::ReduceError(format!(
                            "Method toInt(): Decimal value {d} is not an integer"
                        ))),
                    }
                }
                other => Err(method_not_defined("toInt", &other)),
            }
        }
        "toBigInt" => {
            check_arity("toBigInt", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            match base {
                Expr::GBigInt(v) => Ok(from_expr(Expr::GBigInt(v))),
                Expr::GInt(num) => {
                    cost.charge(Costs::int_to_bigint_cost())?;
                    Ok(from_expr(Expr::GBigInt(BigInt::from(num))))
                }
                Expr::GString(s) => {
                    cost.charge(Costs::to_bigint_cost(&s))?;
                    let v = s.parse::<BigInt>().map_err(|_| {
                        RholangError::ReduceError(format!(
                            "Method toBigInt(): input string \"{s}\" cannot be converted to BigInt"
                        ))
                    })?;
                    Ok(from_expr(Expr::GBigInt(v)))
                }
                Expr::GDecimal(d) => {
                    cost.charge(Costs::to_int_cost_bigint(d.coefficient()))?;
                    match d.to_bigint_exact() {
                        Some(v) => Ok(from_expr(Expr::GBigInt(v))),
                        None => Err(RholangError::ReduceError(format!(
                            "Method toBigInt(): Decimal value {d} is not an integer"
                        ))),
                    }
                }
                other => Err(method_not_defined("toBigInt", &other)),
            }
        }
        "hexToBytes" => {
            check_arity("hexToBytes", 0, args.len())?;
            match eval_single_expr(target, env, cost)? {
                Expr::GString(s) => {
                    cost.charge(Costs::hex_to_bytes_cost(&s))?;
                    let bytes = hex_decode(&s).ok_or_else(|| {
                        RholangError::ReduceError(
                            "Error: exception was thrown when decoding input string to hexadecimal"
                                .to_string(),
                        )
                    })?;
                    Ok(from_expr(Expr::GByteArray(bytes)))
                }
                other => Err(method_not_defined("hexToBytes", &other)),
            }
        }
        "bytesToHex" => {
            check_arity("bytesToHex", 0, args.len())?;
            match eval_single_expr(target, env, cost)? {
                Expr::GByteArray(bytes) => {
                    cost.charge(Costs::bytes_to_hex_cost(&bytes))?;
                    Ok(from_expr(Expr::GString(hex_encode(&bytes))))
                }
                other => Err(method_not_defined("bytesToHex", &other)),
            }
        }
        "toUtf8Bytes" => {
            check_arity("toUtf8Bytes", 0, args.len())?;
            match eval_single_expr(target, env, cost)? {
                Expr::GString(s) => {
                    cost.charge(Costs::hex_to_bytes_cost(&s))?;
                    Ok(from_expr(Expr::GByteArray(s.into_bytes())))
                }
                other => Err(method_not_defined("toUtf8Bytes", &other)),
            }
        }
        "toByteArray" => {
            // Serialize the substituted `Par` to its protobuf wire form (port of the Scala
            // `toByteArray` = `Serialize[Par].encode`, where `Serialize[Par]` is
            // `mkProtobufInstance(Par)` — the protobuf serialization, not UTF-8).
            check_arity("toByteArray", 0, args.len())?;
            let substituted = substitute_par_and_charge(target, 0, env, cost)?;
            let bytes = <Par as rchain_shared::serialize::Serialize<Par>>::encode(&substituted);
            cost.charge(Costs::to_byte_array_cost(&substituted))?;
            Ok(from_expr(Expr::GByteArray(bytes)))
        }
        "union" => {
            check_arity("union", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let other = eval_single_expr(&args[0], env, cost)?;
            match (&base, &other) {
                (Expr::ESet(b), Expr::ESet(o)) => {
                    cost.charge(Costs::union_cost(b.ps.len() as i64, o.ps.len() as i64))?;
                    let mut ps = b.ps.clone();
                    ps.extend(o.ps.clone());
                    let mut s = par_set(ps);
                    s.connective_used = b.connective_used || o.connective_used;
                    s.locally_free = AlwaysEqual(union_free(
                        b.locally_free.0.clone(),
                        o.locally_free.0.clone(),
                    ));
                    s.remainder = None;
                    Ok(from_expr(Expr::ESet(s)))
                }
                (Expr::EMap(b), Expr::EMap(o)) => {
                    cost.charge(Costs::union_cost(b.kvs.len() as i64, o.kvs.len() as i64))?;
                    let mut kvs = b.kvs.clone();
                    kvs.extend(o.kvs.clone());
                    let mut m = par_map(kvs);
                    m.connective_used = b.connective_used || o.connective_used;
                    m.locally_free = AlwaysEqual(union_free(
                        b.locally_free.0.clone(),
                        o.locally_free.0.clone(),
                    ));
                    m.remainder = None;
                    Ok(from_expr(Expr::EMap(m)))
                }
                (o, _) => Err(method_not_defined("union", o)),
            }
        }
        "diff" => {
            check_arity("diff", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let other = eval_single_expr(&args[0], env, cost)?;
            match (&base, &other) {
                (Expr::ESet(b), Expr::ESet(o)) => {
                    cost.charge(Costs::diff_cost(b.ps.len() as i64, o.ps.len() as i64))?;
                    let ps: Vec<Par> = b.ps.iter().filter(|p| !o.ps.contains(p)).cloned().collect();
                    Ok(from_expr(Expr::ESet(par_set(ps))))
                }
                (Expr::EMap(b), Expr::EMap(o)) => {
                    cost.charge(Costs::diff_cost(b.kvs.len() as i64, o.kvs.len() as i64))?;
                    let kvs: Vec<(Par, Par)> = b
                        .kvs
                        .iter()
                        .filter(|(k, _)| !o.kvs.iter().any(|(ok, _)| ok == k))
                        .cloned()
                        .collect();
                    Ok(from_expr(Expr::EMap(par_map(kvs))))
                }
                (o, _) => Err(method_not_defined("diff", o)),
            }
        }
        "add" => {
            check_arity("add", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let element = eval_expr(&args[0], env, cost)?;
            match &base {
                Expr::ESet(b) => set_add(b, element, cost).map(from_expr),
                other => Err(method_not_defined("add", other)),
            }
        }
        "delete" => {
            check_arity("delete", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let element = eval_expr(&args[0], env, cost)?;
            collection_delete(&base, &element, cost).map(from_expr)
        }
        "contains" => {
            check_arity("contains", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let element = eval_expr(&args[0], env, cost)?;
            match &base {
                Expr::ESet(b) => {
                    cost.charge(Costs::lookup_cost().mul(b.ps.len() as i64))?;
                    Ok(from_expr(Expr::GBool(b.ps.contains(&element))))
                }
                Expr::EMap(b) => {
                    cost.charge(Costs::lookup_cost().mul(b.kvs.len() as i64))?;
                    Ok(from_expr(Expr::GBool(
                        b.kvs.iter().any(|(k, _)| k == &element),
                    )))
                }
                Expr::GString(s) => {
                    // `S.contains(T)` (RCHIP #37): is `T` a substring of `S`.
                    let needle = expect_string_par("contains", &element)?;
                    cost.charge(Costs::string_search_worst_cost(
                        s.chars().count() as i64,
                        needle.chars().count() as i64,
                        "contains",
                    ))?;
                    Ok(from_expr(Expr::GBool(s.contains(&needle))))
                }
                other => Err(method_not_defined("contains", other)),
            }
        }
        "get" => {
            check_arity("get", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let key = eval_expr(&args[0], env, cost)?;
            match &base {
                Expr::EMap(b) => {
                    cost.charge(Costs::lookup_cost().mul(b.kvs.len() as i64))?;
                    Ok(b.kvs
                        .iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default())
                }
                other => Err(method_not_defined("get", other)),
            }
        }
        "getOrElse" => {
            check_arity("getOrElse", 2, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let key = eval_expr(&args[0], env, cost)?;
            let default = eval_expr(&args[1], env, cost)?;
            match &base {
                Expr::EMap(b) => {
                    cost.charge(Costs::lookup_cost().mul(b.kvs.len() as i64))?;
                    Ok(b.kvs
                        .iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, v)| v.clone())
                        .unwrap_or(default))
                }
                other => Err(method_not_defined("getOrElse", other)),
            }
        }
        "set" => {
            check_arity("set", 2, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let key = eval_expr(&args[0], env, cost)?;
            let value = eval_expr(&args[1], env, cost)?;
            match base {
                Expr::EMap(b) => {
                    // `set` clones the map, scans it for the key, and rebuilds it through
                    // `par_map`, so the work is the output's canonicalisation rather than one
                    // addition (C345). Charged before the clone.
                    cost.charge(Costs::canonicalization_cost(
                        (b.kvs.len() as i64).saturating_add(1),
                    ))?;
                    let mut kvs = b.kvs.clone();
                    if let Some(slot) = kvs.iter_mut().find(|(k, _)| k == &key) {
                        slot.1 = value;
                    } else {
                        kvs.push((key, value));
                    }
                    Ok(from_expr(Expr::EMap(par_map(kvs))))
                }
                other => Err(method_not_defined("set", &other)),
            }
        }
        "keys" => {
            check_arity("keys", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            match base {
                Expr::EMap(b) => {
                    // `keys` clones the map's keys and canonicalises them into a set; the work is
                    // the output's size, not a flat method charge (C345).
                    cost.charge(Costs::canonicalization_cost(b.kvs.len() as i64))?;
                    let keys: Vec<Par> = b.kvs.iter().map(|(k, _)| k.clone()).collect();
                    Ok(from_expr(Expr::ESet(par_set(keys))))
                }
                other => Err(method_not_defined("keys", &other)),
            }
        }
        "size" => {
            check_arity("size", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let size = match &base {
                Expr::EMap(b) => b.kvs.len(),
                Expr::ESet(b) => b.ps.len(),
                other => return Err(method_not_defined("size", other)),
            };
            cost.charge(Costs::size_method_cost(size as i64))?;
            Ok(from_expr(Expr::GInt(size as i64)))
        }
        "length" => {
            check_arity("length", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            cost.charge(Costs::length_method_cost())?;
            let n = match &base {
                // Characters, not UTF-8 bytes (RCHIP #37: "the length of the string in
                // characters"), consistent with `slice`/`substring`, which index by character.
                Expr::GString(s) => s.chars().count(),
                Expr::GByteArray(b) => b.len(),
                Expr::EList(EList { ps, .. }) => ps.len(),
                other => return Err(method_not_defined("length", other)),
            };
            Ok(from_expr(Expr::GInt(n as i64)))
        }
        "slice" => {
            check_arity("slice", 2, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let from_i = eval_to_long(&args[0], env, cost)?;
            let until_i = eval_to_long(&args[1], env, cost)?;
            // Scala `slice` clamps `from` to [0, len) and returns empty when `from >= until`.
            // `restrict_to_int` wraps a negative index to a huge `usize`, which would make
            // `until - from` underflow — so clamp in `i64` first.
            let from = from_i.max(0);
            let until = until_i.max(0);
            let len = if until > from {
                (until - from) as usize
            } else {
                0
            };
            // Charge the input walk (`skip(from).take(len)` touches `max(from, until)` elements), not
            // just the output length: otherwise `slice(n, n)` walks n elements for ~0 phlo (R23).
            cost.charge(Costs::slice_cost(from.max(until)))?;
            let from = from as usize;
            match base {
                Expr::GString(s) => Ok(from_expr(Expr::GString(
                    s.chars().skip(from).take(len).collect(),
                ))),
                Expr::GByteArray(b) => Ok(from_expr(Expr::GByteArray(
                    b.into_iter().skip(from).take(len).collect(),
                ))),
                Expr::EList(EList {
                    ps,
                    locally_free,
                    connective_used,
                    remainder,
                }) => Ok(from_expr(Expr::EList(EList {
                    ps: ps.into_iter().skip(from).take(len).collect(),
                    locally_free,
                    connective_used,
                    remainder,
                }))),
                other => Err(method_not_defined("slice", &other)),
            }
        }
        "take" => {
            check_arity("take", 1, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            let n_i = eval_to_long(&args[0], env, cost)?;
            // Scala `List.take(n)` returns empty for `n <= 0`. `restrict_to_int` wraps a negative
            // index to a huge `usize`, which would return the whole list (and mint a negative cost).
            let n = if n_i <= 0 { 0 } else { n_i as usize };
            cost.charge(Costs::take_cost(n as i64))?;
            match base {
                Expr::EList(EList {
                    ps,
                    locally_free,
                    connective_used,
                    remainder,
                }) => Ok(from_expr(Expr::EList(EList {
                    ps: ps.into_iter().take(n).collect(),
                    locally_free,
                    connective_used,
                    remainder,
                }))),
                other => Err(method_not_defined("take", &other)),
            }
        }
        "toList" => {
            check_arity("toList", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            match base {
                Expr::EList(_) => Ok(target.clone()),
                Expr::ETuple(_) => Ok(target.clone()),
                Expr::ESet(b) => {
                    cost.charge(Costs::to_list_cost(b.ps.len() as i64))?;
                    Ok(from_expr(Expr::EList(EList {
                        ps: b.ps,
                        locally_free: AlwaysEqual(vec![]),
                        connective_used: false,
                        ..Default::default()
                    })))
                }
                Expr::EMap(b) => {
                    cost.charge(Costs::to_list_cost(b.kvs.len() as i64))?;
                    let ps: Vec<Par> = b
                        .kvs
                        .iter()
                        .map(|(k, v)| make_tuple(k.clone(), v.clone()))
                        .collect();
                    Ok(from_expr(Expr::EList(EList {
                        ps,
                        locally_free: AlwaysEqual(vec![]),
                        connective_used: false,
                        ..Default::default()
                    })))
                }
                other => Err(method_not_defined("toList", &other)),
            }
        }
        "toSet" => {
            check_arity("toSet", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            match base {
                Expr::ESet(_) => Ok(target.clone()),
                Expr::EMap(b) => {
                    // The conversion builds `b.kvs.len()` tuples and canonicalises them into a set;
                    // charge the output it is about to build, before it builds it (C345).
                    cost.charge(Costs::canonicalization_cost(b.kvs.len() as i64))?;
                    let ps: Vec<Par> = b
                        .kvs
                        .iter()
                        .map(|(k, v)| make_tuple(k.clone(), v.clone()))
                        .collect();
                    Ok(from_expr(Expr::ESet(par_set(ps))))
                }
                Expr::EList(EList {
                    ps,
                    connective_used,
                    remainder,
                    ..
                }) => {
                    cost.charge(Costs::canonicalization_cost(ps.len() as i64))?;
                    Ok(from_expr(Expr::ESet(ParSet {
                        ps: par_set(ps).ps,
                        connective_used,
                        locally_free: AlwaysEqual(vec![]),
                        remainder,
                    })))
                }
                other => Err(method_not_defined("toSet", &other)),
            }
        }
        "toMap" => {
            check_arity("toMap", 0, args.len())?;
            let base = eval_single_expr(target, env, cost)?;
            match base {
                Expr::EMap(_) => Ok(target.clone()),
                Expr::ESet(b) => {
                    // Charged from the operand's size, before the tuples are decoded and the map is
                    // canonicalised (C345).
                    cost.charge(Costs::canonicalization_cost(b.ps.len() as i64))?;
                    let mut kvs = Vec::new();
                    for p in b.ps {
                        match unapply_tuple2(&p) {
                            Some(kv) => kvs.push(kv),
                            None => {
                                return Err(method_not_defined(
                                    "toMap",
                                    &Expr::ESet(ParSet::default()),
                                ))
                            }
                        }
                    }
                    Ok(from_expr(Expr::EMap(par_map(kvs))))
                }
                Expr::EList(EList { ps, .. }) => {
                    cost.charge(Costs::canonicalization_cost(ps.len() as i64))?;
                    let mut kvs = Vec::new();
                    for p in ps {
                        match unapply_tuple2(&p) {
                            Some(kv) => kvs.push(kv),
                            None => {
                                return Err(method_not_defined(
                                    "toMap",
                                    &Expr::EList(EList::default()),
                                ))
                            }
                        }
                    }
                    Ok(from_expr(Expr::EMap(par_map(kvs))))
                }
                other => Err(method_not_defined("toMap", &other)),
            }
        }
        // --- String methods (RCHIP #37 "Add string functions") --------------------------
        // All string methods index/count by character (Unicode scalar values), matching
        // `length`/`slice`.
        "substring" => {
            check_arity_between("substring", 1, 2, args.len())?;
            let s = string_arg("substring", target, env, cost)?;
            let from_i = eval_to_long(&args[0], env, cost)?;
            let until_i = if args.len() == 2 {
                eval_to_long(&args[1], env, cost)?
            } else {
                s.chars().count() as i64
            };
            let from = from_i.max(0);
            let until = until_i.max(0);
            cost.charge(Costs::slice_cost(from.max(until)))?;
            // `i64::saturating_sub` clamps at `i64::MIN`, not 0, so clamp explicitly: a negative
            // length cast to `usize` wraps and would take the whole tail.
            let len = if until > from {
                (until - from) as usize
            } else {
                0
            };
            let out: String = s.chars().skip(from as usize).take(len).collect();
            Ok(from_expr(Expr::GString(out)))
        }
        "indexOf" => {
            check_arity_between("indexOf", 1, 2, args.len())?;
            let s = string_arg("indexOf", target, env, cost)?;
            let needle = string_arg("indexOf", &args[0], env, cost)?;
            let from_i = if args.len() == 2 {
                eval_to_long(&args[1], env, cost)?
            } else {
                0
            };
            cost.charge(Costs::string_search_worst_cost(
                s.chars().count() as i64,
                needle.chars().count() as i64,
                "indexOf",
            ))?;
            let hay: Vec<char> = s.chars().collect();
            let ndl: Vec<char> = needle.chars().collect();
            let start = from_i.max(0) as usize;
            let found = if ndl.is_empty() {
                if start <= hay.len() {
                    Some(start)
                } else {
                    None
                }
            } else if ndl.len() > hay.len() || start > hay.len() - ndl.len() {
                None
            } else {
                hay[start..]
                    .windows(ndl.len())
                    .position(|w| w == ndl.as_slice())
                    .map(|i| start + i)
            };
            Ok(from_expr(Expr::GInt(found.map(|i| i as i64).unwrap_or(-1))))
        }
        "toLowerCase" => {
            check_arity("toLowerCase", 0, args.len())?;
            let s = string_arg("toLowerCase", target, env, cost)?;
            cost.charge(Costs::string_transform_cost(
                s.chars().count() as i64,
                "toLowerCase",
            ))?;
            Ok(from_expr(Expr::GString(s.to_lowercase())))
        }
        "toUpperCase" => {
            check_arity("toUpperCase", 0, args.len())?;
            let s = string_arg("toUpperCase", target, env, cost)?;
            cost.charge(Costs::string_transform_cost(
                s.chars().count() as i64,
                "toUpperCase",
            ))?;
            Ok(from_expr(Expr::GString(s.to_uppercase())))
        }
        "capitalize" => {
            check_arity("capitalize", 0, args.len())?;
            let s = string_arg("capitalize", target, env, cost)?;
            cost.charge(Costs::string_transform_cost(
                s.chars().count() as i64,
                "capitalize",
            ))?;
            let out: String = match s.chars().next() {
                Some(first) => first.to_uppercase().collect::<String>() + &s[first.len_utf8()..],
                None => String::new(),
            };
            Ok(from_expr(Expr::GString(out)))
        }
        "reverse" => {
            check_arity("reverse", 0, args.len())?;
            let s = string_arg("reverse", target, env, cost)?;
            cost.charge(Costs::string_transform_cost(
                s.chars().count() as i64,
                "reverse",
            ))?;
            Ok(from_expr(Expr::GString(s.chars().rev().collect())))
        }
        "trim" => {
            check_arity("trim", 0, args.len())?;
            let s = string_arg("trim", target, env, cost)?;
            cost.charge(Costs::string_transform_cost(
                s.chars().count() as i64,
                "trim",
            ))?;
            Ok(from_expr(Expr::GString(s.trim().to_string())))
        }
        "isEmpty" => {
            check_arity("isEmpty", 0, args.len())?;
            let s = string_arg("isEmpty", target, env, cost)?;
            Ok(from_expr(Expr::GBool(s.is_empty())))
        }
        "nonEmpty" => {
            check_arity("nonEmpty", 0, args.len())?;
            let s = string_arg("nonEmpty", target, env, cost)?;
            Ok(from_expr(Expr::GBool(!s.is_empty())))
        }
        "startsWith" => {
            check_arity("startsWith", 1, args.len())?;
            let s = string_arg("startsWith", target, env, cost)?;
            let prefix = string_arg("startsWith", &args[0], env, cost)?;
            cost.charge(Costs::string_search_cost(
                s.chars().count() as i64,
                prefix.chars().count() as i64,
                "startsWith",
            ))?;
            Ok(from_expr(Expr::GBool(s.starts_with(&prefix))))
        }
        "endsWith" => {
            check_arity("endsWith", 1, args.len())?;
            let s = string_arg("endsWith", target, env, cost)?;
            let suffix = string_arg("endsWith", &args[0], env, cost)?;
            cost.charge(Costs::string_search_cost(
                s.chars().count() as i64,
                suffix.chars().count() as i64,
                "endsWith",
            ))?;
            Ok(from_expr(Expr::GBool(s.ends_with(&suffix))))
        }
        "replace" => {
            check_arity("replace", 2, args.len())?;
            let s = string_arg("replace", target, env, cost)?;
            let old = string_arg("replace", &args[0], env, cost)?;
            let new = string_arg("replace", &args[1], env, cost)?;
            cost.charge(Costs::string_replace_cost(
                s.chars().count() as i64,
                old.chars().count() as i64,
                new.chars().count() as i64,
            ))?;
            // An empty `old` would splice `new` between every character; leave the input as-is.
            let out = if old.is_empty() {
                s
            } else {
                s.replace(&old, &new)
            };
            Ok(from_expr(Expr::GString(out)))
        }
        "split" => {
            check_arity("split", 1, args.len())?;
            let s = string_arg("split", target, env, cost)?;
            let sep = string_arg("split", &args[0], env, cost)?;
            cost.charge(Costs::string_split_cost(
                s.chars().count() as i64,
                sep.chars().count() as i64,
            ))?;
            let parts: Vec<Par> = if sep.is_empty() {
                s.chars()
                    .map(|c| from_expr(Expr::GString(c.to_string())))
                    .collect()
            } else {
                s.split(sep.as_str())
                    .map(|p| from_expr(Expr::GString(p.to_string())))
                    .collect()
            };
            Ok(from_expr(Expr::EList(EList {
                ps: parts,
                ..Default::default()
            })))
        }
        "format" => {
            // Variadic: a single list/tuple argument is flattened, so both `S.format(a, b)` and
            // `S.format([a, b])` work. `%s` placeholders are replaced in order; extras are left.
            let s = string_arg("format", target, env, cost)?;
            let mut arg_pars: Vec<Par> = Vec::new();
            if args.len() == 1 {
                match eval_single_expr(&args[0], env, cost)? {
                    Expr::EList(EList { ps, .. }) | Expr::ETuple(ETuple { ps, .. }) => {
                        arg_pars.extend(ps)
                    }
                    other => arg_pars.push(from_expr(other)),
                }
            } else {
                arg_pars.extend_from_slice(args);
            }
            cost.charge(Costs::string_format_cost(
                s.chars().count() as i64,
                arg_pars.len() as i64,
            ))?;
            let mut out = String::with_capacity(s.len());
            let mut rest = s.as_str();
            for p in &arg_pars {
                let Some(i) = rest.find("%s") else { break };
                out.push_str(&rest[..i]);
                let v = eval_single_expr(p, env, cost)?;
                out.push_str(&expr_to_string("format", &v)?);
                rest = &rest[i + 2..];
            }
            out.push_str(rest);
            Ok(from_expr(Expr::GString(out)))
        }
        "toString" => {
            check_arity("toString", 0, args.len())?;
            let v = eval_single_expr(target, env, cost)?;
            let out = expr_to_string("toString", &v)?;
            cost.charge(Costs::to_string_cost(out.chars().count() as i64))?;
            Ok(from_expr(Expr::GString(out)))
        }
        // --- Bit operations (RCHIP #12 "Bitwise operators") -------------------------------
        // Scala's `&`, `|`, `^`, `~`, `<<`, `>>`, `>>>` as methods: `|` and `~` are already the
        // grammar's par and connective negation. Integers are one exact domain (RCHIP #51): a
        // left shift that leaves the `Int` range promotes to `BigInt` rather than wrapping, and a
        // shift count is never masked (Scala's `1L << 64 == 1` is not preserved).
        "bitAnd" | "bitOr" | "bitXor" => {
            check_arity(method, 1, args.len())?;
            let l = eval_single_expr(target, env, cost)?;
            let r = eval_single_expr(&args[0], env, cost)?;
            let out = match method {
                "bitAnd" => {
                    bitwise_binop(method, l, r, cost, |a, b| a & b, |a, b| a & b, |a, b| a & b)
                }
                "bitOr" => {
                    bitwise_binop(method, l, r, cost, |a, b| a | b, |a, b| a | b, |a, b| a | b)
                }
                _ => bitwise_binop(method, l, r, cost, |a, b| a ^ b, |a, b| a ^ b, |a, b| a ^ b),
            }?;
            Ok(from_expr(out))
        }
        "bitNot" => {
            check_arity("bitNot", 0, args.len())?;
            match eval_single_expr(target, env, cost)? {
                Expr::GInt(v) => {
                    cost.charge(Costs::bitwise_int_cost("bitNot"))?;
                    Ok(from_expr(Expr::GInt(!v)))
                }
                Expr::GBigInt(v) => {
                    cost.charge(Costs::bitwise_cost(Costs::big_int_size(&v), "bitNot"))?;
                    Ok(from_expr(Expr::GBigInt(!v)))
                }
                Expr::GByteArray(b) => {
                    cost.charge(Costs::bitwise_cost(b.len() as i64, "bitNot"))?;
                    Ok(from_expr(Expr::GByteArray(b.iter().map(|x| !x).collect())))
                }
                other => Err(method_not_defined("bitNot", &other)),
            }
        }
        "shiftLeft" => {
            check_arity("shiftLeft", 1, args.len())?;
            let v = eval_single_expr(target, env, cost)?;
            let n = shift_count("shiftLeft", &args[0], env, cost)?;
            let Some(x) = as_integer(&v) else {
                return Err(method_not_defined("shiftLeft", &v));
            };
            if let Expr::GInt(w) = v {
                // `x * 2^n` stays an `Int` when it fits; `i128` holds any `i64 << 63`.
                if w == 0 || n < 64 {
                    let wide = if w == 0 { 0 } else { (w as i128) << n };
                    if let Ok(r) = i64::try_from(wide) {
                        cost.charge(Costs::bitwise_int_cost("shiftLeft"))?;
                        return Ok(from_expr(Expr::GInt(r)));
                    }
                }
            }
            // Charged on the result's size *before* it is built, so an absurd count runs out of
            // phlo rather than allocating.
            let bytes = Costs::shift_left_result_size(x.magnitude().bits(), n);
            cost.charge(Costs::bitwise_cost(bytes, "shiftLeft"))?;
            let shift = usize::try_from(n).map_err(|_| {
                RholangError::ReduceError(format!("Error: shiftLeft count out of range: {n}"))
            })?;
            Ok(from_expr(Expr::GBigInt(x << shift)))
        }
        "shiftRight" => {
            // Arithmetic (sign-extending) shift: `floor(x / 2^n)`.
            check_arity("shiftRight", 1, args.len())?;
            let v = eval_single_expr(target, env, cost)?;
            let n = shift_count("shiftRight", &args[0], env, cost)?;
            match v {
                Expr::GInt(w) => {
                    cost.charge(Costs::bitwise_int_cost("shiftRight"))?;
                    Ok(from_expr(Expr::GInt(w >> n.min(63))))
                }
                Expr::GBigInt(x) => {
                    cost.charge(Costs::bitwise_cost(Costs::big_int_size(&x), "shiftRight"))?;
                    let shift = usize::try_from(n).unwrap_or(usize::MAX);
                    Ok(from_expr(Expr::GBigInt(x >> shift)))
                }
                other => Err(method_not_defined("shiftRight", &other)),
            }
        }
        "shiftRightUnsigned" => {
            // Logical (zero-filling) shift of the 64-bit two's-complement word — Scala's `>>>`,
            // which has no meaning for an unbounded integer: a `BigInt` outside the `Int` range is
            // refused rather than given a width.
            check_arity("shiftRightUnsigned", 1, args.len())?;
            let w = eval_to_long(target, env, cost)?;
            let n = shift_count("shiftRightUnsigned", &args[0], env, cost)?;
            cost.charge(Costs::bitwise_int_cost("shiftRightUnsigned"))?;
            let r = if n >= 64 { 0 } else { ((w as u64) >> n) as i64 };
            Ok(from_expr(Expr::GInt(r)))
        }
        _ => Err(RholangError::ReduceError(format!(
            "Unimplemented method: {method}"
        ))),
    }
}

/// The result of a tuplespace produce/consume: the matched continuation, the list of
/// (channel, matched data, removed data, persistent), and whether it was a peek.
pub type Application = Option<(
    TaggedContinuation,
    Vec<(SortedProc, ListParWithRandom, ListParWithRandom, bool)>,
    bool,
)>;

/// The produce between its two scheduled phases (the rholang view of
/// `rchain_rspace::scheduled_space::PendingProduce`): what the phase-two commit needs to
/// re-validate under the full lock set.
#[derive(Debug, Clone)]
pub struct PendingProduce {
    /// The trigger channel the produce was made on.
    pub trigger: SortedProc,
    /// The in-flight datum (not yet stored; the commit re-extracts with it prepended).
    pub data: ListParWithRandom,
    pub persist: bool,
}

/// The result of the scheduled `produce_at` (Law 20): either the op committed inline
/// (`phase_two: None`, the datum was stored and `application` is final), or the matched join
/// channels are returned for the claim queue's `claim_more` and the commit is deferred to
/// `commit_produce`.
#[derive(Debug, Clone)]
pub struct ScheduledProduce {
    /// The matched continuation's channel set (the phase-two claim set). Empty when the datum was
    /// stored with no match.
    pub joins: Vec<SortedProc>,
    /// Final when `phase_two` is `None`; a placeholder otherwise.
    pub application: Application,
    /// `Some` when the commit is deferred to `commit_produce`.
    pub phase_two: Option<PendingProduce>,
    /// Dropped by the caller after enqueueing the continuation's next-step effects.
    pub release: ReleaseToken,
}

/// The result of the scheduled `consume_at` (no split: the full static source set was claimed
/// before the op).
#[derive(Debug, Clone)]
pub struct ScheduledConsume {
    pub application: Application,
    /// Dropped by the caller after enqueueing the continuation's next-step effects.
    pub release: ReleaseToken,
}

/// The tuplespace interface the evaluator produces/consumes against (port of `RhoTuplespace`).
#[async_trait]
pub trait Tuplespace: std::marker::Send + std::marker::Sync {
    async fn produce(
        &self,
        channel: &SortedProc,
        data: ListParWithRandom,
        persist: bool,
    ) -> Result<Application, RholangError>;

    async fn consume(
        &self,
        channels: &[SortedProc],
        patterns: &[BindPattern],
        continuation: TaggedContinuation,
        persist: bool,
        peeks: BTreeSet<usize>,
    ) -> Result<Application, RholangError>;

    /// Install a **persistent** native continuation on `channel`, outside any deploy's term.
    ///
    /// The pair of this and [`Dispatch::register`]: a native system process binds its continuation at
    /// a constant channel when the runtime is built (`runtime.rs::install_system_processes`), which
    /// works while every native channel is known at compile time. The vault capability's handle is
    /// not — it is minted per call — so a handler that mints one has to be able to bind it. The space
    /// side is this; the handler side is the dispatcher's.
    ///
    /// The default refuses, for the same reason the dispatcher's does: a tuplespace that cannot
    /// install must say so rather than hand back a name nothing answers on.
    async fn install(
        &self,
        _channel: &SortedProc,
        _patterns: &[BindPattern],
        _continuation: TaggedContinuation,
    ) -> Result<(), RholangError> {
        Err(RholangError::BugFoundError(
            "this tuplespace cannot install a native continuation".to_string(),
        ))
    }

    /// Produce *at* the given DFS path (the Law 20 scheduling entry point). The default is the
    /// plain produce plus `ReleaseToken::detached()` — non-scheduling tuplespaces "don't
    /// schedule": their ops complete inline and no phase-two claim set is returned.
    async fn produce_at(
        &self,
        _path: Vec<u16>,
        channel: &SortedProc,
        data: ListParWithRandom,
        persist: bool,
    ) -> Result<ScheduledProduce, RholangError> {
        let application = self.produce(channel, data, persist).await?;
        Ok(ScheduledProduce {
            joins: vec![],
            application,
            phase_two: None,
            release: ReleaseToken::detached(),
        })
    }

    /// Consume *at* the given DFS path. Default: the plain consume (no split is ever needed — the
    /// full static source set is claimed before this is called).
    async fn consume_at(
        &self,
        _path: Vec<u16>,
        channels: &[SortedProc],
        patterns: &[BindPattern],
        continuation: TaggedContinuation,
        persist: bool,
        peeks: BTreeSet<usize>,
    ) -> Result<ScheduledConsume, RholangError> {
        let application = self
            .consume(channels, patterns, continuation, persist, peeks)
            .await?;
        Ok(ScheduledConsume {
            application,
            release: ReleaseToken::detached(),
        })
    }

    /// Commit the phase two of a scheduled produce (only the scheduling `RSpace` ever defers one;
    /// the default is therefore unreachable and errors loudly).
    async fn commit_produce(&self, _pending: PendingProduce) -> Result<Application, RholangError> {
        Err(RholangError::ReduceError(
            "commit_produce called on a non-scheduling tuplespace".to_string(),
        ))
    }
}

/// Dispatches a continuation with its matched data, at its DFS path (port of `Dispatch`). The
/// path is the scheduler's linearization key (Laws 20–22): the continuation subtree is ordered
/// under the effect that dispatched it (`path.child(0)`), ahead of that effect's follow-on
/// re-produce and the next sibling.
#[async_trait]
pub trait Dispatch: std::marker::Send + std::marker::Sync {
    async fn dispatch(
        &self,
        continuation: TaggedContinuation,
        data_list: Vec<ListParWithRandom>,
        path: DfsPath,
    ) -> Result<(), RholangError>;

    /// Bind `handler` to `body_ref`, so a continuation carrying that id dispatches to it.
    ///
    /// **Why this exists.** Every native system process is installed at a *constant* channel with a
    /// `BodyRefs` id (`runtime.rs::install_system_processes`), which is enough while the set of native
    /// channels is known at compile time. The vault capability is the first native surface that is
    /// not: `findOrCreate` mints a fresh unforgeable name per call and a send to it must reach Rust
    /// (the multi-signature vault holds REV under exactly such a name). So a handler has to be able to
    /// bind a *minted* channel at runtime, which needs this and nothing else.
    ///
    /// **The default refuses rather than doing nothing.** A dispatcher that owns no table cannot
    /// register into one, and a silent no-op would leave the minted channel answering nothing — the
    /// failure mode where a caller is handed a capability that does not work. `None` is not an
    /// option for the same reason a `Result` is used elsewhere in this tree: the caller must be able
    /// to tell.
    fn register(
        &self,
        _body_ref: i64,
        _handler: crate::dispatch::ScalaBodyFn,
    ) -> Result<(), RholangError> {
        Err(RholangError::BugFoundError(
            "this dispatcher cannot register a handler".to_string(),
        ))
    }
}

/// An owned, flattened term (the borrow-free unit the scheduler reduces).
enum OwnedTerm {
    Send(Send),
    Receive(Receive),
    New(New),
    Match(Match),
    Bundle(Bundle),
    ExprVar(Var),
    ExprMethod(Expr),
}

/// A unit of reduction. `Par` expands into its sub-terms; `Produce`/`Consume`/`ProducePeeks` are
/// resolved tuple-space effects carrying a channel footprint (used to shard disjoint effects
/// concurrently). `Par` is a scheduling barrier: its footprint is unknown until it is expanded, so
/// it is applied at its DFS position.
enum Effect {
    Par(Par, Env<Par>, Blake2b512Random),
    Produce(SortedProc, ListParWithRandom, bool),
    Consume(Vec<(BindPattern, SortedProc)>, ParWithRandom, bool, bool),
    ProducePeeks(Vec<(SortedProc, ListParWithRandom, ListParWithRandom, bool)>),
}

/// Resolve one flattened term to a unit of work (or `None` for a match with no matching case).
/// Pure w.r.t. the tuple space (only atomic cost charges and pre-split RNG), so it can run
/// concurrently across a `Par`'s terms and still produce the same result as the sequential
/// interpreter.
fn resolve_term(
    term: OwnedTerm,
    env: Env<Par>,
    rand: Blake2b512Random,
    urn_map: Arc<BTreeMap<String, Par>>,
    cost: Arc<CostAccounting>,
) -> Result<Option<Effect>, RholangError> {
    match term {
        OwnedTerm::Send(s) => resolve_send(&s, &env, &rand, cost.as_ref()).map(Some),
        OwnedTerm::Receive(r) => resolve_receive(&r, &env, &rand, cost.as_ref()).map(Some),
        OwnedTerm::New(n) => {
            resolve_new(&n, &env, &rand, urn_map.as_ref(), cost.as_ref()).map(Some)
        }
        OwnedTerm::Match(m) => resolve_match(&m, &env, &rand, cost.as_ref()),
        OwnedTerm::Bundle(b) => Ok(Some(Effect::Par(*b.body, env, rand))),
        OwnedTerm::ExprVar(v) => {
            eval_var(&v, &env, cost.as_ref()).map(|p| Some(Effect::Par(p, env, rand)))
        }
        OwnedTerm::ExprMethod(e) => {
            eval_expr_to_par(&e, &env, cost.as_ref()).map(|p| Some(Effect::Par(p, env, rand)))
        }
    }
}

/// Pure part of `eval(Send)`: resolve the channel and data, returning the produce arguments.
fn resolve_send(
    send: &Send,
    env: &Env<Par>,
    rand: &Blake2b512Random,
    cost: &CostAccounting,
) -> Result<Effect, RholangError> {
    cost.charge(Costs::send_eval_cost())?;
    let eval_chan = eval_expr(send.chan.as_ref(), env, cost)?;
    let sub_chan = substitute_par_and_charge(&eval_chan, 0, env, cost)?;
    let unbundled = match single_bundle(&sub_chan) {
        Some(value) => {
            if !value.write_flag {
                return Err(RholangError::ReduceError(
                    "Trying to send on non-writeable channel.".to_string(),
                ));
            }
            (*value.body).clone()
        }
        None => sub_chan.eval(),
    };
    let data: Vec<Name> = send
        .data
        .iter()
        .map(|d| eval_expr(d, env, cost))
        .collect::<Result<_, _>>()?;
    let subst_data: Vec<Name> = data
        .iter()
        .map(|d| substitute_par_and_charge(d, 0, env, cost))
        .collect::<Result<_, _>>()?;
    Ok(Effect::Produce(
        SortedProc::new(unbundled),
        ListParWithRandom {
            pars: subst_data
                .into_iter()
                .map(|d| SortedProc::new(d.eval()))
                .collect(),
            random_state: (*rand).clone(),
        },
        send.persistent,
    ))
}

/// Pure part of `eval(Receive)`: resolve the sources/patterns/body, returning the consume arguments.
fn resolve_receive(
    receive: &Receive,
    env: &Env<Par>,
    rand: &Blake2b512Random,
    cost: &CostAccounting,
) -> Result<Effect, RholangError> {
    cost.charge(Costs::receive_eval_cost())?;
    let mut binds: Vec<(BindPattern, SortedProc)> = Vec::new();
    for rb in &receive.binds {
        let q = unbundle_receive(rb, env, cost)?;
        let subst_patterns: Vec<Name> = rb
            .patterns
            .iter()
            .map(|p| substitute_par_and_charge(p, 1, env, cost))
            .collect::<Result<_, _>>()?;
        binds.push((
            BindPattern {
                patterns: subst_patterns
                    .into_iter()
                    .map(|p| SortedProc::new(p.eval()))
                    .collect(),
                remainder: rb.remainder.as_deref().cloned(),
                // `BindPattern.free_count` is still an `i32` (casper/node construct the struct with
                // literals), so the carrier is discharged here — the one boundary on this path.
                free_count: i32::from(rb.free_count),
            },
            SortedProc::new(q),
        ));
    }
    let subst_body = substitute_par_and_charge(
        &receive.body,
        0,
        &env.shift(i32::from(receive.bind_count)),
        cost,
    )?;
    Ok(Effect::Consume(
        binds,
        ParWithRandom {
            body: SortedProc::new(subst_body),
            random_state: (*rand).clone(),
        },
        receive.persistent,
        receive.peek,
    ))
}

fn unbundle_receive(
    rb: &ReceiveBind,
    env: &Env<Par>,
    cost: &CostAccounting,
) -> Result<Par, RholangError> {
    let eval_src = eval_expr(rb.source.as_ref(), env, cost)?;
    let subst = substitute_par_and_charge(&eval_src, 0, env, cost)?;
    match single_bundle(&subst) {
        Some(value) => {
            if !value.read_flag {
                Err(RholangError::ReduceError(
                    "Trying to read from non-readable channel.".to_string(),
                ))
            } else {
                Ok((*value.body).clone())
            }
        }
        None => Ok(subst.eval()),
    }
}

/// Pure part of `eval(New)`: allocate fresh names and return the body to reduce.
fn resolve_new(
    new: &New,
    env: &Env<Par>,
    rand: &Blake2b512Random,
    urn_map: &BTreeMap<String, Par>,
    cost: &CostAccounting,
) -> Result<Effect, RholangError> {
    cost.charge(Costs::new_bindings_cost(i64::from(i32::from(
        new.bind_count,
    ))))?;
    let mut r = (*rand).clone();
    let new_env = alloc(
        i32::from(new.bind_count),
        &new.uri,
        &new.injections,
        env,
        urn_map,
        &mut r,
    )?;
    // The body must run with the RNG state advanced past the freshly-allocated names: reusing the
    // incoming state would make a *nested* `new` draw the same random bytes as its parent (colliding
    // fresh names) — issue #19.
    Ok(Effect::Par((*new.p).clone(), new_env, r))
}

fn alloc(
    count: i32,
    urns: &[String],
    injections: &BTreeMap<String, Par>,
    env: &Env<Par>,
    urn_map: &BTreeMap<String, Par>,
    rand: &mut Blake2b512Random,
) -> Result<Env<Par>, RholangError> {
    let mut new_env = (*env).clone();
    for _ in 0..(count - urns.len() as i32) {
        let bytes = rand.next();
        let addr = Par {
            unforgeables: vec![GUnforgeable::GPrivate(GPrivate { id: bytes })],
            ..Default::default()
        };
        new_env = new_env.put(addr);
    }
    for urn in urns {
        new_env = add_urn(new_env, urn, injections, urn_map)?;
    }
    Ok(new_env)
}

fn add_urn(
    env: Env<Par>,
    urn: &str,
    injections: &BTreeMap<String, Par>,
    urn_map: &BTreeMap<String, Par>,
) -> Result<Env<Par>, RholangError> {
    if let Some(p) = urn_map.get(urn) {
        Ok(env.put(p.clone()))
    } else if let Some(p) = injections.get(urn) {
        Ok(env.put(p.clone()))
    } else {
        Err(RholangError::BugFoundError(format!(
            "No value set for `{urn}`. This is a bug in the normalizer or on the path from it."
        )))
    }
}

/// Pure part of `eval(Match)`: spatial-match the target against cases, returning the winning body.
fn resolve_match(
    m: &Match,
    env: &Env<Par>,
    rand: &Blake2b512Random,
    cost: &CostAccounting,
) -> Result<Option<Effect>, RholangError> {
    cost.charge(Costs::match_eval_cost())?;
    let evaled_target = eval_expr(m.target.as_ref(), env, cost)?;
    let subst_target = substitute_par_and_charge(&evaled_target, 0, env, cost)?;
    let target = subst_target.eval();
    for case in &m.cases {
        let pattern = substitute_par_and_charge(&case.pattern, 1, env, cost)?;
        if let Some(free_map) = spatial_match_result(&target, &pattern.eval())? {
            let mut new_env = (*env).clone();
            for e in 0..i32::from(case.free_count) {
                // A level the case's pattern names and *this match* did not fill becomes the empty
                // par — the Scala's own `freeMap.getOrElse(e, Par())` (`Reduce.scala:352`), which is
                // semantics rather than a flatten: a greedy connective pattern
                // (`match ("success" | "success") { {x | y} => ... }`) binds `x` to the whole par and
                // leaves `y` empty, and `legacy/.../matching-parallel-processes.rho` documents that as
                // its expected output. U10 refused here; the corpus refuted the reading by measurement
                // (U17, AUDIT C52).
                new_env = new_env.put(free_map.get(&e).cloned().unwrap_or_default());
            }
            return Ok(Some(Effect::Par(
                (*case.source).clone(),
                new_env,
                (*rand).clone(),
            )));
        }
    }
    Ok(None)
}

/// A boxed, `'static` reduction future. The reducer is `Arc`-shared; the recursive reduction methods
/// take `self: Arc<Self>` and return this, so the scheduler can `tokio::spawn` disjoint branches.
type ReducerFuture = Pin<Box<dyn Future<Output = Result<(), RholangError>> + std::marker::Send>>;

/// Spawn a prepared reducer future and await its result, mapping a task panic to a legible
/// [`RholangError`]. Every continuation dispatch and persistent/peek follow-on effect goes through
/// this so no single task's future chain grows with the recursion depth.
async fn join_spawned(
    fut: Result<ReducerFuture, RholangError>,
    what: &'static str,
) -> Result<(), RholangError> {
    let fut = fut?;
    spawn_reduce(fut).join(what).await?
}

/// A reduction task spawned onto the host's executor, awaitable to its result.
///
/// On a threaded target this is a `tokio::task::JoinHandle`. **`wasm32-unknown-unknown` has no tokio
/// runtime** — the reducer is driven by whatever executor the host supplies (the node's runtime, or
/// `wasm-bindgen-futures`' microtask queue in a browser) — so it is a one-shot the spawned task
/// resolves. This is the same seam the store (`KeyValueStoreCodec::with_store`) and the clock
/// (`shared::time`) took: the reducer names the *shape*, the host supplies the mechanism (issue #98).
#[cfg(not(target_arch = "wasm32"))]
struct ReduceTask<T>(tokio::task::JoinHandle<T>);

#[cfg(target_arch = "wasm32")]
struct ReduceTask<T>(tokio::sync::oneshot::Receiver<T>);

impl<T> ReduceTask<T> {
    /// Await the task, mapping a host panic — or a task dropped before it ran — to a legible error.
    async fn join(self, what: &'static str) -> Result<T, RholangError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0
                .await
                .map_err(|e| RholangError::ReduceError(format!("{what} task panicked: {e}")))
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.0.await.map_err(|_| {
                RholangError::ReduceError(format!("{what} task was dropped before it ran"))
            })
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_reduce<T: std::marker::Send + 'static>(
    fut: impl Future<Output = T> + std::marker::Send + 'static,
) -> ReduceTask<T> {
    ReduceTask(tokio::spawn(fut))
}

#[cfg(target_arch = "wasm32")]
fn spawn_reduce<T: 'static>(fut: impl Future<Output = T> + 'static) -> ReduceTask<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    wasm_bindgen_futures::spawn_local(async move {
        chan::oneshot_send(tx, fut.await);
    });
    ReduceTask(rx)
}

#[cfg(not(target_arch = "wasm32"))]
type RelaxedTaskSet = tokio::task::JoinSet<Result<(), RholangError>>;
#[cfg(target_arch = "wasm32")]
type RelaxedTaskSet = Vec<ReduceTask<Result<(), RholangError>>>;

/// Per-evaluation reduction-step budget. A step is a continuation dispatch or a persistent/peek
/// re-produce / re-consume — i.e. one trip around the produce/consume → continuation cycle. An
/// unbounded recursive contract burns one step per iteration, so this cap turns a missing base case
/// into a legible error instead of a stack overflow or an unbounded task chain (issue #11). The
/// counter is reset at the start of every top-level [`DebruijnInterpreter::eval`] (one deploy /
/// exploratory evaluation), so ordinary deploys are unaffected.
pub const DEFAULT_MAX_REDUCE_STEPS: i64 = 100_000;

/// The reducer (port of `DebruijnInterpreter`).
pub struct DebruijnInterpreter<T: Tuplespace, D: Dispatch> {
    space: T,
    dispatcher: D,
    urn_map: Arc<BTreeMap<String, Par>>,
    /// **Write-only, and deliberately so** (AUDIT C256; the deviation is in `spec/audit/passes.md`
    /// §6). The oracle returns this collection as `mergeableChannels`, which becomes
    /// `DeployMergeableData.channels`; here nothing reads it, so that field is `[]` for every deploy.
    /// Wiring it is a **hard fork** — it changes every block's serialized form and therefore every
    /// state hash — so the mechanism is recorded as a deliberate deviation instead of being made live
    /// at the tail of another unit. Named here so the field does not read as a mechanism that works.
    merge_chs: Arc<Mutex<Vec<SortedProc>>>,
    mergeable_tag_name: SortedProc,
    concurrent: bool,
    /// The effect-scheduler mode (Laws 20–22; see `crate::scheduler`). `Sequential` by default —
    /// the plain DFS loop, the sound reference the Gate and Relaxed modes must refine.
    effect_mode: Mutex<EffectMode>,
    /// Reduction steps taken in the current top-level evaluation (see [`DEFAULT_MAX_REDUCE_STEPS`]).
    steps: Arc<AtomicI64>,
    max_steps: Arc<AtomicI64>,
    /// Cooperative cancellation flag, checked wherever a step is charged. Dropping the outer
    /// evaluation future (a `tokio::time::timeout`) does not stop the spawned continuation tasks;
    /// this flag lets the owner tell the in-flight task tree to unwind (issue #12).
    cancelled: Arc<AtomicBool>,
    /// The per-runtime channel claim queue (Law 20). Every relaxed produce/consume task claims its
    /// static footprint at its DFS path and holds the guard until the continuation's next-step
    /// effects are enqueued — the queue-level continuation-prepend. Entries are inserted on claim
    /// and removed on guard drop; the DashMap itself is never trimmed (fork-per-deploy bounds it).
    claims: Arc<ChannelClaimQueue<SortedProc, Vec<u16>>>,
    /// The relaxed-mode task set (Law 20's spawn-only dispatch): effects and continuations are
    /// spawned into it, and only the root evaluation drains it. A produce/consume task must never
    /// await continuation completion while holding its claims — `for(x <- c){ c!(x) }` would
    /// self-deadlock (the continuation claims `c`, the producer holds it) — so relaxed dispatch
    /// enqueues the continuation at `child(0)` and returns.
    relaxed_tasks: Arc<Mutex<RelaxedTaskSet>>,
}

impl<T: Tuplespace + 'static, D: Dispatch + 'static> DebruijnInterpreter<T, D> {
    pub fn new(
        space: T,
        dispatcher: D,
        urn_map: BTreeMap<String, Par>,
        mergeable_tag_name: SortedProc,
    ) -> Self {
        DebruijnInterpreter {
            space,
            dispatcher,
            urn_map: Arc::new(urn_map),
            merge_chs: Arc::new(Mutex::new(Vec::new())),
            mergeable_tag_name,
            concurrent: true,
            effect_mode: Mutex::new(EffectMode::Sequential),
            steps: Arc::new(AtomicI64::new(0)),
            max_steps: Arc::new(AtomicI64::new(DEFAULT_MAX_REDUCE_STEPS)),
            cancelled: Arc::new(AtomicBool::new(false)),
            claims: Arc::new(ChannelClaimQueue::new()),
            relaxed_tasks: Arc::new(Mutex::new(RelaxedTaskSet::default())),
        }
    }

    /// Set the per-evaluation reduction-step budget (see [`DEFAULT_MAX_REDUCE_STEPS`]).
    pub fn set_max_reduce_steps(&self, max_steps: i64) {
        self.max_steps.store(max_steps, Ordering::SeqCst);
    }

    /// Cooperatively cancel the current evaluation: the next step check in the spawned task tree
    /// fails with a cancellation error, unwinding every task back to the caller (issue #12).
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Toggle concurrent per-term resolution (on by default). With concurrency disabled,
    /// `resolve_children` resolves terms in a plain sequential loop — the reference used by the
    /// differential tests.
    pub fn set_concurrent(&mut self, concurrent: bool) {
        self.concurrent = concurrent;
    }

    /// Set the effect-scheduler mode (Laws 20–22). Defaults to [`EffectMode::Sequential`]; the
    /// Gate (Phase 4) and Relaxed (Phase 5) modes replace the sequential `reduce_effects` loop
    /// while preserving its DFS order semantics. The relaxed-validated mode additionally enables
    /// the Law 24 per-commit certificate on the claim queue. Interior-mutable: the casper
    /// block path switches it around the per-deploy sequential fallback re-run.
    pub fn set_effect_mode(&self, mode: EffectMode) {
        *self.effect_mode.lock().unpoison() = mode;
        self.claims
            .set_validation_enabled(mode == EffectMode::RelaxedValidated);
    }

    /// The current effect-scheduler mode.
    pub fn effect_mode(&self) -> EffectMode {
        *self.effect_mode.lock().unpoison()
    }

    /// Whether the current evaluation observed the S.3 enqueue window (a DFS-earlier claim landing
    /// while a DFS-later claim held the channel) — the skew divergence signal the block path reads.
    pub fn observed_skew(&self) -> bool {
        self.claims.observed_skew()
    }

    /// Evaluate a top-level `Par` (port of `Reduce.eval(par)`): reduce the process to normal form via
    /// the recursive reducer. Resets the per-evaluation reduction-step counter and the cancellation
    /// flag.
    pub async fn eval(
        self: Arc<Self>,
        par: &Par,
        env: &Env<Par>,
        rand: &Blake2b512Random,
        cost: &Arc<CostAccounting>,
    ) -> Result<(), RholangError> {
        self.steps.store(0, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        // Reset the write-record layer per evaluation: writes from a prior deploy in this
        // runtime must not invalidate this deploy's commits.
        if matches!(
            self.effect_mode(),
            EffectMode::Relaxed | EffectMode::RelaxedValidated
        ) {
            self.claims.reset_write_record();
            self.claims.reset_skew();
        }
        let result = self
            .clone()
            .reduce_par(
                (*par).clone(),
                (*env).clone(),
                (*rand).clone(),
                cost.clone(),
                DfsPath::root(),
            )
            .await;
        if matches!(
            self.effect_mode(),
            EffectMode::Relaxed | EffectMode::RelaxedValidated
        ) {
            result.and(self.drain_relaxed_tasks().await)
        } else {
            result
        }
    }

    /// Drain the relaxed task set to completion, propagating the first error (Law 20: only the
    /// root reduction drains; the produce/consume tasks never join their continuations). Tasks
    /// spawned during a drain land in the replacement set and are drained in the next round, so
    /// the drain ends exactly when no task has enqueued another.
    async fn drain_relaxed_tasks(&self) -> Result<(), RholangError> {
        loop {
            // `mut` is needed by the host arm's `join_next`; the wasm arm consumes the set by value.
            #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
            let mut set = {
                let mut guard = self.relaxed_tasks.lock().unpoison();
                if guard.is_empty() {
                    return Ok(());
                }
                std::mem::take(&mut *guard)
            };
            #[cfg(not(target_arch = "wasm32"))]
            while let Some(res) = set.join_next().await {
                match res {
                    Err(e) => {
                        return Err(RholangError::ReduceError(format!(
                            "relaxed task panicked: {e}"
                        )))
                    }
                    Ok(Err(e)) => return Err(e),
                    Ok(Ok(())) => {}
                }
            }
            // The wasm arm has no `JoinSet` (it spawns through `spawn_local`); joining in insertion
            // order still awaits every task and surfaces an error, which is the drain's contract.
            #[cfg(target_arch = "wasm32")]
            for task in set {
                task.join("relaxed").await??;
            }
        }
    }

    /// Reduce a `Par` (public: the continuation-dispatch hook used by the dispatcher's eval closure).
    /// Resolves the `Par`'s sub-terms to effects, then reduces those effects in DFS order. `path`
    /// addresses this `Par` node in the reduction tree; its terms are the children `path.child(i)`.
    pub fn reduce_par(
        self: Arc<Self>,
        par: Par,
        env: Env<Par>,
        rand: Blake2b512Random,
        cost: Arc<CostAccounting>,
        path: DfsPath,
    ) -> ReducerFuture {
        Box::pin(async move {
            let effects = self.resolve_children(&par, &env, &rand, &cost).await?;
            self.reduce_effects(effects, cost, path).await
        })
    }

    /// Flatten a `Par` into its owned sub-terms and resolve each term's pure part (concurrently when
    /// `concurrent` is set), returning the resolved effects in DFS order. Each term keeps the same
    /// index-based RNG split as the sequential interpreter.
    async fn resolve_children(
        &self,
        par: &Par,
        env: &Env<Par>,
        rand: &Blake2b512Random,
        cost: &Arc<CostAccounting>,
    ) -> Result<Vec<Effect>, RholangError> {
        let mut terms: Vec<OwnedTerm> = Vec::new();
        for s in &par.sends {
            terms.push(OwnedTerm::Send(s.clone()));
        }
        for r in &par.receives {
            terms.push(OwnedTerm::Receive(r.clone()));
        }
        for n in &par.news {
            terms.push(OwnedTerm::New(n.clone()));
        }
        for m in &par.matches {
            terms.push(OwnedTerm::Match(m.clone()));
        }
        for b in &par.bundles {
            terms.push(OwnedTerm::Bundle(b.clone()));
        }
        for e in &par.exprs {
            match e {
                Expr::EVar(v) => terms.push(OwnedTerm::ExprVar((**v).clone())),
                Expr::EMethod(_) => terms.push(OwnedTerm::ExprMethod(e.clone())),
                _ => {}
            }
        }
        if terms.len() > i16::MAX as usize {
            return Err(RholangError::ReduceError(format!(
                "The number of terms in the Par is {}, which exceeds the limit of {}.",
                terms.len(),
                i16::MAX
            )));
        }
        let n = terms.len();
        let mut effects: Vec<Effect> = Vec::with_capacity(n);
        if self.concurrent {
            // Resolve each term's pure part concurrently. The tokio multi-thread runtime bounds
            // actual CPU parallelism to its worker count; the effects are then applied in DFS order
            // by the scheduler, so the final state is schedule-independent.
            let mut handles = Vec::with_capacity(n);
            for (i, term) in terms.into_iter().enumerate() {
                let r = split_rand(rand, i, n)?;
                let e = (*env).clone();
                let urn_map = self.urn_map.clone();
                let c = cost.clone();
                handles.push(spawn_reduce(
                    async move { resolve_term(term, e, r, urn_map, c) },
                ));
            }
            for h in handles {
                let resolved = h.join("reducer").await??;
                if let Some(w) = resolved {
                    effects.push(w);
                }
            }
        } else {
            // Sequential reference (concurrency off): resolve each term in order, no fork-join.
            for (i, term) in terms.into_iter().enumerate() {
                let r = split_rand(rand, i, n)?;
                let e = (*env).clone();
                let resolved = resolve_term(term, e, r, self.urn_map.clone(), cost.clone())?;
                if let Some(w) = resolved {
                    effects.push(w);
                }
            }
        }
        Ok(effects)
    }

    /// Reduce a sequence of effects in DFS order: the `i`th effect is applied at `path.child(i)`.
    /// The sequential reference loop; the Gate (Law 21) and Relaxed (Law 20) schedulers replace it
    /// under the same per-channel DFS order.
    fn reduce_effects(
        self: Arc<Self>,
        effects: Vec<Effect>,
        cost: Arc<CostAccounting>,
        path: DfsPath,
    ) -> ReducerFuture {
        Box::pin(async move {
            match self.effect_mode() {
                // The sequential DFS loop — the sound reference.
                EffectMode::Sequential | EffectMode::ForkJoin => {
                    for (i, effect) in effects.into_iter().enumerate() {
                        self.clone()
                            .apply_effect(effect, cost.clone(), path.child(i as u16))
                            .await?;
                    }
                    Ok(())
                }
                // Law 21 — the gate: the task for effect `i` runs only after the tasks for effects
                // `0..i−1` have completed (subtree completion = handle completion), so the gate's
                // execution is exactly the sequential `Effect.apply` fold
                // (`gate_exec_refines_apply`). Not a speedup — the sound, sequential-equivalent
                // carrier the Relaxed mode is measured against.
                EffectMode::Gate => {
                    // The gate chain: the task for effect `i` owns the handle of the task for
                    // effect `i−1` and awaits it first, so (transitively) it runs only after
                    // every earlier effect's subtree completed. Awaiting the immediate
                    // predecessor alone suffices because that task itself awaits its own — a
                    // linear chain of awaits, not the quadratic all-predecessors join.
                    let mut predecessor: Option<ReduceTask<Result<(), RholangError>>> = None;
                    for (i, effect) in effects.into_iter().enumerate() {
                        let self_ = self.clone();
                        let cost = cost.clone();
                        let effect_path = path.child(i as u16);
                        let handle = spawn_reduce(async move {
                            if let Some(prev) = predecessor {
                                prev.join("gate predecessor").await??;
                            }
                            self_.apply_effect(effect, cost, effect_path).await
                        });
                        predecessor = Some(handle);
                    }
                    if let Some(last) = predecessor {
                        last.join("gate").await??;
                    }
                    Ok(())
                }
                // Law 20 — the relaxed scheduler: every effect is a task on the per-channel claim
                // queues (spawn-only dispatch into `relaxed_tasks`; the root evaluation drains).
                // Per-channel DFS order is preserved by the queues; cross-channel interleaving is
                // free (the relaxed contract). This arm returns after enqueueing — the tasks run
                // concurrently under the queues. Each effect's claim is made at dispatch time
                // (pre-claiming, inside `apply_effect_relaxed`), so this loop lands same-channel
                // claims in path order before any task runs — a later-path task can never commit
                // before an earlier-path claim has landed (the persistent-produce inversion).
                EffectMode::Relaxed | EffectMode::RelaxedValidated => {
                    for (i, effect) in effects.into_iter().enumerate() {
                        let self_ = self.clone();
                        let cost = cost.clone();
                        let effect_path = path.child(i as u16);
                        let fut = if self_.cancelled.load(Ordering::SeqCst) {
                            Err(RholangError::ReduceError("reduction cancelled".to_string()))
                        } else {
                            Ok(self_
                                .clone()
                                .apply_effect_relaxed(effect, cost, effect_path))
                        };
                        self_.enqueue_relaxed(fut);
                    }
                    Ok(())
                }
            }
        })
    }

    /// Apply one effect at DFS path `path`: expand a nested `Par` (a scheduling barrier), or
    /// perform a produce/consume/peek and reduce any matched continuation inline (the
    /// continuation-prepend invariant). The continuation subtree is addressed at `path.child(0)`
    /// and the persistent/peek follow-on re-produce at `path.child(1)`: both complete before the
    /// next sibling at the parent, matching the sequential reducer's order.
    fn apply_effect(
        self: Arc<Self>,
        effect: Effect,
        cost: Arc<CostAccounting>,
        path: DfsPath,
    ) -> ReducerFuture {
        Box::pin(async move {
            match effect {
                Effect::Par(par, env, rand) => self.reduce_par(par, env, rand, cost, path).await,
                Effect::Produce(chan, data, persistent) => {
                    self.update_mergeable_channels(&chan);
                    let result = self.space.produce(&chan, data.clone(), persistent).await?;
                    if let Some((continuation, data_list, peek)) = result {
                        join_spawned(
                            self.clone().dispatch_owned(
                                continuation,
                                data_list.clone(),
                                path.child(0),
                            ),
                            "continuation dispatch",
                        )
                        .await?;
                        if persistent {
                            join_spawned(
                                self.clone().apply_effect_spawned(
                                    Effect::Produce(chan.clone(), data.clone(), true),
                                    cost,
                                    path.child(1),
                                ),
                                "persistent re-produce",
                            )
                            .await?;
                        } else if peek {
                            join_spawned(
                                self.clone().apply_effect_spawned(
                                    Effect::ProducePeeks(data_list.clone()),
                                    cost,
                                    path.child(1),
                                ),
                                "peek re-produce",
                            )
                            .await?;
                        }
                    }
                    Ok(())
                }
                Effect::Consume(binds, body, persistent, peek) => {
                    let patterns: Vec<BindPattern> = binds.iter().map(|(p, _)| p.clone()).collect();
                    let sources: Vec<SortedProc> = binds.iter().map(|(_, s)| s.clone()).collect();
                    for s in &sources {
                        self.update_mergeable_channels(s);
                    }
                    let peeks: BTreeSet<usize> = if peek {
                        (0..sources.len()).collect()
                    } else {
                        BTreeSet::new()
                    };
                    let result = self
                        .space
                        .consume(
                            &sources,
                            &patterns,
                            TaggedContinuation::ParBody(body.clone()),
                            persistent,
                            peeks.clone(),
                        )
                        .await?;
                    if let Some((continuation, data_list, p)) = result {
                        join_spawned(
                            self.clone().dispatch_owned(
                                continuation,
                                data_list.clone(),
                                path.child(0),
                            ),
                            "continuation dispatch",
                        )
                        .await?;
                        if persistent {
                            join_spawned(
                                self.clone().apply_effect_spawned(
                                    Effect::Consume(binds.clone(), body.clone(), true, peek),
                                    cost,
                                    path.child(1),
                                ),
                                "persistent re-consume",
                            )
                            .await?;
                        } else if p {
                            join_spawned(
                                self.clone().apply_effect_spawned(
                                    Effect::ProducePeeks(data_list.clone()),
                                    cost,
                                    path.child(1),
                                ),
                                "peek re-produce",
                            )
                            .await?;
                        }
                    }
                    Ok(())
                }
                Effect::ProducePeeks(data_list) => {
                    for (i, (chan, _, removed_data, persist)) in data_list.iter().enumerate() {
                        if !persist {
                            self.clone()
                                .apply_effect(
                                    Effect::Produce(chan.clone(), removed_data.clone(), false),
                                    cost.clone(),
                                    path.child(i as u16),
                                )
                                .await?;
                        }
                    }
                    Ok(())
                }
            }
        })
    }

    /// Dispatch a continuation on a fresh task, charged against the per-evaluation step budget.
    /// Spawning (rather than awaiting inline) keeps each task's future chain shallow: an unbounded
    /// recursive contract would otherwise nest one dispatch future per iteration inside a single
    /// task, and re-polling that ever-growing chain overflows the native stack (issue #11).
    fn dispatch_owned(
        self: Arc<Self>,
        continuation: TaggedContinuation,
        data_list: Vec<(SortedProc, ListParWithRandom, ListParWithRandom, bool)>,
        path: DfsPath,
    ) -> Result<ReducerFuture, RholangError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(RholangError::ReduceError("reduction cancelled".to_string()));
        }
        let step = self.steps.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let max_steps = self.max_steps.load(Ordering::SeqCst);
        if step > max_steps {
            return Err(RholangError::ReduceError(format!(
                "reduction step budget exceeded ({max_steps} steps)"
            )));
        }
        let fut = Box::pin(async move {
            let data: Vec<ListParWithRandom> =
                data_list.iter().map(|(_, d, _, _)| d.clone()).collect();
            self.dispatcher.dispatch(continuation, data, path).await
        });
        Ok(fut)
    }

    /// Apply a persistent/peek follow-on effect on a fresh task, charged against the same
    /// per-evaluation step budget. The persistent re-consume loop is the other half of issue #11:
    /// a persistent contract re-installs itself after every invocation, and applying that re-consume
    /// inline nests one `apply_effect` future per iteration in the root task.
    fn apply_effect_spawned(
        self: Arc<Self>,
        effect: Effect,
        cost: Arc<CostAccounting>,
        path: DfsPath,
    ) -> Result<ReducerFuture, RholangError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(RholangError::ReduceError("reduction cancelled".to_string()));
        }
        let step = self.steps.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let max_steps = self.max_steps.load(Ordering::SeqCst);
        if step > max_steps {
            return Err(RholangError::ReduceError(format!(
                "reduction step budget exceeded ({max_steps} steps)"
            )));
        }
        let fut = Box::pin(async move { self.apply_effect(effect, cost, path).await });
        Ok(fut)
    }

    /// Enqueue a relaxed task into the shared set without awaiting it (the spawn-only dispatch
    /// rule — see the `relaxed_tasks` field). A step-budget/cancellation error raised before the
    /// future exists is enqueued as an immediately-failing task so the root drain sees it.
    fn enqueue_relaxed(&self, fut: Result<ReducerFuture, RholangError>) {
        let fut = match fut {
            Ok(fut) => fut,
            Err(e) => Box::pin(async move { Err(e) }),
        };
        let mut guard = self.relaxed_tasks.lock().unpoison();
        #[cfg(not(target_arch = "wasm32"))]
        guard.spawn(fut);
        #[cfg(target_arch = "wasm32")]
        guard.push(spawn_reduce(fut));
    }

    /// Wait for the head lease, mapping a Law 24 validation failure to the block-path fallback
    /// error. `NotHead` is retried inside `wait_at_head` itself, so it never surfaces here —
    /// if it ever does (a future queue change), report it as a deploy error rather than
    /// panicking the node (the type-system audit bans panics in production code).
    async fn wait_at_head_or_invalidate(
        guard: &ClaimGuard<SortedProc, Vec<u16>>,
        at_path: &[u16],
    ) -> Result<HeadLease, RholangError> {
        match guard.wait_at_head().await {
            Ok(lease) => Ok(lease),
            Err(AcquireError::ValidationFailed {
                channel,
                writer_path,
            }) => Err(RholangError::SpeculationInvalid {
                channel,
                writer_path,
                at_path: at_path.to_vec(),
            }),
            Err(AcquireError::NotHead) => Err(RholangError::BugFoundError(
                "wait_at_head retried NotHead internally".to_string(),
            )),
        }
    }

    /// Apply one effect at DFS path `path` under the relaxed scheduler (Law 20): claim the
    /// effect's static footprint at `path`, wait for the head lease, perform the op via the
    /// scheduled `produce_at`/`consume_at`, enqueue the continuation at `child(0)` and the
    /// persistent/peek follow-on at `child(1)`, and only then drop the claim (the queue-level
    /// continuation-prepend). The produce's phase two claims the matched join set, re-waits at
    /// head, and commits with re-validation — see `rchain_rspace::scheduled_space`.
    ///
    /// The claim is made **at dispatch time**, before the task future is even built
    /// (dispatch-time pre-claiming, `docs/src/formal/scheduling.md`): every claim of a
    /// dispatch list therefore lands in the queue in dispatch order, so a later-path task can
    /// never commit before an earlier-path claim has landed — the persistent-produce COMM-order
    /// inversion cannot occur. Continuation claims land at their parent's match time and are
    /// ordered by the queue's path-ordered insert.
    fn apply_effect_relaxed(
        self: Arc<Self>,
        effect: Effect,
        cost: Arc<CostAccounting>,
        path: DfsPath,
    ) -> ReducerFuture {
        match effect {
            Effect::Par(par, env, rand) => {
                Box::pin(async move { self.reduce_par(par, env, rand, cost, path).await })
            }
            Effect::Produce(chan, data, persistent) => {
                // Pre-claim at dispatch time (see the doc above): the guard is created now and
                // moved into the task.
                let mut guard = self.claims.claim(path.0.clone(), &[chan.clone()]);
                Box::pin(async move {
                    self.update_mergeable_channels(&chan);
                    let _lease = Self::wait_at_head_or_invalidate(&guard, &path.0).await?;
                    let scheduled = self
                        .space
                        .produce_at(path.0.clone(), &chan, data.clone(), persistent)
                        .await?;
                    let ScheduledProduce {
                        joins,
                        application,
                        phase_two,
                        release,
                    } = scheduled;
                    let application = match phase_two {
                        Some(pending) => {
                            // Phase two: claim the matched join set and re-wait at head (the
                            // trigger passes through — it is already active under this claim),
                            // then commit. The commit re-validates under the full lock set.
                            guard.claim_more(&joins);
                            let _lease = Self::wait_at_head_or_invalidate(&guard, &path.0).await?;
                            self.space.commit_produce(pending).await?
                        }
                        None => application,
                    };
                    // Law 24's record layer: the produce commits a write on its trigger —
                    // phase-one store or phase-two store/match — with post-value `true`. Stamped
                    // before the guard drops (the order the prefix-visibility check relies on).
                    self.claims.record_write(&chan, &path.0, true);
                    if let Some((continuation, data_list, peek)) = application {
                        self.enqueue_relaxed(self.clone().dispatch_owned(
                            continuation,
                            data_list.clone(),
                            path.child(0),
                        ));
                        if persistent {
                            self.enqueue_relaxed(self.clone().apply_effect_relaxed_spawned(
                                Effect::Produce(chan.clone(), data.clone(), true),
                                cost,
                                path.child(1),
                            ));
                        } else if peek {
                            self.enqueue_relaxed(self.clone().apply_effect_relaxed_spawned(
                                Effect::ProducePeeks(data_list.clone()),
                                cost,
                                path.child(1),
                            ));
                        }
                    }
                    // The token's scope is the claim's scope: released after the enqueue, together
                    // with the guard (`ReleaseToken` is a `Copy` marker, so `let _ = release;` is
                    // the explicit scope end).
                    let _ = release;
                    Ok(())
                })
            }
            Effect::Consume(binds, body, persistent, peek) => {
                // Pre-claim at dispatch time: the full static source set lands in the queue
                // before any later-path task can acquire.
                let patterns: Vec<BindPattern> = binds.iter().map(|(p, _)| p.clone()).collect();
                let sources: Vec<SortedProc> = binds.iter().map(|(_, s)| s.clone()).collect();
                let peeks: BTreeSet<usize> = if peek {
                    (0..sources.len()).collect()
                } else {
                    BTreeSet::new()
                };
                let guard = self.claims.claim(path.0.clone(), &sources);
                Box::pin(async move {
                    for s in &sources {
                        self.update_mergeable_channels(s);
                    }
                    let _lease = Self::wait_at_head_or_invalidate(&guard, &path.0).await?;
                    let scheduled = self
                        .space
                        .consume_at(
                            path.0.clone(),
                            &sources,
                            &patterns,
                            TaggedContinuation::ParBody(body.clone()),
                            persistent,
                            peeks.clone(),
                        )
                        .await?;
                    let ScheduledConsume {
                        application,
                        release,
                    } = scheduled;
                    if let Some((continuation, data_list, p)) = application {
                        // Law 24's record layer: a matched, non-peek consume commits a removal
                        // write (post-value `false`) on every source. An installing consume and
                        // a peek match write nothing (the Lean `applyAt`). Stamped before the
                        // guard drops.
                        if !p {
                            for s in &sources {
                                self.claims.record_write(s, &path.0, false);
                            }
                        }
                        self.enqueue_relaxed(self.clone().dispatch_owned(
                            continuation,
                            data_list.clone(),
                            path.child(0),
                        ));
                        if persistent {
                            self.enqueue_relaxed(self.clone().apply_effect_relaxed_spawned(
                                Effect::Consume(binds.clone(), body.clone(), true, peek),
                                cost,
                                path.child(1),
                            ));
                        } else if p {
                            self.enqueue_relaxed(self.clone().apply_effect_relaxed_spawned(
                                Effect::ProducePeeks(data_list.clone()),
                                cost,
                                path.child(1),
                            ));
                        }
                    }
                    let _ = release;
                    Ok(())
                })
            }
            Effect::ProducePeeks(data_list) => Box::pin(async move {
                // Each peek re-produce dispatches as an ordinary produce at its child path; its
                // claim lands at that dispatch (the recursive apply_effect_relaxed pre-claims).
                for (i, (chan, _, removed_data, persist)) in data_list.iter().enumerate() {
                    if !persist {
                        let self_ = self.clone();
                        let cost = cost.clone();
                        let effect_path = path.child(i as u16);
                        let fut = if self_.cancelled.load(Ordering::SeqCst) {
                            Err(RholangError::ReduceError("reduction cancelled".to_string()))
                        } else {
                            Ok(self_.clone().apply_effect_relaxed(
                                Effect::Produce(chan.clone(), removed_data.clone(), false),
                                cost,
                                effect_path,
                            ))
                        };
                        self_.enqueue_relaxed(fut);
                    }
                }
                Ok(())
            }),
        }
    }

    /// The relaxed re-effect (persistent/peek follow-on) on a fresh task, charged against the same
    /// per-evaluation step budget as `apply_effect_spawned`.
    fn apply_effect_relaxed_spawned(
        self: Arc<Self>,
        effect: Effect,
        cost: Arc<CostAccounting>,
        path: DfsPath,
    ) -> Result<ReducerFuture, RholangError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(RholangError::ReduceError("reduction cancelled".to_string()));
        }
        let step = self.steps.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let max_steps = self.max_steps.load(Ordering::SeqCst);
        if step > max_steps {
            return Err(RholangError::ReduceError(format!(
                "reduction step budget exceeded ({max_steps} steps)"
            )));
        }
        let fut = Box::pin(async move { self.apply_effect_relaxed(effect, cost, path).await });
        Ok(fut)
    }

    /// The collection half of the inert mechanism above (C256): it fills `merge_chs`, and nothing
    /// consumes it. Kept because deleting it would hide *what the oracle does* from the next reader;
    /// named as dead so the port is not read as one that publishes mergeable channels.
    fn update_mergeable_channels(&self, chan: &SortedProc) {
        if self.is_mergeable_channel(chan) {
            let mut chs = self.merge_chs.lock().unpoison();
            if !chs.contains(chan) {
                chs.push(chan.clone());
            }
        }
    }

    fn is_mergeable_channel(&self, chan: &SortedProc) -> bool {
        chan.as_par()
            .exprs
            .iter()
            .find_map(|e| match e {
                Expr::ETuple(ETuple { ps, .. }) => ps.first(),
                _ => None,
            })
            .map_or(false, |head| head == self.mergeable_tag_name.as_par())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_arithmetic() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let p = from_expr(Expr::EPlus(
            Box::new(from_expr(Expr::GInt(2))),
            Box::new(from_expr(Expr::GInt(3))),
        ));
        assert_eq!(eval_single_expr(&p, &e, &cost).unwrap(), Expr::GInt(5));
    }

    // --- The exact decimal (issue #380, option B) --------------------------------------------

    fn decimal_eval(src: &str) -> Result<Expr, RholangError> {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let env = Env::new();
        let p: Par = crate::normalizer::source_to_adt(src).expect("parse").into();
        eval_single_expr(&p, &env, &cost)
    }

    fn d(coefficient: i64, scale: u32) -> Expr {
        Expr::GDecimal(Decimal::new(coefficient.into(), scale).expect("canonical"))
    }

    #[test]
    fn decimal_arithmetic_is_exact_and_stays_decimal() {
        assert_eq!(decimal_eval("1.1d + 2.2d").unwrap(), d(33, 1));
        assert_eq!(decimal_eval("1.5d - 0.5d").unwrap(), d(1, 0));
        assert_eq!(decimal_eval("1.5d * 2d").unwrap(), d(3, 0));
        assert_eq!(decimal_eval("0.1d * 0.2d").unwrap(), d(2, 2));
        assert_eq!(decimal_eval("2 + 1.5d").unwrap(), d(35, 1));
        assert_eq!(decimal_eval("BigInt(2) + 1.5d").unwrap(), d(35, 1));
        assert_eq!(decimal_eval("-1.5d").unwrap(), d(-15, 1));
        assert_eq!(
            decimal_eval("1.5d.toString()").unwrap(),
            Expr::GString("1.5".to_string())
        );
        assert_eq!(
            decimal_eval("2d.toString()").unwrap(),
            Expr::GString("2".to_string())
        );
    }

    #[test]
    fn decimal_division_is_exact_or_an_explicit_error() {
        assert_eq!(decimal_eval("1d / 4d").unwrap(), d(25, 2));
        assert_eq!(decimal_eval("10d / 4d").unwrap(), d(25, 1));
        let err = decimal_eval("1d / 3d").expect_err("1/3 is not representable");
        assert!(err.to_string().contains("not exact within scale"), "{err}");
        let err = decimal_eval("1d / 0d").expect_err("division by zero");
        assert!(err.to_string().contains("/ by zero"), "{err}");
        let err = decimal_eval("0d / 0d").expect_err("division by zero");
        assert!(err.to_string().contains("/ by zero"), "{err}");
    }

    #[test]
    fn decimal_modulo_and_bitwise_are_refused_by_name() {
        for src in ["5d % 2d", "1 % 2d"] {
            let err = decimal_eval(src).expect_err("modulo is not defined on decimals");
            assert!(err.to_string().contains("Decimal"), "{src}: {err}");
        }
        for src in [
            "1.5d.bitAnd(1)",
            "1.5d.bitOr(1)",
            "1.5d.bitXor(1)",
            "1.5d.bitNot()",
            "1.5d.shiftLeft(1)",
            "1.5d.shiftRight(1)",
            "1.5d.shiftRightUnsigned(1)",
        ] {
            let err = decimal_eval(src).expect_err("bitwise/shift is not defined on decimals");
            assert!(err.to_string().contains("Decimal"), "{src}: {err}");
        }
    }

    #[test]
    fn scalar_comparisons_span_integers_and_decimals() {
        assert_eq!(decimal_eval("2 == 2d").unwrap(), Expr::GBool(true));
        assert_eq!(decimal_eval("BigInt(2) == 2d").unwrap(), Expr::GBool(true));
        assert_eq!(decimal_eval("1.5d < 2").unwrap(), Expr::GBool(true));
        assert_eq!(decimal_eval("1 < 1.5d").unwrap(), Expr::GBool(true));
        assert_eq!(decimal_eval("-2d < -1").unwrap(), Expr::GBool(true));
        assert_eq!(decimal_eval("1.5d != 1.6d").unwrap(), Expr::GBool(true));
        assert_eq!(decimal_eval("1d == 1").unwrap(), Expr::GBool(true));
        // Structural equality keeps type identity: a decimal is not an integer in a collection.
        assert_eq!(decimal_eval("[1d] == [1]").unwrap(), Expr::GBool(false));
    }

    #[test]
    fn decimal_conversions_name_what_they_cannot_convert() {
        assert_eq!(decimal_eval("2d.toInt()").unwrap(), Expr::GInt(2));
        assert_eq!(decimal_eval("2.0d.toInt()").unwrap(), Expr::GInt(2));
        let err = decimal_eval("1.5d.toInt()").expect_err("1.5 is not an integer");
        assert!(err.to_string().contains("not an integer"), "{err}");
        assert_eq!(
            decimal_eval("2d.toBigInt()").unwrap(),
            Expr::GBigInt(BigInt::from(2))
        );
        let err = decimal_eval("1.5d.toBigInt()").expect_err("1.5 is not an integer");
        assert!(err.to_string().contains("not an integer"), "{err}");
    }

    #[test]
    fn decimal_costs_are_charged_and_grow_with_the_coefficient() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let env = Env::new();
        let p: Par = crate::normalizer::source_to_adt("1.5d * 2.5d")
            .expect("parse")
            .into();
        eval_single_expr(&p, &env, &cost).unwrap();
        assert!(cost.total_charged() > 0);

        let small = Decimal::new(1.into(), 1).expect("canonical");
        let big = Decimal::new(
            BigInt::parse_bytes("9".repeat(100).as_bytes(), 10).expect("digits"),
            1,
        )
        .expect("canonical");
        assert!(
            Costs::decimal_multiplication(&big, &big).value
                > Costs::decimal_multiplication(&small, &small).value
        );
        assert!(
            Costs::decimal_division(&small, &small).value
                > Costs::decimal_multiplication(&small, &small).value
        );
        assert!(Costs::decimal_comparison(&small, &big).value > 0);
    }

    /// **The evaluation helpers' failure arms, which nothing reached** (TEST-COVERAGE item 11: this
    /// file tops `spec/COVERAGE-LEDGER.md`'s ranking by missed lines, and these branches are what the
    /// ranking is counting).
    ///
    /// Each arm is a refusal a deploy can actually meet, and each is pinned by the **message** it
    /// produces rather than by `is_err()`: the message is what the deploy's error report carries, so a
    /// silent switch from one refusal to another would otherwise be invisible. The arms are reached
    /// directly — these are pure helpers, so no runtime is needed to exercise them — which is also why
    /// no end-to-end test had: nothing in the corpus spells a wildcard in expression position.
    #[test]
    fn the_evaluation_helpers_refuse_what_they_cannot_evaluate() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let env = Env::new();

        // `eval_var`. A pattern variable is not evaluable, `Empty` is not a var instance at all, and
        // a bound level the environment does not carry is unbound.
        let err = eval_var(&Var::Wildcard, &env, &cost).expect_err("a wildcard is a pattern");
        assert!(
            err.to_string().contains("attempting to evaluate a pattern"),
            "{err}"
        );
        let err = eval_var(&Var::FreeVar(2), &env, &cost).expect_err("a free var is a pattern");
        assert!(
            err.to_string().contains("attempting to evaluate a pattern"),
            "{err}"
        );
        let err = eval_var(&Var::Empty, &env, &cost).expect_err("Empty is not a var instance");
        assert!(
            err.to_string().contains("Impossible var instance EMPTY"),
            "{err}"
        );
        let err = eval_var(&Var::BoundVar(7), &env, &cost).expect_err("level 7 is unbound");
        assert!(err.to_string().contains("Unbound variable: 7"), "{err}");

        // `eval_to_long`: a non-integer operand of `nth`, `*`, `repeat` and the rest.
        let string = from_expr(Expr::GString("x".to_string()));
        let err = eval_to_long(&string, &env, &cost).expect_err("a string is not an Int");
        assert!(err.to_string().contains("expected Int"), "{err}");

        // `restrict_to_int`: the boundary that keeps an `i64` index from silently wrapping. In range
        // on both sides of the guard, out of range just past each end.
        assert_eq!(restrict_to_int(0).expect("zero is in range"), 0);
        assert_eq!(
            restrict_to_int(i32::MAX as i64).expect("i32::MAX is the last accepted"),
            i32::MAX as usize
        );
        for n in [i32::MAX as i64 + 1, i64::MAX, i32::MIN as i64 - 1, i64::MIN] {
            let err = restrict_to_int(n).expect_err("out of range");
            assert!(err.to_string().contains("value out of range"), "{n}: {err}");
        }

        // `eval_single_expr`: a `Par` that is not a single expression. The empty par is the shape the
        // check exists for — a term whose value is not an expression at all.
        let empty: Par = Par::default();
        let err =
            eval_single_expr(&empty, &env, &cost).expect_err("an empty par holds no expression");
        assert!(
            err.to_string().contains("Expected a single expression"),
            "{err}"
        );

        // `split_rand`: the three arms, including the `n > 256` branch that uses the 16-bit split and
        // the `u16::try_from` refusal past its range.
        let rand = Blake2b512Random::from_init(&[3u8; 32]);
        assert!(
            split_rand(&rand, 0, 1).is_ok(),
            "n == 1 passes the RNG through"
        );
        assert!(split_rand(&rand, 3, 256).is_ok(), "n <= 256 splits by byte");
        assert!(
            split_rand(&rand, 300, 257).is_ok(),
            "n > 256 splits by short"
        );
        let err = split_rand(&rand, 70_000, 257).expect_err("beyond a u16 index");
        assert!(err.to_string().contains("out of range"), "{err}");
    }

    /// `++` is defined for String, ByteArray, List, **Map** and **Set** (`Reduce.scala`'s
    /// `EPlusPlusBody`: the Map/Set arms go through `union`). The Map/Set arms were missing in the
    /// port, so `Set(1) ++ Set(2)` and `{"a": 1} ++ {"b": 2}` — both valid rholang — errored instead
    /// of reducing, which is a replay divergence on any block that uses them.
    #[tokio::test]
    async fn plus_plus_concatenates_byte_arrays_and_unions_maps_and_sets() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let eval = |src: &str| {
            // `source_to_adt` yields the typed `Closed`; the consuming `From` is how a term reaches
            // the reducer, so the test crosses the same boundary the runtime does.
            let p: Par = crate::normalizer::source_to_adt(src).expect("parse").into();
            eval_single_expr(&p, &e, &cost)
        };

        // Byte arrays concatenate (`GByteArray(lhs) ++ GByteArray(rhs)`). Built as an AST rather
        // than parsed: rholang has no byte-array *literal* (the BNFC `Ground` is Bool/Int/BigInt/
        // String/Uri only), so a byte array reaches the reducer from a native such as
        // `keccak256Hash` — which the corpus exercises, and which has no syntax to write here.
        let bytes = from_expr(Expr::EPlusPlus(
            Box::new(from_expr(Expr::GByteArray(vec![97, 98]))),
            Box::new(from_expr(Expr::GByteArray(vec![99, 100]))),
        ));
        assert_eq!(
            eval_single_expr(&bytes, &e, &cost).expect("byte arrays"),
            Expr::GByteArray(vec![97, 98, 99, 100])
        );

        // Sets union and are re-canonicalised (the duplicate `2` collapses).
        let set = eval("Set(1, 2) ++ Set(2, 3)").expect("set union");
        let Expr::ESet(s) = &set else {
            panic!("expected a set, got {set:?}")
        };
        assert_eq!(s.ps.len(), 3, "the duplicate element is not kept twice");

        // Maps union, right-biased on a colliding key (Scala `baseMap ++ otherMap`).
        let Expr::EMap(m) = eval("{\"a\": 1} ++ {\"a\": 2}").expect("map union") else {
            panic!("expected a map")
        };
        assert_eq!(m.kvs.len(), 1, "one key, not two");
        assert_eq!(m.kvs[0].1, from_expr(Expr::GInt(2)), "the other map wins");

        // A mismatched operand reports the type the *left* operand established, as Scala's
        // `OperatorExpectedError` does for each of its four arms. The variant is asserted rather
        // than the message, because the message does **not** contain `expected`: Scala's
        // `OperatorExpectedError` formats identically to `OperatorNotDefined` (both are
        // "Operator `op` is not defined on type."), and the port is faithful to that — so a
        // `contains("Set")` assertion would be testing the port's *unfaithfulness*.
        for (src, expected) in [
            ("Set(1) ++ [2]", "Set"),
            ("{\"a\": 1} ++ 3", "Map"),
            ("[1] ++ 3", "List"),
            ("\"a\" ++ 3", "String"),
        ] {
            let err = eval(src).expect_err("a mismatched operand must be a type error");
            match &err {
                RholangError::OperatorExpectedError {
                    op,
                    expected: got,
                    other_type,
                } => {
                    assert_eq!(op, "++", "{src}: wrong operator");
                    assert_eq!(got, expected, "{src}: wrong expected type");
                    assert!(!other_type.is_empty(), "{src}: the offending type is named");
                }
                other => panic!("{src}: expected an OperatorExpectedError, got {other:?}"),
            }
        }

        // A `ByteArray` left operand has no arm in Scala either (the four arms are String, List,
        // Map, Set), so it falls through to `OperatorNotDefined` — the same shape here.
        let err = eval("3 ++ 4").expect_err("int ++ int");
        assert!(
            matches!(&err, RholangError::OperatorNotDefined { op, .. } if op == "++"),
            "{err:?}"
        );
    }

    /// `+` and `-` are not only arithmetic: Scala's `EPlusBody` has a `(lhs: ESetBody, rhs)` arm
    /// that inserts, and `EMinusBody` has arms for both sets and maps that remove — each calling the
    /// *same* `add`/`delete` the corresponding method does. The port had neither, so
    /// `Set(1, 2) + 3`, `Set(1, 2) - 1` and `{"a": 1} - "a"` — all valid rholang — errored as
    /// `OperatorNotDefined`, which is the failure `convenience_methods_test.rho` reports.
    #[tokio::test]
    async fn plus_and_minus_also_insert_into_and_delete_from_collections() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let eval = |src: &str| {
            let p: Par = crate::normalizer::source_to_adt(src).expect("parse").into();
            eval_single_expr(&p, &e, &cost)
        };
        let set_len = |src: &str| match eval(src).expect("a set") {
            Expr::ESet(s) => s.ps.len(),
            other => panic!("expected a set, got {other:?}"),
        };

        // Insert: a new element grows the set, an existing one does not (it is re-canonicalised).
        assert_eq!(set_len("Set(1, 2) + 3"), 3);
        assert_eq!(
            set_len("Set(1, 2) + 1"),
            2,
            "inserting a duplicate is a no-op"
        );
        // Delete: from a set, and from a map by key.
        assert_eq!(set_len("Set(1, 2, 3) - 2"), 2);
        assert_eq!(
            set_len("Set(1, 2) - 9"),
            2,
            "deleting an absent element is a no-op"
        );
        let Expr::EMap(m) = eval("{\"a\": 1, \"b\": 2} - \"a\"").expect("a map") else {
            panic!("expected a map")
        };
        assert_eq!(m.kvs.len(), 1);
        assert_eq!(m.kvs[0].0, from_expr(Expr::GString("b".to_string())));

        // Arithmetic is untouched (the collection arm is checked first, so this is the arm order).
        assert_eq!(eval("1 + 2").expect("int sum"), Expr::GInt(3));
        assert_eq!(eval("5 - 3").expect("int difference"), Expr::GInt(2));
        // And a non-integer, non-collection operand still reports the right error per operand:
        // `GInt` + other is `OperatorExpectedError`, anything else is `OperatorNotDefined`.
        assert!(matches!(
            eval("1 + \"a\"").expect_err("int + string"),
            RholangError::OperatorExpectedError { ref op, .. } if op == "+"
        ));
        assert!(matches!(
            eval("\"a\" + 1").expect_err("string + int"),
            RholangError::OperatorNotDefined { ref op, .. } if op == "+"
        ));
    }

    #[test]
    fn division_and_modulo_by_zero_are_errors() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();

        let div_zero = from_expr(Expr::EDiv(
            Box::new(from_expr(Expr::GInt(1))),
            Box::new(from_expr(Expr::GInt(0))),
        ));
        assert!(eval_single_expr(&div_zero, &e, &cost).is_err());

        let mod_zero = from_expr(Expr::EMod(
            Box::new(from_expr(Expr::GInt(1))),
            Box::new(from_expr(Expr::GInt(0))),
        ));
        assert!(eval_single_expr(&mod_zero, &e, &cost).is_err());

        let big_div_zero = from_expr(Expr::EDiv(
            Box::new(from_expr(Expr::GBigInt(BigInt::from(1i64)))),
            Box::new(from_expr(Expr::GBigInt(BigInt::from(0i64)))),
        ));
        assert!(eval_single_expr(&big_div_zero, &e, &cost).is_err());

        let big_mod_zero = from_expr(Expr::EMod(
            Box::new(from_expr(Expr::GBigInt(BigInt::from(1i64)))),
            Box::new(from_expr(Expr::GBigInt(BigInt::from(0i64)))),
        ));
        assert!(eval_single_expr(&big_mod_zero, &e, &cost).is_err());
    }

    #[test]
    fn division_and_modulo_overflow_promote_to_bigint() {
        // RCHIP #51: `i64::MIN / -1` and `i64::MIN % -1` overflow `i64`; they are now computed
        // exactly (2^63 and 0) instead of erroring.
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();

        let div_overflow = from_expr(Expr::EDiv(
            Box::new(from_expr(Expr::GInt(i64::MIN))),
            Box::new(from_expr(Expr::GInt(-1))),
        ));
        assert_eq!(
            eval_single_expr(&div_overflow, &e, &cost).unwrap(),
            Expr::GBigInt(-BigInt::from(i64::MIN))
        );

        let mod_overflow = from_expr(Expr::EMod(
            Box::new(from_expr(Expr::GInt(i64::MIN))),
            Box::new(from_expr(Expr::GInt(-1))),
        ));
        assert_eq!(
            eval_single_expr(&mod_overflow, &e, &cost).unwrap(),
            Expr::GBigInt(BigInt::from(0))
        );
    }

    #[test]
    fn integer_arithmetic_never_wraps_and_mixes_types() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let i = |v: i64| from_expr(Expr::GInt(v));
        let bi = |v: i64| from_expr(Expr::GBigInt(BigInt::from(v)));
        let eval = |ex: Expr| eval_expr_to_expr(&ex, &e, &cost).unwrap();

        // `i64::MAX + 1` promotes instead of wrapping to `i64::MIN`.
        assert_eq!(
            eval(Expr::EPlus(Box::new(i(i64::MAX)), Box::new(i(1)))),
            Expr::GBigInt(BigInt::from(i64::MAX) + BigInt::from(1))
        );
        // `i64::MIN - 1` promotes instead of wrapping to `i64::MAX`.
        assert_eq!(
            eval(Expr::EMinus(Box::new(i(i64::MIN)), Box::new(i(1)))),
            Expr::GBigInt(BigInt::from(i64::MIN) - BigInt::from(1))
        );
        // `i64::MAX * 2` promotes instead of wrapping.
        assert_eq!(
            eval(Expr::EMult(Box::new(i(i64::MAX)), Box::new(i(2)))),
            Expr::GBigInt(BigInt::from(i64::MAX) * BigInt::from(2))
        );
        // `-i64::MIN` promotes instead of staying `i64::MIN`.
        assert_eq!(
            eval(Expr::ENeg(Box::new(i(i64::MIN)))),
            Expr::GBigInt(-BigInt::from(i64::MIN))
        );
        // Small results stay `Int` on the fast path.
        assert_eq!(
            eval(Expr::EPlus(Box::new(i(2)), Box::new(i(3)))),
            Expr::GInt(5)
        );

        // Mixed `Int`/`BigInt` operands work (the small operand promotes).
        assert_eq!(
            eval(Expr::EPlus(Box::new(i(2)), Box::new(bi(3)))),
            Expr::GBigInt(BigInt::from(5))
        );
        assert_eq!(
            eval(Expr::EMult(Box::new(bi(3)), Box::new(i(4)))),
            Expr::GBigInt(BigInt::from(12))
        );

        // Comparisons and equality span both integer forms.
        assert_eq!(
            eval(Expr::ELt(Box::new(i(1)), Box::new(bi(2)))),
            Expr::GBool(true)
        );
        assert_eq!(
            eval(Expr::EGte(Box::new(bi(2)), Box::new(i(2)))),
            Expr::GBool(true)
        );
        assert_eq!(
            eval(Expr::EEq(Box::new(i(2)), Box::new(bi(2)))),
            Expr::GBool(true)
        );
        assert_eq!(
            eval(Expr::ENeq(Box::new(i(2)), Box::new(bi(3)))),
            Expr::GBool(true)
        );

        // Division by zero is still an error, in both forms.
        assert!(eval_expr_to_expr(&Expr::EDiv(Box::new(i(1)), Box::new(i(0))), &e, &cost).is_err());
        assert!(
            eval_expr_to_expr(&Expr::EDiv(Box::new(i(1)), Box::new(bi(0))), &e, &cost).is_err()
        );
        assert!(
            eval_expr_to_expr(&Expr::EMod(Box::new(i(1)), Box::new(bi(0))), &e, &cost).is_err()
        );
    }

    #[test]
    fn evaluates_boolean_and() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let p = from_expr(Expr::EAnd(
            Box::new(from_expr(Expr::GBool(true))),
            Box::new(from_expr(Expr::GBool(false))),
        ));
        assert_eq!(eval_single_expr(&p, &e, &cost).unwrap(), Expr::GBool(false));
    }

    #[test]
    fn string_append() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let p = from_expr(Expr::EPlusPlus(
            Box::new(from_expr(Expr::GString("a".to_string()))),
            Box::new(from_expr(Expr::GString("b".to_string()))),
        ));
        assert_eq!(
            eval_single_expr(&p, &e, &cost).unwrap(),
            Expr::GString("ab".to_string())
        );
    }

    struct MockSpace {
        produced: Mutex<Vec<(SortedProc, ListParWithRandom, bool)>>,
    }
    #[async_trait]
    impl Tuplespace for MockSpace {
        async fn produce(
            &self,
            channel: &SortedProc,
            data: ListParWithRandom,
            persist: bool,
        ) -> Result<Application, RholangError> {
            self.produced
                .lock()
                .unwrap()
                .push((channel.clone(), data, persist));
            Ok(None)
        }
        async fn consume(
            &self,
            _channels: &[SortedProc],
            _patterns: &[BindPattern],
            _continuation: TaggedContinuation,
            _persist: bool,
            _peeks: BTreeSet<usize>,
        ) -> Result<Application, RholangError> {
            Ok(None)
        }
    }
    struct MockDispatch;
    #[async_trait]
    impl Dispatch for MockDispatch {
        async fn dispatch(
            &self,
            _continuation: TaggedContinuation,
            _data_list: Vec<ListParWithRandom>,
            _path: DfsPath,
        ) -> Result<(), RholangError> {
            Ok(())
        }
    }

    /// **The witness AUDIT C151 found missing: the effect-scheduler *mode* is what enables Law 24's
    /// per-commit certificate.**
    ///
    /// `set_validation_enabled` has exactly one production caller — `set_effect_mode`'s
    /// `mode == EffectMode::RelaxedValidated` — and every other caller in the tree is a test that
    /// sets the flag *directly* on a queue it built (`rspace`'s `channel_queue` tests, this crate's
    /// property tests). So the certificate's behaviour is witnessed and the wiring from the mode to
    /// it was not: replacing that expression with `false` left every test green — measured, 250
    /// rholang and 292 casper lib tests — because a test that never consults the mode cannot notice
    /// that the mode stopped mattering.
    ///
    /// The arms below observe the *behaviour* the wiring controls rather than the flag: a write is
    /// stamped on the channel at the claim's own path — equal is already a violation, since the
    /// certificate requires the newest write to be strictly DFS-earlier — and the acquisition must be
    /// refused under `RelaxedValidated` and accepted under every other mode, with nothing differing
    /// between the arms but the mode.
    #[tokio::test(flavor = "multi_thread")]
    async fn law24_the_effect_mode_is_what_enables_the_certificate() {
        let interp = DebruijnInterpreter::new(
            MockSpace {
                produced: Mutex::new(Vec::new()),
            },
            MockDispatch,
            BTreeMap::new(),
            SortedProc::default(),
        );
        let channel = SortedProc::default();
        let at = vec![0u16];

        interp.set_effect_mode(EffectMode::RelaxedValidated);
        interp.claims.record_write(&channel, &at, true);
        let refused = interp
            .claims
            .claim(at.clone(), &[channel.clone()])
            .wait_at_head()
            .await;
        assert!(
            matches!(refused, Err(AcquireError::ValidationFailed { .. })),
            "the validated mode must refuse a claim whose channel was last written at its own path — \
             this is the certificate the block path turns into the sequential fallback"
        );

        // The control: the same stamp and the same claim, and only the mode differs. All three other
        // modes leave the certificate off — `Relaxed` is the one that matters, and `Sequential` and
        // `Gate` are here so the wiring is pinned as an *equality* with `RelaxedValidated` rather
        // than as a truthiness that any non-default mode would satisfy.
        for mode in [
            EffectMode::Sequential,
            EffectMode::Gate,
            EffectMode::Relaxed,
        ] {
            interp.set_effect_mode(mode);
            interp.claims.reset_write_record();
            interp.claims.record_write(&channel, &at, true);
            assert!(
                interp
                    .claims
                    .claim(at.clone(), &[channel.clone()])
                    .wait_at_head()
                    .await
                    .is_ok(),
                "{mode:?} must not enforce the certificate"
            );
        }
    }

    /// **Law 22 (`next_step_closure_computable`).** The next-step closure is computable *at
    /// dispatch*, from the term alone: `resolve_children` turns each top-level term of a par into one
    /// effect (a produce, a consume, or a nested `Par` to walk) without touching the tuple space, so
    /// a scheduler can compute an effect's static footprint before committing to it. The spike for a
    /// full property harness measured >150 lines (it needs the effect stream the integration tests
    /// already assert on), so this is the structural half: one effect per term, no space I/O, for
    /// every arity from one term to six.
    #[tokio::test]
    async fn law22_the_next_step_closure_is_computable_at_dispatch() {
        let interp = Arc::new(DebruijnInterpreter::new(
            MockSpace {
                produced: Mutex::new(Vec::new()),
            },
            MockDispatch,
            BTreeMap::new(),
            SortedProc::default(),
        ));
        let cost = Arc::new(CostAccounting::from_initial(Costs::unsafe_max()));
        let env = Env::new();
        let rand = Blake2b512Random::from_init(&[0u8; 32]);

        for n in 1..=6usize {
            let sends: Vec<Send> = (0..n)
                .map(|i| Send {
                    chan: Box::new(from_expr(Expr::GInt(i as i64)).quote()),
                    data: vec![from_expr(Expr::GInt(1)).quote()],
                    persistent: false,
                    locally_free: AlwaysEqual(vec![]),
                    connective_used: false,
                })
                .collect();
            let par = Par {
                sends: sends.clone(),
                ..Default::default()
            };

            let effects = interp
                .resolve_children(&par, &env, &rand, &cost)
                .await
                .expect("the closure resolves");
            assert_eq!(
                effects.len(),
                n,
                "one effect per top-level term ({n} term(s))"
            );
            assert!(
                interp.space.produced.lock().unpoison().is_empty(),
                "computing the closure must not touch the tuple space"
            );
        }
    }

    #[tokio::test]
    async fn eval_send_produces_on_evaluated_channel() {
        let space = MockSpace {
            produced: Mutex::new(Vec::new()),
        };
        let interp = Arc::new(DebruijnInterpreter::new(
            space,
            MockDispatch,
            BTreeMap::new(),
            SortedProc::default(),
        ));
        let cost = Arc::new(CostAccounting::from_initial(Costs::unsafe_max()));
        let env = Env::new();
        let rand = Blake2b512Random::new_random(128);

        let send = Send {
            chan: Box::new(from_expr(Expr::GInt(1)).quote()),
            data: vec![from_expr(Expr::GInt(2)).quote()],
            persistent: false,
            locally_free: AlwaysEqual(vec![]),
            connective_used: false,
        };
        let par = Par {
            sends: vec![send],
            ..Default::default()
        };
        interp.clone().eval(&par, &env, &rand, &cost).await.unwrap();

        let produced = interp.space.produced.lock().unpoison();
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].0.as_par().exprs, vec![Expr::GInt(1)]);
        assert_eq!(
            produced[0].1.pars,
            vec![SortedProc::new(from_expr(Expr::GInt(2)))]
        );
        assert!(!produced[0].2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_term_resolution_matches_sequential_count() {
        // Many independent sends resolve concurrently (the fork-join in `reduce_par`); the produce
        // effects are applied in DFS order, so none are lost or duplicated.
        let space = MockSpace {
            produced: Mutex::new(Vec::new()),
        };
        let interp = Arc::new(DebruijnInterpreter::new(
            space,
            MockDispatch,
            BTreeMap::new(),
            SortedProc::default(),
        ));
        let cost = Arc::new(CostAccounting::from_initial(Costs::unsafe_max()));
        let env = Env::new();
        let rand = Blake2b512Random::new_random(128);
        let sends: Vec<Send> = (0..64)
            .map(|i| Send {
                chan: Box::new(from_expr(Expr::GInt(i)).quote()),
                data: vec![from_expr(Expr::GInt(i + 1000)).quote()],
                persistent: false,
                locally_free: AlwaysEqual(vec![]),
                connective_used: false,
            })
            .collect();
        let par = Par {
            sends,
            ..Default::default()
        };
        interp.clone().eval(&par, &env, &rand, &cost).await.unwrap();

        let produced = interp.space.produced.lock().unpoison();
        assert_eq!(produced.len(), 64);
    }

    #[test]
    fn byte_methods_round_trip() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let target = from_expr(Expr::GString("ab".to_string()));
        let bytes = eval_method("toUtf8Bytes", &target, &[], &e, &cost).unwrap();
        let hex = eval_method("bytesToHex", &bytes, &[], &e, &cost).unwrap();
        assert_eq!(hex, from_expr(Expr::GString("6162".to_string())));
    }

    /// **`hexToBytes` reads bytes, and a multi-byte character is a refusal rather than a panic**
    /// (AUDIT C252).
    ///
    /// The falsifier for the byte-index defect, and it is the *panic* half that makes it one: `"aéa"`
    /// is four bytes (1 + 2 + 1), so the even-length check passes and the old `&s[i..i + 2]` took a
    /// window starting inside `é` — a slice off a character boundary, which **panics**. A stranger
    /// could do that with a deploy, on any node, from the public deploy endpoint.
    ///
    /// The test asserts the *method's own refusal* rather than the absence of a crash, because "does
    /// not panic" is satisfied by any wrong answer: the term is not hex, and the answer must say so in
    /// the shape every other bad `hexToBytes` input gets.
    #[test]
    fn hex_to_bytes_reads_bytes_so_a_multibyte_character_is_refused_not_a_panic() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let call = |s: &str| {
            eval_method(
                "hexToBytes",
                &from_expr(Expr::GString(s.to_string())),
                &[],
                &e,
                &cost,
            )
        };

        // Valid hex is unchanged, byte for byte — including upper case, which the radix parser
        // accepted and `to_digit` accepts too.
        assert_eq!(
            call("6162").unwrap(),
            from_expr(Expr::GByteArray(vec![0x61, 0x62]))
        );
        assert_eq!(
            call("ABab").unwrap(),
            from_expr(Expr::GByteArray(vec![0xab, 0xab]))
        );

        // The falsifier: this panicked before the fix.
        let refused = call("aéa").expect_err("a multi-byte character is not hex");
        assert!(
            matches!(refused, RholangError::ReduceError(_)),
            "and it is the method's own refusal, not a panic: {refused:?}"
        );

        // An odd *byte* length is still a refusal, and still not a panic.
        assert!(call("abc").is_err(), "three bytes cannot be a byte string");

        // The one behaviour difference the fix introduces, stated rather than discovered: the radix
        // parser accepted a leading `+` and `to_digit` does not, so `"+1"` is now a refusal. It is not
        // hex, and refusing it is the stricter and correct answer.
        assert!(call("+1").is_err(), "a sign is not a hex digit");
    }

    /// **The four logical-connective bodies, and both short-circuits.** `EShortAnd`/`EShortOr` were
    /// never *evaluated* by any test: the sort corpus (law 1a's rows 23 and the boundary list) uses
    /// them as terms to order, and nothing ran `1 && 2`. The failure arm is the short-circuit itself,
    /// and it is asserted the only way it is observable — the side that must be skipped is a division
    /// by zero, so an implementation that evaluated both operands eagerly answers `Err` where this
    /// expects a boolean. That is the defect the arm's `if b1 { … } else { false }` exists to prevent,
    /// and the reason `&&`/`||` cannot be desugared to `EAnd`/`EOr`.
    #[test]
    fn the_logical_connectives_short_circuit() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let b = |v: bool| from_expr(Expr::GBool(v));
        let int = |v: i64| from_expr(Expr::GInt(v));
        let div_by_zero = || Box::new(from_expr(Expr::EDiv(Box::new(int(1)), Box::new(int(0)))));
        let eval = |ex: Expr| eval_expr_to_expr(&ex, &e, &cost);

        // `&&`: `true && x` is `x`; `false && x` is `false` without touching `x`.
        assert_eq!(
            eval(Expr::EShortAnd(Box::new(b(true)), Box::new(b(false)))).unwrap(),
            Expr::GBool(false)
        );
        assert_eq!(
            eval(Expr::EShortAnd(Box::new(b(true)), Box::new(b(true)))).unwrap(),
            Expr::GBool(true)
        );
        assert_eq!(
            eval(Expr::EShortAnd(Box::new(b(false)), div_by_zero())).unwrap(),
            Expr::GBool(false),
            "a false left operand must skip the right one — reaching the division by zero is an error"
        );

        // `||`: `false || x` is `x`; `true || x` is `true` without touching `x`.
        assert_eq!(
            eval(Expr::EShortOr(Box::new(b(false)), Box::new(b(true)))).unwrap(),
            Expr::GBool(true)
        );
        assert_eq!(
            eval(Expr::EShortOr(Box::new(b(false)), Box::new(b(false)))).unwrap(),
            Expr::GBool(false)
        );
        assert_eq!(
            eval(Expr::EShortOr(Box::new(b(true)), div_by_zero())).unwrap(),
            Expr::GBool(true),
            "a true left operand must skip the right one"
        );
    }

    /// **`toInt` and `toBigInt`, every arm including both refusals.** The refusals are the point: a
    /// string that is not a number and a `BigInt` outside `i64` must be *reported*, not truncated or
    /// wrapped, and the message names the input it could not convert — the difference between a
    /// diagnostic and a shrug. `toInt` is also not defined on a boolean, which is a third refusal.
    #[test]
    fn the_int_conversions_report_what_they_cannot_convert() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let s = |v: &str| from_expr(Expr::GString(v.to_string()));

        // The identity and promoting arms.
        assert_eq!(
            eval_method("toInt", &from_expr(Expr::GInt(3)), &[], &e, &cost).unwrap(),
            from_expr(Expr::GInt(3))
        );
        assert_eq!(
            eval_method(
                "toBigInt",
                &from_expr(Expr::GBigInt(BigInt::from(3))),
                &[],
                &e,
                &cost
            )
            .unwrap(),
            from_expr(Expr::GBigInt(BigInt::from(3)))
        );
        assert_eq!(
            eval_method("toInt", &s("42"), &[], &e, &cost).unwrap(),
            from_expr(Expr::GInt(42))
        );
        assert_eq!(
            eval_method("toBigInt", &from_expr(Expr::GInt(7)), &[], &e, &cost).unwrap(),
            from_expr(Expr::GBigInt(BigInt::from(7)))
        );
        assert_eq!(
            eval_method("toBigInt", &s("42"), &[], &e, &cost).unwrap(),
            from_expr(Expr::GBigInt(BigInt::from(42)))
        );

        // The refusals.
        let err = eval_method("toInt", &s("nope"), &[], &e, &cost)
            .expect_err("a non-numeric string is not an Int");
        assert!(
            format!("{err:?}").contains("nope"),
            "the refusal names the input it could not convert: {err:?}"
        );
        let too_big = BigInt::from(i64::MAX) + BigInt::from(1);
        assert!(
            eval_method("toInt", &from_expr(Expr::GBigInt(too_big)), &[], &e, &cost).is_err(),
            "a BigInt outside i64 is refused, not truncated"
        );
        assert!(eval_method("toBigInt", &s("nope"), &[], &e, &cost).is_err());
        assert!(
            eval_method("toInt", &from_expr(Expr::GBool(true)), &[], &e, &cost).is_err(),
            "`toInt` is not defined on a boolean"
        );
    }

    /// **`take`, the list arm and its two refusals.** The zero/negative case is the one the arm's own
    /// comment calls out: `restrict_to_int` wraps a negative index to a huge `usize`, so `take(-1)`
    /// would return the *whole* list — and mint a negative cost for the privilege — without the
    /// explicit `if n_i <= 0 { 0 }` guard. The refusals are a receiver that is not a list and a
    /// non-integer argument.
    #[test]
    fn take_slices_a_list_and_refuses_what_it_is_not_defined_on() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let list = || {
            from_expr(Expr::EList(EList {
                ps: vec![
                    from_expr(Expr::GInt(1)),
                    from_expr(Expr::GInt(2)),
                    from_expr(Expr::GInt(3)),
                ],
                ..Default::default()
            }))
        };
        let take = |n: i64| eval_method("take", &list(), &[from_expr(Expr::GInt(n))], &e, &cost);
        let len = |p: &Par| match single_expr(p).unwrap() {
            Expr::EList(l) => l.ps.len(),
            other => panic!("expected a list, got {other:?}"),
        };

        assert_eq!(len(&take(2).unwrap()), 2, "the first two elements");
        assert_eq!(len(&take(0).unwrap()), 0, "take(0) is empty");
        assert_eq!(
            len(&take(-1).unwrap()),
            0,
            "a negative count is empty, not the whole list"
        );
        assert_eq!(
            len(&take(5).unwrap()),
            3,
            "taking more than there is takes all"
        );

        let set = from_expr(Expr::ESet(par_set(vec![from_expr(Expr::GInt(1))])));
        assert!(
            eval_method("take", &set, &[from_expr(Expr::GInt(1))], &e, &cost).is_err(),
            "`take` is a list method; a set is refused by name"
        );
        assert!(
            eval_method(
                "take",
                &list(),
                &[from_expr(Expr::GString("x".into()))],
                &e,
                &cost
            )
            .is_err(),
            "the count must be an integer"
        );
    }

    /// **Every arm's wrong-receiver refusal, and the name it refuses by.** That a call on the wrong
    /// kind of thing is *reported* — with the method's name and the receiver's type — rather than
    /// answered with something plausible is the port's convention, and it was never asserted: the
    /// existing tests pin the values the arms produce, and the uncovered lines in `eval_method` are
    /// exactly these refusals. Two shapes produce them, and the table covers both without caring
    /// which: an arm may end in its own `other => Err(method_not_defined(<name>, &other))`
    /// (`toList`, `toMap`, `take`, `nth`-style collection arms), or it may route through a shared
    /// helper (`string_arg` → `expect_string` → `method_not_defined`, which is how the thirteen
    /// string methods refuse). What is asserted is therefore the *contract* — `MethodNotDefined`
    /// carrying this method's name and the receiver's type — and a refusal that named the wrong
    /// method, dropped the type, or stopped refusing fails here. (One row per arm, so an arm that
    /// gains no refusal is one row's worth of visible absence rather than a number.)
    #[test]
    fn every_method_refuses_a_receiver_it_is_not_defined_on() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let boolean = from_expr(Expr::GBool(true));
        let int = |v: i64| from_expr(Expr::GInt(v));
        let s = |v: &str| from_expr(Expr::GString(v.to_string()));
        let one_set = || from_expr(Expr::ESet(par_set(vec![from_expr(Expr::GInt(1))])));

        let cases: Vec<(&str, Vec<Par>)> = vec![
            ("toList", vec![]),
            ("toSet", vec![]),
            ("toMap", vec![]),
            ("length", vec![]),
            ("size", vec![]),
            ("keys", vec![]),
            ("get", vec![int(0)]),
            ("getOrElse", vec![int(0), int(1)]),
            ("set", vec![s("k"), int(1)]),
            ("add", vec![int(1)]),
            ("diff", vec![one_set()]),
            ("union", vec![one_set()]),
            ("contains", vec![int(1)]),
            ("take", vec![int(1)]),
            ("slice", vec![int(0), int(1)]),
            ("toLowerCase", vec![]),
            ("toUpperCase", vec![]),
            ("capitalize", vec![]),
            ("reverse", vec![]),
            ("trim", vec![]),
            ("startsWith", vec![s("a")]),
            ("endsWith", vec![s("a")]),
            ("indexOf", vec![s("a")]),
            ("split", vec![s("a")]),
            ("replace", vec![s("a"), s("b")]),
            ("hexToBytes", vec![]),
            ("toUtf8Bytes", vec![]),
            ("bytesToHex", vec![]),
        ];

        for (method, args) in cases {
            match eval_method(method, &boolean, &args, &e, &cost) {
                Err(RholangError::MethodNotDefined {
                    method: got,
                    other_type,
                }) => {
                    assert_eq!(got, method, "the refusal names the method that was called");
                    assert_eq!(
                        other_type, "Bool",
                        "and the type it is not defined on (`{method}`)"
                    );
                }
                other => {
                    panic!("`{method}` on a Bool is refused by name, never answered: {other:?}")
                }
            }
        }
    }

    /// **`toSet`/`toMap`: the conversions and the non-pair refusal.** `toSet` takes a set as itself, a
    /// map's `(k, v)` pairs, or a list's elements; `toMap` takes a map as itself and a collection *of
    /// pairs* — and its failure arm is the one worth having: a member that is not a two-element tuple
    /// is refused (`unapply_tuple2` returning `None`), rather than a map being built from half-pairs.
    #[test]
    fn the_collection_conversions_convert_and_refuse_non_pairs() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let int = |v: i64| from_expr(Expr::GInt(v));
        let pair = |a: i64, b: i64| make_tuple(int(a), int(b));
        let set_of = |ps: Vec<Par>| from_expr(Expr::ESet(par_set(ps)));
        let list_of = |ps: Vec<Par>| {
            from_expr(Expr::EList(EList {
                ps,
                ..Default::default()
            }))
        };

        // toSet: a list's elements, and a map's pairs.
        match single_expr(
            &eval_method("toSet", &list_of(vec![int(1), int(2)]), &[], &e, &cost).unwrap(),
        )
        .unwrap()
        {
            Expr::ESet(s) => assert_eq!(
                s.ps.len(),
                2,
                "a two-element list becomes a two-element set"
            ),
            other => panic!("expected a set, got {other:?}"),
        }
        match single_expr(
            &eval_method(
                "toSet",
                &from_expr(Expr::EMap(par_map(vec![(int(1), int(2))]))),
                &[],
                &e,
                &cost,
            )
            .unwrap(),
        )
        .unwrap()
        {
            Expr::ESet(s) => assert_eq!(s.ps.len(), 1, "a map becomes a set of its pairs"),
            other => panic!("expected a set, got {other:?}"),
        }

        // toMap: a set of pairs, and a list of pairs.
        match single_expr(
            &eval_method(
                "toMap",
                &set_of(vec![pair(1, 2), pair(3, 4)]),
                &[],
                &e,
                &cost,
            )
            .unwrap(),
        )
        .unwrap()
        {
            Expr::EMap(m) => assert_eq!(m.kvs.len(), 2, "two pairs become a two-entry map"),
            other => panic!("expected a map, got {other:?}"),
        }
        assert!(
            eval_method(
                "toMap",
                &list_of(vec![pair(1, 2), pair(3, 4)]),
                &[],
                &e,
                &cost
            )
            .is_ok(),
            "a list of pairs converts too"
        );

        // The failure arms: a member that is not a pair, from a set and from a list.
        match eval_method("toMap", &set_of(vec![int(1), int(2)]), &[], &e, &cost) {
            Err(RholangError::MethodNotDefined { method, other_type }) => {
                assert_eq!(method, "toMap");
                assert_eq!(
                    other_type, "Set",
                    "the refusal names what it could not convert"
                );
            }
            other => panic!("a set of non-pairs is refused, not half-converted: {other:?}"),
        }
        assert!(
            eval_method("toMap", &list_of(vec![int(1), int(2)]), &[], &e, &cost).is_err(),
            "and so is a list of non-pairs"
        );
    }

    /// **`nth`: the value, both out-of-bound refusals, and the wrong-receiver message.** `nth` is the
    /// one arm whose refusal is *not* `MethodNotDefined` — it answers `ReduceError("Error: index out of
    /// bound: {n}")` or the "wasn't a list or tuple" message — so its three arms are pinned here
    /// rather than in the refusal table above. The byte-array arm is reached through `toUtf8Bytes`,
    /// which is the only way to build one from a test without guessing the constructor.
    #[test]
    fn nth_reports_an_out_of_bound_index_and_a_wrong_receiver() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let int = |v: i64| from_expr(Expr::GInt(v));
        let list = from_expr(Expr::EList(EList {
            ps: vec![int(1), int(2), int(3)],
            ..Default::default()
        }));

        assert_eq!(
            eval_method("nth", &list, &[int(1)], &e, &cost).unwrap(),
            int(2),
            "nth(1) is the second element"
        );
        let out_of_bound = eval_method("nth", &list, &[int(9)], &e, &cost)
            .expect_err("index 9 of a three-element list");
        assert!(
            format!("{out_of_bound:?}").contains("index out of bound: 9"),
            "the refusal names the index: {out_of_bound:?}"
        );

        // The byte-array arm, both directions.
        let bytes = eval_method(
            "toUtf8Bytes",
            &from_expr(Expr::GString("ab".to_string())),
            &[],
            &e,
            &cost,
        )
        .unwrap();
        assert_eq!(
            eval_method("nth", &bytes, &[int(0)], &e, &cost).unwrap(),
            int(97)
        );
        assert!(
            eval_method("nth", &bytes, &[int(9)], &e, &cost).is_err(),
            "past the end of a byte array"
        );

        // The wrong receiver, whose message says what it *was*: this is the message a rholang
        // programmer sees when they index a boolean.
        let wrong = eval_method("nth", &from_expr(Expr::GBool(true)), &[int(0)], &e, &cost)
            .expect_err("nth on a Bool");
        assert!(
            format!("{wrong:?}").contains("wasn't a list or tuple"),
            "the refusal says `nth` needs a list or tuple: {wrong:?}"
        );
    }

    #[test]
    fn set_union() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let s1 = from_expr(Expr::ESet(par_set(vec![
            from_expr(Expr::GInt(1)),
            from_expr(Expr::GInt(2)),
        ])));
        let s2 = from_expr(Expr::ESet(par_set(vec![
            from_expr(Expr::GInt(2)),
            from_expr(Expr::GInt(3)),
        ])));
        let result = eval_method("union", &s1, &[s2], &e, &cost).unwrap();
        match single_expr(&result).unwrap() {
            Expr::ESet(set) => assert_eq!(set.ps.len(), 3),
            _ => panic!("expected a set"),
        }
    }

    #[test]
    fn set_difference_operator() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let s1 = from_expr(Expr::ESet(par_set(vec![
            from_expr(Expr::GInt(1)),
            from_expr(Expr::GInt(2)),
        ])));
        let s2 = from_expr(Expr::ESet(par_set(vec![
            from_expr(Expr::GInt(2)),
            from_expr(Expr::GInt(3)),
        ])));
        let diff =
            eval_expr_to_expr(&Expr::EMinusMinus(Box::new(s1), Box::new(s2)), &e, &cost).unwrap();
        match diff {
            Expr::ESet(set) => {
                assert_eq!(set.ps.len(), 1);
                assert_eq!(set.ps[0], from_expr(Expr::GInt(1)));
            }
            _ => panic!("expected a set"),
        }
    }

    /// **`shiftLeft` is charged on the full width of the integer it builds**, including when the
    /// operand's top bits and the shift cross a byte boundary together: `0x7f << 1` is 8 bits, two
    /// bytes by `big_int_size`, where `big_int_size(0x7f) + 1 / 8` said one.
    #[test]
    fn shift_left_is_charged_on_the_results_size() {
        for x in [
            0,
            1_i64,
            0x7f,
            0x80,
            0xff,
            0x7fff,
            -1,
            -0x80,
            i64::MAX,
            i64::MIN,
        ] {
            let big = BigInt::from(x);
            for n in 0..=70_u64 {
                assert_eq!(
                    Costs::shift_left_result_size(big.magnitude().bits(), n),
                    Costs::big_int_size(&(big.clone() << n as usize)),
                    "x = {x}, n = {n}"
                );
            }
        }
        assert_eq!(Costs::shift_left_result_size(7, 1), 2);
        assert_eq!(Costs::shift_left_result_size(7, 0), 1);
        assert_eq!(Costs::shift_left_result_size(0, 8), 1);
        assert_eq!(Costs::shift_left_result_size(0, u64::MAX), 1);

        // At the call site: a `BigInt` operand takes the charged path, and is charged exactly the
        // result's size.
        let e = Env::new();
        let charged = |x: &str, n: i64| {
            let cost = CostAccounting::from_initial(Costs::unsafe_max());
            let before = cost.total_charged();
            let target = from_expr(Expr::GBigInt(x.parse::<BigInt>().unwrap()));
            let out = eval_method("shiftLeft", &target, &[from_expr(Expr::GInt(n))], &e, &cost)
                .expect("shiftLeft");
            let Some(Expr::GBigInt(r)) = single_expr(&out) else {
                panic!("expected a BigInt")
            };
            (cost.total_charged() - before, Costs::big_int_size(r))
        };
        for (x, n) in [
            ("0", 8),
            ("0", 1_000_000),
            ("127", 1),
            ("127", 9),
            ("255", 1),
            ("1", 7),
            ("1", 8),
            ("-128", 1),
        ] {
            let (paid, size) = charged(x, n);
            assert_eq!(paid, size, "{x}.shiftLeft({n})");
        }
    }

    #[test]
    fn bit_operations_from_rchip_12() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let i = |v: i64| from_expr(Expr::GInt(v));
        let n = |v: &str| from_expr(Expr::GBigInt(v.parse::<BigInt>().unwrap()));
        let bytes = |v: &[u8]| from_expr(Expr::GByteArray(v.to_vec()));
        let call = |m: &str, target: &Par, args: &[Par]| eval_method(m, target, args, &e, &cost);
        let ok = |m: &str, target: &Par, args: &[Par]| call(m, target, args).expect(m);

        // &, |, ^, ~ on Int
        assert_eq!(ok("bitAnd", &i(0b1100), &[i(0b1010)]), i(0b1000));
        assert_eq!(ok("bitOr", &i(0b1100), &[i(0b1010)]), i(0b1110));
        assert_eq!(ok("bitXor", &i(0b1100), &[i(0b1010)]), i(0b0110));
        assert_eq!(ok("bitNot", &i(0), &[]), i(-1));
        assert_eq!(ok("bitAnd", &i(-1), &[i(0xff)]), i(0xff));
        // a BigInt operand promotes, in two's complement
        assert_eq!(ok("bitAnd", &n("-1"), &[i(0xff)]), n("255"));
        assert_eq!(
            ok("bitOr", &n("18446744073709551616"), &[i(1)]),
            n("18446744073709551617")
        );
        assert_eq!(ok("bitNot", &n("5"), &[]), n("-6"));
        // ByteArray: byte by byte, equal lengths only
        assert_eq!(
            ok("bitXor", &bytes(&[0xf0, 0x0f]), &[bytes(&[0xff, 0xff])]),
            bytes(&[0x0f, 0xf0])
        );
        assert_eq!(
            ok("bitNot", &bytes(&[0x00, 0xa5]), &[]),
            bytes(&[0xff, 0x5a])
        );
        assert!(call("bitAnd", &bytes(&[1]), &[bytes(&[1, 2])]).is_err());
        assert!(call("bitAnd", &bytes(&[1]), &[i(1)]).is_err());
        assert!(call("bitAnd", &i(1), &[bytes(&[1])]).is_err());

        // << is exact: it promotes rather than wrapping, and the count is never masked
        assert_eq!(ok("shiftLeft", &i(1), &[i(10)]), i(1024));
        assert_eq!(ok("shiftLeft", &i(-3), &[i(2)]), i(-12));
        assert_eq!(ok("shiftLeft", &i(1), &[i(62)]), i(1 << 62));
        assert_eq!(ok("shiftLeft", &i(1), &[i(63)]), n("9223372036854775808"));
        assert_eq!(ok("shiftLeft", &i(1), &[i(64)]), n("18446744073709551616"));
        assert_eq!(ok("shiftLeft", &i(-1), &[i(63)]), i(i64::MIN));
        assert_eq!(ok("shiftLeft", &i(0), &[i(1_000_000)]), i(0));
        assert!(call("shiftLeft", &i(1), &[i(-1)]).is_err());
        // >> is arithmetic: floor(x / 2^n)
        assert_eq!(ok("shiftRight", &i(1024), &[i(3)]), i(128));
        assert_eq!(ok("shiftRight", &i(-7), &[i(1)]), i(-4));
        assert_eq!(ok("shiftRight", &i(-7), &[i(200)]), i(-1));
        assert_eq!(ok("shiftRight", &i(7), &[i(200)]), i(0));
        assert_eq!(ok("shiftRight", &n("-7"), &[i(1)]), n("-4"));
        assert_eq!(
            ok("shiftRight", &n("18446744073709551616"), &[i(64)]),
            n("1")
        );
        // >>> is the 64-bit logical shift
        assert_eq!(ok("shiftRightUnsigned", &i(-1), &[i(60)]), i(0xf));
        assert_eq!(ok("shiftRightUnsigned", &i(-1), &[i(64)]), i(0));
        assert!(call("shiftRightUnsigned", &n("18446744073709551616"), &[i(1)]).is_err());

        // the RCHIP's motivating case: floor(log2(x)) by shifting
        let mut x = 1000_i64;
        let mut log2 = -1;
        while x > 0 {
            x = match single_expr(&ok("shiftRight", &i(x), &[i(1)])) {
                Some(Expr::GInt(v)) => *v,
                _ => unreachable!(),
            };
            log2 += 1;
        }
        assert_eq!(log2, 9);
    }

    #[test]
    fn string_methods_from_rchip_37() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let s = |v: &str| from_expr(Expr::GString(v.to_string()));
        let i = |v: i64| from_expr(Expr::GInt(v));
        let b = |v: bool| from_expr(Expr::GBool(v));
        let call = |m: &str, target: &Par, args: &[Par]| {
            eval_method(m, target, args, &e, &cost).expect("method call")
        };

        // substring(i) / substring(i, j)
        assert_eq!(call("substring", &s("hello"), &[i(1)]), s("ello"));
        assert_eq!(call("substring", &s("hello"), &[i(1), i(3)]), s("el"));
        assert_eq!(call("substring", &s("hello"), &[i(3), i(1)]), s(""));
        // length counts characters, not UTF-8 bytes
        assert_eq!(call("length", &s("héllo"), &[]), i(5));
        // indexOf(T) / indexOf(T, i)
        assert_eq!(call("indexOf", &s("banana"), &[s("na")]), i(2));
        assert_eq!(call("indexOf", &s("banana"), &[s("na"), i(3)]), i(4));
        assert_eq!(call("indexOf", &s("banana"), &[s("zz")]), i(-1));
        // case, reverse, trim
        assert_eq!(call("toUpperCase", &s("aBc"), &[]), s("ABC"));
        assert_eq!(call("toLowerCase", &s("aBc"), &[]), s("abc"));
        assert_eq!(call("capitalize", &s("abc"), &[]), s("Abc"));
        assert_eq!(call("reverse", &s("abc"), &[]), s("cba"));
        assert_eq!(call("trim", &s("  x  "), &[]), s("x"));
        // predicates
        assert_eq!(call("isEmpty", &s(""), &[]), b(true));
        assert_eq!(call("nonEmpty", &s("x"), &[]), b(true));
        assert_eq!(call("startsWith", &s("hello"), &[s("he")]), b(true));
        assert_eq!(call("endsWith", &s("hello"), &[s("lo")]), b(true));
        assert_eq!(call("contains", &s("hello"), &[s("ell")]), b(true));
        assert_eq!(call("contains", &s("hello"), &[s("zz")]), b(false));
        // replace / split
        assert_eq!(call("replace", &s("a-b-c"), &[s("-"), s("+")]), s("a+b+c"));
        assert_eq!(
            call("split", &s("a,b,c"), &[s(",")]),
            from_expr(Expr::EList(EList {
                ps: vec![s("a"), s("b"), s("c")],
                ..Default::default()
            }))
        );
        // toString: the value -> string conversion (so `42.toString() ++ " units"` works)
        assert_eq!(call("toString", &i(42), &[]), s("42"));
        assert_eq!(call("toString", &b(false), &[]), s("false"));
        // format: variadic and single-list forms
        assert_eq!(call("format", &s("a=%s b=%s"), &[i(1), i(2)]), s("a=1 b=2"));
        assert_eq!(
            call(
                "format",
                &s("x=%s"),
                &[from_expr(Expr::EList(EList {
                    ps: vec![i(7)],
                    ..Default::default()
                }))]
            ),
            s("x=7")
        );
    }

    /// **A level the pattern names and this match did not fill is padded, as the oracle pads.**
    ///
    /// `resolve_match` reads the continuation's environment out of the matcher's free map, one level
    /// at a time, for `0..free_count`, and the Scala answers a missing one with the empty par
    /// (`Reduce.scala:352`'s `freeMap.getOrElse(e, Par())`). That is *semantics*, not a flatten: the
    /// matcher is greedy by design, so a connective pattern binds its first free var to the whole par
    /// and leaves the trailing ones empty — `legacy/.../matching-parallel-processes.rho` documents
    /// exactly that as its expected output (`@1!("success" | "success") | @2!(Nil)`).
    ///
    /// U10 refused here instead, on the reading that such a level contradicts the count's meaning.
    /// **The corpus refuted the reading by measurement** (U17, AUDIT C52): the state is reachable
    /// from a well-formed term. The count means "the levels the pattern names", not "the levels a
    /// given match filled".
    ///
    /// Falsifier, in the form that matters: restore the `BugFoundError` and this fails — and so does
    /// `legacy_contracts`, whose corpus program is the oracle's own vector for it.
    #[test]
    fn resolve_match_pads_a_level_the_pattern_does_not_bind() {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let e = Env::new();
        let rand = Blake2b512Random::from_init(&[]);
        // A well-formed term first: `match 1 { 1 => 2 }` — one case, binding no free level.
        let mut p: Par = crate::normalizer::source_to_adt("match 1 { 1 => 2 }")
            .expect("parse")
            .into();
        let mut m = p.matches.pop().expect("one match");
        assert_eq!(
            i32::from(m.cases[0].free_count),
            0,
            "the normalizer's own count agrees with the pattern"
        );
        // Now declare one binding more than the pattern has — the shape a count read unvalidated
        // from the wire can carry.
        m.cases[0].free_count = rchain_models::types::FreeCount::new(1).expect("1 >= 0");

        match resolve_match(&m, &e, &rand, &cost) {
            Ok(Some(effect)) => {
                let Effect::Par(_, env, _) = effect else {
                    panic!("a match evaluates to a par effect")
                };
                assert_eq!(
                    env.get(0),
                    Some(Par::default()),
                    "the level the pattern names and the match did not fill is the empty par"
                );
            }
            _ => panic!("a case whose pattern matches must reduce, padding the unfilled level"),
        }
    }

    // --- C345: the collection conversions and unions charge their work ------------------------

    /// What one term charges to evaluate, method dispatch included.
    ///
    /// `eval_single_expr` charges the operand's evaluation and the method; it does **not** charge
    /// storage (that is `ChargingRSpace`'s), so a difference of differences between the same term
    /// shape at two sizes is exactly the operation's marginal charge.
    fn charged(src: &str) -> i64 {
        let cost = CostAccounting::from_initial(Costs::unsafe_max());
        let env = Env::new();
        let p: Par = crate::normalizer::source_to_adt(src).expect("parse").into();
        eval_single_expr(&p, &env, &cost).expect("eval");
        cost.total_charged()
    }

    fn ints(n: usize) -> String {
        let items: Vec<String> = (0..n).map(|i| i.to_string()).collect();
        format!("[{}]", items.join(","))
    }

    fn pairs(n: usize) -> String {
        let items: Vec<String> = (0..n).map(|i| format!("({i},{i})")).collect();
        format!("[{}]", items.join(","))
    }

    /// The method's marginal charge at size `n`: the same expression is the operand of both arms, so
    /// only the method's own charge differs.
    fn marginal(
        baseline: impl Fn(&str) -> String,
        method: impl Fn(&str) -> String,
        operand: impl Fn(usize) -> String,
        n: usize,
    ) -> i64 {
        let base = operand(n);
        charged(&method(&base)) - charged(&baseline(&base))
    }

    /// **`toSet` charges 3 phlo per element it canonicalises** (C345). The old charge was flat — 18
    /// phlo at 500 elements and at 40 000 — while the sort's wall time grew from 7 ms to 724 ms.
    #[test]
    fn to_set_charges_its_elements() {
        let identity = |t: &str| t.to_string();
        assert_eq!(
            marginal(identity, |t| format!("{t}.toSet()"), ints, 9)
                - marginal(identity, |t| format!("{t}.toSet()"), ints, 8),
            Costs::add_cost().value,
            "one more element must cost one more canonicalisation"
        );
    }

    /// **`toMap` charges 3 phlo per element**, through the same `par_map` canonicalisation.
    #[test]
    fn to_map_charges_its_elements() {
        let identity = |t: &str| t.to_string();
        assert_eq!(
            marginal(identity, |t| format!("{t}.toMap()"), pairs, 9)
                - marginal(identity, |t| format!("{t}.toMap()"), pairs, 8),
            Costs::add_cost().value
        );
    }

    /// **`keys` charges 3 phlo per key**, where it used to charge a flat 10. The baseline is the map
    /// the method reads, so the conversion's own charge is cancelled out.
    #[test]
    fn keys_charges_its_output() {
        let to_map = |t: &str| format!("{t}.toMap()");
        assert_eq!(
            marginal(to_map, |t| format!("{t}.toMap().keys()"), pairs, 9)
                - marginal(to_map, |t| format!("{t}.toMap().keys()"), pairs, 8),
            Costs::add_cost().value
        );
    }

    /// **`set` charges 3 phlo for the map it rebuilds**, where it used to charge 3 for one addition.
    #[test]
    fn set_charges_the_map_it_rebuilds() {
        let to_map = |t: &str| format!("{t}.toMap()");
        assert_eq!(
            marginal(to_map, |t| format!("{t}.toMap().set(0,0)"), pairs, 9)
                - marginal(to_map, |t| format!("{t}.toMap().set(0,0)"), pairs, 8),
            Costs::add_cost().value
        );
    }

    /// **`add` — and so `set + x`, which shares its body — charges the set it rebuilds** (C353).
    ///
    /// `set_add` clones every element and re-canonicalises the whole set, so the charge must grow with
    /// the set. The flat `add_cost` this replaced made a fold of N insertions buy `O(N² log N)` work
    /// for `O(N)` phlo. The baseline is the set the method reads, so the conversion's own charge is
    /// cancelled out.
    #[test]
    fn a_set_add_charges_the_set_it_rebuilds() {
        let to_set = |t: &str| format!("{t}.toSet()");
        assert_eq!(
            marginal(to_set, |t| format!("{t}.toSet().add(0)"), ints, 9)
                - marginal(to_set, |t| format!("{t}.toSet().add(0)"), ints, 8),
            Costs::add_cost().value,
            "one more element in the set must cost one more canonicalisation"
        );
        // The `+` operator shares `set_add`, so it is charged the same. Its own `op_call_cost` is a
        // constant and cancels in the marginal.
        assert_eq!(
            marginal(to_set, |t| format!("{t}.toSet() + 0"), ints, 9)
                - marginal(to_set, |t| format!("{t}.toSet() + 0"), ints, 8),
            Costs::add_cost().value,
            "`set + x` is the same body and the same charge"
        );
    }

    /// **A union charges both operands.** The defect was the asymmetry: the old charge was the right
    /// operand only, so `bigSet ++ Set()` was free while it rebuilt `bigSet`. These two terms must
    /// cost the same, and both must scale with the large operand.
    #[test]
    fn a_union_charges_both_operands_alike() {
        let big = format!("{}.toSet()", ints(64));
        let empty = "Set()".to_string();
        let left = charged(&format!("{big} ++ {empty}"));
        let right = charged(&format!("{empty} ++ {big}"));
        assert_eq!(
            left, right,
            "a union must not be cheaper because its large operand is on the left"
        );
        let small = format!("{}.toSet()", ints(4));
        assert!(
            right > charged(&format!("{small} ++ {empty}")),
            "a 64-element union must cost more than a 4-element one"
        );
    }
}
