# s6-f735095c746556f3-20261118

**Evidence locality.** `single-host`

## Scope

A merge diffs each affected channel once, not once per produce/consume event.

The witness is `rspace/src/merger/state_change.rs::a_consume_and_replace_diffs_its_channel_once`. The mutation is:

```text
-            .map(|p| p.channels_hash)
+            .map(|p| p.hash)
```

## Provenance

| | |
|---|---|
| gate | `S6-f735095c746556f3` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `4f4235033a6edc639787c6a0ce3d5ab0f862efcd95e5261c43e77fbc0bb46a70` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation reintroduces the per-event diff and the same test fails.

PASS log tail:

```text
running 1 test
test merger::state_change::diff_tests::a_consume_and_replace_diffs_its_channel_once ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 194 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.39s
     Running unittests src/lib.rs (target/debug/deps/rchain_rspace-3e8cc9b76677a749)
```

## What this does not establish

The in-process merge diff only. The live 840-to-841 M5 replay is unavailable and remains a release obligation.

## Reproduce

```sh
cargo test -p rchain-rspace --lib a_consume_and_replace_diffs_its_channel_once
```
