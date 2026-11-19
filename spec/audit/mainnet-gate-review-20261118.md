# Mainnet gate measurement review — 2026-11-18

**Review only. No gate is met, no row is flipped, and no release is authorized.** This
document attacks the gates as they exist, records where a defective build or record could
pass, and separates what can be narrowed now with formal logic or a Docker-free test from
what needs a human or an external artefact.

## Scope and pinned sources

The repo's mainnet gate is external **#428 M1–M6**; the live acceptance gate is **#584
G1–G6**; the hard-fork compatibility tracker is **#51/§6**; the network criteria are
**testnet acceptance §3**; and **C343/C357** are the two open audit rows. The canonical
register is `spec/audit/mainnet-gates.json` (111 gates: 6 M, 6 G, 3 TA, 44 #51, 49 §6
(47 inventory A candidates plus the two PR #600 prose-only rules), C343, C357, #520).

| source | pin |
|---|---|
| #428 body | SHA-256 `1b6fd74cdf5c865a629b2cac9d89d358f7f45aad280e4e647e5bd3ac23540dda` |
| #428 owner decision, 2026-11-15 | `https://github.com/rhovision-org/rholo/issues/428#issuecomment-6892934659` |
| #584 body | SHA-256 `24a4ba26caf22add60522df4689f76fd40db89133cbd21d81994b0c65b5c9610` |
| testnet criteria, lines 64–68 | SHA-256 `5457d7cdd53bf4985b91e96b078d41c13361fe999a4807444ad8863c25623f0a` |
| #51 matrix/inventory ledger | `e33125f65a95cff4f12baf8139a7361c7d158e43` |

## Method

Nine lenses, run as two waves of three agents plus synthesis and an adversarial verifier:

1. capacity and fault domains (M1);
2. ingress and overload (M2);
3. bootstrap and networking (M3);
4. finality, liveness and recovery (M4);
5. state integrity and merge accounting (M5);
6. reproducibility and sign-off (M6);
7. hard-fork compatibility (#51/§6);
8. operational and evidence locality (#584, testnet §3, C343/C357);
9. measurement validity (cross-cutting).

Each lens answers, per gate: what is the pass/fail condition; what is the current witness;
**can a concrete defective build or record still pass**; can the witness fail; what is the
locality; what is foggy; and what Docker-free test or typed record narrows it. The
witness-quality rubric: asserts the invariant rather than a constant/proxy/substring; has a
recorded negative control; does not stage its own precondition; is deterministic; is wired
in CI; declares an honest locality.

## Cross-cutting fog register

| # | fog | evidence | narrowing | Docker-free |
|---|---|---|---|---|
| F1 | "Independent" reviewer is name inequality | `tools/c357-acceptance.py:154-164` | independence attestation + git-author check | partly (semantic fact remains human) |
| F2 | "Category-A" is a free string | `c357-acceptance.py:159`; `fork_class` in the inventory | derive A from merged-state hash / replay verdict / wire change | yes |
| F3 | Lockstep window is an arbitrary 60 s | `c357-acceptance.py:98-100` | derive from finality/round time or pin it as a network parameter | yes |
| F4 | A campaign's `aggregate: PASS` is trusted | `check-hardfork-51-closeout.py:160-180` | require the criterion fields (anchor, dwell, window, every validator) | yes (record check) |
| F5 | One audit-signoff row covers 47 A candidates | `hardfork-51-closeout.json` `Audit:section6-categoryA` | 47 per-candidate dispositions with reviewer/digest | yes (record check) |
| F6 | Open-row owners are free text ("Jim / governance") | `check-hardfork-51-closeout.py:148`; 18 ledger rows | accountable `@login` in the register; owner-owned ledger change | human |
| F7 | 42 of 47 §6 A candidates use a generic acceptance template | `section6-gap-inventory.json` | named witness + negative control per candidate; 3 already name tests | yes |
| F8 | The machine inventory retains a row the prose says is superseded | `passes.md:395` vs `S6:dc8df48ecb6197ee` | a `withdrawn` disposition via #527 | yes |
| F9 | 107 `done` findings have a `candidate:` law, not an instantiated law | `spec/findings.tsv`; `emit-findings-register.sh:110` | gate-cited rows need a non-candidate witness; no blanket promotion | yes |
| F10 | Load-bearing terms are undefined | mainnet, category-A, independent verdict, divergence; fault/isolation senses | a cited glossary | yes (document) |
| F11 | C343 block-6 disposition is a substring match | `tools/c343-live.py:356-366`; "block 6 unknown" passes | structural block/disposition/height parse + fixture | yes |
| F12 | The C343/C357 self-tests never run in CI | `.github/workflows/ci.yml:178-200` | wire the tests and emitted TSVs; workflow edit needs the owner | yes (local) |
| F13 | The locality gate cannot see packed records | `evidence-pack.py:360-362` vs `check-evidence-locality.py:48-66` | teach the packed layout; keep the existing labels as aliases | yes |
| F14 | A Rust witness is proven to have run, not to assert the invariant | `check-rust-witnesses.sh:149-154` | a recorded negative control per witness; no control = unproven | yes |
| F15 | Done-row evidence is presence-of-identifier | `emit-findings-register.sh:137-139` | gate-cited evidence resolves to a definition or test | yes |
| F16 | 143 proved laws lack the formal/mutant/falsifier rung | `formalMissingCeiling := 143` | do not formalise wholesale; require the rung only where a gate cites the law | partly |
| F17 | Testnet rows rest on `unmeasured`, `symbol read` or `[self-referential]` | `docs/src/spec/testnet-acceptance.md:465,674,1161,1442` | run the named arm or downgrade the row | no (live) |
| F18 | `CasperFinality.tla` is described as formalized but has no Spec | `passes.md:4092` | write `Spec`/`Init`/`Next` or relabel it as a reference tree | yes |
| F19 | Negative tests pin a vulnerability rather than close it | `node/tests/validator_identity_record.rs:77,126` | never read a green negative test as a gate; name the startup check | yes |
| F20 | M5's historical bundle is unavailable | #428 comment 6892934659, 2026-11-15 | keep it a continuing blocker; no waiver; component/hot-cell evidence only | external |
| F21 | A1.3 (N=8) fails on stake, 400/800 | `testnet-acceptance.md:79` | map it to finality/supermajority (M4/TA), not a blind rerun | no (live) |
| F22 | M4, G4 and C357 are at risk of being conflated | gate text vs the criterion maps | criterion-level crosswalk; one sign-off never discharges another | yes (record) |
| F23 | M5's report is computed from the same fold that builds the state | `casper/src/merging.rs:2390-2398` | a state readback of each kept datum/kont/native effect | yes |
| F24 | The final-scope ledger trusts `block.rejected_deploys` | `multi_parent_casper.rs:258-264` | derive rejection from the conflict relation, or fault the omission | yes |
| F25 | `evidence-pack.py verify` is pin-format-blind | `tools/evidence-pack.py:298-345` | require 40/64-hex pins and a non-claims boundary | yes |
| F26 | G1 checks four-way agreement, not the selected candidate | `tools/live-gate.py:472-551` | cross-check the owner's selected pins with `releaseSelected` | yes |
| F27 | G6 has no typed reviewer-verdict record | `tools/live-gate.py:980-989` | a typed G6 record with identity, independence, verdict and cited digest | yes (record) |
| F28 | M1's supported tier is a policy table, not a measurement | `validator-requirements.md:13,21,29-32`; `policy.rs:29-52` | a typed tier record checked against measured CPU/RAM/RSS/replay | yes (record) |
| F29 | M1's host topology is advisory and PID-namespace-blind | `node/src/resources/colocation.rs:7-27` | a stake/host map plus a host-kill arm | unit yes; arm multi-host |
| F30 | M2's in-flight bound is opt-in/default-off and tests bypass the listener | `http.rs:135-146,2581-2631` | a real-listener pressure test with the default configuration | yes |
| F31 | M2 has no direct-HTTP witness; the nginx rotation is explicitly not one | #428 body, provenance note 2 | a direct-to-40403 pressure arm | needs a routable path |
| F32 | M2 refusal classification is a three-string prefix match | `block_api.rs:750-754`; `http.rs:750-758` | one closed refusal vocabulary; property test for 429 vs 400 | yes |
| F33 | M3 transport completion was not correlated into a record; the per-peer outcome exists since `431305f67` | `transport_layer_syntax.rs:20-28,47-58`; `node_running.rs:644-670` | a typed transport-outcome line and a stage-6 served/restored/walk chain | yes (`tools/m3-join-chain.py`); Docker for the replay |
| F34 | M3's off-host lint is gameable by wording, and failover is absent | `check-evidence-locality.py:158-200`; `m3-m5-pareto` M3-11 | a typed six-stage chain (`join_chain`), the real second-host negative arm, and failover | detector yes; arm multi-host |
| F35 | M4's N=4/N=8 arms are single-host Docker at 1.5 GiB, below the 8 GiB floor; host loss is unmodeled | `consensus-quiet-finality.py:190-201` | multi-host N=4/N=8 on the selected candidate; a two-validator-per-host arm | no (multi-host) |
| F36 | M4's anchor-retention cell is strong; equal moving heads is the superseded criterion | `consensus-quiet-finality.py:96-116`; `consensus-partition-rejoin.py:376-396` | keep anchor retention; do not regress to equal heads | yes (typed tests) |

## Gate-family findings

The per-gate attack rows from the nine lenses are recorded in the sections below. A row
with no concrete defect-passing scenario is a no-defect argument, not an omission.

### M1 — capacity and fault domains

| sub-criterion | current measurement | can a defective build pass? | locality | narrowing |
|---|---|---|---|---|
| Supported RAM/CPU tier | A policy table: 2–4 vCPU/8 GB "minimal", 8+/16–32 GB "comfortable", explicitly "estimate, not a measurement" (`validator-requirements.md:13,21,29-32`); the preflight floor is "unmeasured above 5k" (`node/src/resources/policy.rs:29-52`) | **Yes.** A validator declared 8 GB minimal that needs 24 GB to replay 5k blocks passes; `--allow-under-minimum` + `--accept-under-minimum-risk` (`policy.rs:216-229`) lets it run, and no test contradicts a wrong tier | preflight Docker-free; soak Docker | A typed `{tier, cores, RAM, disk, peak RSS, replay time at height}` record and a check that the declared tier equals the measured one; reuse `n420-soak.py`, `resource-restart-arm.py` |
| Peak RSS | One 1142-block replay point (57→284 MB); the 3200-block row is from a tree the repo no longer builds (`validator-requirements.md:96-136`); `state.rs:82` publishes RSS but nothing compares it to a bound | **Yes.** RSS can grow without bound; it is a gauge with no ceiling or gate | parse Docker-free; soak Docker | Soak at several heights with an RSS/time budget assertion |
| Replay headroom | `policy.rs:46-51` says "unmeasured above 5k"; H-RS-04 says "unmeasured at target height" | **Yes.** Superlinear replay (`validator-requirements.md:126-136`) makes any headroom number unsupported | Docker | A measured replay curve keyed to height |
| Stake/failure-domain map | `node/src/resources/colocation.rs:7-27` counts `rnode` processes in one PID namespace, advisory and never refuses; F-U18-04 is explicitly owed | **Yes.** The #420 topology (two validators per host) passes: containers hide siblings, `count_rnode_processes` returns 1, the warning is advisory, and no host-kill arm checks finality | unit Docker-free; multi-host required | A typed stake-map/host-map record plus an N=4/N=8 host-kill arm; reuse `tests/consensus-partition-rejoin.py`, `wargame-arms.sh` |

### M2 — ingress and overload

| sub-criterion | current measurement | can a defective build pass? | locality | narrowing |
|---|---|---|---|---|
| HTTP boundedness | Per-IP 100/s limiter (`http.rs:2581-2597`) and an **opt-in** in-flight cap (`http.rs:135-146,2618-2631`); pool bounds (`casper/src/dag.rs:37,82`); tests call the router directly with `UNATTRIBUTED_PEER` | **Yes.** With the env unset there is no in-flight bound; a proxy that strips client IP makes everyone share one bucket; no real listener or JSON decode is exercised | Docker-free unit; independent pressure absent | A real-TCP-listener integration test with N concurrent bodies asserting 429 + Retry-After, no 5xx, and pool stats |
| Direct HTTP | None; the node has one HTTP listener, and the issue says the nginx rotation is "explicitly not a measurement of direct HTTP or gRPC" | **Yes.** Zero direct-HTTP evidence; a listener that 5xxes under burst has no contradicting witness | would need a routable path | A direct-to-40403 pressure arm without nginx |
| gRPC boundedness | The keyed limiter is unit-tested (`mod.rs:217-237`); the `InterceptedService` and the `RESOURCE_EXHAUSTED`/retry-after mapping (`tonic.rs:468-480`) are not | **Yes.** An interceptor that admits unconditionally, or returns `unauthenticated` instead of `resource_exhausted`, stays green; the 16 MiB decode cap has no witness | Docker-free unit | Test the interceptor with a mocked request and assert `resource_exhausted` + retry-after |
| Bounded queues | `check-bounded-ingress-queues.sh` greps four named bindings in `node_runtime.rs` for `unbounded_channel` | **Yes.** An absent binding is printed but does not count as a violation (`:33,52-54`); `mpsc::channel(usize::MAX)` and `channel::unbounded()` pass; a queue elsewhere in the ingress path is not scanned; the bound N is not pinned | Docker-free source check | Make absent a violation, scan the whole ingress path for both spellings, and pin the bound |
| Explicit backpressure | The C105 handler waits when the routing queue is full (`comm/src/rp/handle_messages.rs:343-395`), a genuine negative control | **Yes on the relay path.** F-U8-02/S6 says the relay spawn discards per-peer results and an admitted deploy no peer takes is stranded; B42 relay is opt-in | Docker-free | Extend the C105 shape to the deploy-copy/relay path and add a capacity-refusal counter |
| Deterministic acceptance | A three-string prefix match classifies bounded refusals (`block_api.rs:750-754`, `http.rs:750-758`) | **Yes.** A new refusal class ("deploy pool is at its byte ceiling") falls through to HTTP 400 — a bound reported as malformed — and no test catches it | Docker-free | A property test: every refusal in one closed vocabulary maps to 429; everything else to 400 |

### M3 — bootstrap and networking

| sub-criterion | current measurement | can a defective build pass? | locality | narrowing |
|---|---|---|---|---|
| Off-host receive and apply | The retained M3 run is instrument PASS / join FAIL: one Docker host, `local-devnet`, `execution_proven:false`. The per-peer stream outcome now drives the stage-4 line (`node_running.rs:644-670`), and `tools/m3-join-chain.py` requires all six stages, a `delivered` outcome, and the served/restored/walk hash equality | **Partly closed.** The typed chain refuses a send-log-only claim, but no in-tree record is an off-host positive; the joiner's walk root is content-addressed, not re-derived from a replay | Docker, single host | A routable two-host observer and a packed off-host `chain.json`; failover |
| Loopback negative control | A runtime classifier (`handle_messages.rs:279-290`) with Docker-free unit tests, plus a document-level lint (`check-evidence-locality.py:158-200`) | **Partly closed.** `local_address` already refuses RFC1918, ULA, link-local, reserved and legacy-numeric forms, and `join_chain` refuses a local advertised peer for an off-host policy. The remaining gap is the real second-host refusal, not the string classifier | Docker-free classifier; end-to-end negative absent | A real second-host negative arm asserting the named refusal |
| Alternative-peer failover | Absent; M3-11 never landed and is default-off | **Yes, trivially** — there is no test to fail | none | Implement M3-11 and an arm |
| State hydration | Locally real (put + read-back, `node_syncing.rs:651-679`); the walk completion and the served/accepted/restored hashes are now named in the log and required by the chain | **Narrowed.** The chain records the served hash, the walked root and the read-back; for a certified fringe the accepted run root is a different root by design and is recorded separately | Docker | Pack the stage-6 chain from a real join; failover remains |

### M4 — finality, liveness and recovery

| sub-criterion | current measurement | can a defective build pass? | locality | narrowing |
|---|---|---|---|---|
| N=4/N=8 finality | Quiet-finality and partition arms exist, but run as Docker containers on one GitHub-hosted runner, **1.5 GiB per node**, below the documented 8 GiB floor (`consensus-quiet-finality.py:190-201`) | **Yes.** A build that finalizes only when every validator shares one bridge/host passes; "adequate independent-host capacity" is unmeasured | Docker, single host | Multi-host N=4/N=8 on the selected candidate; reuse the quiet-finality and partition harnesses |
| Partition/rejoin | `docker network disconnect` on one bridge; frozen-fence + anchor-retention predicates with strong negative controls | **Yes.** A real cross-host partition (route flap, minority online side) is not exercised; the bridge disconnect is not the #420 two-host fault domain | Docker, single host | A cross-host partition/heal arm; keep the anchor-retention predicate |
| Host loss | Not separately tested. Partition disconnects a container; restart kills a process | **Yes.** The #420 topology (one host carries two of four equally bonded validators) is unmodeled; no test can fail | none | A physical/2-validator-domain arm |
| Restart/replay and safe recovery | Restart-under-load, post-restart proposal, natural trigger and resource-restart arms; Docker-free predicate controls for wrong genesis, incomplete ancestry and interrupted restart | **Yes, weaker.** Every arm is Docker/single-host; the installed release-candidate restart envelope is unexecuted (#584 G4 is INCOMPLETE) | Docker; controls Docker-free | The #584 G4 live arm on the selected candidate |
| Anchor retention / approved root | **The strongest cell.** `/api/is-finalized/<old hash>` must stay true while the fringe representative advances (`consensus-quiet-finality.py:96-116`); the partition arm keeps a frozen finalized fence and a three-poll dwell | **No defect found** within the local predicate: a build that rewrites the root fails `verify_finality_progress`. The remaining gap is that the predicate has not run on the selected candidate at N=4/N=8 | Docker; typed tests Docker-free | Run the existing predicate on the selected candidate; do not replace it with equal-heads |

### M5 — state integrity and merge accounting

| sub-criterion | current measurement | can a defective build pass? | locality | narrowing |
|---|---|---|---|---|
| M5-01 historical 840→841 replay | The `n420-root-reads` record is INCOMPLETE and no node retains 835–845; the capture instrument exists (`tools/n420-root-capture.py`), the bytes do not | It cannot pass or fail: the input is absent. A fabricated fixture would be green over non-incident data | external, perishable input | Keep the owner's UNAVAILABLE decision as a continuing blocker; the `capture.json` shape (per-preimage `verify_ok`, byte-identical Root reads, `complete:true`) is the pass shape |
| M5-04 final-scope rejected deploys | `final_scope_rejections` propagates a causal finaliser's `block.rejected_deploys` verbatim (`multi_parent_casper.rs:258-264`); only aggregate counters are emitted | **Yes.** A proposer that emits `rejectedDeploys: []` while excluding the deploy yields `final_scope_rejected_chains == 0`, a quiet report and a missing effect — the #420 class — and no in-tree falsifier injects that omission | Docker-free | Add a falsifier that feeds a rejection map missing a known-excluded chain, or derive the rejection from the conflict relation instead of trusting the block field |
| M5-06 kept-effect coverage | `kept_effect_coverage` compares per-chain `state_changes` with `all_changes` (`merging.rs:2390-2398`), the same fold that builds the state | **Yes.** A drop in trie application, number-channel merge or checkpoint write leaves `outcome.state` missing the effect while every report field stays quiet; no test re-opens the produced state | Docker-free | Add a readback test: after `MergeScope::merge`, re-open `outcome.state` and assert each kept datum/kont/native effect is present |
| M5-03 per-chain effect verdict | Unimplemented: only aggregate counters exist; M5-03 is a Pareto item gated on M5-01/M5-02 | Nothing to pass; the pass condition's last clause is unmeasured | Docker-free once the inputs exist | The Lean model already types `Verdict`/`classify` (`MergeAccounting.lean:60-74`); emit that DTO from `MergeReport` and check totality/coverage |

### M6 — reproducibility and sign-off

| sub-criterion | current measurement | can a defective build pass? | locality | narrowing |
|---|---|---|---|---|
| G1 installed pins | `live-gate.py:472-551` checks four records, distinct identities, agreement and digest format | **Yes.** Four agreeing records pinning the wrong (non-selected) binary pass; G1 checks agreement, not that the pins are the owner's selected candidate | external; checker Docker-free | Reuse `c357-acceptance.py releaseSelected` (owner pins == campaign pins, `decision_url` https) as a typed G1 cross-check |
| G5 independent observations | `live-gate.py:824-931` checks four aliases, per-sample digests, `execution_proven:false`; `evidence-pack.py verify` checks bytes/digests/placeholders | **Yes.** A self-consistent fabricated run dir with matching digests verifies; `verify` does not require 40/64-hex pins or a non-claims boundary | external; checker Docker-free | Require pin formats and a non-empty non-claims section in `evidence-pack.py verify`; add a fake-pin negative self-test |
| G6 independent verdict | Structurally absent: `live-gate.py:980-989` hardcodes `G6: PENDING`, `overall: NOT_A_VERDICT` | **Yes.** A free-text PASS README is the only artefact; nothing validates reviewer identity, independence or the cited record | external | Add a typed G6 record (reviewer, relationship, independence, verdict enum, evidence digest, verdict URL, signed_at), reusing the c357 shape |
| M6 reproducibility record | `evidence-pack.py` checks digests and placeholders; it does not require source/binary pin formats or host/config/stake/clock boundaries | **Yes.** A self-consistent record with arbitrary pin strings and no non-claims passes | Docker-free record check | A schema-2 record: pins 40/64-hex, host/config/stake/clock, log hashes, deploy IDs, block hashes, commands, CI runs, raw outcomes, reviewer conclusion, non-claims |

## Ranked findings (wave one)

1. **M1's supported tier is not measured.** The only numbers are a policy table that says
   "estimate, not a measurement" and "unmeasured above 5k"; a wrong tier is unfalsifiable, and
   `--allow-under-minimum` lets it run.
2. **M1's failure-domain map is advisory and PID-namespace-blind.** The #420 topology (two
   validators per host) passes every current check; there is no host-loss arm.
3. **M3 has no off-host positive.** The transport outcome is per-peer since `431305f67` and the
   six-stage chain is now typed and checked (`tools/m3-join-chain.py`), but no in-tree record is an
   off-host positive; failover (M3-11) is absent; the real second-host refusal is unrun.
4. **M4's N=4/N=8 arms are single-host Docker at 1.5 GiB**, below the documented 8 GiB floor;
   partition is a bridge disconnect, and host loss is unmodeled. The anchor-retention cell is
   the strongest predicate in the tree and should not be replaced by equal heads.
5. **M5's final-scope ledger trusts `block.rejected_deploys`**, and the report is computed from
   the same fold that builds the state; a proposer that emits `[]` or a trie-application drop
   leaves a quiet report and a missing effect. M5-01's bundle is unavailable.
6. **M6's evidence verify is pin-format-blind**; G1 checks four-way agreement, not the selected
   candidate; G6 has no typed reviewer record.
7. **M2's in-flight bound is opt-in/default-off**, its tests bypass the listener, direct HTTP
   has no witness, the queue check treats an absent binding as green, and a new refusal class
   falls through to HTTP 400.

### #51 and §6

| gate | finding | narrowing |
|---|---|---|
| #51 closeability | `closeable` is four conjuncts: no open rows, the single §6 sign-off "verified", `closure_requested`, and `--verify-local-git`. The sign-off needs only a 64-hex string, two URLs and a name; the digest is never checked against a review. `--assert-closeable` does not imply `--verify-local-git`, so it cannot pass alone | Bind the register statuses; require a per-candidate disposition and a digest-bound evidence record; document that both flags are needed |
| 16 `IMPLEMENTED / EVIDENCE INCOMPLETE` | The witness is `merge_commit` ancestry. A later regressing commit is also an ancestor, so a build where the fix was reverted passes. No test is run | Name and run the acceptance test, or hold the gate `owes`; the register does the latter |
| 3 release blockers | No witness at all; a fabricated `acceptance_records` entry (40-hex source, 64-hex binary, `aggregate: PASS`, any artifact string) passes the shape check | Typed records: vault conservation, state manifest, attachment rollout (the three new schemas/checker) |
| 24 decision/deferred rows | The owner-and-reason record satisfies the close clause, but 11 "requires maintainer decision" rows describe unlanded work, not only a choice. `B:breaking-release` sat at `decision` while its attestations are absent; `Batch:crypto-agility` is not landed | The register overrides those two to `blocked`; the rest stay `decision` with the unlanded work named |
| 47 §6 A candidates | Every candidate's only evidence is an authored sentence. The 3 "named" tests: the merge-diff and replay-order tests exist; `tests/liveness-enforcement.py` **does not exist**. The Decimal row is still an A candidate although #527 removed the type; the supersession is prose outside the row digest | Correct the missing-test status, mark the Decimal `superseded`, and add the two PR #600 rules |
| PR #600 | Checkpoint v3 (Law 117) and the merge fork rule (Law 115) are prose after the §6 table and absent from the 94-row inventory, the 47 candidates and the 44 obligations; "all 94 rows" is stale | Add `S6-PR600-CHECKPOINT-V3` and `S6-PR600-MERGE-FORK` to the register as `reviewable` |
| Pinned-blob check | `check-section6-inventory.py` hashes the local `passes.md`, not the `source_dev_sha` blob; the pin is only string-compared across the three JSONs | Record the limitation; a full fix needs the pinned blob or a network fetch |
| Deferral fog | `Audit:law53-estrangement` (C173) and `Audit:inactivity-leak` are deferred for non-technical reasons while their §6/liveness obligations remain A-candidates or M4 inputs | Keep the deferral but record the underlying obligation in the register |

**Landed by this readiness commit:** the #51 gate binding (`require_gates`, default off, exit 3
preserved); the 47 per-candidate dispositions (46 reviewable, 1 withdrawn), with the register's S6
rows at 45 reviewable, 3 owes and 1 superseded (43 inventory candidates plus the 2 PR #600 rules);
the register's `S6-0963` correction, the Decimal `superseded` status and the two PR #600 rows;
the three release-blocker contract schemas/checkers. **Remaining:** independent dispositions,
the §6 sign-off's digest binding, a test-running acceptance checker, and the pinned-blob check.

### #584, testnet §3, C343/C357

| gate | finding | narrowing |
|---|---|---|
| G1 installed pins | `live-gate.py:472-551` checks four-way **agreement** on digests, distinct names/boot IDs and formats; it never binds the pins to the owner's selected candidate. Four agreeing records on the wrong binary, or four fabricated boot IDs from one host, pass | Cross-check the owner's selected pins with `c357-acceptance.py releaseSelected` |
| G2 signed lifecycle | The four stages are built from the **union of all aliases** (`live-gate.py:588-616`); a validator that never observes pooled/included still contributes. "Accepted" is only the operator-authored receipt | Require per-alias stage coverage |
| G3 no false settlement | The check cross-checks the record's own `claims.json`; there is no required negative pre-finality not-finalized sample. A record that declines to claim settlement skips the positive assertion | Require a pre-finality not-finalized sample before the first finalizedSuccess |
| G4 recovery without loss | The before/after/lost lists and `same_deploy_finalized` are self-authored; no sampled evidence proves a restart occurred | Bind recovery to sampled post-window events |
| G5 independent observations | Four arbitrary 64-hex endpoint digests with matching body digests and a covering `SHA256SUMS` pass; `execution_proven` is deliberately false | Require 40/64-hex pins and a non-claims boundary in `evidence-pack.py verify`; add a fake-pin negative self-test |
| G6 verdict | `live-gate.py:980-989` hardcodes `G6: PENDING`; a free-text README is the only artefact | A typed G6 record: reviewer, relationship, independence, verdict enum, cited evidence digest, URL, signed_at |
| C343 | The block-6 cell was a substring match: "block 6 unknown" passed. The 16 Nov observation supports reachability only; the operator route 404s and the file logs are unstructured | Structural block/disposition/height parse; a 404 stays PRECONDITION; self-test cases |
| C357 | Independence was reviewer-name inequality; category-A was a string literal; the 60 s window was arbitrary; the reconstructed-trace fields were prefix-checks | Independence attestation; derived/declared window; digest-bound URL/evidence |
| TA-C3 | The register gave a census a pass/fail shape and its reason wrongly said the bounded-adversary statement does not exist | Correct the reason to `docs/src/spec/adversary.md`; mark it explicitly not pass/fail |
| Locality | Packed `manifest.json`/`raw/` records were invisible to the gate; a single-host run could self-label off-host | Scan the packed layout; add `single-host`; correct `n564-binary-qualification` to `single-host` |

**Landed by this readiness commit:** the structural block-6 parse and its self-test cases; the
C357 independence attestation, declared lockstep window and self-test cases; the packed-record
locality scan and the `n564` correction; the TA-C3 correction; and the register's C357 wording.
**Remaining:** the G1–G6 typed records/checkers and CI wiring; the live G1–G6 run itself.
The principle-level layer that makes these criteria executable and owner-independent is
[`gate-principles-20261118.md`](gate-principles-20261118.md).

### Measurement validity

The cross-cutting lens attacked the new register itself. The first version was gameable:
a bare existing file was a valid witness; a `struct` definition counted; `negative_control` was
a never-run string; evidence digests were format-only; independence was self-asserted;
`source_ref` could bind a gate to an unrelated matrix row; the `sources` block was unread;
`register_dev_sha` was any 40-hex; and `does_not_claim` was never read. The register was also
untracked and unwired, so none of it ran anywhere.

**Landed by this readiness commit:** the checker now requires a `path::symbol` witness whose
`#[test]` attribute is verified (or a theorem/`def` matching the kind); a control object with a
resolvable test `control_ref` and an expected `FAIL`/`PRECONDITION`; a retained evidence record
with `manifest.json` and `SHA256SUMS.json`; an owner-independent attestation with a verdict;
a `source_ref` that names the gate's own row and a title that matches the ledger; a `sources`
block that matches every family; `register_dev_sha` ancestry; a resolving law citation with a
formal rung for a met gate; and a non-empty `does_not_claim`. The tests add one mutant per rule.

**Remaining:** wire `tests.test_mainnet_gates` and the other self-tests into CI (workflow scope);
run `evidence-pack.py verify` from the gate checker; make the crosswalk criterion text
directional and checkable; and add an assertion-level runner for Rust witnesses (the current
check proves a test ran, not that it asserts the gate's invariant).

## Ranked findings (wave two)

1. **The §6 sign-off is forgeable.** One 64-hex string, two URLs and a name satisfy the only
   row that closes the tracker; the digest is never checked against a review.
2. **The register itself was gameable.** A bare file, a `struct`, a never-run control, a
   format-only digest and a self-asserted independence all passed; the checker is now hardened,
   but the self-tests are not in CI.
3. **The 16 evidence-incomplete rows are ancestry-only.** A regressing follow-up commit is also
   an ancestor; no test runs.
4. **The 47 §6 candidates are authored sentences.** Two named tests exist, one is missing, and
   the Decimal row is a phantom; the register now marks it `superseded` and adds the two PR #600
   rules, but 43 inventory candidates plus the 2 PR #600 rules remain `reviewable`.
5. **G1–G6 have no typed verdict/selection binding.** G1 checks agreement, not the selected
   candidate; G2 unions aliases; G3 has no negative pre-finality sample; G4's no-loss facts are
   self-authored; G5 is self-consistent-fabricable; G6 is a hardcoded pending stub.
6. **C343's block-6 cell was a substring** ("block 6 unknown" passed); the structural parse and
   self-test cases now land, but the self-test is not in CI.
7. **C357's independence, category-A and 60 s window were strings**; the attestation and the
   declared window now land, but the category-A derivation and CI wiring remain.
8. **The locality gate was blind to packed records and gameable by wording**; the packed scan,
   `single-host` label and the `n564` correction now land.
9. **The 3 release blockers had no witness**; the vault/state/attachment contracts and checker
   now exist, but the operator exports and independent reconciliation remain external.
10. **M4/M5 had no criterion record check**; the rejoin and merge-accounting checkers now exist,
    but the N=4/N=8 and 840→841 runs remain external, and M5's bundle is unavailable.

## What this review does not claim

No gate is met; no live campaign ran; no independent reviewer signed anything; no
`findings.tsv` row moved. The register records what the ledgers and pinned gate text say,
and the checker refuses a `met` gate without a resolving witness, a negative control,
pinned evidence and a satisfied dependency. An absent artefact is `PRECONDITION`/`owes`,
never `PASS`.

## State

- 111 registered gates; **0 met**; every gate has a status and a reason.
- 439 findings — 0 todo, 2 in progress (C343, C357), 437 done.
- `check-mainnet-gates.py --assert-closeable` exits **3**; the #51 tracker also exits 3
  with its 18 open obligations.
