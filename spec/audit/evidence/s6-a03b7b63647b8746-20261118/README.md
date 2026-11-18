# s6-a03b7b63647b8746-20261118

**Evidence locality.** `single-host`

## Scope

A block's justification set is bounded by max-number-of-parents and refused on the receiving side.

The witness is `casper/src/validate.rs::a_block_justifying_more_parents_than_the_network_allows_is_refused`. The mutation is:

```text
-    if max_number_of_parents <= 0 || b.justifications.len() as u128 <= max_number_of_parents as u128
+    if max_number_of_parents <= 0 || (b.justifications.len() as u128) < max_number_of_parents as u128
```

## Provenance

| | |
|---|---|
| gate | `S6-a03b7b63647b8746` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `287370e1473811d206daf9c37fde3dc48c4ea4360248c5a99c101c56012f5148` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation admits one extra parent and the same test fails.

PASS log tail:

```text
running 1 test
test validate::tests::a_block_justifying_more_parents_than_the_network_allows_is_refused ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 747 filtered out; finished in 0.00s

   Compiling rchain-casper v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/casper)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.93s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
```

## What this does not establish

A current-build regression witness; the committed-bound rollout is a separate release obligation.

## Reproduce

```sh
cargo test -p rchain-casper --lib a_block_justifying_more_parents_than_the_network_allows_is_refused
```
