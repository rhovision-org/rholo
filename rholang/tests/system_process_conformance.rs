//! End-to-end conformance test for the native system processes (issue #24).
//!
//! Each process is called from rholang the way the documentation describes — by its `rho:*` urn,
//! with an acknowledgement channel — and the test asserts on the *shape of the answer*, not only
//! that the call succeeded. The unit tests in `system_processes.rs` exercise the handlers directly;
//! these tests additionally pin the urn wiring (`rho:qucalc:zfa`, `rho:gov:*`, `rho:registry:*`)
//! and the rholang-level argument shapes a caller actually uses.

mod common;

use std::collections::BTreeMap;

use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::{Expr, Par};
use rchain_models::par_ops::from_expr;
use rchain_models::rholang::RhoType::{
    RhoBoolean, RhoDeployerId, RhoList, RhoMap, RhoNil, RhoNumber, RhoString, RhoTupleN, RhoUri,
};
use rchain_models::sorted::SortedProc;
use rchain_rholang::runtime::RhoRuntime;

use common::build_runtime_pair;

/// A fixed, deterministic random seed so fresh-name allocation is reproducible.
fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[0u8; 32])
}

fn chan(name: &str) -> SortedProc {
    SortedProc::new(from_expr(Expr::GString(name.to_string())))
}

/// Evaluate `term` with the normalizer `env` and read the (single) result produced at `out_name`.
async fn eval_out(
    rt: &RhoRuntime,
    term: &str,
    env: &BTreeMap<String, Par>,
    out_name: &str,
) -> Vec<Par> {
    let res = rt
        .evaluate_with_env(term, env, &fixed_rand())
        .await
        .expect("evaluate returns Ok");
    assert!(
        res.succeeded(),
        "conformance term failed: {term:?}\nerrors: {:?}",
        res.errors
    );
    let data = rt
        .get_data_par(&chan(out_name))
        .await
        .expect("read out channel");
    assert!(
        !data.is_empty(),
        "term produced nothing at @{out_name:?}: {term:?}"
    );
    data
}

fn map_get_string_int(p: &Par, key: &str) -> Option<i64> {
    let kvs = RhoMap::unapply(p)?;
    kvs.iter().find_map(|(k, v)| {
        RhoString::unapply(k)
            .filter(|s| *s == key)
            .and_then(|_| RhoNumber::unapply(v))
    })
}

fn list_strings(p: &Par) -> Option<Vec<String>> {
    let ps = RhoList::unapply(p)?;
    ps.iter()
        .map(|p| RhoString::unapply(p).map(|s| s.to_string()))
        .collect()
}

fn list_numbers(p: &Par) -> Option<Vec<i64>> {
    RhoList::unapply(p)?
        .iter()
        .map(RhoNumber::unapply)
        .collect()
}

#[tokio::test]
async fn qucalc_zfa_reports_zfa_and_phase_for_closed_and_open_histories() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    // ^v = [0, 1] is Pauli-closed and count-balanced: (true, -1).
    let closed = eval_out(
        &rt,
        r#"new zfa(`rho:qucalc:zfa`), ack in { zfa!([0, 1], *ack) | for (@res <- ack) { @"out"!(res) } }"#,
        &env,
        "out",
    )
    .await;
    let parts = RhoTupleN::unapply(&closed[0]).expect("qucalc:zfa returns a (zfa, phase) tuple");
    assert_eq!(parts.len(), 2);
    assert_eq!(RhoBoolean::unapply(&parts[0]), Some(true));
    assert_eq!(RhoNumber::unapply(&parts[1]), Some(-1));

    // A single twist is neither Pauli-closed nor count-balanced: (false, 0).
    let open = eval_out(
        &rt,
        r#"new zfa(`rho:qucalc:zfa`), ack in { zfa!([0], *ack) | for (@res <- ack) { @"out2"!(res) } }"#,
        &env,
        "out2",
    )
    .await;
    let parts = RhoTupleN::unapply(&open[0]).expect("qucalc:zfa returns a (zfa, phase) tuple");
    assert_eq!(RhoBoolean::unapply(&parts[0]), Some(false));
    assert_eq!(RhoNumber::unapply(&parts[1]), Some(0));
}

