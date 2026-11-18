# C357 / #564 — pinned integrated binary qualification, 2026-11-13

**Scope:** disposable Docker devnets on one GitHub runner. **Qualification verdict: PASS. Release/live readiness: NO-GO.** This is an unselected qualification build, not an installed or maintainer-approved release candidate.

**Evidence locality.** `single-host` — disposable Docker devnets on one GitHub runner; this is not an off-host positive.

## Provenance

| Field | Retained value |
|---|---|
| Runtime source | `1ceabd2917a2d4f41104116b6cf8b5fd0317964b`, pristine checkout |
| Build | Rust 1.95.0; locked Cargo dependencies; debug profile with debug symbols disabled |
| Executable SHA-256 | `5ff2b4f9d77a4694d3efa4df46cb1a86fb6a31a157929cc4770a5e43436cd1b8` |
| Docker image ID | `sha256:ec527b36491baaae9c008bd783babf97c8cba93236fd34ebdf32bdd00721f0a6` |
| Harness revision | `4c0ec2f1ceda8354690b00d798df01ca0b052c74` on the personal fork |
| Run / attempt | [43479787916, attempt 1](https://github.com/aixaria0/rchain-rust/actions/runs/43479787916/attempts/1) |
| Evidence locality | `single-host`; the runner's validators share one disposable host |
| Evidence correction (2026-11-18) | The earlier `off-host-claim` label contradicted the one-host description; this is a single-host qualification, not an off-host positive. |
| Installed public candidate | **Not supplied** |
| Maintainer release selection | **Not supplied** |

The executable digest is recorded before packaging, checked inside the image, and checked on both natural-trigger containers. The recovery arms use that same image. Build logs, version strings, source/lockfile pins, image identity, commands, HTTP bodies/headers and run/artifact metadata are retained. Full binary bytes remain a CI artifact, subject to provider retention; rebuilding from source is not a promise of byte-identical output.

`execution_proven:false`: hashes establish retained byte identity, not independent proof of execution or a human acceptance verdict. The packer's canonical upstream-run field is null because this run occurred on the fork; `external_run` records the actual fork URL/attempt without inventing an upstream run.

## Operational readings

| Arm | Result | Retained observation |
|---|---|---|
| Natural pending-peek trigger, no injector | **PASS** | Both nodes hold and finalize block `c01651a6e945509a573cdf4b80298e34c82fe6da74c88f5a231ab1d7d8542768`; registry deploy cost **13317**, `errored:false`, identical nonempty deploy accounting and pre/post-state |
| Independent content-address check | **PASS** | Both `/raw` responses are identical; 10,647 encoded preimage bytes hash with Blake2b-256 to the expected block hash |
| Attestation recovery (`deploy:1`) | **PASS** | Injected deploy-bearing self-replay refusal recovers by attestation, without reset; finality advances |
| Reset / catch-up / automatic resume (`all:2`) | **PASS** | Reset rewinds to finalised anchor, catch-up passes it, automatic resume clears the latch without operator POST; finality advances |
| Halt / operator resume (`all:2 --no-self-replay-reset`) | **PASS** | Halt remains set and finality does not advance during the 20-second observation; operator resume then clears the halt and finality advances |
| Independent-capture harness controls | **9/9 PASS** | One valid fixture plus eight rejected cases: missing state, empty deploy list, wrong deploy ID, zero cost, execution error, false finalization, differing raw responses and wrong executable digest |

Natural block pre-state: `af3071658de28282f6e0f1d4145062cb235b3f6e1e1b19fee390b69650da3c3f`; post-state: `8e1846b9fb36d4add2cbcb883abcd3fbd2ff49a6216e72d8832aa6a524181b06`. Raw log whitespace and carriage returns are retained byte-for-byte. All **104 original runner files** match their original checksum index; provider/archive and review-time checks are retained separately. The operational ZIP digest is `2441cb7f15aee4149ffe74e54ab9d94940c390bf6469bf1f37e6e4541a6a76f2`. The binary artifact expires on 2027-02-11 unless retained elsewhere; no permanent download is promised.

The natural arm has **no injector**. Four pending requests precede the registry contract, exercising the synthetic pending-peek shape used in #573. The three recovery arms intentionally use the existing DevNet-only injector; they establish the recovery ladder under controlled refusals, not an independent reproduction of the 274-phlo incident.

The earlier [red/green record](../n564-natural-trigger-20261113/README.md) remains the pre-#570 negative control. This campaign does not re-label that earlier binary as the current build.

## Public-route observation, separately scoped

A read-only observation of `https://testnet.rhobot.net` on 2026-11-13 reported:

- source self-report `0a1e02362a7c34e474e068ffd47469c8d4be707a`;
- height 1178, 3 peers, empty pool and `acceptsDeploys:true`;
- `/api/v1/finality-observation`: HTTP 404.

Raw receipts and UTC times are under `raw/public-route/`. The health response is a public wrapper, not the current native finality contract. This route does **not** establish four independent validator identities, executable digests, a quorum view or a live verdict. An idle height alone is not evidence of a new stall; admission and peer count do not establish settlement.

## Compatibility / release decision still required

The [canonical §6 category-A boundary](../../passes.md#6-scala-deviation-register) remains authoritative. The COMM-binding correction is ungated and consensus-critical; the prior reconstruction shows old refusal/corrected acceptance, not compatibility of all accepted history. **No accepted-history compatibility sweep is supplied by this campaign.**

Jim/Patrick must select and independently approve one concrete boundary before release:

1. **Unswept history:** coordinated new genesis, with every validator on the identical selected corrected build from genesis.
2. **Existing-chain continuation:** an independently accepted retained-history sweep through the agreed checkpoint, covering every required block/state and relevant runtime changes, plus coordinated activation before production resumes.

Neither option has been selected or executed here. A rolling mixed-version deployment is not an accepted procedure. This finite qualification run cannot make that decision or certify all historical blocks.

## Remaining acceptance inputs / owners

| Owner | Input / evidence still required |
|---|---|
| Jim/Patrick | Independent compatibility decision and selected source/artifact/binary; approve the scope of this qualification or require the selected release artifact to be re-run |
| Designated validator operator | Installed source/executable/chain/membership pins on all four public validators, using [#559's preflight](https://github.com/rhovision-org/rholo/blob/f092cc9e3c30cdd7e37b2fa5ca96c68c28313bc4/spec/audit/runbooks/n487-live-postdeployment-20261108.md#11-collect-the-private-candidate-record-with-the-merged-probe) |
| Jim/Patrick | Named operator, independent reviewer, rollback owner, UTC window and authorization for the exact native-REV acceptance transfer |
| Independent reviewer | Review actual observations against the frozen criteria and publish the live verdict |

**C357 and #564 remain open.** [#553](https://github.com/rhovision-org/rholo/issues/553) remains the separate public acceptance gate. No live validator, store, chain, signing key, deployment or transaction was changed by this campaign.

## Historical reconstruction boundary

The original rejected serialized `BlockMessage` and original processed trace remain unavailable. Authenticated incident deploys and Merkle-verified state closures **were recovered** in [#570's evidence](../n564-rcb-recovered-20261111/README.md); that reconstruction regenerated the trace and reproduced the 5576/5302 cost pair. This campaign instead uses fresh DevNet genesis/state and a synthetic workload. It is not the original incident execution, complete live-store recovery, historical-binary provenance or accepted-chain compatibility.

## Reproduce and inspect

The fork-only workflow and independent capture script are retained under `raw/workflow.yml` and `raw/c357-verify-capture.py`. At the harness revision, push/run `.github/workflows/c357-binary.yml` on `verify/c357-binary-20261113`; it builds the pristine pinned runtime and executes the stock natural-trigger and recovery drills. It has no public validator targets.

Verify the retained record and rerun the independent-capture controls:

```sh
python3 tools/evidence-pack.py verify --record spec/audit/evidence/n564-binary-qualification-20261113
python3 spec/audit/evidence/n564-binary-qualification-20261113/raw/offline-verifier-controls.py spec/audit/evidence/n564-binary-qualification-20261113/raw
```

A successful digest verification is not permission to promote the binary or close the incident.
