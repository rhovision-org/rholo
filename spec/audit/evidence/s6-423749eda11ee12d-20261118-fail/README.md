# s6-423749eda11ee12d-20261118-fail

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
| binary | `bbf6d91eaf074d5b956f4d7199bb188330f30ef5c65bb1fb24952ec2172e54aa` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation redirects the delegator share and the same test fails.

FAIL log tail:

```text
warning: unused variable: `delegator`
    --> rholang/src/native_state.rs:2909:18
     |
2909 |             for (delegator, share) in delegators.iter().zip(shares.iter()) {
     |                  ^^^^^^^^^ help: if this is intentional, prefix it with an underscore: `_delegator`
     |
     = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-rholang` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-rholang --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.39s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the live slashing campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib a_slash_fans_out_to_each_delegators_own_vault_at_every_tier
```
