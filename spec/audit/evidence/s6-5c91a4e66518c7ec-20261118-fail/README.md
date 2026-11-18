# s6-5c91a4e66518c7ec-20261118-fail

**Evidence locality.** `single-host`

## Scope

The active validator set is a stake-weighted draw without replacement, seeded from the epoch.

The witness is `rholang/src/native_state.rs::splitting_a_stake_across_keys_does_not_buy_slots`. The mutation is:

```text
-        i128::from(i64::from(self.1))
+        1
```

## Provenance

| | |
|---|---|
| gate | `S6-5c91a4e66518c7ec` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `b989074778563a0d0ae8c64ff11d37d31ddf814ec49b3d734cfb56237e54d6ce` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the draw per-key uniform and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    native_state::tests::splitting_a_stake_across_keys_does_not_buy_slots

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.04s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.08s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the live active-set campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib splitting_a_stake_across_keys_does_not_buy_slots
```
