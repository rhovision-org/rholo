# n-audit-chain-20261115

> **Superseded by `n-audit-chain-20261115-r1r5`.** The review's R1/R3 fixes changed the
> `metadataRoot` projection and the checkpoint digest, so the claims in this pack are not claims a
> current build can verify. This pack is retained as the historical record of its own source
> (`d2e97c835…`, binary `d84e128d…`); do not compare its digests with the current encoding.

**Evidence locality.** `local-devnet`

## Scope

One validator (the bootstrap, also the audit service) and one archive observer on a disposable local
devnet. The run answers two questions: does `rnode audit` reproduce the checkpoint the node serves on
a chain with genesis contracts (AGREE, with the derived root equal to the claimed root), and does the
attested-audit service sign that result and chain a second claim to the first — with both claims
verifiable offline against their local reports and both entering the node's accepted store, no
conflict recorded?

## Provenance

| | |
|---|---|
| gate | `n-audit-chain` |
| source | `d2e97c83542c782bdb96e089008781d490afd833` (the image's source; the host verifier is built from `c3df3ddbd` and differs only in the offline checker's optional head arguments) |
| binary | `d84e128d665446e8cccc130f769ff016189fccd6408d9673253f0303a7c42d53` (`/usr/local/bin/rnode` in `rnode:local`) |
| run | local devnet; no CI URL |
| packaged | `2026-11-15T16:18:25+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

- **`ev-1`**: `POST /api/v1/audit/request {"nonce":"ev-1"}` answered `AGREE` at anchor height 15.
  `derivedRoot` equals the certificate's `claimedRoot`
  (`8c02223a2a9265338845a46da68707e00fdd9a5df1b20bdf61bd24bd2013237a`); `metadataCommitted` and
  `windowCommitted` are both true; a signed claim (`signed-hash-chain-v1`) is present.
  `rnode verify-audit-attestation --report ev1-report.json < ev1-attestation.json` printed
  `OK ... sequence 0 digest aacf85d8...` (the claim's statement digest).
- **`ev-2`**: the chained request (`previous = aacf85d8...`, `sequence = 1`) answered `AGREE` at
  anchor height 44 with its own signed claim. The offline check with `--head-digest aacf85d8...
  --head-sequence 0` printed `OK ... sequence 1 digest 0ce3e9b8...`.
- **The store**: `GET /api/v1/audit/attestations` returned two accepted claims (sequences 0 and 1);
  `GET /api/v1/audit/conflicts` returned none.
- **What made it work.** The audit executes the genesis body's blessed deploys to establish the
  genesis post-state (C390), and reads the last eight window headers' fringe state roots from
  `GET /api/v1/block/{hash}/header` to check `historyRoot` (C391). Before those two fixes the same
  sequence answered DISAGREE/INCOMPLETE.

## What this does not establish

- It is a **local devnet**, not an off-host or public acceptance. It says the mechanism works where
  the node can serve its own certificate, bodies and headers; it does not measure a join, a
  partition or a live network.
- **One signer.** Both claims are signed by the validator's key, which is also the service's key.
  It is a witness that the sign/verify/chain path is real, not that independent operators will
  produce independent claims.
- **The verifier shares the codebase.** The offline check uses the same canonical encoding and
  verifier implementation as the producer; a second implementation would be the stronger witness.
- **No payment.** No REV moved; the history service was unmetered (`--archive-open`) and no credit
  receipt or authorization was exercised here. Law 110/111 are evidenced by their unit tests, not by
  this run.
- **`execution_proven` is false** and the chain is short (44 heights): a finite witness, not a
  universal claim about replay cost or every chain shape.

## Reproduce

The exact command sequence is `commands.txt`, with the source and binary pins above:

1. `tools/devnet.sh build` (image `rnode:local` at `d2e97c835`).
2. `POS_EPOCH_LENGTH=4 POS_ACTIVE_VALIDATORS=1 DEVNET_OBSERVER_ARCHIVE=1
   DEVNET_EXTRA_FLAGS_devnet_bootstrap=--audit-attest tools/devnet.sh up --validators 1 --observers 1`.
3. `POST /api/v1/audit/request` for `ev-1`, then for `ev-2` with the first claim's digest as
   `previous` and `sequence: 1`.
4. Verify each claim against its report with
   `rnode verify-audit-attestation --report <report> [--head-digest <d> --head-sequence <n>]`.
5. Read `/api/v1/audit/attestations` and `/api/v1/audit/conflicts`.

The raw request/response bodies, the extracted reports and claims, the verification lines, the
status and the served certificate are the files in this directory.
