# rc5-transport-20261123-361680e

**Evidence locality.** `local-devnet`

## Scope

The RC-5 transport arm: one signed deploy, admitted once through the gRPC CLI, read back from the
node's own `PooledDeploy`, and replayed byte-for-byte through an HTTP client, a raw TCP socket and a
transparent TCP forwarder, one fresh single-node devnet per surface. Each row reads the exec status
and settlement stage on the node that accepted that surface's submission. A tampered signature is
the negative control: the HTTP surface must refuse it by name, not echo it.

## Provenance

| | |
|---|---|
| gate | `RC-5` |
| source | `361680eec25ab2fb16d790a1c4ef33b3ee1e3ecb` |
| binary | `3fec983c06866a15636636b035368c20479dabd20fb2db50c2ca9ab07d2eeeb3` |
| run | local Docker devnet; no GitHub Actions run |
| reference | none |
| packaged | `2026-11-23T18:22:11+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The claim's own `check_transport` returned `pass`. One deploy id was returned by all four surfaces,
and every surface read `ProcessedWithSuccess` / `finalizedSuccess` / no error class. The deploy was
signed with a future `validAfter` (8; the signing node was at 3), so the pool read preceded
inclusion; autopropose supplied the attestation block that finalises a lone validator. The tampered
signature was refused with HTTP 400 `Invalid signature.` The rows and the negative control are in
`raw/claim.json`.

## What this does not establish

Four fresh single-node devnets are four chains, not one: the invariant is the deploy id and the
node's status/stage, not block equality (the block hashes differ by construction). The proxy is a
transparent TCP relay, not an HTTP proxy: it does not exercise CONNECT, absolute-form request
targets, chunked re-framing or proxy-generated errors. The gRPC row's submission is `doDeploy` and
its status is read through `deployStatus`; the settlement stage is the chain fact read on the
receiving node because the gRPC service has no settlement RPC. A `local-devnet` pass is not public
evidence and not release readiness. It is one run on one candidate: a finite witness.

## Reproduce

```sh
RNODE_IMAGE=rnode:campaign-20261123 tools/devnet.sh build
python3 tools/release-check.py run --claim RC-5 --arm transport
python3 tools/release-check.py record --claim RC-5 --arm transport \
  --record target/release-claims/RC-5/transport
```
