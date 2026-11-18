# s6-83825d04fdb174bc-20261118-fail

**Evidence locality.** `single-host`

## Scope

A resolved parent at or above the block's number is refused (H1b), failed parents included.

The witness is `casper/src/dag.rs::h1b_a_failed_parent_above_the_childs_height_is_refused`. The mutation is:

```text
-    let mut max_block_number = -1i64;
    for j in &b.justifications {
        let meta = dag
            .lookup(j)
            .await?
            .ok_or_else(|| format!("missing justification {}", j.to_hex()))?;
        // The descent bound, for *every* resolved parent (H1b).
        if i64::from(meta.block_num) >= i64::from(b.block_number) {
            return Ok(Err(BlockStatus::InvalidBlockNumber));
        }
        // The maximum, over **every** resolved parent. A failed block still counts: skipping it made
        // the failure hide the block, refusing its child for the parent's record alone (AUDIT C173).
        max_block_number = max_block_number.max(i64::from(meta.block_num));
    }
    if max_block_number + 1 == i64::from(b.block_number) {
        Ok(Ok(()))
    } else {
        Ok(Err(BlockStatus::InvalidBlockNumber))
    }
+    Ok(Ok(()))
```

## Provenance

| | |
|---|---|
| gate | `S6-83825d04fdb174bc` |
| source | `98e8681f156ab0d171fdd7f727576d25744334f4` |
| binary | `4230099ed06f1eec6ee8eede8f8f5dabedf7499d4baed9a177b51d0c95936df3` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation admits the block and the same test fails.

FAIL log tail:

```text
    = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: unused variable: `b`
   --> casper/src/validate.rs:320:5
    |
320 |     b: &BlockMessage,
    |     ^ help: if this is intentional, prefix it with an underscore: `_b`

warning: `rchain-casper` (lib test) generated 2 warnings (run `cargo fix --lib -p rchain-casper --tests` to apply 2 suggestions)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 15.37s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the historical replay and rollout remain separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib h1b_a_failed_parent_above_the_childs_height_is_refused
```
