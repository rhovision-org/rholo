# s6-4cefbdfed1e1520b-20261118

**Evidence locality.** `single-host`

## Scope

A validator's bond may be other people's stake: pos:bonds carries the aggregate and pos:delegations attributes it.

The witness is `rholang/src/native_state.rs::a_delegation_enlarges_the_pool_and_a_second_one_accumulates`. The mutation is:

```text
-        ledger.insert(key, balance_plus(held, stake, "delegated principal")?);
+        ledger.insert(key, balance_plus(NonNegI64::zero(), stake, "delegated principal")?);
```

## Provenance

| | |
|---|---|
| gate | `S6-4cefbdfed1e1520b` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation burns the earlier principal and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::delegation_tests::a_delegation_enlarges_the_pool_and_a_second_one_accumulates ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.20s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live delegation campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_delegation_enlarges_the_pool_and_a_second_one_accumulates
```
