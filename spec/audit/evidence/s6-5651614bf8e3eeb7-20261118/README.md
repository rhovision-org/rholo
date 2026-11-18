# s6-5651614bf8e3eeb7-20261118

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
| binary | `ec5bb36ddc3498e49d7e71ff3e6116c0179f36725a64f7e2bf1eba48b8447880` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation changes the replay order and the same test fails.

PASS log tail:

```text
running 1 test
test pending_peeks_replay_with_exact_cost_and_state ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.72s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 26.16s
     Running tests/replay_trace_order.rs (target/debug/deps/replay_trace_order-321f300e8cd1a65c)
```

## What this does not establish

The in-process replay order only. The recorded-history sweep, second-node replay and coordinated rollout are release obligations.

## Reproduce

```sh
cargo test -p rchain-casper --test replay_trace_order -- --exact pending_peeks_replay_with_exact_cost_and_state
```
