# s6-5651614bf8e3eeb7-20261118-fail

**Evidence locality.** `single-host`

## Scope

Replay COMM candidates follow chronological execution order; strict replay verdicts equal the play verdict.

The witness is `casper/tests/replay_trace_order.rs::pending_peeks_replay_with_exact_cost_and_state`. The mutation is:

```text
-        for event in log.iter().rev() {
+        for event in log.iter() {
```

## Provenance

| | |
|---|---|
| gate | `S6-5651614bf8e3eeb7` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `3af5a8f7ca481239a1f858a10b9df3350f3c77e1555cb73fc7fcf49cf94c1ae1` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation changes the replay order and the same test fails.

FAIL log tail:

```text

failures:
    pending_peeks_replay_with_exact_cost_and_state

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.74s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 15.48s
     Running tests/replay_trace_order.rs (target/debug/deps/replay_trace_order-321f300e8cd1a65c)
error: test failed, to rerun pass `-p rchain-casper --test replay_trace_order`
```

## What this does not establish

The in-process replay order only. The recorded-history sweep, second-node replay and coordinated rollout are release obligations.

## Reproduce

```sh
cargo test -p rchain-casper --test replay_trace_order -- --exact pending_peeks_replay_with_exact_cost_and_state
```
