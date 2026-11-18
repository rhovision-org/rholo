# Mainnet gate review, round two — 2026-11-18

**Review of the PR #606 readiness round, run as a second adversarial pass.** No gate is met, no
audit row is flipped, and no release is claimed. The target is the remaining false-pass paths in
the hardened register, the operational fixes and the contract/criterion checkers.

## Method

Three lenses attacked the landed artifacts with the common schema
`attack, where, current_check, caught?, minimal_fix, severity`:

1. **Register and closeout** — witness-to-gate binding, control coupling, evidence verification,
   attestation, source identity, law/crosswalk, `gate_binding`.
2. **Operational** — C343 block-6 parsing, C357 window/independence/category-A/trace, locality.
3. **Contracts, criteria and claims** — vault/state/attachment, M4/M5, dispositions, counts.

## Findings

### Register, checker and closeout

| ID | sev | finding | fix in this round |
|---|---|---|---|
| R1 | P0 | A `met` gate accepted any real `#[test]` anywhere; the fixture used an HTTP readiness test as the M5 witness. | `witness_binding` with the gate's `asserts_sha256`; a `control_of`/mutation/control-test object; an unrelated-witness/control/record refusal test. |
| R2 | P0 | The control was any test, never run, with a decorative `expected`. | `control_of == witness`, a real mutation (`file`/`before`/`after`), a resolving test or `self_test`, and a `--run-controls` mode. |
| R3 | P0 | Evidence was any directory with a manifest and sums; the checker never ran `evidence-pack verify` or read `gate_id`. | The checker verifies the pack, requires the gate id (or `component_of`), `execution_proven: false`, and the binary/source pins to match. |
| R4 | P0 | `gate_binding` trusted status strings, so a fabricated all-`met` register could flip closeability. | `gate_binding` imports the register checker and runs `validate()` before trusting a status. |
| R5 | P0 | `--assert-closeable` skipped `--check`, so a hand-edited register was never compared to the build. | `--assert-closeable` implies `--check`. |
| R6 | P1 | Human attestation was self-asserted; `signed_at` was presence-only. | UTC parse, owner/reviewer substring rejection, and a review record carrying the reviewer/verdict/digest. |
| R7 | P1 | External gate titles/criteria could be rewritten while the source hash “matched”. | Four pinned source excerpts under `spec/audit/sources/`, a per-gate source phrase, and an excerpt/criteria hash. |
| R8 | P1 | §6 `accepted` needed only strings and any 64-hex digest. | A review record, a resolving digest, and a derived category-A effect. |
| R9 | P2 | The landed review/pass counts were stale (45/1/1, 45 candidates). | Corrected to 46/1 dispositions and 45/3/1 register S6 (43 + 2). |

### Operational

| ID | sev | finding | fix in this round |
|---|---|---|---|
| O1 | P0 | The C343 JSON branch was a substring: `unaccepted`/`unfinalized` read as PASS. | `\b` matching in the JSON branch and two new self-test cases. |
| O2 | P0 | The C357 lockstep window was author-chosen (999999), the rationale unread, validators unbounded/under-filled. | `round × liveness` derivation, rationale required, declared == derived, 600 s cap, ≥2 validators with version/binary/height. |
| O3 | P0 | C357 independence was a boolean plus strings; `signed_at: "yes"` and `@alice` vs `@alice-smith` passed. | UTC parse, substring rejection, a review record, and a git-author check where committed. |
| O4 | P1 | C343's bare `#` matched “issue #6”; first-match-wins hid a later orphan. | The bare `#` is removed; all lines are scanned and the worst verdict wins. |
| O5 | P1 | The locality scan ignored `.sh`/`.yml`/`.body`/suffix-less files. | The scan reads every file in the record; a `raw/drill.sh` mutation test. |
| O6 | P1 | C357 category-A was a literal; the reconstructed trace was unbound. | A `category_a_effect` derivation and a reconstruction record carrying the trace digest and reviewer. |
| O7 | P2 | The C343 journal digest was never re-checked; the register's C343 control was incomplete. | The journal digest is compared to `run.json`; the control names the JSON cases. |

### G1–G6 worksheet

