# s6-9297637b16a7e18c-20261118-fail

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
| binary | `7fe558dae9868fef6e5e017f9832c5b3a2f71e5a8222bf009bdc9e85e465ac2c` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation empties the delegated split and the same test fails.

FAIL log tail:

```text
warning: unused variable: `amounts`
    --> rholang/src/native_state.rs:2137:17
     |
2137 |             let amounts: Vec<i64> = held.iter().map(|(_, amount)| *amount).collect();
     |                 ^^^^^^^ help: if this is intentional, prefix it with an underscore: `_amounts`
     |
     = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-rholang` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-rholang --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.24s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the live reward campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_boundary_splits_the_reward_across_the_delegations
```
