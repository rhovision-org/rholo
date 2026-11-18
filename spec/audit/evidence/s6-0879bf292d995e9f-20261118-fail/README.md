# s6-0879bf292d995e9f-20261118-fail

**Evidence locality.** `single-host`

## Scope

An equivocating block is refused at insert (H-1). The store-restore arm (H1c) is a sibling test, not this witness.

The witness is `casper/src/dag.rs::insert_rejects_equivocation_same_seq_num`. The mutation is:

```text
-            // enter the DAG nor stall finalization.
            let equivocating = {
+            // enter the DAG nor stall finalization.
            let equivocating = false; {
```

## Provenance

| | |
|---|---|
| gate | `S6-0879bf292d995e9f` |
| source | `98e8681f156ab0d171fdd7f727576d25744334f4` |
| binary | `64396b7bd16ce5a4028d88be4f6784430ad4b8c028af024023fd0a0616c4ca27` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation admits the equivocating block and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    dag::tests::insert_rejects_equivocation_same_seq_num

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.01s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.45s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness for the insert arm; the restore arm and a live rollout remain separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib insert_rejects_equivocation_same_seq_num
```
