# s6-ccb08bb8dcafdd3c-20261118

**Evidence locality.** `single-host`

## Scope

Cost charges track the work of collection canonicalisation, union and rho:gov:censure.

The witness is `rholang/src/accounting.rs::collection_and_censure_charges_are_the_models`. The mutation is:

```text
-        Self::canonicalization_cost(left.saturating_add(right))
+        Self::canonicalization_cost(right)
```

## Provenance

| | |
|---|---|
| gate | `S6-ccb08bb8dcafdd3c` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `b9054d88d965f054bf2482b033a0d4c87eb344f7a1583aec7ff2b35c865b422e` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation undercharges the union and the same test fails.

PASS log tail:

```text
running 1 test
test accounting::tests::collection_and_censure_charges_are_the_models ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.93s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

The in-process charge model only. The pre-fork replay and two-binary acceptance are release obligations.

## Reproduce

```sh
cargo test -p rchain-rholang --lib collection_and_censure_charges_are_the_models
```
