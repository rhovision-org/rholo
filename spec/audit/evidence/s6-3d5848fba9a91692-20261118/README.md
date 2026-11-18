# s6-3d5848fba9a91692-20261118

**Evidence locality.** `single-host`

## Scope

The block's producer is paid a share of what its deploys burned, sized by executor_share.

The witness is `rholang/src/native_state.rs::the_producer_is_paid_a_share_of_what_the_deploy_burned`. The mutation is:

```text
-        let share = i64::from(self.params().await?.executor_share);
+        let share = 0i64;
```

## Provenance

| | |
|---|---|
| gate | `S6-3d5848fba9a91692` |
| source | `714e5d9346ed554ea29fe7dba91d635b84c98268` |
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation zeroes the share and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::tests::the_producer_is_paid_a_share_of_what_the_deploy_burned ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.58s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live reward campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib the_producer_is_paid_a_share_of_what_the_deploy_burned
```
