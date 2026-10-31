//! **The `decimal` corpus, Law 87a–d, driven through the Rust value algebra.**
//!
//! `spec/conformance/decimal.tsv` is emitted from `spec/Rchain/Decimal.lean`: every row is a shape
//! the Lean canonicaliser, comparator, arithmetic or division decides. This consumer runs the same
//! shape through `models::decimal` — the exact type the reducer and the sorter use — and compares the
//! rendered outcome, so a drift between the proof's model and the code fails here rather than in a
//! note. The seven columns are: layer, case, op, a, b, outcome kind, payload.
//!
//! `new` and `parts` take a raw `coefficient:scale` pair because those constructors are where a
//! non-canonical input is legal (and refused by name); every other operand is built with
//! `from_parts`, because the Rust cannot construct a non-canonical `Decimal` at all.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::str::FromStr;

use num_bigint::BigInt;
use rchain_models::decimal::{Decimal, DecimalError};

const LAYER: &str = "decimal";
const CASES: usize = 42;

fn pair(field: &str) -> (BigInt, u32) {
    let (coefficient, scale) = field
        .split_once(':')
        .unwrap_or_else(|| panic!("{field:?} is not a coefficient:scale pair"));
    (
        BigInt::from_str(coefficient)
            .unwrap_or_else(|e| panic!("{coefficient:?} is not a coefficient: {e}")),
        scale
            .parse::<u32>()
            .unwrap_or_else(|e| panic!("{scale:?} is not a u32 scale: {e}")),
    )
}

fn canonical(field: &str) -> Decimal {
    let (coefficient, scale) = pair(field);
    Decimal::from_parts(coefficient, scale)
        .unwrap_or_else(|e| panic!("{field:?} is not a canonical corpus operand: {e}"))
}

fn render(d: &Decimal) -> String {
    format!("{}:{}", d.coefficient(), d.scale())
}

fn render_error(e: &DecimalError) -> String {
    match e {
        DecimalError::ScaleOutOfRange { scale } => format!("scale_out_of_range:{scale}"),
        DecimalError::ZeroWithScale { scale } => format!("zero_with_scale:{scale}"),
        DecimalError::TrailingZero { scale } => format!("trailing_zero:{scale}"),
        DecimalError::DivisionByZero => "division_by_zero".to_string(),
        DecimalError::InexactDivision => "inexact_division".to_string(),
        other => panic!("the Lean model emitted a wire-only error: {other:?}"),
    }
}

fn render_result(result: Result<Decimal, DecimalError>) -> (&'static str, String) {
    match result {
        Ok(d) => ("ok", render(&d)),
        Err(e) => ("error", render_error(&e)),
    }
}

fn render_cmp(o: Ordering) -> &'static str {
    match o {
        Ordering::Less => "lt",
        Ordering::Equal => "eq",
        Ordering::Greater => "gt",
    }
}

#[test]
fn the_decimal_algebra_matches_the_lean_model() {
    let corpus = include_str!("../../spec/conformance/decimal.tsv");
    let mut seen = 0usize;
    let mut ops = BTreeSet::new();
    let mut verdicts = BTreeSet::new();

    for line in corpus.lines() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 7, "seven columns: {line}");
        assert_eq!(fields[0], LAYER, "layer: {line}");
        let (case, op, a, b, kind, payload) = (
            fields[1], fields[2], fields[3], fields[4], fields[5], fields[6],
        );

        let (got_kind, got_payload): (&str, String) = match op {
            "new" => {
                let (coefficient, scale) = pair(a);
                render_result(Decimal::new(coefficient, scale))
            }
            "parts" => {
                let (coefficient, scale) = pair(a);
                render_result(Decimal::from_parts(coefficient, scale))
            }
            "scaled" => ("scaled", canonical(a).scaled_value().to_string()),
            "cmp" => (
                "cmp",
                render_cmp(canonical(a).cmp_numeric(&canonical(b))).to_string(),
            ),
            "add" => render_result(canonical(a).add(&canonical(b))),
            "sub" => render_result(canonical(a).sub(&canonical(b))),
            "mul" => render_result(canonical(a).mul(&canonical(b))),
            "div" => render_result(canonical(a).div(&canonical(b))),
            other => panic!("{case}: the Lean corpus emitted an unknown decimal op: {other}"),
        };

        assert_eq!(
            (got_kind, got_payload.as_str()),
            (kind, payload),
            "{case}: the Rust algebra disagrees with the Lean model"
        );
        ops.insert(op);
        if op == "cmp" {
            verdicts.insert(payload);
        }
        seen += 1;
    }

    assert_eq!(seen, CASES, "the layer is pinned at {CASES} cases");
    assert_eq!(ops.len(), 8, "all eight operations are exercised");
    assert_eq!(
        verdicts.len(),
        3,
        "all three comparison verdicts are exercised"
    );
}
