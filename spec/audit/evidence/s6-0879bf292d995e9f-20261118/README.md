# s6-0879bf292d995e9f-20261118

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
| binary | `287370e1473811d206daf9c37fde3dc48c4ea4360248c5a99c101c56012f5148` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation admits the equivocating block and the same test fails.

PASS log tail:

```text
running 1 test
test dag::tests::insert_rejects_equivocation_same_seq_num ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-block-storage v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/block-storage)
   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 11.16s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness for the insert arm; the restore arm and a live rollout remain separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib insert_rejects_equivocation_same_seq_num
```
