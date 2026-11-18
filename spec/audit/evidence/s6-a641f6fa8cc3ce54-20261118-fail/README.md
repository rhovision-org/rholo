# s6-a641f6fa8cc3ce54-20261118-fail

**Evidence locality.** `single-host`

## Scope

A slash is graded by the offence; the existing witness takes the worst tier and prices a Misdemeanour at a quarter.

The witness is `rholang/src/native_state.rs::a_slash_counts_the_escrowed_claim_and_takes_the_worst_tier`. The mutation is:

```text
-            SlashSeverity::Misdemeanour => 2_500,
+            SlashSeverity::Misdemeanour => 10_000,
```

## Provenance

| | |
|---|---|
| gate | `S6-a641f6fa8cc3ce54` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `270e6dff03d2bd8215e47fdfd2da402e5080bbc7facff343b02c03126cd1f6d2` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation prices the tier as the whole bond and the same test fails.

FAIL log tail:

```text


failures:
    native_state::tests::a_slash_counts_the_escrowed_claim_and_takes_the_worst_tier

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.01s

   Compiling rchain-models v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/models)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.81s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness for the Misdemeanour tier; the other tier constants are the same table and the live campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_slash_counts_the_escrowed_claim_and_takes_the_worst_tier
```
