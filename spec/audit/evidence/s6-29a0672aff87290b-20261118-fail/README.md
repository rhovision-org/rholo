# s6-29a0672aff87290b-20261118-fail

**Evidence locality.** `single-host`

## Scope

Number-channel merge diffs are checked; the subtraction, the merge and the accumulation all refuse i64 overflow.

The witness is `rspace/src/merger/event_log_index.rs::combining_refuses_a_diff_that_leaves_i64`. The mutation is:

```text
-            let sum = entry.checked_add(*v).ok_or_else(|| {
+            let sum = entry.checked_add(*v).or(Some(0)).ok_or_else(|| {
```

## Provenance

| | |
|---|---|
| gate | `S6-29a0672aff87290b` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `f91f85fca6019d51243018e01c341218839e02b7af8a598aa0b76bffcf92c6ce` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation hides the overflow and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    merger::event_log_index::tests::combining_refuses_a_diff_that_leaves_i64

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 194 filtered out; finished in 0.00s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.29s
     Running unittests src/lib.rs (target/debug/deps/rchain_rspace-3e8cc9b76677a749)
error: test failed, to rerun pass `-p rchain-rspace --lib`
```

## What this does not establish

A current-build regression witness; the historical replay is separate.

## Reproduce

```sh
cargo test -p rchain-rspace --lib combining_refuses_a_diff_that_leaves_i64
```
