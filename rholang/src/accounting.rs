//! Gas (phlogiston) cost accounting.
//!
//! Mirrors `rholang/.../interpreter/accounting/` (`Cost`, `Costs`, `Chargeable`, `CostAccounting`).
//! The proto-size-dependent costs (`equalityCheckCost`, `storageCost*`, `toByteArrayCost`, and
//! `Chargeable.fromProtobuf`) are implemented (backed by the `Serialize` wire-size codecs) and
//! wired into the reducer. The cats-mtl `_cost[F]` monad stack is modeled as the synchronous
//! [`CostAccounting`] state cell.

use std::sync::atomic::{AtomicI64, Ordering};

use num_bigint::BigInt;

use rchain_models::decimal::{Decimal, MAX_SCALE};
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_shared::serialize::Serialize;

use crate::errors::RholangError;

/// A gas cost (port of `Cost`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cost {
    pub value: i64,
    pub operation: String,
}

impl Cost {
    pub fn new(value: i64, operation: impl Into<String>) -> Cost {
        Cost {
            value,
            operation: operation.into(),
        }
    }

    /// Scale a cost (port of `Cost.*`).
    pub fn mul(&self, base: i64) -> Cost {
        Cost {
            value: self.value * base,
            operation: format!("({} * {})", self.operation, base),
        }
    }

    /// Add two costs (port of `Cost.+`).
    pub fn add(&self, other: &Cost) -> Cost {
        Cost {
            value: self.value + other.value,
            operation: String::new(),
        }
    }

    /// Subtract two costs (port of `Cost.-`).
    pub fn sub(&self, other: &Cost) -> Cost {
        Cost {
            value: self.value - other.value,
            operation: String::new(),
        }
    }
}

/// The cost table (port of the `Costs` trait).
pub struct Costs;

