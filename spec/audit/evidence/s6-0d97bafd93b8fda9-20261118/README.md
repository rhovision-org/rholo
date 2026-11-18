# s6-0d97bafd93b8fda9-20261118

**Evidence locality.** `single-host`

## Scope

A boundary's participation comes from the last finalised fringe; the participation map reaches play and replay arithmetic alike.

The witness is `casper/src/runtime_manager.rs::a_boundarys_participation_reaches_play_and_replay_alike`. The mutation is:

```text
-    if behind <= grace {
+    if behind <= i64::MAX {
```

## Provenance

| | |
|---|---|
| gate | `S6-0d97bafd93b8fda9` |
| source | `714e5d9346ed554ea29fe7dba91d635b84c98268` |
| binary | `59478c363827879f9cf0a47d4b199c3fa1c8f9cba4185262752c23b2b34f17b0` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation removes the participation difference and the same test fails.

PASS log tail:

```text
running 1 test
test runtime_manager::tests::a_boundarys_participation_reaches_play_and_replay_alike ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.28s

   Compiling rchain-models v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/models)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
   Compiling rchain-comm v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/comm)
   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 18.02s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness; the live boundary campaign is a release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_boundarys_participation_reaches_play_and_replay_alike
```
