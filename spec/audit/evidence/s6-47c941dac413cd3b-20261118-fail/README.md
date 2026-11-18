# s6-47c941dac413cd3b-20261118-fail

**Evidence locality.** `single-host`

## Scope

Epoch length 0 (the permissive default) means every block is an epoch boundary.

The witness is `rholang/src/native_state.rs::an_epoch_length_of_zero_makes_every_block_a_boundary`. The mutation is:

```text
-fn epoch_divisor(params: &PosParams) -> i64 {
    if params.epoch_length <= 0 {
        1
+fn epoch_divisor(params: &PosParams) -> i64 {
    if params.epoch_length <= 0 {
        0
```

## Provenance

| | |
|---|---|
| gate | `S6-47c941dac413cd3b` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `d07350e780cf4fa4d7552ce0404c332b5d1d7e23a5575cabd1d42c7bd10ccf9e` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the divisor zero and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    native_state::tests::an_epoch_length_of_zero_makes_every_block_a_boundary

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.48s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; a live epoch campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib an_epoch_length_of_zero_makes_every_block_a_boundary
```