impl Costs {
    pub fn sum_cost() -> Cost {
        Cost::new(3, "sum")
    }
    pub fn subtraction_cost() -> Cost {
        Cost::new(3, "subtraction")
    }
    pub fn boolean_and_cost() -> Cost {
        Cost::new(2, "boolean and")
    }
    pub fn boolean_or_cost() -> Cost {
        Cost::new(2, "boolean or")
    }
    pub fn comparison_cost() -> Cost {
        Cost::new(3, "comparison")
    }
    pub fn multiplication_cost() -> Cost {
        Cost::new(9, "multiplication")
    }
    pub fn division_cost() -> Cost {
        Cost::new(9, "division")
    }
    pub fn modulo_cost() -> Cost {
        Cost::new(9, "modulo")
    }
    pub fn lookup_cost() -> Cost {
        Cost::new(3, "lookup")
    }
    pub fn remove_cost() -> Cost {
        Cost::new(3, "removal")
    }
    pub fn add_cost() -> Cost {
        Cost::new(3, "addition")
    }
    pub fn hex_to_bytes_cost(s: &str) -> Cost {
        Cost::new(s.len() as i64, "hex to bytes")
    }
    pub fn bytes_to_hex_cost(bytes: &[u8]) -> Cost {
        Cost::new(bytes.len() as i64, "bytes to hex")
    }
    /// The cost of a set/map difference, **proportional to the product of the two operands** (audit
    /// F-3).
    ///
    /// This used to charge `3 · right`, the size of one operand, while `--` and `.diff` do
    /// `left.iter().filter(|p| !right.contains(p))` — and `contains` on a set is a linear scan, so the
    /// work is `left · right`. Charging one operand for a product is the sublinear-charge shape that
    /// lets an attacker buy unbounded work: a `|left| = |right| = n` difference cost `3n` and did `n²`
    /// comparisons, so a phlo bought an unbounded amount of work and the per-block cap could not
    /// contain it. The constant stays 3 per comparison; what changes is that both factors are counted.
    pub fn diff_cost(left: i64, right: i64) -> Cost {
        Cost::new(
            Self::remove_cost().mul(left.saturating_mul(right)).value,
            format!("{left} x {right} elements diff cost"),
        )
    }
    /// The cost of a `rho:gov:*` fold, proportional to the square of the member universe (audit F-3).
    ///
    /// These four handlers were charging **nothing at all** — not the storage path (which charges for
    /// what moves, and their output is proportional to their input, so it cannot see a cubic fold) and
    /// not the reducer. `censure` was measured at 10.73 s for a universe of 8000. The squared term is
    /// the leading term of the measured shape; the constant is the same 1-per-unit the table uses
    /// elsewhere, so the charge is a count rather than a tuned number.
    pub fn gov_fold_cost(universe: i64) -> Cost {
        Cost::new(
            universe.saturating_mul(universe),
            format!("{universe}-member governance fold"),
        )
    }
    /// The cost of `rho:gov:censure`, proportional to the **cube** of the member universe (C346).
    ///
    /// `qucalc::censure` iterates a decreasing fixed point for at most `universe + 1` rounds, and each
    /// round asks, of every member, how many censurers clear a level over the whole universe — so the
    /// work is Θ(U³), not the Θ(U²) `gov_fold_cost` charged. At `MAX_GOV_UNIVERSE` that is the
    /// leading term by a wide margin, which is why the bound exists (see the measurement in
    /// `system_processes.rs`).
    pub fn gov_censure_cost(universe: i64) -> Cost {
        let rounds = universe.saturating_add(1);
        Cost::new(
            rounds.saturating_mul(rounds).saturating_mul(rounds),
            format!("{universe}-member censure fold"),
        )
    }
    /// The cost of a **ranked** tally: every round scans every ranked entry, and a round may
    /// eliminate only one option, so there are at most `longest` rounds (C354).
    ///
    /// The charge the handler used to take was `(ballots + weights)²` — a function of two counts that
    /// the option lists do not appear in at all, while the work is linear in them and round-multiplied
    /// by the longest ballot. This is the `diff_cost`/`string_search_worst_cost` shape: a scan whose
    /// worst case is a product, so the product is what is charged. The floor keeps a one-entry ballot
    /// — or a ballot list where every round eliminates everything at once — from costing nothing.
    pub fn gov_tally_ranked_cost(longest: i64, entries: i64) -> Cost {
        Cost::new(
            longest.saturating_mul(entries).max(entries),
            format!("{longest}-deep ranked tally over {entries} entries"),
        )
    }
    /// The cost of an **approval** tally: every approved option is counted once per ballot (C354).
    pub fn gov_tally_approval_cost(entries: i64) -> Cost {
        Cost::new(entries, format!("{entries}-option approval tally"))
    }
    /// The cost of canonicalising a collection of `elements` items (C345).
    ///
    /// `par_set` and `par_map` sort and deduplicate their input, so every method that builds one — the
    /// `toSet`/`toMap` conversions, `set`, `keys`, and the set/map unions — does work proportional to
    /// the output it is about to build. Charged at the same constant-3-per-element the table uses for
    /// `lookup`/`add`/`remove`, and **computed from the operand size before the work**, so the phlo
    /// budget is what stops it rather than the node's CPU.
    ///
    /// **What it does not claim:** the sort is `O(n log n)` and the charge is linear, so a phlo buys
    /// work that grows by a logarithm. That is bounded (`charge · log charge`), not unbounded, which is
    /// the defect C345 names; it is not an exact model of the comparison count.
    pub fn canonicalization_cost(elements: i64) -> Cost {
        Cost::new(
            Self::add_cost().mul(elements).value,
            format!("{elements}-element canonicalisation"),
        )
    }
    /// The cost of a set or map union: both operands are canonicalised into one result (C345).
    ///
    /// The old charge was `3 · right`, so `bigSet ++ {}` cost nothing while the union still rebuilt
    /// `bigSet`. Charging the sum is what makes the charge symmetric in the operands, which is the
    /// property the Lean model states (`Rchain.unionCost_symmetric`).
    pub fn union_cost(left: i64, right: i64) -> Cost {
        Self::canonicalization_cost(left.saturating_add(right))
    }
    pub fn list_append_cost(size: i64) -> Cost {
        Cost::new(size, "list append")
    }
    pub fn string_append_cost(n: i64, m: i64) -> Cost {
        Cost::new(n + m, "string append")
    }
    pub fn interpolate_cost(str_length: i64, map_size: i64) -> Cost {
        Cost::new(str_length * map_size, "interpolate")
    }
    pub fn to_int_cost_bigint(bi: &BigInt) -> Cost {
        Cost::new(Self::big_int_size(bi), "bigint to int")
    }
    pub fn to_int_cost_string(s: &str) -> Cost {
        Cost::new(s.len() as i64, "string to int")
    }
    pub fn int_to_bigint_cost() -> Cost {
        Cost::new(8, "int to bigint")
    }
    pub fn to_bigint_cost(s: &str) -> Cost {
        Cost::new(s.len() as i64, "string to bigint")
    }
    pub fn big_int_negation(bi: &BigInt) -> Cost {
        Cost::new(Self::big_int_size(bi), "bigint negation")
    }
    pub fn big_int_comparison(left: &BigInt, right: &BigInt) -> Cost {
        Cost::new(
            Self::big_int_size(left).min(Self::big_int_size(right)),
            "bigint comparison",
        )
    }
    pub fn big_int_sum(left: &BigInt, right: &BigInt) -> Cost {
        Cost::new(
            Self::big_int_size(left).max(Self::big_int_size(right)) + 1,
            "bigint sum",
        )
    }
    pub fn big_int_subtraction(left: &BigInt, right: &BigInt) -> Cost {
        Cost::new(
            Self::big_int_size(left).max(Self::big_int_size(right)) + 1,
            "bigint subtraction",
        )
    }
    pub fn big_int_multiplication(left: &BigInt, right: &BigInt) -> Cost {
        Cost::new(
            Self::big_int_size(left) * Self::big_int_size(right),
            "bigint multiplication",
        )
    }
    pub fn big_int_division(left: &BigInt, right: &BigInt) -> Cost {
        Cost::new(
            Self::big_int_size(left) * Self::big_int_size(right),
            "bigint division",
        )
    }
    pub fn big_int_modulo(left: &BigInt, right: &BigInt) -> Cost {
        Cost::new(
            Self::big_int_size(left) * Self::big_int_size(right),
            "bigint modulo",
        )
    }
    /// The number of bytes needed to store a bigint (port of `bigIntSize`).
    pub fn big_int_size(bi: &BigInt) -> i64 {
        (bi.magnitude().bits() / 8 + 1) as i64
    }

