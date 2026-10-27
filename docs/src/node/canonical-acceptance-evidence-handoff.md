# Canonical N=4/N=8 acceptance evidence handoff

**Status: proposed review contract, NOT an executed acceptance run, release approval, or deployment authority.**
**Scope:** source-pinned, disposable networks. This documentation changes no consensus semantics.

## Existing instruments: reuse, do not duplicate

- `tools/consensus-acceptance-drill.py`: verifies finalized marker retention, not general rollback safety.
- N=4/N=8 quiet-finality and recovery workflows from #435/#453: verify exact candidate HEAD; #453 remains under review, and a green unrelated workflow is not acceptance.
- `docs/src/node/issue-447-application-acceptance.md` and the merged #448/#450/#451 work: verify valid internally connected 2+2 topology, causal merge and application-state integrity. #452 was closed because its target implementation is already present through #450.
- #428's M1-M6 assurance proposal: distinct from an approval to launch mainnet. The draft evaluator in #429 was closed pending agreement on the policy and evidence schema.

## Run identity and required artifacts

Each independent arm and repetition gets its own `run_id`, immutable `manifest.json`, and artifact directory. Required fields: repository; exact candidate SHA; exact test-harness SHA; container image SHA-256 digest; GitHub workflow run URL and attempt; UTC timestamps; N=4 or N=8; unique disposable network prefix; validator public identities and bonds digest; genesis hash; epoch settings; memory/CPU limits; declared timeout; arm name; repetition number; result; relative artifact paths and SHA-256 digests.

Preserve per-node raw status, finalized block **height and hash**, causal/DAG parent IDs, accepted and rejected deploy IDs, state roots, marker/registry readbacks, topology and peer-reachability snapshots, restart sequence, stdout/stderr and timeout traces. Sanitize secrets. Never replace a failed run with a later pass: link both independently.

A missing artifact or unmatched SHA is **INVALID/INCOMPLETE**, never PASS. A runner with 2 GiB per validator cannot certify an 8–16 GiB production-capacity target.

## Acceptance matrix

| Arm | Required positive witness | Blocking falsifier |
| --- | --- | --- |
| N=4 quiet | Signed marker included and finalized by **all four** nodes, without helper deploys in declared quiet window | Marker pending, height-only check, or node disagreement |
| N=8 quiet | Same witness across **all eight** nodes | Any validator misses deadline or hash differs |
| N=4 recovery | Failure/rejoin recorded; surviving nodes finalize where required; after restart all four report identical finalized height **and block hash**, marker retained | Database secretly reset, lost marker, divergence |
| N=8 recovery | Same evidence for eight validators | Unrecovered peer, divergent finalized hash or missing parent |
| N=4 2+2 split cycle A | Two internally communicating pairs, no cross-pair links; distinct tips observed; after heal both tip histories covered, expected merge and rejection outcomes, all final states equal | Partition topology invalid, one tip absent, state disagreement |
| N=4 2+2 split cycle B | A **separate** full repetition with distinct run ID; identical acceptance standard | Reusing cycle A's witness or losing artifacts |
| Same-channel conflicts | Both winner outcomes, explicit accepted/rejected deploy IDs, fresh-signature retry where applicable | Wrong winner semantics or partial state effects |
| Consumed-seed merge | Exact-byte multiset cancellation; consumed datum not resurrected after heal | Old seed present alongside winning value |
| Distinct-channel writes | Both independent effects preserved after merge and restart | Either effect missing or duplicated |
| r-cash workload | Pinned 17-step application sequence with individual submission, inclusion, finality and resulting committed keys | Treating synthetic unit tests as application acceptance |
| Durable state | Same finalized block identity and state readback after node restart | Volatile tip mistaken for finality or divergent post-restart state |

In an equal-stake 2+2 split neither 50% side is required to finalize independently. Docker topology tests do not establish Casper correctness. Execution is not inclusion; inclusion is not finality; same height is not same finalized hash.

## Decision protocol: fail closed

- **INVALID:** bad topology, mismatched source/image/genesis identity, corrupted or contradictory evidence.
- **FAIL:** a valid run falsifies an invariant or exceeds a declared deadline.
- **INCOMPLETE:** a required arm, repetition, validator observation or independent review is missing.
- **PASS:** all required arms and repetitions pass on the **same intended candidate SHA**, with complete independently verifiable evidence.

The decision is **EVIDENCE-ONLY** unless every scoped arm passes and all remaining consensus or state-integrity blockers (#420, #435 and #447) receive explicit evidence-based disposition, and the merged #450 evidence matches the candidate. Only then may the candidate be labelled **MERGE-SAFE FOR REVIEW**, subject to Jim's review and repository merge policy. Neither outcome grants deployment authority. Hard-fork transition, network-specific migration and operator approval are separate gates.

## Reviewer handoff

- [ ] Candidate SHA, harness SHA, image digest, bond set, genesis and host resources pinned.
- [ ] N=4 quiet and N=8 quiet passes, all validators checked.
- [ ] N=4 and N=8 recovery passes with equal finalized height **and hash** after restart.
- [ ] Two complete independent N=4 2+2 split/rejoin cycles preserved.
- [ ] Both same-channel winner outcomes, consumed-seed cancellation, distinct-channel state and r-cash sequence independently verified.
- [ ] Negative controls and failures preserved with hashes; no expired or silently excluded artifacts.
- [ ] Open blockers dispositioned; independent reviewer and workflow URLs supplied.

### Copyable verdict record

```text
Candidate SHA:
Harness SHA:
Image digest:
Manifest/run URLs:
Independent reviewer:
N4 quiet: NOT RUN
N8 quiet: NOT RUN
N4 recovery: NOT RUN
N8 recovery: NOT RUN
N4 2x2 cycle A/B: NOT RUN
Both winners / consumed seed / distinct channel: NOT RUN
r-cash / restart persistence: NOT RUN
Artifact bundle SHA-256:
Open blockers:
Verdict: EVIDENCE-ONLY
Merge approval: NOT GRANTED
Deployment approval: NOT GRANTED
```

This document defines an acceptance contract for existing instruments. **No test has been executed or certified by this documentation change.**
