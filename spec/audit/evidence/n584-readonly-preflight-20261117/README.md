# RHOLO #584 — public-testnet preinstallation observation (20261117T182557Z)

Result: **NO-GO / G1 NOT SATISFIED**. Direct read-only observations of four active service instances across two SSH hosts. This is **not** a G1 four-validator identity attestation and not a G2–G6 live test.

## Pinned candidate

- Source: `70b8ea255f92f06dac59050eab014078dec67712`
- Expected executable SHA-256: `c414f979414a038f08cb0a39008f6abc591b92aaa382da64e6315d77af77d6b0`

## Method

Each alias was queried twice over its SSH host, using host-local `systemctl is-active`, `systemctl show`, `/proc/<pid>/exe` read (if permitted), `store-format` read (if permitted), and GET-only `/health`, `/api/status`, `/api/last-finalized-block`, `/api/v1/finality-observation`. Parsed response data, host command outputs, and SHA-256 hashes of exact HTTP response bodies are retained in the operator's private records. The raw HTTP body bytes were not retained for this preparatory snapshot; this is not the G5 live evidence bundle. The public version includes response SHA-256 digests, available status codes and redacted fields. `observations.json` is the audit record.

## Limits / blocker

The user-level SSH session cannot read protected store-format or process executable on any of the four instances when PermissionError is recorded. A source-version string is a self-report, **not** an installed-process hash. Four services on two hosts do not prove four distinct bonded identities. The separate read-only endpoint status check returned HTTP **404** for `/api/v1/finality-observation` on all four services, while `/api/last-finalized-block` returned HTTP **200**. HTTP 200 and acceptsDeploys do **not** prove finality or settlement. The approved post-#592 binary has not been proven installed; no GO and no execution.

## Reproduction and integrity

All observations are from the live public testnet, never a local/devnet fixture. Run the operator's SSH/GET capture procedure from a trusted orchestration host. Hashes of the private original JSON records are embedded as commitments; private host and node details are deliberately not placed in git. Verify public files with `sha256sum -c SHA256SUMS`.