    /// The byte size of a decimal's coefficient, plus one for the scale.
    pub fn decimal_size(d: &Decimal) -> i64 {
        Self::big_int_size(d.coefficient()) + 1
    }

    pub fn decimal_negation(d: &Decimal) -> Cost {
        Cost::new(Self::big_int_size(d.coefficient()), "decimal negation")
    }

    /// A comparison aligns the two scales, so it costs the smaller coefficient plus the scale gap.
    pub fn decimal_comparison(left: &Decimal, right: &Decimal) -> Cost {
        let scale_gap = i64::from(left.scale()).abs_diff(i64::from(right.scale()));
        Cost::new(
            Self::big_int_size(left.coefficient()).min(Self::big_int_size(right.coefficient()))
                + 1
                + scale_gap as i64,
            "decimal comparison",
        )
    }

    pub fn decimal_sum(left: &Decimal, right: &Decimal) -> Cost {
        Cost::new(
            Self::big_int_size(left.coefficient()).max(Self::big_int_size(right.coefficient()))
                + 1
                + i64::from(left.scale()).abs_diff(i64::from(right.scale())) as i64,
            "decimal sum",
        )
    }

    pub fn decimal_subtraction(left: &Decimal, right: &Decimal) -> Cost {
        Cost::new(
            Self::big_int_size(left.coefficient()).max(Self::big_int_size(right.coefficient()))
                + 1
                + i64::from(left.scale()).abs_diff(i64::from(right.scale())) as i64,
            "decimal subtraction",
        )
    }

    /// Multiplication is charged on the product of the coefficient sizes, as `BigInt` is.
    pub fn decimal_multiplication(left: &Decimal, right: &Decimal) -> Cost {
        Cost::new(
            Self::big_int_size(left.coefficient())
                .saturating_mul(Self::big_int_size(right.coefficient())),
            "decimal multiplication",
        )
    }

