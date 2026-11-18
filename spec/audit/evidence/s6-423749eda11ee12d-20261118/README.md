# s6-423749eda11ee12d-20261118

**Evidence locality.** `single-host`

## Scope

A slash fans the untaken remainder out to each delegator's own vault and clears the delegation ledgers, at every tier.

The witness is `rholang/src/native_state.rs::a_slash_fans_out_to_each_delegators_own_vault_at_every_tier`. The mutation is:

```text
-            for (delegator, share) in delegators.iter().zip(shares.iter()) {
                if *share <= 0 {
                    continue;
                }
                let address = self.vault_address(delegator)?;
+            for (delegator, share) in delegators.iter().zip(shares.iter()) {
                if *share <= 0 {
                    continue;
                }
                let address = self.vault_address(validator)?;
```

## Provenance

| | |
|---|---|
| gate | `S6-423749eda11ee12d` |
| source | `714e5d9346ed554ea29fe7dba91d635b84c98268` |
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation redirects the delegator share and the same test fails.

PASS log tail:

```text
running 1 test
test native_state::delegation_tests::a_slash_fans_out_to_each_delegators_own_vault_at_every_tier ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.02s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 12.15s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live slashing campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_slash_fans_out_to_each_delegators_own_vault_at_every_tier
```
