# s6-002725fa6a14a386-20261118-fail

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
| binary | `c857f144049b2d47f7275bafbea8a876c20a9845a1d70fa34860ad813e877373` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the same test fail.

FAIL log tail:

```text
warning: unused variable: `window`
   --> block-storage/src/dag/liveness.rs:106:5
    |
106 |     window: i64,
    |     ^^^^^^ help: if this is intentional, prefix it with an underscore: `_window`
    |
    = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-block-storage` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-block-storage --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.67s
     Running unittests src/lib.rs (target/debug/deps/rchain_block_storage-29bd045d5a9311b6)
error: test failed, to rerun pass `-p rchain-block-storage --lib`
```

## What this does not establish

A current-build regression witness; not a historical replay or a coordinated rollout.

## Reproduce

```sh
cargo test -p rchain-block-storage --lib n8_proposer_outlier_cannot_set_the_staleness_clock
```
