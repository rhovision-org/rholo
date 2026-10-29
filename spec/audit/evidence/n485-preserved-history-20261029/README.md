# Preserved height-3122 justification-regression witness

The original executable reproduces the proposer rejection on a copy of the preserved store. The correction from #492, integrated onto the current dev baseline, accepts block 3122 through the unchanged validator, survives restart, processes a signed deploy, and accepts blocks through 3131 across the epoch boundary at 3130.

## Identities

| Item | Identity |
| --- | --- |
| Baseline executable source | a5259969a7c5673ccdfc5dad97981d0a3607fcf0 |
| Baseline executable SHA-256 | a5b9e9de4f3a75833da50aef67a845ddc350248ec3113040bc727dbcb9bdfa01 |
| Integrated candidate source | 3250f479ca9c511d16db877db4514c035c5b4c6e |
| Candidate base | 0a1e02362a7c34e474e068ffd47469c8d4be707a |
| Candidate production diff | Only the existing #492 change to block-storage/src/dag/message_state.rs |
| Candidate executable SHA-256 | f5c18e3ad2e74322d58377a37b79616fcd47cf61365b7519f19ea64bd86b246b |
| Rollback-reader source | d1af45d1db3f5fe2fa6047ad6c8a72b654d041df |
| Rollback-reader SHA-256 | 76547fa988a4b2e216feb373cb17affa9cc4217ac9595ea97e77a2e1db3a8745 |
| Historical genesis | c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26 |
| Historical accepted head | 0881c1a75e3f72e7e47dc050eb588ea06b4f186f0ee0afebd7625f1a05709a53 (height/sequence 3121) |
| Historical finalized anchor, before and after | ec5a08ef2307dc9a4e44a75e3f123ac345776a7c8b7a7811859060a78021f941 (1759) |

These identities belong to the abandoned historical chain. They must not be substituted for the rebuilt public network's genesis 133b1d30… or its heights.

## Preserved inputs and provenance boundary

[fixture-manifest.json](fixture-manifest.json) describes a read-only export from a copy of the closed pre-rebuild store at /var/lib/rnode.prewipe-20261029T130125Z. It contains 9,198 public metadata records through height 3121. The original database files have identical SHA-256 values before and after the campaign; [source-preservation.json](source-preservation.json) records all eight database hashes.

The store's head matches #485's historical head, and the original binary reproduces the same failure. **This campaign did not materialize DigitalOcean snapshot 252064726 or establish byte identity with that pre-restart droplet snapshot.** The source here is the preserved pre-rebuild directory after the historical restart. That distinction remains an acceptance/provenance limit.

No private keys, node identity files, tuple-space data or deploy payloads are included in the exported metadata fixture. Full runtime copies stayed on the authorized host. Signing used a temporary owner-only encrypted key file on that host; it was deleted after use. The experimental services ran with PrivateNetwork=yes and distinct network namespaces. Live validator processes were not used as experimental signers or restarted by this historical campaign.

## Mechanism

The reconstructed round boundary is 2063. A's latest self-message is 0881c1a7… at 3121. It already justifies peer message 043f249c… at height 2064, sequence 1699. The original escape selects e9858696… at height 2063, sequence 1698 for that peer. Its seen set does not cover the prior message, so the existing justification-regression validator correctly refuses the child.

The candidate selects bd6aca86… at height 3079, sequence 2714, which covers the prior history. Its actual block-3122 parent set agrees with the diagnostic selection. No block-validation rule, finalized record or original store was changed.

diagnose.py independently reconstructs this input set from the complete public metadata export. It is a diagnostic implementation, not a replacement for the native result. The native baseline rejection and candidate acceptance are retained separately.

## Native result

- Original binary: self-created block 3122 rejected with JustificationRegression.
- Candidate: blocks 3122–3127 accepted.
- Restart of the candidate's own reconstructed store: blocks 3128–3131 accepted, including epoch boundary 3130; zero justification regressions in either candidate journal.
- Signed deploy 3045022100a5768cc57bdf24e65cc97859c15bf1ca7a5ba4c2e878e77caaa26f8c3cdc8ec602206be68f0fcfdf6eb30ffd1b8c2477ae835fed586424816979ffcd588f0dee249b processed successfully in block 3129, hash 652f47a1216c23786230b2f0f209860bcba1a73a3d410594f211f9c31e5ae062.
- A later passive candidate reader and a passive d1af45d1 reader on separate copies return identical blocks 3122–3131 and the same successful deploy status. Both retain finalized height 1759 and its original hash.
- The status field latestBlockNumber is 3132 after accepted block 3131. It is not used as an accepted-block witness.

The isolated node has no live quorum. Successful processing here does **not** mean the deploy finalized. Historical finality deliberately remained 1759. The restarts were orderly process stops/starts; no power-loss qualification was attempted.

## Resource boundary

The first 2 GiB cap was correctly refused by the mature-store preflight, whose recorded floor is 4 GiB for this record count. The accepted experiments used MemoryMax=6G, CPUQuota=50%, Nice=15 and bounded runtime.

| Process window | Peak RSS, KiB | User CPU, s | System CPU, s |
| --- | ---: | ---: | ---: |
| Original baseline | 2,799,768 | 19.36 | 3.18 |
| Candidate before restart | 2,862,948 | 67.08 | 12.95 |
| Candidate restart/forward validation | 2,867,096 | 70.48 | 13.66 |

The approximately four-minute wall times include the observation/proposal windows. They are **not cold independent-recovery times** or a paired performance benchmark. This populated-store startup does not discharge C339's mature-history fresh replay, temporary/persistent storage, budget or power-loss acceptance.

## Verification

- [Native build](https://github.com/rhovision-org/rholo/actions/runs/40946232143): PASS; executable and artifact checksum verified.
- [Integrated CI](https://github.com/rhovision-org/rholo/actions/runs/40947228350): PASS, all four jobs.
- [Lean/Coq/conformance](https://github.com/rhovision-org/rholo/actions/runs/40947352148): PASS.
- [N4 recovery and partition/rejoin controls](https://github.com/rhovision-org/rholo/actions/runs/40947398731): PASS.
- Native build artifact ZIP SHA-256: 313cdf835a9bee796387b577a2d788c5349b87fa3d720a7b53c2171e5ae740a3.
- N4 artifact ZIP SHA-256: 3349a15875150fa402feb74a0f876415725f38fb47585b5f0f41feadc99e6e42.

Run the retained-evidence checks from this directory:

```sh
python3 diagnose.py
python3 verify.py
```

The scripts verify the frozen metadata diagnosis and consistency of the retained native observations. They do not rerun the unavailable full historical chain stores. Raw native logs and runtime copies remain on the authorized host; the filtered logs included here retain all acceptance lines and the process measurements.

PR #492 remains draft pending the remaining provenance/review and integration requirements. No live recovery or release qualification is asserted by this evidence package.
