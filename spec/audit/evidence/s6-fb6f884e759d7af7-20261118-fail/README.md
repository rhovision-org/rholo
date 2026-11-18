# s6-fb6f884e759d7af7-20261118-fail

**Evidence locality.** `single-host`

## Scope

The parent-relative timestamp bound is retired; header time is informational and must not change a verdict.

The witness is `casper/src/validate.rs::a_block_timestamp_is_never_a_validity_input`. The mutation is:

```text
-    if let Err(status) = justification_regressions(dag, block).await? {
+    if block.timestamp == 0 {
        return Ok(Err(BlockStatus::InvalidBlockNumber));
    }
    if let Err(status) = justification_regressions(dag, block).await? {
```

## Provenance

| | |
|---|---|
| gate | `S6-fb6f884e759d7af7` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `f76700b987c634245a027b62abafd8582043203d978b1b42fea8ec5c2af5101f` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the injected stamp-0 refusal changes the verdict and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    validate::effectful_tests::a_block_timestamp_is_never_a_validity_input

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 29.07s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the old/new validator comparison and coordinated activation remain release obligations.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_block_timestamp_is_never_a_validity_input
```
