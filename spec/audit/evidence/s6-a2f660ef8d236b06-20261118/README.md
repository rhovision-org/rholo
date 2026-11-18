# s6-a2f660ef8d236b06-20261118

**Evidence locality.** `single-host`

## Scope

withdraw refuses while delegations are outstanding, and delegate refuses while a withdrawal is pending.

The witness is `rholang/src/native_state.rs::a_validator_with_delegations_cannot_withdraw`. The mutation is:

```text
-        if ledger.keys().any(|key| key.operator == *validator) {
+        if false && ledger.keys().any(|key| key.operator == *validator) {
```

## Provenance

| | |
|---|---|
| gate | `S6-a2f660ef8d236b06` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation allows the withdrawal and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::delegation_tests::a_validator_with_delegations_cannot_withdraw ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.46s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live staking campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_validator_with_delegations_cannot_withdraw
```
