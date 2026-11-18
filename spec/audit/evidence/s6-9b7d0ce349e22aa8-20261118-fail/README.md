# s6-9b7d0ce349e22aa8-20261118-fail

**Evidence locality.** `single-host`

## Scope

Vaults are a balance map keyed by REV address; transfer uses the caller's deployerId and an unauthenticated deposit is refused.

The witness is `rholang/src/system_processes.rs::rev_vault_transfer_uses_deployer_capability_and_deposit_is_rejected`. The mutation is:

```text
-                        Err(illegal_arg(
                            "revVault: deposit is not callable (REV is minted at genesis only)",
                        ))
+                        Ok(())
```

## Provenance

| | |
|---|---|
| gate | `S6-9b7d0ce349e22aa8` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `38bf407b1ba8bcd76e27ec36e1ca0bff9a123743d358312a9df452c563dac28b` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation accepts the mint path and the same test fails.

FAIL log tail:

```text
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    system_processes::tests::rev_vault_transfer_uses_deployer_capability_and_deposit_is_rejected

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.73s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the wallet release campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib rev_vault_transfer_uses_deployer_capability_and_deposit_is_rejected
```