#[tokio::test]
async fn qucalc_grant_and_verify_round_trip() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    // A ZFA-closed history mints a capability URI.
    let uri_par = eval_out(
        &rt,
        r#"new grant(`rho:qucalc:grant`), ack in { grant!([0, 1], *ack) | for (@uri <- ack) { @"out"!(uri) } }"#,
        &env,
        "out",
    )
    .await;
    let uri = RhoUri::unapply(&uri_par[0])
        .expect("qucalc:grant returns a uri for a ZFA-closed history")
        .to_string();

    // A history that is not ZFA-closed yields Nil.
    let nil = eval_out(
        &rt,
        r#"new grant(`rho:qucalc:grant`), ack in { grant!([0], *ack) | for (@res <- ack) { @"out2"!(res) } }"#,
        &env,
        "out2",
    )
    .await;
    assert!(
        RhoNil::unapply(&nil[0]),
        "qucalc:grant returns Nil for a non-ZFA history"
    );

    // The minted capability verifies; an unknown uri does not.
    let ok = eval_out(
        &rt,
        &format!(
            r#"new verify(`rho:qucalc:verify`), ack in {{ verify!(`{uri}`, *ack) | for (@res <- ack) {{ @"out3"!(res) }} }}"#
        ),
        &env,
        "out3",
    )
    .await;
    assert_eq!(RhoBoolean::unapply(&ok[0]), Some(true));

    let bad = eval_out(
        &rt,
        r#"new verify(`rho:qucalc:verify`), ack in { verify!(`rho:id:deadbeef`, *ack) | for (@res <- ack) { @"out4"!(res) } }"#,
        &env,
        "out4",
    )
    .await;
    assert_eq!(RhoBoolean::unapply(&bad[0]), Some(false));
}

#[tokio::test]
async fn qucalc_fuse_returns_geometry_and_capability_or_nil() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    // Socrates syllogism: subject ^< ([0,3]) + predicate >v ([2,1]) -> ^<>v, a stable fluxoid.
    let fused = eval_out(
        &rt,
        r#"new fuse(`rho:qucalc:fuse`), ack in { fuse!([0, 3], [2, 1], *ack) | for (@res <- ack) { @"out"!(res) } }"#,
        &env,
        "out",
    )
    .await;
    let parts = RhoTupleN::unapply(&fused[0]).expect("qucalc:fuse returns (geometry, cap)");
    assert_eq!(parts.len(), 2);
    let geometry = RhoList::unapply(&parts[0]).expect("geometry is a twist list");
    let twists: Vec<i64> = geometry
        .iter()
        .map(|p| RhoNumber::unapply(p).expect("twist number"))
        .collect();
    assert_eq!(twists, vec![0, 3, 2, 1]);
    assert!(
        RhoUri::unapply(&parts[1]).is_some(),
        "fuse mints a capability uri"
    );

    // A subject/predicate whose residue is not ZFA-closed yields Nil.
    let nil = eval_out(
        &rt,
        r#"new fuse(`rho:qucalc:fuse`), ack in { fuse!([0], [2], *ack) | for (@res <- ack) { @"out2"!(res) } }"#,
        &env,
        "out2",
    )
    .await;
    assert!(
        RhoNil::unapply(&nil[0]),
        "qucalc:fuse returns Nil for a non-ZFA residue"
    );
}

#[tokio::test]
async fn gov_resolve_weights_reports_weight_map_from_tuple_shaped_inputs() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    let weights = eval_out(
        &rt,
        r#"new resolve(`rho:gov:resolveWeights`), ack in { resolve!(["A", "C"], {"B": "A", "C": "B"}, {}, *ack) | for (@res <- ack) { @"out"!(res) } }"#,
        &env,
        "out",
    )
    .await;
    assert!(
        RhoMap::unapply(&weights[0]).is_some(),
        "resolveWeights returns a map"
    );
    assert_eq!(map_get_string_int(&weights[0], "A"), Some(2));
    assert_eq!(map_get_string_int(&weights[0], "C"), Some(1));
}

