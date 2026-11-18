# s6-7b9fe37e74b318db-20261118

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
| binary | `287370e1473811d206daf9c37fde3dc48c4ea4360248c5a99c101c56012f5148` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the differing floors encode identically and the same test fails.

PASS log tail:

```text
running 1 test
test genesis::tests::the_validity_bounds_are_committed_in_the_genesis_params ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.83s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness; the genesis-hash/config/restore acceptance is a release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib the_validity_bounds_are_committed_in_the_genesis_params
```