| ID | sev | finding | fix in this round |
|---|---|---|---|
| G1 | P0 | The preflight checked four-way agreement but not that the agreed pins were the owner's selected candidate. | `selected_source_sha`, `selected_artifact_digest` and `selected_binary_sha256` are required preflight fields; each validator must match the selection; a missing selection is INCOMPLETE. |
| G2 | P0 | The lifecycle was built from the union of all aliases, so one validator could miss pooled/included/finalized. | Each alias must observe pooled, an included/executed stage and finalized. |
| G3 | P0 | A record could omit any pre-finality not-finalized sample and still pass. | A pre-finality not-finalized settlement sample is required. |
| G4 | P0 | A self-authored `post_recovery_settlement` skipped the sampled post-window check. | A finalizedSuccess sample after the recovery window is required regardless; the claimed dict is an additional check. |
| G5 | P0 | `evidence-pack.py verify` accepted arbitrary source/binary pin strings. | `source_sha` must be 40-hex and `binary_sha256` 64-hex. |
| G6 | P1 | G6 was a hardcoded PENDING; no typed reviewer record existed. | A `--verdict` record is validated (reviewer, named reviewer, verdict enum, evidence digest, https URL, UTC timestamp, cited record); the tool still reports `NOT_A_VERDICT` and never records the verdict. |

### Contracts, criteria and claims

| ID | sev | finding | fix in this round |
|---|---|---|---|
| C1 | P0 | Vault/state/attachment returned a bare `PASS` for a self-consistent fabrication; state entries were optional. | A provenance/evidence envelope with a verified pack; entries and counts mandatory; zero roots refused; `record_check: true`. |
| C2 | P0 | The rejoin record was self-asserted with no campaign/evidence tie. | An evidence record naming `G428-M4` and carrying the anchor; the anchor/dwell/finalizer checks remain. |
| C3 | P0 | `bundle.available: true` was a self-asserted boolean. | A bundle capture record with `complete: true` and per-preimage `verify_ok`, plus a verified evidence record. |
| C4 | P0 | §6 all-`accepted` was forgeable. | A review record, a resolving digest, and the derived effect. |
| C5 | P1 | `S6-0963` said a named test exists; `tests/liveness-enforcement.py` is absent. | The derivation now says the test is absent; the register says `owes`. |
| C6 | P1 | Register↔disposition drift was undetected and the named/missing sets were hardcoded. | The register checker cross-checks the dispositions; the builder derives the test status. |
| C7 | P1 | The landed counts/claims were wrong; “every rule has a mutant” was overstated. | Corrected counts; added the missing contract/criterion mutants; stated exactly which tests are CI-wired. |
| C8 | P2 | Synthetic `PASS` fixtures were footguns. | `record_check: true` and a test that a contract `PASS` does not flip a register gate. |
| C9 | P1 | Crosswalk direction was unenforced. | A cross-family edge must use a directional word (`not`/`only`/`subset`/`peer`/`separate`/`prerequisite`/`component`). |

## Sound halves (not re-litigated)

The HF51/S6 source-row identity and hash binding; dependency acyclicity and met-implies-deps-met;
the symmetric crosswalk; law resolution; the required non-claim; the 111/49/0 counts; the
`n564` single-host correction; the packed-record locality scan; the free-text C343 `\b` match;
the honest C343/C357 register statuses; and the boundary stated in the contract/criterion
docstrings and the register.

## Corrected counts

- **47 dispositions**: 46 `reviewable`, 1 `withdrawn` (the #527 Decimal row); there is no
  "owed" disposition verdict.
- **Register S6**: 45 `reviewable`, 3 `owes` (`S6-f735`, `S6-5651` named tests without a control;
  `S6-0963` missing test), 1 `superseded` (Decimal). Of the 45 reviewable, 43 are inventory
  candidates and 2 are the PR #600 rules.
- The 42-templated/3-named split is correct: 39 "Replay a minimal fixture" + 3 "Independently
  reproduce" templates; the 3 named include the missing `tests/liveness-enforcement.py`.

## What this round does not claim

No gate is met; no campaign, export or independent verdict happened; M5's bundle remains
unavailable; C343's live snapshot remains precondition-only; C357 remains external. The
contract/criterion checkers validate records, not that a campaign ran. The new tests are not
wired into CI; the request is in `spec/audit/ci-wiring-request.md`.

## State

**111 gates, 0 met.** 439 findings — 0 todo, 2 in progress (C343, C357), 437 done. No row flips,
no release claim, no merge.
