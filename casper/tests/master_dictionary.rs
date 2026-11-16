//! The rooted-name master dictionary, **against the dictionary as deployed in genesis** (issue #99).
//!
//! The issue closes when "the namespace rule is stated (a `decide`-able model) **and the probes pass
//! against the deployed dictionary, the four must-fail cases included**". This is the second half:
//! every probe below resolves the facet a client would, at the constant key genesis publishes it
//! under, and runs as a *deploy signed by a real key* — so the caller's identity is derived exactly
//! the way the contract derives it.
//!
//! The probes are one ordered genesis: the dictionary bootstraps itself, then each probe runs as its
//! own deploy. That ordering is load-bearing, not stylistic — `publish` is append-only, so a probe
//! that pins version 0 only means something after a second version exists.

mod common;

use common::dictionary::*;

/// The alias tier the genesis bootstrap built — every name points somewhere, and an unknown name
/// answers `Nil` ("absence is not an error").
#[test]
fn the_genesis_alias_tier_resolves() {
    with_big_stack(async {
        let r = resolve_uri();
        let mut probes: Vec<(String, u8)> = [
            "Directory",
            "Echo",
            "Log",
            "Inbox",
            "Issue",
            "Kudos",
            "Roll",
            "Chat",
            "Ballot",
            "Group",
            "GetMe",
            "SendThem",
        ]
        .iter()
        .map(|name| {
            // The exact target is the *operator's* own rooted path, derived at genesis; the claim
            // that matters here is that the name is aliased at all, and `Nil` is what an unaliased
            // name answers. The name-by-name targets are pinned in `genesis_registry.rs`.
            (
                probe_cond(&r, "targetOf", &format!(r#"["{name}"]"#), false, "v != Nil"),
                11,
            )
        })
        .collect();
        probes.push((probe(&r, "targetOf", r#"["NoSuchName"]"#, false, "Nil"), 11));
        let tags = run(probes).await;
        assert_eq!(
            tags,
            vec!["ok"; 13],
            "every genesis name is aliased, and an unknown name is absent"
        );
    });
}

/// Append-only publish, pinned versions, the owner prefix, and absence-is-not-an-error.
#[test]
fn publish_is_append_only_and_versions_are_pinned() {
    with_big_stack(async {
        let (r, p) = (resolve_uri(), publish_uri());
        let alice = rev_address_of(11);
        let own = format!("{alice}/inbox");
        let tags = run(vec![
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{own}", "v0"]"#),
                    true,
                    &format!(r#"("published", "{own}", 0)"#),
                ),
                11,
            ),
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{own}", "v1"]"#),
                    true,
                    &format!(r#"("published", "{own}", 1)"#),
                ),
                11,
            ),
            (
                probe(&r, "resolve", &format!(r#"["{own}"]"#), false, r#""v1""#),
                11,
            ),
            (
                probe(
                    &r,
                    "resolveAt",
                    &format!(r#"["{own}", 0]"#),
                    false,
                    r#""v0""#,
                ),
                11,
            ),
            (
                probe(&r, "resolveAt", &format!(r#"["{own}", 9]"#), false, "Nil"),
                11,
            ),
            (
                probe(&r, "resolve", r#"["no-such-path"]"#, false, "Nil"),
                11,
            ),
            (
                probe(
                    &r,
                    "ownerOf",
                    &format!(r#"["{own}"]"#),
                    false,
                    &format!(r#""{alice}""#),
                ),
                11,
            ),
            (
                probe_cond(
                    &r,
                    "versionsOf",
                    &format!(r#"["{own}"]"#),
                    false,
                    r#"v == ["v0", "v1"]"#,
                ),
                11,
            ),
        ])
        .await;
        assert_eq!(tags, vec!["ok"; 8], "the publish/resolve round trip");
    });
}

/// The four must-fail cases, plus a forged identity, an unknown verb and a wrong arity.
#[test]
fn the_four_must_fail_cases() {
    with_big_stack(async {
        let (r, p, a) = (resolve_uri(), publish_uri(), root_uri());
        let alice = rev_address_of(11);
        let tags = run(vec![
            // 1. writing under another's root
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{alice}/inbox", "bob"]"#),
                    true,
                    r#"("dir-error", "not your namespace")"#,
                ),
                12,
            ),
            // a forged / absent identity
            (
                probe(
                    &p,
                    "publish",
                    r#"["not-an-id", "x/y", "v"]"#,
                    false,
                    r#"("dir-error", "no identity")"#,
                ),
                11,
            ),
            // 4. a non-root identity setting a short name
            (
                probe(
                    &a,
                    "alias",
                    r#"[*deployerId, "Inbox", "rho:id:evil"]"#,
                    true,
                    r#"("dir-error", "not the root authority")"#,
                ),
                12,
            ),
            // an unknown verb, and a known verb at the wrong arity
            (
                probe(
                    &r,
                    "nonsense",
                    r#"[]"#,
                    false,
                    r#"("dir-error", "bad verb or arity")"#,
                ),
                11,
            ),
            (
                probe(
                    &r,
                    "resolve",
                    r#"[]"#,
                    false,
                    r#"("dir-error", "bad verb or arity")"#,
                ),
                11,
            ),
        ])
        .await;
        assert_eq!(tags, vec!["ok"; 5], "every refusal answers");
    });
}

/// `seal` closes a path to further versions — the other two of the four must-fail cases, which need
/// a published path first.
#[test]
fn seal_closes_a_path_and_the_key_still_resolves() {
    with_big_stack(async {
        let (r, p) = (resolve_uri(), publish_uri());
        let p0 = format!("{}/sealed", rev_address_of(11));
        let tags = run(vec![
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{p0}", "v0"]"#),
                    true,
                    &format!(r#"("published", "{p0}", 0)"#),
                ),
                11,
            ),
            (
                probe(
                    &p,
                    "seal",
                    &format!(r#"[*deployerId, "{p0}"]"#),
                    true,
                    &format!(r#"("sealed", "{p0}")"#),
                ),
                11,
            ),
            // 3. publishing to a sealed path
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{p0}", "v1"]"#),
                    true,
                    r#"("dir-error", "sealed")"#,
                ),
                11,
            ),
            (
                probe(&r, "resolve", &format!(r#"["{p0}"]"#), false, r#""v0""#),
                11,
            ),
            (
                probe(&r, "sealed", &format!(r#"["{p0}"]"#), false, "true"),
                11,
            ),
        ])
        .await;
        assert_eq!(
            tags,
            vec!["ok"; 5],
            "seal closes the path and it still resolves"
        );
    });
}

/// The second of the four must-fail cases: a granted writekey dies at `revoke`.
///
/// The probe is written the way a consumer writes it — `grant`, take the key, publish through it —
/// and it is the shape that caught a real mistake while this was being written: `for (@wk <- key)`
/// binds the key as a **value**, and a value cannot be called. It must be `for (wk <- key)`.
#[test]
fn a_granted_key_works_until_it_is_revoked() {
    with_big_stack(async {
        let (r, p) = (resolve_uri(), publish_uri());
        let path = format!("{}/keyed", rev_address_of(11));
        let use_key = |then: &str, value: &str, expect: String| {
            format!(
                r#"new deployerId(`rho:rchain:deployerId`), rl(`rho:registry:lookup`),
                     fCh, key, ret, r2
                   in {{
                     rl!(`{p}`, *fCh) |
                     for (f <- fCh) {{
                       f!("grant", [*deployerId, "{path}"], *key) |
                       for (wk <- key) {{
                         f!("{then}", [*deployerId, "{path}"], *ret) |
                         for (@_a <- ret) {{
                           wk!("{value}", *r2) |
                           for (@v <- r2) {{
                             if (v == {expect}) {{ @"out"!("ok") }} else {{ @"out"!("ne") }}
                           }}
                         }}
                       }}
                     }}
                   }}"#
            )
        };
        let tags = run(vec![
            (
                use_key("publish", "a", format!(r#"("published", "{path}", 0)"#)),
                11,
            ),
            // ...and after a revoke the same shape is refused: that is the must-fail case.
            (
                use_key("revoke", "b", r#"("dir-error", "revoked")"#.to_string()),
                11,
            ),
            // The revoked key did not append, so version 1 is still absent.
            (
                probe(&r, "resolveAt", &format!(r#"["{path}", 1]"#), false, "Nil"),
                11,
            ),
        ])
        .await;
        assert_eq!(tags, vec!["ok"; 3], "the key works, then revoke kills it");
    });
}
