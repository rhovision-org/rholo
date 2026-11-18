# s6-e2cdf44a5cc0c81e-20261118-fail

**Evidence locality.** `single-host`

## Scope

The merge composes cost accounting per accepted deploy instead of a block carrying it.

The witness is `casper/src/merging.rs::a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge`. The mutation is:

```text
-    for chain in accepted {
+    for chain in accepted.iter().take(1) {
```

## Provenance

| | |
|---|---|
| gate | `S6-e2cdf44a5cc0c81e` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `cc88a6ff627d4b3b3b778a5224d878435d23aae8e6f67d6d1c28bf17ed6edd67` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation drops the other chains' moves and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    merging::boundary_merge_tests::a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 748 filtered out; finished in 0.02s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 15.22s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the live M5 replay is a release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge
```
