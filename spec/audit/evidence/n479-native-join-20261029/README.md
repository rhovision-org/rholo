# Historical native join evidence and the live acceptance boundary

## A. Root cause

The October 27 fresh join received its finalized fringe and both Tuple-Space pages, then
stalled during block ancestry acquisition. The retained original logs are
`/home/aria/tmp/rholo-live-proof-20261027/runs/fresh-correct.log` and
`extended-runs/extended-fresh-correct.log`; the latter campaign ended after 1120.53 seconds
at local height zero. This was not an absence of a fringe reply or initial state pages.

The ancestry requester selected a random discovered peer, while delayed duplicates could
trigger a resend of every outstanding request. These mechanisms and their failing-before
regressions were already corrected in [#474](https://github.com/rhovision-org/rholo/pull/474),
merge `2f956667c7cf5d9d7930602ae593cf6ec327312d`:

- `casper/src/engine/lfs_block_requester.rs::process_block` schedules new demand only after
  a validated, persisted block; rejected/duplicate traffic does not manufacture progress.
- `request_next` uses `casper/src/protocol/comm_util.rs::request_lfs_block` to obtain ancestry
  from the configured bootstrap that supplied the admitted fringe and pages.
- `RecoveryReplay` independently validates downloaded history before durable admission.

#472 supplied missing ancestry fringe-state roots. It did not correct request selection,
duplicate amplification, independent replay, or persistent completion accounting. Its
root-hydration correction is retained.

The new private probe also exposed a configuration boundary: `--network-id` alone changed
the reported/server identity while outbound traffic remained `testnet` in these executables.
The setup attempt in `network-id-negative-result.json` failed explicitly with `WrongNetwork`.
The completed run sets **both** `protocol-server.network-id` and `protocol-client.network-id`
in a private configuration file. The CLI mapping writes only the server key
(`node/src/configuration/commandline/config_mapper.rs`); transport and `RPConf` consume the
client key (`node/src/runtime/node_runtime.rs`). This private-setup rejection is separate
from the October 27 live stall, whose network identity was `testnet`.

## B. Implemented correction

The runtime correction above is already merged and present in the tested `d1af45d1` binary.
This change adds retained native observations, complete compressed journals, the exact
executed driver, checksums, and an offline verifier. It changes no runtime or consensus rule.

[#490](https://github.com/rhovision-org/rholo/pull/490) separately corrects the recovery tool's
stale range inventories: post-recovery verification must read refreshed node inventories,
cover current finalized heights, and discard failed/malformed range transfers. That tool
correction does not repair a running validator's receiver queue.

## C. Reproduction results

Run: **2026-10-29 16:47:28 UTC**, bounded passive native processes on loopback, a private
network identity, and fresh TLS identities. Only the bootstrap received a chain-state copy.
Each fresh joiner's directory was created empty; no chain databases or identity were copied
to it. Public genesis inputs were frozen outside those directories.

The historical bootstrap independently reported tip **218**, finalized **214**, hash
`8d973fd877d12663c2638121db48c08880bef579eb8fdf67bcee09d7dddd52c0`, and genesis
`c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26`.
These are the observed **private historical-chain baseline**, not current live heights.

| Bootstrap | Joiner | Initial state | Result | Seconds |
| --- | --- | --- | --- | ---: |
| `a5259969` | `a5259969` | Empty | PASS, tip 218 / finalized 214 | 45.491 |
| `a5259969` | `3607cf9f` | Empty | PASS, matching genesis and finalized anchor | 45.557 |
| `a5259969` | `3607cf9f` | Newly restored persistent state | PASS restart | 2.480 |
| `a5259969` | `3607cf9f` | Same state, wrong genesis pin | PASS explicit rejection | 2.512 |
| `a5259969` | `d1af45d1` | Empty | PASS, replay and matching anchor | 133.281 |
| `a5259969` | `d1af45d1` | Newly restored persistent state | PASS restart | 2.487 |
| `a5259969` | `d1af45d1` | Same state, wrong genesis pin | PASS explicit rejection | 2.538 |
| `d1af45d1` | `d1af45d1` | Empty / persistent | NOT TESTED in this campaign | — |
| Current live network | Corrected joiner | Empty | NOT TESTED in this campaign | — |

All three private fresh joins logged successful LFS restoration before Running, reached the
bootstrap height, and matched its finalized hash. No active-bonds error, receiver overflow,
or OOM was observed in those joiner journals. The old executable has no runtime genesis-pin
flag; its baseline genesis was checked through the API. It is not a wrong-pin control.

The results demonstrate mixed-version interoperability when the bootstrap can answer.
They do not reproduce the earlier multi-peer asymmetric topology or show that #472 alone
resolved it. One passive bootstrap is not four independently producing validators.

| Source revision | Executable SHA-256 |
| --- | --- |
| `a5259969a7c5673ccdfc5dad97981d0a3607fcf0` | `a5b9e9de4f3a75833da50aef67a845ddc350248ec3113040bc727dbcb9bdfa01` |
| `3607cf9f7832054376ac33a4c1c56fd0b701e89d` | `b4b2c8c3b7ae12ab16fae4cfd31d9918e687f6c41f0c28befc4f05c9721b8fd2` |
| `d1af45d1db3f5fe2fa6047ad6c8a72b654d041df` | `76547fa988a4b2e216feb373cb17affa9cc4217ac9595ea97e77a2e1db3a8745` |

## D. Acceptance results

| Gate | Result for full live acceptance | Evidence / boundary |
| --- | --- | --- |
| 1. Static quality | NOT TESTED by this native campaign | Separate source-pinned repository CI; no inference from checking retained logs |
| 2. Fresh state | NOT TESTED on the current live network | Private historical joins PASS; the original #472 live attempt FAIL |
| 3. Restart | NOT TESTED on a new current-live joiner | Both newly restored private joiners PASS |
| 4. Negative controls | NOT TESTED in full | Private wrong-pin and network-identity refusals PASS; interrupted transfer, missing-state and inbound-fault arms not repeated here |
| 5. Network safety / health | FAIL for a healthy-network claim | Isolation checks PASS; live receiver overflow is independently observed |
| 6. Independent repetition | NOT TESTED | A second corrected live fresh join was not run |

Protected A/D service PIDs/start times and firewall-policy checksums matched before and after.
All four live APIs remained available, and the closed source snapshot's chain-store hashes
were unchanged. All private units were stopped. No validator lifecycle/configuration/identity
operation, chain-store write, transaction, or firewall mutation was performed by the campaign.
These checks are not a claim that an actively running validator's database bytes stayed static.

The live follow-up at **16:55:13 UTC** observed A at **76 / finalized 3**, with D/B/C at
**6 / finalized 2**. The block sets at both finalized heights matched across all four APIs;
different finalized heights alone are not evidence of conflicting finality.

However, D's live journal contains repeated `block receiver state full (1024 blocks)`.
The first sampled A/D overflows are **14:37:35 UTC**, before any private probe. A later
status of `degraded: normal` and an available HTTP API do not clear that failure.
The preserved read-only follow-ups record it. Further host experiments were deferred.

## E. GitHub changes and retained artifacts

- [Deployment documentation and this evidence: #491](https://github.com/rhovision-org/rholo/pull/491).
- [Recovery inventory correction: #490](https://github.com/rhovision-org/rholo/pull/490), source
  `47c821e45512e3258856018c82e273412174e1ce` after integrating `dev@1a0009f5`.
- [Correction CI](https://github.com/rhovision-org/rholo/actions/runs/40916002246) and
  [Docker lifecycle/regressions](https://github.com/rhovision-org/rholo/actions/runs/40916002283).
- [Earlier exact-source recovery tests and negative controls](https://github.com/rhovision-org/rholo/actions/runs/40623695821)
  belong to #474's `6ff5947b` source; they do not replace this native measurement.
- `result.json`: exact commands, source/binary identity, observations, timings and safety snapshots.
- `private_native_compat.py`: exact executed driver, SHA-256
  `17ffd8aacdaff931eb1207c60224e21108b6e0a4a53a48f419aa4d0e47abf7f2`.
- `*.log.gz`: complete private-unit journals; no key files or chain databases are included.
- `SHA256SUMS.json`, `verify.py`: recomputable digests and independent offline consistency checks.
- `live-*.json`: dated read-only live API/log observations, separate from private acceptance.

Check retained evidence without running any node:

```sh
python3 spec/audit/evidence/n479-native-join-20261029/verify.py
```

The dedicated workflow checks the PR head and these recorded observations. A green result means
that the retained private evidence is internally consistent, not that a new live join ran in CI.
Original host evidence remains under `/home/aria/tmp/rholo-native-compat-20261029-164728/`.
The earlier incomplete-snapshot setup and the network-identity setup rejection are also retained;
neither is reported as a protocol acceptance result.

## Follow-up: old-chain hash in the rebuilt network's receiver

The first A/D overflow hash, `c20645c3f68aad1717bed1290e7097d12ac1d5f14cd393263273fdbe38828c80`,
is an exact key in the immutable public metadata fixture at `15e2e162` (#480).
Decoding `BlockMetadataProto` gives **height 1571**. Traversing all **6,148** ancestor records
reaches only the previous genesis `c596bef4…`, while the rebuilt validators report `133b1d30…`.
The fixture's compressed and uncompressed SHA-256 values are independently checked.

This establishes that an old-chain hash reached the rebuilt network's receiver-capacity path.
It does not establish which peer sent it, the complete contents of the 1024-entry queue,
or that this input alone caused exhaustion. The documented third-host observer has an established
TCP connection to A/D, but its HTTP API timed out; sender attribution remains unproven.
No peer was stopped, disconnected or reconfigured.

`cross-genesis.py` and `cross-genesis-result.json` reproduce the exact-key lookup, protobuf
height decode, strict parent-height checks and complete ancestry traversal. Reproduce using a
checkout containing the public fixture at `15e2e16240424fe4a5a8aa9131e5462bdbe56f2a`:

```sh
python3 spec/audit/evidence/n479-native-join-20261029/cross-genesis.py --fixture-repo /path/to/fixture-checkout
```

A matching `networkId=testnet` does not establish matching genesis. The newer merged queue/retry
changes are not a demonstrated cross-genesis isolation fix. Attribution and a safe chain-identity
compatibility strategy remain required before claiming this live incident resolved.

## F. Residual risks

The running network remains on `d1af45d1`; current `dev@1a0009f5` also contains merged ingress,
retriever-capacity and insertion-lock changes from #486 that are absent from that executable.
Source comparison establishes their absence. It does not prove that deploying them cures this
specific live queue failure. No deployment was performed.

Cold recovery of the longer 2,436-height history, peak memory, restart under load, an independent
current-live repeat, and all negative controls remain unqualified. This bounded run used a
756-record historical bootstrap, constrained CPU, and observer memory caps below the published
capacity floor; it is not a validator sizing measurement. Staging replay warnings about nondurable
merge-health counters are retained rather than concealed.

## G. Deployment recommendation

**NO-GO — Unresolved or failed acceptance.** The private fresh joins and restarts are verified.
Full live acceptance is not established, and the observed live receiver failure prevents a
healthy-network claim. No merge, issue closure, or validator deployment is requested by this record.

Refs #457, #456, #435, #339, #479, #485.
