# s6-0b46037b3783e4ca-20261118

**Evidence locality.** `single-host`

## Scope

Integer arithmetic promotes to BigInt on i64 overflow and never wraps; mixed Int/BigInt operands promote.

The witness is `rholang/src/reduce.rs::division_and_modulo_overflow_promote_to_bigint`. The mutation is:

```text
-            None => int_bigop(&BigInt::from(*l), &BigInt::from(*r), cost, big, big_cost),
+            None => Err(RholangError::ReduceError("integer overflow".to_string())),
```

## Provenance

| | |
|---|---|
| gate | `S6-0b46037b3783e4ca` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `b9054d88d965f054bf2482b033a0d4c87eb344f7a1583aec7ff2b35c865b422e` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation errors instead of promoting and the same test fails.

PASS log tail:

```text
running 1 test
test reduce::tests::division_and_modulo_overflow_promote_to_bigint ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.22s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the historical replay and rollout are separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib division_and_modulo_overflow_promote_to_bigint
```
