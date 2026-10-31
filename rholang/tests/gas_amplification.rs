//! Gas-amplification probes for the October 2026 security re-audit (report-only instrumentation).
//!
//! The question: is an operation reachable from a deploy's term superlinear in the input it is handed
//! while its phlo charge is flat? These probes measure the **charged cost** and the **wall time** of
//! the collection conversions and methods in `reduce.rs` at several input sizes.
//!
//! They are env-gated (`RNODE_GAS_PROBE=1`) so the ordinary suite stays fast; run them release:
//!
//! ```text
//! RNODE_GAS_PROBE=1 cargo test -p rchain-rholang --test gas_amplification --release -- --nocapture
//! ```
//!
//! The committed evidence in `spec/audit/evidence/` carries the numbers.

mod common;

use std::time::Instant;

use common::build_runtime;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;

fn probe_enabled() -> bool {
    std::env::var("RNODE_GAS_PROBE").ok().as_deref() == Some("1")
}

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[0u8; 32])
}

fn list_of_ints(n: usize) -> String {
    let items: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    format!("[{}]", items.join(","))
}

fn list_of_pairs(n: usize) -> String {
    let items: Vec<String> = (0..n).map(|i| format!("({i},{i})")).collect();
    format!("[{}]", items.join(","))
}

/// One evaluation: `(charged phlo, wall millis)`.
async fn measure(term: &str) -> (i64, u128) {
    let rt = build_runtime(false).await;
    let start = Instant::now();
    let res = rt
        .evaluate(term, &fixed_rand())
        .await
        .expect("no node fault");
    assert!(
        res.errors.is_empty(),
        "the probe term must evaluate cleanly; got {:?}",
        res.errors
    );
    (res.cost.value, start.elapsed().as_millis())
}

