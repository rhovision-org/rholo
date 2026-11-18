# s6-002725fa6a14a386-20261118

**Evidence locality.** `single-host`

## Scope

The finality gate's partition is the live weight set; an outlier proposer outside the live weight must not set the staleness clock.

The witness is `block-storage/src/dag/liveness.rs::n8_proposer_outlier_cannot_set_the_staleness_clock`. The mutation is:

```text
-                .is_some_and(|h| heights_behind(tip, *h) <= window)
+                .is_some_and(|h| heights_behind(tip, *h) <= i64::MAX)
```

## Provenance

| | |
|---|---|
| gate | `S6-002725fa6a14a386` |
| source | `98e8681f156ab0d171fdd7f727576d25744334f4` |
| binary | `44044386d0ab84aa2ae2804dbd512514db96c5f75eb1ae26cbe1c0b8d4c67257` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the same test fail.

PASS log tail:

```text
running 1 test
test dag::liveness::tests::n8_proposer_outlier_cannot_set_the_staleness_clock ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 97 filtered out; finished in 0.00s

   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.84s
     Running unittests src/lib.rs (target/debug/deps/rchain_block_storage-29bd045d5a9311b6)
```

## What this does not establish

A current-build regression witness; not a historical replay or a coordinated rollout.

## Reproduce

```sh
cargo test -p rchain-block-storage --lib n8_proposer_outlier_cannot_set_the_staleness_clock
```
