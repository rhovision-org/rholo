# s6-47c941dac413cd3b-20261118

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
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the divisor zero and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::tests::an_epoch_length_of_zero_makes_every_block_a_boundary ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.67s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; a live epoch campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib an_epoch_length_of_zero_makes_every_block_a_boundary
```