#[tokio::test]
async fn gov_trust_levels_reports_admin_rooted_level_map() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    let levels = eval_out(
        &rt,
        r#"new trust(`rho:gov:trustLevels`), ack in { trust!([("Alice", "Bob", 3), ("Bob", "Carol", 2)], ["Alice"], *ack) | for (@res <- ack) { @"out"!(res) } }"#,
        &env,
        "out",
    )
    .await;
    assert!(
        RhoMap::unapply(&levels[0]).is_some(),
        "trustLevels returns a map"
    );
    assert_eq!(map_get_string_int(&levels[0], "Alice"), Some(5));
    assert_eq!(map_get_string_int(&levels[0], "Bob"), Some(3));
    assert_eq!(map_get_string_int(&levels[0], "Carol"), Some(2));
}

#[tokio::test]
async fn gov_censure_reports_discredited_and_slashed_levels() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    let out = eval_out(
        &rt,
        r#"new censure(`rho:gov:censure`), ack in { censure!([("A", "D"), ("B", "D")], {"A": 5, "B": 5, "C": 5, "D": 0}, [("A", "D", 2), ("B", "D", 1)], *ack) | for (@res <- ack) { @"out"!(res) } }"#,
        &env,
        "out",
    )
    .await;
    let parts = RhoTupleN::unapply(&out[0]).expect("gov:censure returns (discredited, newLevels)");
    assert_eq!(parts.len(), 2);
    assert_eq!(list_strings(&parts[0]), Some(vec!["D".to_string()]));
    assert_eq!(map_get_string_int(&parts[1], "A"), Some(3));
    assert_eq!(map_get_string_int(&parts[1], "B"), Some(4));
    assert_eq!(map_get_string_int(&parts[1], "C"), Some(5));
}

#[tokio::test]
async fn gov_tally_reports_winner_for_ranked_and_approval_modes() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    // A, B favour X; C favours Z, then X. Weighted ranked-choice elects X.
    let ranked = eval_out(
        &rt,
        r#"new tally(`rho:gov:tally`), ack in { tally!({"A": ["X", "Y"], "B": ["Y", "X"], "C": ["Z", "X"]}, {"A": 2, "B": 2, "C": 1}, "ranked", *ack) | for (@res <- ack) { @"out"!(res) } }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(RhoString::unapply(&ranked[0]), Some("X"));

    // Approval mode with the same ballots also elects X (X appears on every ballot).
    let approval = eval_out(
        &rt,
        r#"new tally(`rho:gov:tally`), ack in { tally!({"A": ["X", "Y"], "B": ["Y", "X"], "C": ["Z", "X"]}, {"A": 2, "B": 2, "C": 1}, "approval", *ack) | for (@res <- ack) { @"out2"!(res) } }"#,
        &env,
        "out2",
    )
    .await;
    assert_eq!(RhoString::unapply(&approval[0]), Some("X"));
}

#[tokio::test]
async fn registry_insert_arbitrary_and_lookup_round_trip() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    let uri_par = eval_out(
        &rt,
        r#"new insert(`rho:registry:insertArbitrary`), ack in { insert!(42, *ack) | for (@uri <- ack) { @"out"!(uri) } }"#,
        &env,
        "out",
    )
    .await;
    let uri = RhoUri::unapply(&uri_par[0])
        .expect("insertArbitrary returns a uri")
        .to_string();

    let looked_up = eval_out(
        &rt,
        &format!(
            r#"new lookup(`rho:registry:lookup`), ack in {{ lookup!(`{uri}`, *ack) | for (@res <- ack) {{ @"out2"!(res) }} }}"#
        ),
        &env,
        "out2",
    )
    .await;
    // The reply is the stored value alone, never wrapped in its own uri (C18): an oracle-era client
    // consumes it as `for (X <- ch) { X!(…) }`, and a pair would bind to the name, making the send a
    // silent no-op rather than a type error.
    assert_eq!(
        RhoNumber::unapply(&looked_up[0]),
        Some(42),
        "lookup replies with the stored value alone, not (uri, value)"
    );

    // A uri that was never registered answers Nil (not silence).
    let missing = eval_out(
        &rt,
        r#"new lookup(`rho:registry:lookup`), ack in { lookup!(`rho:id:never-registered`, *ack) | for (@res <- ack) { @"out3"!(res) } }"#,
        &env,
        "out3",
    )
    .await;
    assert!(
        RhoNil::unapply(&missing[0]),
        "lookup of an unknown uri returns Nil"
    );
}

