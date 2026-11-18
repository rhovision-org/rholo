# s6-d36503203399d227-20261118

**Evidence locality.** `single-host`

## Scope

bonds_map/stake are carried as NonNegI64; a negative stake is refused by the carrier, not clamped.

The witness is `shared/src/refined.rs::non_neg_i64_accepts_non_negative_rejects_negative`. The mutation is:

```text
-                if v >= 0 {
+                if true {
```

## Provenance

| | |
|---|---|
| gate | `S6-d36503203399d227` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `acee082ea254c636f30927c71b73c71c24c02db18e6b32352caeb55da50a69a9` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation accepts the negative and the same test fails.

PASS log tail:

```text
running 1 test
test refined::tests::non_neg_i64_accepts_non_negative_rejects_negative ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 105 filtered out; finished in 0.00s

   Compiling tokio v1.54.1
   Compiling rchain-shared v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/shared)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 10.36s
     Running unittests src/lib.rs (target/debug/deps/rchain_shared-6ba55e676870ee2a)
```

## What this does not establish

A current-build regression witness; the genesis-hash acceptance is separate.

## Reproduce

```sh
cargo test -p rchain-shared --lib non_neg_i64_accepts_non_negative_rejects_negative
```
