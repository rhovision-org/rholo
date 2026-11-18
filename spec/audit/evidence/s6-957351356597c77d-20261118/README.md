# s6-957351356597c77d-20261118

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
| binary | `98c62deb1fdaa588f7c1b369b0d26d9635508a9dca547120fb9b4169356e1f68` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation skips the debit and the same test fails.

PASS log tail:

```text
running 1 test
test system_processes::tests::pos_fund_pot_moves_the_callers_rev_into_the_staking_vault ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 375 filtered out; finished in 0.00s

   Compiling rchain-rholang v0.1.0 (/home/patrick/RNodeRust/.claude/worktrees/pr-567/rholang)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.89s
     Running unittests src/lib.rs (target/debug/deps/rchain_rholang-a11f031a6ae27227)
```

## What this does not establish

A current-build regression witness; the live governance campaign is separate.

## Reproduce

```sh
cargo test -p rchain-rholang --lib pos_fund_pot_moves_the_callers_rev_into_the_staking_vault
```
