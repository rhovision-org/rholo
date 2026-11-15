# n-audit-chain-20261115-r1r5

**Evidence locality.** `local-devnet`

## Scope

One validator (the bootstrap, also the audit service) and one archive observer on a disposable local
devnet, built at the R1–R5 source. The run answers: does the attested-audit service sign a
reproduced audit (`AGREE`), and can a second claim be chained to the first and both be verified
offline against their local reports — with both entering the node's accepted store and no conflict
recorded? This is the replacement for `n-audit-chain-20261115`, whose digests predate the R1/R3
commitment and digest changes.

## Provenance

| | |
|---|---|
| gate | `n-audit-chain` |
| source | `a1e013490cde6cd56411f1720b12b6cd5ed008a5` |
| binary | `04a65bda48953d03a7a71e3cac390ec72922b89e9f49eb449a9dcf00f0d07998` (`/usr/local/bin/rnode` in `rnode:local`) |
| run | local devnet; no CI URL |
| packaged | `2026-11-15T19:08:55+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

- **`r1r5-ev-1`**: `POST /api/v1/audit/request {"nonce":"r1r5-ev-1"}` answered `AGREE` at anchor
  height 21. `derivedRoot` is `318bfa69…`; `metadataCommitted` and `windowCommitted` are both true;
  the signed claim (`signed-hash-chain-v1`, sequence 0) names certificate digest
  `c9c232a6…`. `rnode verify-audit-attestation --report r1r5-ev1-report.json <
  r1r5-ev1-attestation.json` printed `OK … sequence 0 digest 1d614e5f…`.
- **`r1r5-ev-2`**: the chained request (`previous = 1d614e5f…`, `sequence = 1`) answered `AGREE` at
  anchor height 50 with its own signed claim (certificate digest `ae553145…`). The offline check
  with `--head-digest 1d614e5f… --head-sequence 0` printed `OK … sequence 1 digest 839cc953…`.
- **The store**: `accepted.json` holds both claims (sequences 0 and 1); `conflicts.json` is empty.
- **What made it work.** The audit re-executes the genesis body's blessed deploys to establish the
  genesis post-state (C390), and hydrates every ancestor-closure header from
  `GET /api/v1/block/{hash}/header` to check the fringe commitment and `historyRoot` (C391, C392).
  Before those fixes the same sequence answered DISAGREE/INCOMPLETE.

`status.json` and `checkpoint.json` are a **post-run reading** of the same observer, taken after the
claims: the chain had advanced past anchor 50 (the capture shows a later anchor). They identify the
source (`a1e013490`) and the served anchor; they are not the certificate either claim was made over.

## What this does not establish

- It is a **local devnet**, not an off-host or public acceptance. It says the mechanism works where
  the node can serve its own certificate, bodies and headers; it does not measure a join, a
  partition or a live network.
- **One signer.** Both claims are signed by the validator's key, which is also the service's key. It
  is a witness that the sign/verify/chain path is real, not that independent operators will produce
  independent claims.
- **The verifier shares the codebase.** The offline check uses the same canonical encoding and
  verifier implementation as the producer; a second implementation would be the stronger witness.
- **No restart control in this run.** The validator restart in the same session failed because the
  boot refused a genesis master with a verified certificate and no recorded accepted fringe — the
  defect registered under C393 and fixed after this source. It is not exercised, or claimed, here;
  a final-source mature-chain cold-restore/restart measurement remains owed.
- **No payment.** No REV moved; the history service was unmetered and no credit receipt or
  authorization was exercised. Laws 110/111 are evidenced by their unit tests, not by this run.
- **`execution_proven` is false** and the chain is short (anchor 50): a finite witness, not a
  universal claim about replay cost or every chain shape.

## Reproduce

The exact command sequence is `raw/commands.txt`, with the source and binary pins above:

1. `tools/devnet.sh build` (image `rnode:local` at `a1e013490`).
2. `POS_EPOCH_LENGTH=4 POS_ACTIVE_VALIDATORS=1 DEVNET_OBSERVER_ARCHIVE=1
   DEVNET_EXTRA_FLAGS_devnet_bootstrap=--audit-attest tools/devnet.sh up --validators 1
   --observers 1`.
3. `POST /api/v1/audit/request` for `r1r5-ev-1`, then for `r1r5-ev-2` with the first claim's
   statement digest as `previous` and `sequence: 1`.
4. Verify each claim against its report with `rnode verify-audit-attestation --report <report>
   [--head-digest <d> --head-sequence <n>]`.
5. Read `/api/v1/audit/attestations` and `/api/v1/audit/conflicts`.

The raw request/response bodies, the extracted reports and claims, the verification lines, the
server's accepted set and the post-run status/checkpoint are the files in this directory.
