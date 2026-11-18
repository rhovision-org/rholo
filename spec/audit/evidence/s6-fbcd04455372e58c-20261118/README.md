# s6-fbcd04455372e58c-20261118

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
| binary | `b9054d88d965f054bf2482b033a0d4c87eb344f7a1583aec7ff2b35c865b422e` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation charges instead of refunding and the same test fails.

PASS log tail:

```text
running 1 test
test storage::tests::a_matched_produce_refunds_its_storage_before_the_event_costs ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 376 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.03s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the historical replay is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_matched_produce_refunds_its_storage_before_the_event_costs
```
