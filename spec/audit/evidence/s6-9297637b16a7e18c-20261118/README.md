# s6-9297637b16a7e18c-20261118

**Evidence locality.** `single-host`

## Scope

An epoch's reward for a key with delegators is split pro-rata, and a delegator's share is credited to pos:delegated_rewards.

The witness is `rholang/src/native_state.rs::a_boundary_splits_the_reward_across_the_delegations`. The mutation is:

```text
-            let (shares, operator_share) = split_reward(i64::from(*reward), own, &amounts)?;
+            let (shares, operator_share) = (Vec::new(), i64::from(*reward));
```

## Provenance

| | |
|---|---|
| gate | `S6-9297637b16a7e18c` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation empties the delegated split and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::delegation_tests::a_boundary_splits_the_reward_across_the_delegations ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.39s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live reward campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_boundary_splits_the_reward_across_the_delegations
```
