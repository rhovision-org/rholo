# rc6-local-20261120-0669419

**Evidence locality.** `local-devnet`

## Scope

RC-6, join and audit: does a validator whose store is wiped rejoin from a bootstrap, restore and
hydrate to the bootstrap's finalised state, and does `rnode audit` reproduce the joiner's checkpoint
claim from genesis?

## Provenance

| | |
|---|---|
| gate | `RC-6` |
| source | `066941917e1b066b2a704141d1904be8329e1f7a` |
| binary | `d664fc030f1d4bc3d35ddd3414ea39b3aeff8638b14453f2e07460be0e16c0b8` |
| run | not a GitHub Actions run (local disposable devnet) |
| reference | none |
| packaged | `2026-11-20T12:06:51+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

**FAIL.** The join half passed: `tools/n457-mature-join-arm.py --supported-recovery` reset
validator 3 as a passive replicator, it caught up, was activated, and the six-stage chain check
reported no problems. The joiner's operator status named checkpoint
`7008aa5e97434c214bc603efadad9f8a…` at anchor height 27.

The audit half failed on the CLI:
`rnode audit --bootstrap http://127.0.0.1:40403 --checkpoint <digest> --data-dir /tmp/rc6-audit-data
--out /tmp/rc6-audit-out` exited 1 with
`Configuration error: `run` subcommand is required to build a node configuration`
(`raw/audit.log`, first line). `rnode audit` builds its configuration through
`Configuration::build`, which currently accepts only the `run` subcommand
(`node/src/configuration/configuration.rs:84-88`) — so the audit tool the docs describe cannot run.
No `audit.json` was written.

## What this does not establish

The join half is one local reset on one devnet; the audit failed before it replayed anything, so
this record says nothing about whether a replay would agree. It does not establish a public-join
result, and it does not test the legacy/no-commitment certificate arm.

## Reproduce

```sh
python3 tools/release-check.py run --claim RC-6 --env local
```

The test boots `DEVNET_PREFIX=n457join DEVNET_NETWORK=n457net` with 4 GiB per validator, runs the
mature-join arm, checks `chain.json` with `tools/m3-join-chain.py check-chain`, then runs `rnode
audit` inside the joiner. The raw join, chain and audit output are under `raw/`.