/// **`toSet` on a list of N ints** — the conversion sorts N elements. The baseline is the same
/// literal with no conversion, so the parse charge is common to both and the delta is the method's.
///
/// Before C345 the delta was flat at 18 phlo while the wall time grew 7 → 724 ms. After it the delta
/// must grow with N; the wall time is printed beside it so the same instrument shows both.
#[tokio::test]
async fn a_conversion_charges_its_output() {
    if !probe_enabled() {
        return;
    }
    println!("case,n,plain_phlo,converted_phlo,delta_phlo,plain_ms,converted_ms");
    for n in [500usize, 1_000, 2_000, 4_000, 10_000, 20_000, 40_000] {
        let list = list_of_ints(n);
        let (plain, plain_ms) = measure(&format!(r#"@"out"!({list})"#)).await;
        let (converted, ms) = measure(&format!(r#"@"out"!({list}.toSet())"#)).await;
        println!(
            "toSet,{n},{plain},{converted},{},{plain_ms},{ms}",
            converted - plain
        );
    }
}

/// **`toMap` on a list of N pairs** — same shape as `toSet`, through `par_map`.
#[tokio::test]
async fn a_map_conversion_charges_its_output() {
    if !probe_enabled() {
        return;
    }
    println!("case,n,plain_phlo,converted_phlo,delta_phlo,plain_ms,converted_ms");
    for n in [500usize, 1_000, 2_000, 4_000, 10_000] {
        let pairs = list_of_pairs(n);
        let (plain, plain_ms) = measure(&format!(r#"@"out"!({pairs})"#)).await;
        let (converted, ms) = measure(&format!(r#"@"out"!({pairs}.toMap())"#)).await;
        println!(
            "toMap,{n},{plain},{converted},{},{plain_ms},{ms}",
            converted - plain
        );
    }
}

/// **`keys` on a map of N pairs** — builds and sorts N keys. The baseline is `toMap()` of the same
/// list, so the conversion's own charge is held fixed and the delta is `keys`'.
#[tokio::test]
async fn keys_charges_its_output() {
    if !probe_enabled() {
        return;
    }
    println!("case,n,baseline_phlo,keys_phlo,delta_phlo,baseline_ms,keys_ms");
    for n in [500usize, 1_000, 2_000, 4_000, 10_000] {
        let pairs = list_of_pairs(n);
        let (base, base_ms) = measure(&format!(r#"@"out"!({pairs}.toMap())"#)).await;
        let (keys, ms) = measure(&format!(r#"@"out"!({pairs}.toMap().keys())"#)).await;
        println!("keys,{n},{base},{keys},{},{base_ms},{ms}", keys - base);
    }
}

/// **`set` on a map of N pairs** — clones the map, scans it, re-sorts it.
#[tokio::test]
async fn set_charges_its_output() {
    if !probe_enabled() {
        return;
    }
    println!("case,n,baseline_phlo,set_phlo,delta_phlo,baseline_ms,set_ms");
    for n in [500usize, 1_000, 2_000, 4_000, 10_000] {
        let pairs = list_of_pairs(n);
        let (base, base_ms) = measure(&format!(r#"@"out"!({pairs}.toMap())"#)).await;
        let (set, ms) = measure(&format!(r#"@"out"!({pairs}.toMap().set(0,0))"#)).await;
        println!("set,{n},{base},{set},{},{base_ms},{ms}", set - base);
    }
}

/// **The September union amplification, re-measured**: the charge must track its work now.
#[tokio::test]
async fn the_union_charge_tracks_its_work() {
    if !probe_enabled() {
        return;
    }
    println!("case,n,plain_phlo,union_phlo,delta_phlo,plain_ms,union_ms");
    for n in [1_000usize, 5_000, 10_000] {
        let list = list_of_ints(n);
        let (plain, plain_ms) = measure(&format!(r#"@"out"!({list})"#)).await;
        let term = format!(r#"@"out"!(({list}).toSet().union(({list}).toSet()))"#);
        let (union, ms) = measure(&term).await;
        let delta = union - plain;
        println!("union,{n},{plain},{union},{delta},{plain_ms},{ms}");
        assert!(
            delta >= 3 * n as i64,
            "a union of two N-element sets must charge for its elements (>= 3N); {n}: {delta}"
        );
    }
}

/// **`rho:gov:censure`'s fixed point, timed at the sizes the universe bound admits** (C346).
///
/// `MAX_GOV_UNIVERSE` bounds the **sum** `censures + levels + vouchers`, which is what makes the cubic
/// path hard to reach: a discredit needs a quorum of about two thirds of the eligible set, and the
/// universe counts the censure pairs that supply it. These four shapes are the extremes at 512 members;
/// the numbers are what the bound and the charge are set from.
#[tokio::test]
async fn censure_is_cubic_in_the_universe() {
    if !probe_enabled() {
        return;
    }
    use std::collections::BTreeMap;

    // `(name, members, censures, vouchers)`, each summing to `MAX_GOV_UNIVERSE` (512).
    for (name, members, censures, vouchers) in [
        ("members", 512usize, 0usize, 0usize),
        ("balanced", 171, 170, 171),
        ("censure-heavy", 86, 340, 86),
        ("voucher-heavy", 86, 86, 340),
    ] {
        let universe = members + censures + vouchers;
        let levels: BTreeMap<String, i64> = (0..members).map(|i| (format!("m{i:04}"), 5)).collect();
        // Every censure and every voucher names `m0000`, so the fixed point actually discredits and
        // then slashes: a shape whose quorum is never met would measure the loop's exit, not its work.
        let cens: Vec<(String, String)> = (0..censures)
            .map(|_| ("m0000".to_string(), "m0001".to_string()))
            .collect();
        let vou: Vec<(String, String, i64)> = (0..vouchers)
            .map(|i| ("m0001".to_string(), format!("m{:04}", 2 + i % members), 1))
            .collect();
        let start = Instant::now();
        let (_disc, _levels) = qucalc::gov::censure(&cens, &levels, &vou);
        println!(
            "{name},{universe},{members},{censures},{vouchers},{}",
            start.elapsed().as_millis()
        );
    }
}

/// **The three `rho:gov:*` folds C346 left unmeasured** (C354), timed against the charge they are
/// given.
///
/// The handler charges `gov_fold_cost(ballots + weights)` — two counts — for folds whose work is a
/// function of other inputs. This probe builds the shapes: a trust chain (one level of propagation per
/// round), a delegation chain (one walk per member), ranked ballots (one scan per round per ballot),
/// and the two tally modes with long option lists, **which the charged universe does not count at
/// all**. `charged` is the phlo the handler would charge for the same call, so `ms / charged`-order
/// amplification is readable directly.
#[tokio::test]
async fn the_other_governance_folds_are_measured() {
    if !probe_enabled() {
        return;
    }
    use std::collections::BTreeMap;

    println!("case,members,charged_phlo,ms");

    // `trustLevels`: an admin-rooted chain of depth `d` needs `d` rounds, each scanning `d` ratings.
    for d in [128usize, 256, 511] {
        let admins = vec!["m0000".to_string()];
        let ratings: Vec<(String, String, i64)> = (0..d)
            .map(|i| (format!("m{i:04}"), format!("m{:04}", i + 1), 5))
            .collect();
        let charged = (ratings.len() as i64 + admins.len() as i64).pow(2);
        let start = Instant::now();
        let _ = qucalc::gov::trust_levels(&ratings, &admins);
        println!("trust_levels,{d},{charged},{}", start.elapsed().as_millis());
    }

    // `resolveWeights`: every member walks the same delegation chain, so the work is `U * L`.
    for l in [128usize, 256, 511] {
        let mut delegations = BTreeMap::new();
        for i in 0..l {
            delegations.insert(format!("m{i:04}"), format!("m{:04}", i + 1));
        }
        let direct_voters = vec![format!("m{l:04}")];
        let charged = (direct_voters.len() + delegations.len()) as i64;
        let charged = charged * charged;
        let start = Instant::now();
        let _ = qucalc::gov::resolve_weights(&direct_voters, &delegations, &BTreeMap::new());
        println!(
            "resolve_weights,{l},{charged},{}",
            start.elapsed().as_millis()
        );
    }

    // `tally_ranked`: `b` ballots, each ranking `k` options. The handler charges `(2b)^2` — the
    // option lists are not in the universe at all.
    for (b, k) in [
        (16usize, 8usize),
        (64, 16),
        (128, 64),
        (100, 1_000),
        (100, 4_000),
    ] {
        let ballots: BTreeMap<String, Vec<String>> = (0..b)
            .map(|i| {
                (
                    format!("m{i:04}"),
                    (0..k).map(|o| format!("o{o:05}")).collect(),
                )
            })
            .collect();
        let weights: BTreeMap<String, i64> = (0..b).map(|i| (format!("m{i:04}"), 1)).collect();
        let charged = ((b + weights.len()) as i64).pow(2);
        let start = Instant::now();
        let _ = qucalc::gov::tally_ranked(&ballots, &weights);
        println!(
            "tally_ranked,{b}x{k},{charged},{}",
            start.elapsed().as_millis()
        );
    }

    // `tally_approval`: `b` ballots approving `k` options each — the approval lists are uncounted too.
    for (b, k) in [(128usize, 64usize), (128, 4_000)] {
        let ballots: BTreeMap<String, Vec<String>> = (0..b)
            .map(|i| {
                (
                    format!("m{i:04}"),
                    (0..k).map(|o| format!("o{o:05}")).collect(),
                )
            })
            .collect();
        let weights: BTreeMap<String, i64> = (0..b).map(|i| (format!("m{i:04}"), 1)).collect();
        let charged = ((b + weights.len()) as i64).pow(2);
        let start = Instant::now();
        let _ = qucalc::gov::tally_approval(&ballots, &weights);
        println!(
            "tally_approval,{b}x{k},{charged},{}",
            start.elapsed().as_millis()
        );
    }
}
