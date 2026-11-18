# s6-d39645b95737497d-20261118

**Evidence locality.** `single-host`

## Scope

The node's block metadata carries the slashable flag (proto field 22); it must survive the store round trip.

The witness is `models/src/block_metadata.rs::a_slashable_flag_survives_the_store_round_trip`. The mutation is:

```text
-            slashable: self.slashable,
+            slashable: false,
```

## Provenance

| | |
|---|---|
| gate | `S6-d39645b95737497d` |
| source | `22f17d10310859262924799a5f594ed6c12c13ba` |
| binary | `cdd9a5fb6a67e6bee576c2c28ee5e18a5087b376133ac02e23b68db6a4127857` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The named test passes on the current build; the mutation loses the flag and the same test fails.

PASS log tail:

```text
running 1 test
test block_metadata::tests::a_slashable_flag_survives_the_store_round_trip ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 192 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.18s
     Running unittests src/lib.rs (target/debug/deps/rchain_models-ba348ab9afbaf01c)
```

## What this does not establish

A current-build regression witness; it does not replay historical block metadata.

## Reproduce

```sh
cargo test -p rchain-models --lib a_slashable_flag_survives_the_store_round_trip
```
