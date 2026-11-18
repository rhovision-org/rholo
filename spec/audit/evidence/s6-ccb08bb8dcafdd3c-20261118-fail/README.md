# s6-ccb08bb8dcafdd3c-20261118-fail

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
| binary | `d43d2e245cfbeb50da2ca784f5231f93f9cf8446143e3345898bc4f33602270c` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation undercharges the union and the same test fails.

FAIL log tail:

```text
warning: unused variable: `left`
   --> rholang/src/accounting.rs:210:23
    |
210 |     pub fn union_cost(left: i64, right: i64) -> Cost {
    |                       ^^^^ help: if this is intentional, prefix it with an underscore: `_left`
    |
    = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-rholang` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-rholang --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.46s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

The in-process charge model only. The pre-fork replay and two-binary acceptance are release obligations.

## Reproduce

```sh
cargo test -p rchain-rholang --lib collection_and_censure_charges_are_the_models
```
