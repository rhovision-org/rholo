# s6-a03b7b63647b8746-20261118-fail

**Evidence locality.** `single-host`

## Scope

A block's justification set is bounded by max-number-of-parents and refused on the receiving side.

The witness is `casper/src/validate.rs::a_block_justifying_more_parents_than_the_network_allows_is_refused`. The mutation is:

```text
-    if max_number_of_parents <= 0 || b.justifications.len() as u128 <= max_number_of_parents as u128
+    if max_number_of_parents <= 0 || (b.justifications.len() as u128) < max_number_of_parents as u128
```

## Provenance

| | |
|---|---|
| gate | `S6-a03b7b63647b8746` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `3d8d2c52edefa3f17f7f6dcd8ba5419f1c842a52fba3e32f6c8d129e2df15648` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation admits one extra parent and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    validate::tests::a_block_justifying_more_parents_than_the_network_allows_is_refused

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.00s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the committed-bound rollout is a separate release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_block_justifying_more_parents_than_the_network_allows_is_refused
```