    /// Division is charged for the worst case: the `big_int_division` cost of the aligned
    /// operands, repeated for every digit the exact long division may need (`MAX_SCALE`).
    pub fn decimal_division(left: &Decimal, right: &Decimal) -> Cost {
        Cost::new(
            Self::big_int_size(left.coefficient())
                .saturating_mul(Self::big_int_size(right.coefficient()))
                .saturating_mul(i64::from(MAX_SCALE) + 1),
            "decimal division",
        )
    }
    pub fn size_method_cost(size: i64) -> Cost {
        Cost::new(size, "size")
    }
    pub fn slice_cost(to: i64) -> Cost {
        Cost::new(to, "slice")
    }
    pub fn take_cost(to: i64) -> Cost {
        Cost::new(to, "take")
    }
    /// Cost of a string transform that walks the input once (`toLowerCase`, `toUpperCase`,
    /// `capitalize`, `reverse`, `trim`).
    pub fn string_transform_cost(len: i64, what: &str) -> Cost {
        Cost::new(len, what)
    }
    /// Cost of a substring search (`indexOf`, `contains`, `startsWith`, `endsWith`).
    ///
    /// Right for the *anchored* tests — `startsWith` and `endsWith` compare at one position, so
    /// `n + m` is the work — and wrong for the *scanning* ones, which is what
    /// [`Self::string_search_worst_cost`] exists for. Kept for the anchored pair.
    pub fn string_search_cost(haystack: i64, needle: i64, what: &str) -> Cost {
        Cost::new(haystack.saturating_add(needle), what)
    }
    /// Cost of a *scanning* substring search, proportional to the product (audit F-3).
    ///
    /// `indexOf` and `contains` walk the haystack a window at a time and compare each window against
    /// the needle, so the worst case is `n · m` — `"aaaa…a"` against `"aaaa…b"` compares nearly the
    /// whole needle at every position. Charging `n + m` for that is the sublinear-charge shape: the
    /// amplification factor is unbounded in the ratio of needle to haystack, and this is the same
    /// defect as `diff_cost` one table entry over. The `.max(n + m)` floor keeps a one-character
    /// search from costing nothing.
    pub fn string_search_worst_cost(haystack: i64, needle: i64, what: &str) -> Cost {
        Cost::new(
            haystack.saturating_mul(needle).max(haystack + needle),
            format!("{what} (worst case)"),
        )
    }
    /// Cost of `replace`, proportional to the **output it is about to build** (audit F-3).
    ///
    /// The old charge was `input + old + new` — linear in the *inputs* — while `str::replace`
    /// allocates `input + k·(new − old)` for `k` occurrences, and `k` can be `input / old`. With `old`
    /// one character and `new` as long as the input, a 100 KB × 100 KB call builds a 10 GB string for
    /// a charge proportional to 200 KB. The amplification is unbounded in the ratio of `new` to `old`,
    /// which is the same shape as `diff_cost`.
    ///
    /// **Charged, not refused**, and deliberately: the bound below is computable in constant time and
    /// the charge lands *before* the allocation, so the deploy's phlo budget is what stops it. That is
    /// strictly less invasive than a size cap — no new constant, and no term that used to be accepted
    /// is now rejected. Where a charge can bound the work, a refusal is not needed.
    ///
    /// **The ordering is the whole justification, and it is not pinned by a test.** A test was written
    /// and removed: it could not discriminate, because the storage path also charges for the result
    /// once it is produced, and that charge dominates every budget a rholang-level test can construct
    /// — so the test refused the term with this charge reverted to its old form, and would have gone
    /// on passing if the charge were deleted. What the charge uniquely buys is that the refusal
    /// happens *before* the allocation rather than after it, which is the difference between a refused
    /// deploy and a node that has just tried to build a 10 GB string; that is an ordering property and
    /// a test would need to observe a non-allocation. Recorded here rather than left as a green test
    /// that proves nothing — the same trade the DAG write-order fix made.
    pub fn string_replace_cost(input: i64, old: i64, new: i64) -> Cost {
        // `k <= input / old`, so this upper-bounds the result without building it. `old <= 0` cannot
        // splice (the caller leaves the input as-is for an empty `old`), so it produces no growth.
        let occurrences = if old <= 0 { 0 } else { input / old };
        Cost::new(
            input.saturating_add(occurrences.saturating_mul(new)),
            "replace",
        )
    }
    /// Cost of `split` (walk the input and the separator).
    pub fn string_split_cost(input: i64, sep: i64) -> Cost {
        Cost::new(input.saturating_add(sep), "split")
    }
    /// Cost of `format` (walk the format string once per argument).
    pub fn string_format_cost(format_len: i64, args: i64) -> Cost {
        Cost::new(format_len.saturating_mul(args.max(1)), "format")
    }
    /// Cost of converting a value to its string form (the inverse of `toInt`/`toBigInt`).
    pub fn to_string_cost(len: i64) -> Cost {
        Cost::new(len, "toString")
    }
    /// Cost of a bitwise method (RCHIP #12) on an `Int`: a word operation, priced as `+`.
    pub fn bitwise_int_cost(what: &str) -> Cost {
        Cost::new(3, what)
    }
    /// Cost of a bitwise method on a `BigInt` or `ByteArray`: the bytes walked (the larger operand,
    /// or for a left shift the result's size), never less than one.
    pub fn bitwise_cost(bytes: i64, what: &str) -> Cost {
        Cost::new(bytes.max(1), what)
    }
    /// [`Self::big_int_size`] of `x << n`, computed from `x`'s magnitude bit width without building
    /// the result: the shift adds exactly `n` bits, so the size is taken over the *total* width
    /// (`x`'s remaining bits and the shift can cross a byte boundary together, which summing
    /// `big_int_size(x)` and `n / 8` misses).
    pub fn shift_left_result_size(x_bits: u64, n: u64) -> i64 {
        // Zero shifted is still zero: no width is added.
        if x_bits == 0 {
            return 1;
        }
        i64::try_from(x_bits.saturating_add(n) / 8 + 1).unwrap_or(i64::MAX)
    }
    pub fn to_list_cost(size: i64) -> Cost {
        Cost::new(size, "toList")
    }
    pub fn parsing_cost(term: &str) -> Cost {
        Cost::new(term.len() as i64, "parsing")
    }
    pub fn nth_method_call_cost() -> Cost {
        Cost::new(10, "nth method call")
    }
    pub fn length_method_cost() -> Cost {
        Cost::new(10, "length method")
    }
    pub fn method_call_cost() -> Cost {
        Cost::new(10, "method call")
    }
    pub fn op_call_cost() -> Cost {
        Cost::new(10, "op call")
    }
    pub fn var_eval_cost() -> Cost {
        Cost::new(10, "var eval")
    }
    pub fn send_eval_cost() -> Cost {
        Cost::new(11, "send eval")
    }
    pub fn receive_eval_cost() -> Cost {
        Cost::new(11, "receive eval")
    }
    pub fn channel_eval_cost() -> Cost {
        Cost::new(11, "channel eval")
    }
    pub fn new_binding_cost() -> Cost {
        Cost::new(2, "new binding")
    }
    pub fn new_eval_cost() -> Cost {
        Cost::new(10, "new eval")
    }
    pub fn new_bindings_cost(n: i64) -> Cost {
        Cost::new(
            Self::new_binding_cost()
                .mul(n)
                .add(&Self::new_eval_cost())
                .value,
            format!("{n} new bindings"),
        )
    }
    pub fn match_eval_cost() -> Cost {
        Cost::new(12, "match eval")
    }

