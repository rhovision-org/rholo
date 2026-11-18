# s6-0b46037b3783e4ca-20261118-fail

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
| binary | `280c76a68162c04ccdb96160c71cac0a66a06d2c0c7b0a829461247331dbb7af` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation errors instead of promoting and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    reduce::tests::division_and_modulo_overflow_promote_to_bigint

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.16s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the historical replay and rollout are separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib division_and_modulo_overflow_promote_to_bigint
```
