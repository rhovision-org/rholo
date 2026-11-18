# s6-f7d55fe093312364-20261118-fail

**Evidence locality.** `single-host`

## Scope

Native writes join the merge's conflict relation; two branches writing one slot conflict.

The witness is `casper/src/merging.rs::two_branch_blocks_writing_one_native_slot_do_not_panic_the_merge`. The mutation is:

```text
-        self.native_effects
            .iter()
            .map(NativeStoreAction::slot)
            .collect()
+        BTreeSet::new()
```

## Provenance

| | |
|---|---|
| gate | `S6-f7d55fe093312364` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `1b65f6e0be52a96b79eedc422efbc9bebec8fe981a5896f566c1f5197344c14c` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation makes the writers disjoint and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    merging::native_merge_tests::two_branch_blocks_writing_one_native_slot_do_not_panic_the_merge

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 748 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.03s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; a live merge campaign is separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib two_branch_blocks_writing_one_native_slot_do_not_panic_the_merge
```