    /// Size-proportional equality check (port of `equalityCheckCost`): the smaller serialized size.
    pub fn equality_check_cost<T, P>(x: &T, y: &P) -> Cost
    where
        T: Serialize<T>,
        P: Serialize<P>,
    {
        Cost::new(
            (<T as Serialize<T>>::encode(x).len() as i64)
                .min(<P as Serialize<P>>::encode(y).len() as i64),
            "equality check",
        )
    }

    /// Serializing a term into a byte array allocates + copies `serializedSize` bytes (port of
    /// `toByteArrayCost`).
    pub fn to_byte_array_cost<T: Serialize<T>>(a: &T) -> Cost {
        Cost::new(<T as Serialize<T>>::encode(a).len() as i64, "to byte array")
    }

    /// Storage cost: the sum of the serialized sizes (port of `storageCost`).
    pub fn storage_cost<T: Serialize<T>>(terms: &[T]) -> Cost {
        Cost::new(
            terms
                .iter()
                .map(|a| <T as Serialize<T>>::encode(a).len() as i64)
                .sum(),
            "storage cost",
        )
    }

    /// Consume storage cost: channels + patterns + the `ParBody` continuation body (port of
    /// `storageCostConsume`).
    pub fn storage_cost_consume(
        channels: &[SortedProc],
        patterns: &[BindPattern],
        continuation: &TaggedContinuation,
    ) -> Cost {
        let channels_cost: i64 = channels
            .iter()
            .map(|c| <SortedProc as Serialize<SortedProc>>::encode(c).len() as i64)
            .sum();
        let patterns_cost: i64 = patterns
            .iter()
            .map(|p| <BindPattern as Serialize<BindPattern>>::encode(p).len() as i64)
            .sum();
        let body_cost: i64 = match continuation {
            TaggedContinuation::ParBody(pwr) => {
                <SortedProc as Serialize<SortedProc>>::encode(&pwr.body).len() as i64
            }
            _ => 0,
        };
        Cost::new(channels_cost + patterns_cost + body_cost, "consume storage")
    }