#[tokio::test]
async fn registry_insert_signed_binds_deployer_id_from_the_normalizer_env() {
    let (rt, _) = build_runtime_pair().await;
    // `rho:rchain:deployerId` is unbound in an exploratory deploy; a signed deploy gets it from the
    // normalizer env. Inject the same shape the deploy path uses.
    let mut env = BTreeMap::new();
    env.insert(
        "rho:rchain:deployerId".to_string(),
        RhoDeployerId::apply(vec![0x11; 65]),
    );

    let uri_par = eval_out(
        &rt,
        r#"new insert(`rho:registry:insertSigned:secp256k1`), deployerId(`rho:rchain:deployerId`), ack in { insert!((1, "data"), *deployerId, *ack) | for (@uri <- ack) { @"out"!(uri) } }"#,
        &env,
        "out",
    )
    .await;
    let uri = RhoUri::unapply(&uri_par[0])
        .expect("insertSigned returns a uri")
        .to_string();

    let looked_up = eval_out(
        &rt,
        &format!(
            r#"new lookup(`rho:registry:lookup`), ack in {{ lookup!(`{uri}`, *ack) | for (@res <- ack) {{ @"out2"!(res) }} }}"#
        ),
        &env,
        "out2",
    )
    .await;
    // The reply is the stored value alone — and for `insertSigned` that value is itself the
    // `(nonce, data)` pair it recorded, which is why consumers of *system* contracts destructure it
    // as `@(_, X)`. The uri is not echoed back (C18).
    let stored =
        RhoTupleN::unapply(&looked_up[0]).expect("insertSigned stores a (nonce, data) tuple");
    assert_eq!(RhoNumber::unapply(&stored[0]), Some(1));
    assert_eq!(RhoString::unapply(&stored[1]), Some("data"));
}

/// The idiom **every** oracle-era client is written in: look a contract up, then send to the reply.
///
/// This is the assertion whose absence let C18 ship. Checking only that the reply *contains* the
/// right value misses the failure mode entirely: a `(uri, value)` wrapper still carries the value,
/// so a shape assertion on a scalar reply can pass by reading the right element — but the client
/// binds the pair to a name and its send becomes a **silent no-op**. Nothing errors; the deploy
/// simply produces no result, which is how the whole rgov contract family came to return `[]`.
/// So assert the reachability, not just the shape.
#[tokio::test]
async fn a_looked_up_contract_can_be_called_through_its_lookup_reply() {
    let (rt, _) = build_runtime_pair().await;
    let env = BTreeMap::new();

    let reached = eval_out(
        &rt,
        r#"new target, ins(`rho:registry:insertArbitrary`), lookup(`rho:registry:lookup`), ack, call in {
             contract target(@x, ret) = { ret!(["got", x]) } |
             ins!(bundle+{*target}, *ack) |
             for (@uri <- ack) {
               lookup!(uri, *call) |
               for (@T <- call) {
                 new reply in { @T!("ping", *reply) | for (@r <- reply) { @"out"!(r) } }
               }
             }
           }"#,
        &env,
        "out",
    )
    .await;

    assert_eq!(
        list_strings(&reached[0]),
        Some(vec!["got".to_string(), "ping".to_string()]),
        "the looked-up contract must be reachable through its lookup reply"
    );
}

