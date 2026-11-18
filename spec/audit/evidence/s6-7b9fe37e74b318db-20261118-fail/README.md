# s6-7b9fe37e74b318db-20261118-fail

**Evidence locality.** `single-host`

## Scope

The fee floor and parent bound are committed in the genesis params, not only in node config.

The witness is `casper/src/genesis/mod.rs::the_validity_bounds_are_committed_in_the_genesis_params`. The mutation is:

```text
-        min_phlo_price: proof_of_stake.min_phlo_price,
+        min_phlo_price: NonNegI64::zero(),
```

## Provenance

| | |
|---|---|
| gate | `S6-7b9fe37e74b318db` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `fd72e7705dff5d199381ee977848b7f52d2190f85fe18895f2f2877a407eac13` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the differing floors encode identically and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    genesis::tests::the_validity_bounds_are_committed_in_the_genesis_params

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.08s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the genesis-hash/config/restore acceptance is a release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib the_validity_bounds_are_committed_in_the_genesis_params
```
