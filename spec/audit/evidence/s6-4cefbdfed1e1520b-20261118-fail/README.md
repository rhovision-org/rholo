# s6-4cefbdfed1e1520b-20261118-fail

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
| binary | `dcbb7b3ff86138de82df9caf2bf096ebce4d7bd26462a2dda12089100bb40ba8` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation burns the earlier principal and the same test fails.

FAIL log tail:

```text
warning: unused variable: `held`
    --> rholang/src/native_state.rs:7539:13
     |
7539 |         let held = ledger.get(&key).copied().unwrap_or(NonNegI64::zero());
     |             ^^^^ help: if this is intentional, prefix it with an underscore: `_held`
     |
     = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-rholang` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-rholang --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.28s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the live delegation campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_delegation_enlarges_the_pool_and_a_second_one_accumulates
```