/// A collection pattern may name only *part* of the collection: `..._` absorbs the rest, `...rest`
/// absorbs it and binds it. Every rgov contract reaches its capabilities through such a pattern —
/// `@{"read": *MCA, ..._}` against a three-key dictionary — so while a partial *map* (or *set*)
/// pattern could not match, all of their bodies were unreachable, and *silently*: a `for` whose
/// pattern does not match is not an error, it just never fires. Lists happened to work and maps did
/// not, so both must be pinned.
///
/// Each case gets its **own runtime**, because a shared one is exactly how this defect hid. The
/// first version of this test evaluated all five shapes against one runtime, reading a shared
/// `@"out"`: once the first (list) case had produced `"ok"`, every later case passed by re-reading
/// that datum, so the map and set cases could not fail — and did not run their patterns at all. A
/// conformance test that cannot fail is not evidence about the node (C20).
#[tokio::test]
async fn collection_patterns_match_a_subset_of_their_collection() {
    let env = BTreeMap::new();

    for (label, term) in [
        (
            "list, wildcard remainder",
            r#"new a in { a!([1, 2, 3]) | for (@[1, ..._] <- a) { @"out"!("ok") } }"#,
        ),
        (
            "list, named remainder",
            r#"new a in { a!([1, 2, 3]) | for (@[1, ...rest] <- a) { @"out"!("ok") } }"#,
        ),
        (
            "map, wildcard remainder",
            r#"new a in { a!({"x": 1, "y": 2}) | for (@{"x": *v, ..._} <- a) { @"out"!("ok") } }"#,
        ),
        (
            "map, named remainder",
            r#"new a in { a!({"x": 1, "y": 2}) | for (@{"x": *v, ...rest} <- a) { @"out"!("ok") } }"#,
        ),
        (
            "set, wildcard remainder",
            r#"new a in { a!(Set(1, 2, 3)) | for (@Set(1, ..._) <- a) { @"out"!("ok") } }"#,
        ),
        (
            "set, named remainder",
            r#"new a in { a!(Set(1, 2, 3)) | for (@Set(1, ...rest) <- a) { @"out"!("ok") } }"#,
        ),
    ] {
        let (rt, _) = build_runtime_pair().await;
        let got = eval_out(&rt, term, &env, "out").await;
        assert_eq!(got.len(), 1, "{label}: exactly one result");
        assert_eq!(
            RhoString::unapply(&got[0]),
            Some("ok"),
            "{label}: a partial collection pattern must match"
        );
    }

    // A remainder is what makes a collection pattern *non-concrete*, and a map pattern was the one
    // place the port forgot to say so: the list and set folds OR it into `connective_used`
    // (`normalizer.rs`), the map fold did not, so `spatial_match` saw a concrete pattern, took its
    // `pattern == target` short-circuit, and a partial map matched nothing but itself — silently.
    // The cases above all carry a free variable (`*v`), which sets the flag for an unrelated reason;
    // these carry only *ground* entries and a remainder, which is the shape that was never covered.
    // (AUDIT C19-C21's family; `CollectionNormalizeMatcher.scala:92` is the reference.)
    let (rt, _) = build_runtime_pair().await;
    let got = eval_out(
        &rt,
        r#"new a in { a!({"x": 1, "y": 2}) | for (@{"x": 1, ..._} <- a) { @"out"!("matched") } }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(
        got.len(),
        1,
        "a ground map pattern with a remainder must match"
    );
    assert_eq!(RhoString::unapply(&got[0]), Some("matched"));

    // ...and it must still *fail* to match when the ground entry differs — the fix must not turn a
    // partial pattern into a wildcard. A must-not-match case cannot be observed by absence alone
    // (`eval_out` requires output, and "the term produced nothing" is exactly the failure mode this
    // file exists to distinguish), so the same term carries a control: the control datum is present
    // and the match's tag is not.
    let (rt, _) = build_runtime_pair().await;
    let got = eval_out(
        &rt,
        r#"new a in { a!({"x": 1, "y": 2}) |
             for (@{"x": 9, ..._} <- a) { @"out"!("matched") } |
             @"out"!("control") }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(got.len(), 1, "only the control datum may appear: {got:?}");
    assert_eq!(
        RhoString::unapply(&got[0]),
        Some("control"),
        "a partial pattern must still be a *pattern*: {{\"x\": 9}} does not match {{\"x\": 1, ...}}"
    );

    // The named remainder binds the entries it absorbs, ground ones included.
    let (rt, _) = build_runtime_pair().await;
    let rest = eval_out(
        &rt,
        r#"new a in { a!({"x": 1, "y": 2}) | for (@{"x": 1, ...rest} <- a) { @"out"!(rest) } }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(rest.len(), 1);
    assert_eq!(
        map_get_string_int(&rest[0], "y"),
        Some(2),
        "`...rest` captured the unnamed ground entry"
    );
    assert_eq!(
        map_get_string_int(&rest[0], "x"),
        None,
        "`...rest` must not capture the named entry"
    );

    // The `MemberDirectory.rho:15` gate itself, with production shapes: a three-key dictionary of
    // *bundles* (not ground terms) peeked with a partial map pattern. Two claims in one — the body
    // runs, and the named entry binds its own value rather than one the wildcard absorbed.
    let (rt, _) = build_runtime_pair().await;
    let reached = eval_out(
        &rt,
        r#"new dict in {
             dict!({"read": 1, "write": 2, "grant": 3}) |
             for (@{"read": *MCAread, ..._} <<- dict) { @"out"!(*MCAread) }
           }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(reached.len(), 1, "the peek gate fires once");
    assert_eq!(
        RhoNumber::unapply(&reached[0]),
        Some(1),
        "`*MCAread` is the value at \"read\", not at a key the wildcard absorbed"
    );

    let (rt, _) = build_runtime_pair().await;
    let reached = eval_out(
        &rt,
        r#"new read, write, grant, dict in {
             dict!({"read": bundle+{*read}, "write": bundle+{*write}, "grant": bundle+{*grant}}) |
             for (@{"read": *MCAread, ..._} <<- dict) { @"out"!("gate-open") }
           }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(
        reached.len(),
        1,
        "the gate fires once over bundle-valued entries"
    );
    assert_eq!(RhoString::unapply(&reached[0]), Some("gate-open"));

    // A *named* map remainder captures the unnamed entries and not the named one. The remainder is
    // a *process* variable — referenced unstarred (`rest`), unlike the quoted name patterns `*v` —
    // which is how the rgov contracts write it (`Group.rho:68-71`).
    let (rt, _) = build_runtime_pair().await;
    let rest = eval_out(
        &rt,
        r#"new dict in {
             dict!({"read": 1, "write": 2, "grant": 3}) |
             for (@{"read": *v, ...rest} <- dict) { @"out"!(rest) }
           }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(rest.len(), 1);
    assert_eq!(
        map_get_string_int(&rest[0], "write"),
        Some(2),
        "`...rest` captured \"write\""
    );
    assert_eq!(
        map_get_string_int(&rest[0], "grant"),
        Some(3),
        "`...rest` captured \"grant\""
    );
    assert_eq!(
        map_get_string_int(&rest[0], "read"),
        None,
        "`...rest` must not capture the named entry"
    );

    // Lists keep positional/suffix semantics: the remainder is a *suffix*, not "any elements".
    let (rt, _) = build_runtime_pair().await;
    let rest = eval_out(
        &rt,
        r#"new a in { a!([1, 2, 3]) | for (@[1, 2, ...rest] <- a) { @"out"!(rest) } }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(rest.len(), 1);
    assert_eq!(
        list_numbers(&rest[0]),
        Some(vec![3]),
        "`@[1, 2, ...rest]` on [1, 2, 3] leaves rest = [3]"
    );
}

/// An `if` means the same thing wherever it sits in its `par`. A condition normalized against the
/// terms that *precede* the `if` makes the condition be those terms, so the `Match` target matches
/// neither `true` nor `false` and the `if` reduces to **nothing** — silently, because an unmatched
/// `match` is not an error. `MemberDirectory.rho:77-79` is exactly this shape (a log send, then
/// `if (everyone.contains(you) == false)`), which is why `getMe` logged its line and then neither
/// created the member nor answered (AUDIT C21).
///
/// The first case is the shape in the wild, the second is the property that makes it a defect rather
/// than a quirk: wrapping the `par` changes nothing, so the `if` cannot depend on its position. Each
/// case gets its own runtime and reports through its own channel — the discipline C20's post-mortem
/// demands, because a shared runtime let a previous case's datum satisfy a later case's assertion.
#[tokio::test]
async fn an_if_fires_the_same_way_wherever_it_sits_in_a_par() {
    let env = BTreeMap::new();

    for (label, term) in [
        (
            "if after a send, at the top level",
            r#"new x in { x!(["before"]) |
                 if (1 == 1) { @"out"!("then") } else { @"out"!("else") } }"#,
        ),
        (
            "if after a send, inside a receive body",
            r#"new m, y in { m!({}) | y!((true, "addr")) |
                 for (@(true, you) <- y) { for (@everyone <<- m) { Nil
                   |  @"before"!("line") |
                   if (everyone.contains(you) == false ) { @"out"!("then") } else { @"out"!("else") } } } }"#,
        ),
        (
            "if first, then the same send",
            r#"new x in {
                 if (1 == 1) { @"out"!("then") } else { @"out"!("else") } | x!(["after"]) }"#,
        ),
    ] {
        let (rt, _) = build_runtime_pair().await;
        let got = eval_out(&rt, term, &env, "out").await;
        assert_eq!(got.len(), 1, "{label}: exactly one branch must run");
        assert_eq!(
            RhoString::unapply(&got[0]),
            Some("then"),
            "{label}: the branch the condition selects"
        );
    }

    // The other side of the same coin: a condition that is *false* takes the else branch, and an
    // `if` preceded by a send must not be swallowed on the way there either.
    let (rt, _) = build_runtime_pair().await;
    let got = eval_out(
        &rt,
        r#"new x in { x!(["before"]) |
             if (1 == 2) { @"out"!("then") } else { @"out"!("else") } }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(got.len(), 1, "exactly one branch must run");
    assert_eq!(RhoString::unapply(&got[0]), Some("else"));

    // ...and a `match` in the same position keeps working (it always did: its target was already
    // normalized against a fresh par — the two desugarings must not disagree).
    let (rt, _) = build_runtime_pair().await;
    let got = eval_out(
        &rt,
        r#"new x in { x!(["before"]) | match 1 { 1 => @"out"!("matched") } }"#,
        &env,
        "out",
    )
    .await;
    assert_eq!(got.len(), 1, "the match must fire");
    assert_eq!(RhoString::unapply(&got[0]), Some("matched"));
}

