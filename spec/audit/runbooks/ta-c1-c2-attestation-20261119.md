# TA-C1 / TA-C2 independent attestation — pre-registered protocol

**Status: DRAFT — NOT EXECUTED.** This runbook fixes the protocol before a review so the
attestation cannot be written around the result. It does not claim that any review has happened.

- Gates: `TA-C1` (anyone can propose, production is bounded) and `TA-C2` (validators can be
  dropped and joined without risk).
- Evidence destination: the verified record each attestation cites, plus
  `spec/audit/evidence/ta-attestation-<date>/review.md`.
- Template: `spec/audit/runbooks/ta-attestation.template.json`.
- Prerequisite reading: `spec/audit/gate-principles-20261118.md` (`human_attestation`).

## 1. What this window must establish

An independent reviewer reads a verified evidence record for the gate, writes and signs a typed
attestation, and cites the record's manifest digest. The criterion checks the attestation's shape,
the reviewer's independence from the gate owner, the digest's equality to the cited record's
manifest, and that the cited record is inside the repository.

There is no tool producer by design: the observation is a human judgement, and manufacturing one
would defeat the gate.

## 2. Hard NO-GO conditions

- **The reviewer is the gate's owner, its operator, or an author of the cited record.**
- **A missing or mismatched signed digest.** The `evidence_sha256` must equal
  `sha256(<record>/manifest.json)`.
- **A cited record outside the repository**, or one without a `manifest.json`.
- **No https verdict URL.**
- **An INCONCLUSIVE verdict** for a promotion.
- **No review record** naming the reviewer, the verdict and the digest.
- **Any gate promotion is out of scope of the runbook itself.** Promotion is a separate register
  decision after the attestation is reviewed.

## 3. Preflight (fill before the review; a missing field is NO-GO)

| field | value |
|---|---|
| gate (TA-C1 or TA-C2) | |
| cited evidence record (in-repo path) | |
| the record's manifest digest | |
| reviewer name and relationship to the owner/operator/authors | |
| conflicts of interest | |
| signed-at UTC | |
| verdict URL (https) | |
| reviewer signature | |

## 4. Procedure

1. The reviewer reads the cited record, its README and its non-claims. A record that does not
   exist verifies, or is not the gate's record, is a NO-GO.
2. Compute the digest:
   ```sh
   sha256sum <record>/manifest.json
   ```
3. Fill the template; keep it in the review record, not in the attestation alone:
   ```sh
   python3 tools/evidence-pack.py verify --record <record> --require-pins
   ```
4. Sign the attestation and write `review.md` with the reviewer, the verdict, the digest and the
   conflicts.
5. Write the gate observation (`{"attestation": ...}`) into the record, pack and verify, then
   judge:
   ```sh
   python3 tools/gate-check.py --gate TA-C1 --observations <record>/raw/observations.json
   ```
   The same procedure serves TA-C2 with its own record and reviewer.

## 5. Negative controls (each captured, never described)

- **The owner signs their own gate.** The criterion FAILs the independence check.
- **No signed digest.** PRECONDITION.
- **A nonexistent record.** FAIL.
- **A digest that matches no record.** FAIL.
- **A record outside the repo.** FAIL.
- **An INCONCLUSIVE verdict.** PRECONDITION.

## 6. What this window does not establish

- The criterion checks shape and independence, not the quality of the reviewer's judgement.
- No reviewer is manufactured; the independence is a human fact the record states.
- It does not promote either gate; the register's witness/control/evidence binding is separate.
  Both gates depend on `G584-G1`.
