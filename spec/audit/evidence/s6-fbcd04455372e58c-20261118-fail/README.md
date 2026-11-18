# s6-fbcd04455372e58c-20261118-fail

**Evidence locality.** `single-host`

## Scope

Storage is refunded when a produce/consume matches, before the event costs.

The witness is `rholang/src/storage.rs::a_matched_produce_refunds_its_storage_before_the_event_costs`. The mutation is:

```text
-                .charge(Cost::new(-cost.value, "consume storage refund"))?;
+                .charge(Cost::new(cost.value, "consume storage refund"))?;
```

## Provenance

| | |
|---|---|
| gate | `S6-fbcd04455372e58c` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `ad160a23811f328dfa4c4e3aeab49fd59b5cd037216afb3061772f5e95c03dac` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation charges instead of refunding and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    storage::tests::a_matched_produce_refunds_its_storage_before_the_event_costs

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.76s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the historical replay is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_matched_produce_refunds_its_storage_before_the_event_costs
```
