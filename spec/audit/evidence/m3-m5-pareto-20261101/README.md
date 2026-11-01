# M3 × M5 Pareto review, 2026-11-01

Issue **#428** leaves two release-blocking gates unadvanced by the diagnostics work: **M3** (off-host
bootstrap/fringe completion) and **M5** (finalized state integrity and merge accounting). This is a
six-agent review of the pieces that could close them, ranked for **reward per unit risk**.

## Method

Two inventories (M3 code/harnesses/issues; M5 merge code/evidence/registers), a risk assessor, a
gate-reward assessor against #428's own wording, an adversarial reviewer, and a referee that
reconciled their disagreements against the code. Reward is scored 0–5 against the gate's measured
sentences; risk is blast radius (0 = test/tooling, 1–2 = additive report/diagnostic, 3+ = a runtime
path, 4–5 = merge/consensus); effort includes the evidence. **The gates are lexicographic** — both
are release-blocking, so reward is never traded across M3 and M5.

Two corrections the review made to the working assumptions:

1. **M5 already emits the accounting.** `MergeReport` (`casper/src/merging.rs`) already carries
   `final_scope_rejected_chains`, `rejections_without_a_conflict`, `unapplied_kept_writes`,
   `dropped_native_slots` and `native_writer_of_slot` (C260). What is missing is the **source-pinned
   replay** and the **per-chain deploy fates**, not the counters.
2. **M3's live evidence is invalid as an off-host positive.** The 2026-10-29 observer advertises
   `127.0.0.1` (and bootstraps to `127.0.0.1`). The record must say so before any further M3 claim;
   the corrected record is the first negative fixture for the locality lint.

## The Pareto frontier

| id | gate | piece | closes | reward | risk | effort |
|---|---|---|---|---|---|---|
| **M5-06** | M5 | Tuple-space counterpart to I2: every kept chain's datum/kont change appears in the combined batch; emit `unapplied_kept_datums`/`_konts` | partial | 5 | 1 | 2 |
| **M3-01** | M3 | Off-host positive join: routable `--host`, no validator key, bootstrap **dials back** the advertised address before the join | partial | 4 | 1 | 3 |
| **M5-01** | M5 | Commit the live 840/841 source bundle: five verified block preimages + Root state reads (perishable) | partial | 4 | 1 | 2 |
| **M3-02** | M3 | Log the advertised `rnode://` peer; warn/refuse a loopback host with a remote bootstrap; same-host loopback stays legal | partial | 3 | 1 | 1 |
| **M5-04** | M5 | `final_scope_rejected_deploys`; fix the fringe merge's slot winners being dropped from the aggregate | partial | 4 | 2 | 1 |
| **M5-03** | M5 | Per-chain fates DTO: decision, deploy ids, tuple-space diff, native slots, reason | **full** | 5 | 1 | 3 |
| **M5-02** | M5 | Merge-replay surface for an arbitrary historical block (it appends a checkpoint; not strictly read-only) | partial | 5 | 2 | 3 |
| **M5-05** | M5 | Sibling-vs-sibling hot-cell merge test (the parallel arm as a parallel merge) | partial | 3 | 0 | 2 |
| **M5-08** | M5 | Fail-closed offline verifier for the M5 bundle | **full** | 3 | 0 | 2 |
| **M3-08** | M3 | Retained M3 evidence record + offline verifier + workflow | partial | 3 | 0 | 2 |
| **M3-00** | M3 | The assembled off-host campaign (positive + delivery + negative + provenance) | partial | 5 | 1 | 4 |
| **M3-04** | M3 | Key-bearing requester/responder logs (block hash, state path) | partial | 2 | 0 | 1 |
| **M3-06** | M3 | Name the responder's fringe refusal and construction, not only the send | partial | 2 | 0 | 1 |
| **M3-14** | M3 | Mine the 2026-10-27 fresh-join journal against the six-stage chain | none | 2 | 0 | 1 |

### Dominated but retained

Each is the only instance of its artefact, so none is dropped: **M3-03** (the negative witness;
numerically dominated by M3-02), **M3-05** (transport completion — an enabler for M3-01/00),
**M3-11** (failover — the only runtime liveness change, default-off), **M5-07** (devnet witness when
the live bundle is gone), **M5-11/M5-12** (hash invariance / provenance), **M3-12/M3-13/M3-15**
(provenance hygiene). **Zero gate value:** M3-09/M3-10 (sync diagnostics), M5-09 (per-merge
persistence — the only new validation-path write), M5-10 (docs after evidence).

## Sequencing

- **Wave 0 — evidence hygiene.** M3-12, M3-15, M3-14. Cheap; prevents the false-positive class.
- **Wave 1 — best reward per unit risk.** M5-06, M5-01, M5-04 + M5-11, M5-05.
- **Wave 2 — the replay.** M5-02 → M5-03 → M5-08; M5-07 in parallel if the live data is gone.
- **Wave 3 — the M3 delivery chain.** M3-02 → M3-04/M3-06/M3-05 → M3-01 → M3-03 → M3-08 → M3-00.
  M3-00 stays partial until M3-11's failover arm exists.
- **Separate units:** M3-11 (default-off failover) and M5-09 (default-off persistence) are never
  bundled with the evidence work.

## What the first unit lands

Wave 0 and the Wave-1 pieces that do not depend on the live bundle: the M3 record correction and
locality lint; M5-06 (tuple-space I2), M5-04 (final-scope deploy ids, fringe slot winners), M5-05
(sibling hot-cell) and M5-11's deterministic re-merge; M5-01's capture instrument and an
**INCOMPLETE** record — no node retaining 835..845 was reachable from this session, and no fixture
was substituted. M5-01's data and M5-02's replay are the next unit; the pinned-constant hash
invariance over the live bytes belongs with them.

M3-14 is also **INCOMPLETE**: the 2026-10-27 journal is pinned by digest but not committed and was
not retrievable here (`M3-14-journal.md` names the stage table and the recovery rules).

The positive-arm host guard moved to M3-01 with the runner that takes a `--host`: this unit's
machine check is the evidence lint, which is the same property applied to the claim.

## Evidence standard

Every claim needs a preserved, independently rerunnable witness with provenance: committed under
`spec/audit/evidence/` with byte digests; source/binary/command/UTC; an instrument that fails when a
stage, hash or deploy fate is removed; M3 correlated by peer id/address and block hash or state path
on **both** ends (never a send log); M5 counts re-derived from the verified bytes.

## Non-claims

No candidate closes M3 alone; M3-00 is partial until failover. No M5 candidate recomputes 840→841
until M5-01's pinned inputs and M5-02's replay both exist. The 2026-10-29 record is a loopback
configured-input diagnosis, not an off-host positive. M5-02 is not strictly read-only (it appends a
checkpoint). No candidate changes consensus validity, block encoding, attestation, finality or
RCHIP-55.
