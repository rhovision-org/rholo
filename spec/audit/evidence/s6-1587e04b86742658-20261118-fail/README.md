# s6-1587e04b86742658-20261118-fail

**Evidence locality.** `single-host`

## Scope

A validator's epoch reward is scaled by its participation; the absence slack is the knee and the grace the recovery edge.

The witness is `rholang/src/native_state.rs::the_absence_rule_takes_income_only_and_releases_it_on_a_single_block`. The mutation is:

```text
-    if behind <= grace {
        10_000
+    if behind <= grace {
        0
```

## Provenance

| | |
|---|---|
| gate | `S6-1587e04b86742658` |
| source | `714e5d9346ed554ea29fe7dba91d635b84c98268` |
| binary | `12b684b4c4eaa5e93e192f4954b9ec00a537a6e35d6474b81eeb35fcc6922e7a` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation removes the full share and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    native_state::tests::the_absence_rule_takes_income_only_and_releases_it_on_a_single_block

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.01s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.32s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the historical replay and rollout are separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib the_absence_rule_takes_income_only_and_releases_it_on_a_single_block
```
