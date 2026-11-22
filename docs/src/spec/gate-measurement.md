# Gate measurement

A gate is only as strong as its witness. This page is the standard every release gate in
`spec/audit/mainnet-gates.json` is checked against; the terms are in [Gate terms](gate-terms.md).

**A gate is a boolean test of the current build.** Run the build, get pass or fail. A decision, a
reviewer's verdict, an unavailable historical archive, or an operator export is not a gate; those
are tracked separately.

## What a gate record must name

Every gate names:

1. **Pass condition** and **fail/block condition** — the observable, not the intention.
2. **Observable** — the field, hash, count, verdict or state the witness reads.
3. **Witness** — a resolving `path::symbol`, a script, or a typed human/external record.
4. **Negative control** — a recorded mutant or red fixture showing the witness can fail. A
   witness with no control is `unproven` and cannot make a gate `met`.
5. **Locality** — `unit`, `integration`, `two-binary`, `local-devnet`, `single-host`,
   `live-host`, `external`, or `decision`.
6. **Evidence** — a source commit, a binary digest, an artifact digest and a verdict URL, all
   pinned.
7. **Owner and close condition** — the accountable person and the artefact that ends the gate.
8. **Non-claims** — what the record does not establish.

## Rules

- **A test that cannot fail is not a witness.** The negative control is part of the witness,
  not optional evidence.
- **A record check does not prove a campaign ran.** `execution_proven` is false for the C343 and
  C357 record tools by construction; a record validates its claims, not the world.
- **An aggregate is not a pass.** A campaign must carry its criterion fields: the fixed anchor,
  the dwell samples, the healing window and every-validator finalization for a rejoin; the
  selected pins and the non-claims for a reproducibility record; the per-chain effects for a
  merge replay.
- **A single sign-off does not discharge several gates.** M6, G6, the section-6 category-A
  sign-off and C357 acceptance are separate; a cross-family `related` edge must name the
  criterion it shares.
- **Docker is never the sole evidence.** A local-devnet arm is a diagnosis; a release gate also
  names an independent record or a human attestation.
- **`PRECONDITION` is never `PASS`.** A missing route, a pre-fix field, an unavailable bundle or
  an unplaced observation is `PRECONDITION`; a contradiction is `FAIL`.
- **A name is not independence.** Reviewer independence needs an attestation of relationship,
  affiliation, prior authorship and conflicts, plus the author/committer check where the
  selected source is in this tree.
- **A category is derived, not asserted.** A section-6 row is category-A when its fix changes
  merged-state hashes, a replay verdict, or the wire format.
- **Heights are not blocks.** A height counts rounds; one block per bonded validator per round.

## Status lifecycle

`owes` → a named witness or record is missing. `blocked` → an external input is absent.
`external` → only a human or an external campaign can supply it. `decision` → an owner must
record a decision. `precondition` → the observation cannot be completed. `reviewable` → an
authored disposition, not a measurement. `superseded` → a recorded correction replaced it.
`met` → the witness resolves, the negative control is recorded, the evidence is pinned and every
dependency is `met`.

A gate is `met` only when those conditions hold; the register records how many and every other row
names its blocker. `check-mainnet-gates.py --assert-closeable` exits 3 while any gate is unresolved.

## Cross-family crosswalk

`G428-M4` (finality, liveness and recovery), `G584-G4` (restart recovery) and `C357`
(lockstep/replay acceptance) share events but not criteria. `G428-M6` (reproducibility),
`G584-G6` (live verdict), the section-6 category-A sign-off and C357 acceptance are four
different sign-offs. A `related` edge in the register names the shared criterion; it does not
let one campaign discharge another gate.

**#584's G6 is retired (2026-11-22).** A named reviewer's verdict is not a release condition. The
G1–G5 criteria are superseded by the release-claim readings: G2/G3 → **RC-1/public**, G4 →
**RC-4/public**, G1 → the public campaign's own prerequisite (the manifest the run records), and G5 →
the evidence rule. `tools/release-check.py --assert-ready` is the gate; a `not-observed` arm is
satisfied only by a fresh run on the current candidate, never by a previous build's pins, a previous
window's samples or an attestation.

## Round two — the witness, control and evidence binding

A `met` gate now carries a `witness_binding`: the gate's acceptance hash, a control whose
`control_of` is the witness, a real mutation and the control test (the witness or its
`self_test`), and the expected red. The evidence record is verified by `evidence-pack.py`
and must name the gate; `execution_proven` stays false. The contract and criterion checkers
emit `record_check: true` and never flip a register gate. A record check is not a campaign.
