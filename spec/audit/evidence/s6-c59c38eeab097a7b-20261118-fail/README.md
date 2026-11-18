# s6-c59c38eeab097a7b-20261118-fail

**Evidence locality.** `single-host`

## Scope

The Coop multisig public keys are read into the genesis trusted stakeholder set alongside the bond holders.

The witness is `casper/src/genesis/mod.rs::the_multisig_keys_are_in_the_trusted_stakeholder_set`. The mutation is:

```text
-            trusted.insert(validator);
+            let _ = validator;
```

## Provenance

| | |
|---|---|
| gate | `S6-c59c38eeab097a7b` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `ce6b50764458ba853432a798bc657e90172c652438173f0e354b79689966d12f` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation leaves the multisig key out and the same test fails.

FAIL log tail:

```text
   |
66 |     let mut trusted: BTreeSet<ModelsValidator> = bonds.keys().copied().collect();
   |         ----^^^^^^^
   |         |
   |         help: remove this `mut`
   |
   = note: `#[warn(unused_mut)]` (part of `#[warn(unused)]`) on by default

warning: `rchain-casper` (lib test) generated 1 warning (run `cargo fix --lib -p rchain-casper --tests` to apply 1 suggestion)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.12s
     Running unittests src/lib.rs (target/debug/deps/rchain_casper-64d507b1a93523ca)
error: test failed, to rerun pass `-p rchain-casper --lib`
```

## What this does not establish

A current-build regression witness; the governance ceremony and key custody are separate.

## Reproduce

```sh
cargo test -p rchain-casper --lib the_multisig_keys_are_in_the_trusted_stakeholder_set
```