    /// Produce storage cost: channel + the produced data (port of `storageCostProduce`).
    pub fn storage_cost_produce(channel: &SortedProc, data: &ListParWithRandom) -> Cost {
        let channel_cost = <SortedProc as Serialize<SortedProc>>::encode(channel).len() as i64;
        let data_cost: i64 = data
            .pars
            .iter()
            .map(|p| <SortedProc as Serialize<SortedProc>>::encode(p).len() as i64)
            .sum();
        Cost::new(channel_cost + data_cost, "produces storage")
    }

    const HASH_LEN: i64 = 32;

    pub fn event_storage_cost(channels_involved: i64) -> Cost {
        Cost::new(
            Self::HASH_LEN + channels_involved * Self::HASH_LEN,
            "event storage cost",
        )
    }
    pub fn comm_event_storage_cost(channels_involved: i64) -> Cost {
        let consume = Self::event_storage_cost(channels_involved);
        let produces = Self::event_storage_cost(1).mul(channels_involved);
        let mut c = consume.add(&produces);
        c.operation = "comm event storage cost".to_string();
        c
    }

    pub fn max_value() -> i64 {
        i32::MAX as i64
    }
    pub fn unsafe_max() -> Cost {
        Cost::new(i32::MAX as i64, "")
    }
}

/// Typeclass for charging a term (port of `Chargeable`).
pub trait Chargeable<A> {
    fn cost(a: &A) -> i64;
}

/// The protobuf `fromProtobuf` instance: charge a term by its serialized (wire) size.
impl<T> Chargeable<T> for T
where
    T: Serialize<T>,
{
    fn cost(a: &T) -> i64 {
        <T as Serialize<T>>::encode(a).len() as i64
    }
}

/// The synchronous cost-accounting state (models the `_cost[F]` monad stack).
#[derive(Default)]
pub struct CostAccounting {
    value: AtomicI64,
    /// Running total phlo charged since construction. Kept as an atomic sum rather than a log of
    /// `Cost`s, so a long-running node does not accumulate an unbounded `Vec` (each entry carried a
    /// heap `String`) and `total_charged` stays O(1).
    total: AtomicI64,
}

impl CostAccounting {
    pub fn new() -> Self {
        CostAccounting::default()
    }

    pub fn from_initial(init: Cost) -> Self {
        CostAccounting {
            value: AtomicI64::new(init.value),
            total: AtomicI64::new(0),
        }
    }

    pub fn get(&self) -> Cost {
        Cost::new(self.value.load(Ordering::SeqCst), "get")
    }

    /// Total phlo charged so far (the sum of every `charge`d amount, i.e. consumed phlo).
    pub fn total_charged(&self) -> i64 {
        self.total.load(Ordering::SeqCst)
    }

    /// Set the current cost balance (port of `_cost.set`).
    pub fn set(&self, cost: Cost) {
        self.value.store(cost.value, Ordering::SeqCst);
    }

