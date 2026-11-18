# s6-e2cdf44a5cc0c81e-20261118

**Evidence locality.** `single-host`

## Scope

The merge composes cost accounting per accepted deploy instead of a block carrying it.

The witness is `casper/src/merging.rs::a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge`. The mutation is:

```text
-    for chain in accepted {
+    for chain in accepted.iter().take(1) {
```

## Provenance

| | |
|---|---|
| gate | `S6-e2cdf44a5cc0c81e` |
| source | `c87ade74356ebd33f964b10d47942b99a5fe86e8` |
| binary | `80e901131f572b59aa34a9e1ebdbd07ee5bd04f08d9aa7ad6d08c0823f61d777` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation drops the other chains' moves and the same test fails.

PASS log tail:

```text
running 1 test
test merging::boundary_merge_tests::a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 748 filtered out; finished in 0.03s

   Compiling rchain-rspace v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rspace)
   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 26.25s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness; the live M5 replay is a release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge
```