#[tokio::test]
async fn txn_recover_unknown_returns_nil() {
    let (rt, _replay) = build_runtime_pair().await;
    let data = eval_out(
        &rt,
        r#"new txn(`rho:txn`), ret in { txn!("recover", "deadbeef".hexToBytes(), *ret) | for (@r <- ret) { @"out"!(r) } }"#,
        &BTreeMap::new(),
        "out",
    )
    .await;
    assert_eq!(data.len(), 1);
    assert!(
        RhoNil::unapply(&data[0]),
        "recover on an unknown transaction must return Nil"
    );
}

/// **`rho:crypto:p256Verify` from a deploy, by urn** (issue #560). The unit test in
/// `system_processes.rs` checks the definition and its handler; this one resolves the urn through
/// the runtime a deploy actually gets, and pins both verdicts. The fixture is a real
/// WebCrypto-generated raw `r || s` over the 59-byte message whose hex is passed here — the same
/// fixture as `crypto/src/signatures/p256.rs`.
#[tokio::test]
async fn p256_verify_checks_a_webcrypto_signature_by_urn() {
    const MESSAGE: &str = "42594f49442044494420637265617465206576656e743a207b22736571223a312c2274797065223a22637265617465227d";
    const SIGNATURE: &str = "8e4492b9c343eac74a0b3c973e0e1a0f418256dcf1bad4f7481a6f66da500db38774f4c9159debb82a5f125107ca9e24beb7cdbd6042e9cbe13f2e208bc3a5e4";
    const PUBKEY: &str = "04e5afd086093b7e48b087683b8e1d3416d3dc8fc80c2a9fbf6080ed223602b49b9d0cc80f98afc9b1e738944a49517c322cce297f9c5c2072b7942143425684e3";
    let (rt, _replay) = build_runtime_pair().await;
    let env = BTreeMap::new();

    let term = |out: &str, message: &str| {
        format!(
            r#"new verify(`rho:crypto:p256Verify`), ack in {{
                 verify!("{message}".hexToBytes(), "{SIGNATURE}".hexToBytes(), "{PUBKEY}".hexToBytes(), *ack) |
                 for (@res <- ack) {{ @"{out}"!(res) }}
               }}"#
        )
    };

    let ok = eval_out(&rt, &term("out", MESSAGE), &env, "out").await;
    assert_eq!(
        RhoBoolean::unapply(&ok[0]),
        Some(true),
        "a WebCrypto P-256 signature must verify through the urn a deploy resolves"
    );

    let bad = eval_out(&rt, &term("out2", "00"), &env, "out2").await;
    assert_eq!(
        RhoBoolean::unapply(&bad[0]),
        Some(false),
        "and a different message must not"
    );
}