    /// Charge `amount` phlogistons, raising `OutOfPhlogistonsError` if the balance goes negative
    /// (port of `charge`).
    ///
    /// The decrement is a single atomic read-modify-write (`fetch_update`), not a load-then-store,
    /// so concurrent charges cannot lose an update (two racing `charge`s each observe the other's
    /// effect on the balance).
    pub fn charge(&self, amount: Cost) -> Result<(), RholangError> {
        let amount_value = amount.value;
        match self
            .value
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                if current < 0 {
                    // Already exhausted: abort without changing the balance or logging.
                    None
                } else {
                    // Clamp at 0 rather than leaving the cost cell negative on exhaustion (the error is
                    // still raised by the caller below).
                    Some((current - amount_value).max(0))
                }
            }) {
            Ok(prev) => {
                self.total.fetch_add(amount_value, Ordering::SeqCst);
                if prev - amount_value < 0 {
                    Err(RholangError::OutOfPhlogistonsError)
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(RholangError::OutOfPhlogistonsError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_arithmetic() {
        let a = Cost::new(3, "sum");
        let b = Cost::new(5, "sub");
        assert_eq!(a.mul(2).value, 6);
        assert_eq!(a.mul(2).operation, "(sum * 2)");
        assert_eq!(a.add(&b).value, 8);
        assert_eq!(a.add(&b).operation, "");
        assert_eq!(a.sub(&b).value, -2);
    }

    #[test]
    fn charge_decrements_and_errors_when_exhausted() {
        let acc = CostAccounting::from_initial(Cost::new(10, "init"));
        assert!(acc.charge(Cost::new(4, "x")).is_ok());
        assert_eq!(acc.get().value, 6);
        assert!(acc.charge(Cost::new(7, "y")).is_err());
    }

    #[test]
    fn charge_is_atomic_under_concurrency() {
        use std::sync::Arc;
        use std::thread;

        let acc = Arc::new(CostAccounting::from_initial(Cost::new(1_000_000, "init")));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let acc = Arc::clone(&acc);
            handles.push(thread::spawn(move || {
                for _ in 0..10_000 {
                    let _ = acc.charge(Cost::new(1, "x"));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 8 threads * 10_000 charges of 1 each = 80_000 consumed; no charge may be lost to a race.
        assert_eq!(acc.total_charged(), 80_000);
        assert_eq!(acc.get().value, 1_000_000 - 80_000);
    }

    #[test]
    fn size_proportional_costs_match_serialized_size() {
        let par = rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GInt(42));
        let encoded = <rchain_models::ast::Par as Serialize<rchain_models::ast::Par>>::encode(&par);
        let len = encoded.len() as i64;

        assert_eq!(Costs::to_byte_array_cost(&par).value, len);
        assert_eq!(Costs::to_byte_array_cost(&par).operation, "to byte array");
        assert_eq!(Costs::storage_cost(std::slice::from_ref(&par)).value, len);
        assert_eq!(Costs::equality_check_cost(&par, &par).value, len);
    }

    /// **The canonicalisation charge is linear in the elements and the union charges both operands**
    /// (C345), and the censure charge is the cube of its universe (C346). The Lean model carries the
    /// same three statements (`Rchain/Cost.lean`, law 91).
    #[test]
    fn collection_and_censure_charges_are_the_models() {
        // A canonicalisation is 3 phlo per element, at every size, and computed from the size alone.
        for n in [0i64, 1, 7, 512, 40_000] {
            assert_eq!(Costs::canonicalization_cost(n).value, 3 * n);
        }
        // A union charges the sum: symmetric, and exactly the two canonicalisations.
        for (l, r) in [(0i64, 0i64), (1, 1), (0, 40_000), (40_000, 0), (7, 11)] {
            assert_eq!(
                Costs::union_cost(l, r),
                Costs::union_cost(r, l),
                "the union charge must not depend on which operand is which"
            );
            assert_eq!(
                Costs::union_cost(l, r).value,
                Costs::canonicalization_cost(l).value + Costs::canonicalization_cost(r).value
            );
        }
        // Censure is cubic in the universe: the fixed point runs up to `U + 1` rounds, each scanning
        // the universe per member.
        for u in [0i64, 1, 8, 512] {
            assert_eq!(Costs::gov_censure_cost(u).value, (u + 1).pow(3));
        }
        assert_eq!(
            Costs::gov_censure_cost(512).value,
            135_005_697,
            "at the bound the charge is (512+1)³"
        );
    }

    /// **A tally charges the option lists it scans** (C354): the ranked mode charges the worst case —
    /// one scan per entry per round — and the approval mode charges its entries. The floor keeps a
    /// one-round ballot from costing nothing.
    #[test]
    fn the_tally_charges_are_the_models() {
        assert_eq!(Costs::gov_tally_ranked_cost(10, 1_000).value, 10_000);
        assert_eq!(Costs::gov_tally_ranked_cost(1, 10).value, 10);
        assert_eq!(Costs::gov_tally_ranked_cost(0, 0).value, 0);
        assert_eq!(Costs::gov_tally_approval_cost(0).value, 0);
        assert_eq!(Costs::gov_tally_approval_cost(1_000).value, 1_000);
    }
}
