# s6-957351356597c77d-20261118-fail

**Evidence locality.** `single-host`

## Scope

fundPot adds REV to the validators' reward pot with no claim, debiting the caller's own vault.

The witness is `rholang/src/system_processes.rs::pos_fund_pot_moves_the_callers_rev_into_the_staking_vault`. The mutation is:

```text
-        self.set_vault_balance(&address, remaining);
+        self.set_vault_balance(&address, balance);
```

## Provenance

| | |
|---|---|
| gate | `S6-957351356597c77d` |
| source | `0d4e1abf564cc3bc5044b7b8e862dbfd955c8e57` |
| binary | `c55074141d5e3d38f500fd06db34178039ac6fde5749ec8bec86f8b7422912de` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation skips the debit and the same test fails.

FAIL log tail:

```text
warning: unused variable: `remaining`
    --> rholang/src/native_state.rs:7585:13
     |
7585 |         let remaining = balance_plus(balance, -stake, "pot funding debit")?;
     |             ^^^^^^^^^ help: if this is intentional, prefix it with an underscore: `_remaining`
     |
     = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-rholang` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-rholang --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.46s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
error: test failed, to rerun pass `-p rchain-rholang --lib`
```

## What this does not establish

A current-build regression witness; the live governance campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib pos_fund_pot_moves_the_callers_rev_into_the_staking_vault
```
