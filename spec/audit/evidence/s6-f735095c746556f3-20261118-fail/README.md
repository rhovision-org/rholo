# s6-f735095c746556f3-20261118-fail

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
| binary | `1e79ab689bfe69477d2bf7fc5d750d53f91d2b1549fbf79e1e13cdb25106688d` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation reintroduces the per-event diff and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    merger::state_change::diff_tests::a_consume_and_replace_diffs_its_channel_once

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 194 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.24s
     Running unittests src/lib.rs (target/debug/deps/rchain_rspace-3e8cc9b76677a749)
error: test failed, to rerun pass `-p rchain-rspace --lib`
```

## What this does not establish

The in-process merge diff only. The live 840-to-841 M5 replay is unavailable and remains a release obligation.

## Reproduce

```sh
cargo test -p rchain-rspace --lib a_consume_and_replace_diffs_its_channel_once
```
