# RHOLO #584 — public-testnet preinstallation observation (20261117T182557Z)

Result: **NO-GO / G1 NOT SATISFIED**. Direct read-only observations of four active service instances across two SSH hosts. This is **not** a G1 four-validator identity attestation and not a G2–G6 live test.

## Pinned candidate

- Historical candidate: **PR #598 merge commit** `70b8ea255f92f06dac59050eab014078dec67712` (merged into `dev` 2026-11-16T17:21:09Z). This is not the installed public-testnet version.
- Historical #598 candidate expected executable SHA-256 (not reusable for a new release): `c414f979414a038f08cb0a39008f6abc591b92aaa382da64e6315d77af77d6b0`

## Baseline, deployed state and next gate

- At the 2026-11-17T18:26Z capture, `dev` was `6d490f9a945504861662dc6f1afd303cabaf2843`, following #599, #600 and two CI commits; #598 was four commits behind this baseline.
- PR #602 subsequently merged into `dev` as `d06b48ab04af94dc8ab8225ac08598af3c6e086b`, introducing current-build C343/C357 gates and the failure-tell matrix. This is a committed source baseline, **not** a deployed release.
- All four observed public service instances self-reported the older 2026-10-29 #491 live pin `0a1e02362a7c34e474e068ffd47469c8d4be707a`. Protected executable/store reads failed with `PermissionError`; no independent executable or deployed-commit attestation exists.
- Required sequence: **#602 merged → select explicit post-#602 release commit → freeze build artifact and regenerate executable SHA-256 → privileged host-local installed-pin/store/identity evidence (#572) → independent preflight review and agreed UTC window → #584 G1–G6 live acceptance**. Do not reuse the historical #598 executable hash. **NO-GO remains; #584 stays open.**

## Method

Each alias was queried twice over its SSH host, using host-local `systemctl is-active`, `systemctl show`, `/proc/<pid>/exe` read (if permitted), `store-format` read (if permitted), and GET-only `/health`, `/api/status`, `/api/last-finalized-block`, `/api/v1/finality-observation`. Parsed response data, host command outputs, and SHA-256 hashes of exact HTTP response bodies are retained in the operator's private records. The raw HTTP body bytes were not retained for this preparatory snapshot; this is not the G5 live evidence bundle. The public version includes response SHA-256 digests, available status codes and redacted fields. `observations.json` is the audit record.

## Limits / blocker

The user-level SSH session cannot read protected store-format or process executable on any of the four instances when PermissionError is recorded. A source-version string is a self-report, **not** an installed-process hash. Four services on two hosts do not prove four distinct bonded identities. The separate read-only endpoint status check returned HTTP **404** for `/api/v1/finality-observation` on all four services, while `/api/last-finalized-block` returned HTTP **200**. HTTP 200 and acceptsDeploys do **not** prove finality or settlement. The historical #598 candidate binary has not been proven installed; no GO and no execution.

## Reproduction and integrity

All observations are from the live public testnet, never a local/devnet fixture. Run the operator's SSH/GET capture procedure from a trusted orchestration host. Hashes of the private original JSON records are embedded as commitments; private host and node details are deliberately not placed in git. Verify public files with `sha256sum -c SHA256SUMS`.
