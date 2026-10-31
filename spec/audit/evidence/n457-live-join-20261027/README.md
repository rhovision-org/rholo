# Live-network acceptance check — source-pinned observer and four-node history

**Disposition: PARTIAL / NO-GO for closing #457, #435 or #339.**
Recorded on 2026-10-27 UTC. Exact candidate:
`dev@3607cf9f7832054376ac33a4c1c56fd0b701e89d` (merged #472).

The accompanying [acceptance.json](acceptance.json) is a sanitized, independently
reduced account of the observations. No validator secrets, private DAG data,
SSH material, or deployment credentials are included.

## Integrity / provenance

The exact candidate executable was built from the merged source SHA on
Ubuntu x86-64, using Rust 1.95.0 and `cargo build --locked --release`.
Binary evidence: [Actions run 40577865467](https://github.com/rhovision-org/rholo/actions/runs/40577865467),
artifact `rholo-live-proof-binary-40577865467`.

- Executable SHA-256:
  `b4b2c8c3b7ae12ab16fae4cfd31d9918e687f6c41f0c28befc4f05c9721b8fd2`.
- `rnode --version`: `rnode 0.1.0 (3607cf9f7832054376ac33a4c1c56fd0b701e89d)`.
- Host-side SHA-256 matched the build artifact; the binary was executed from
  an isolated operator work directory, not installed in `/usr/local/bin`.
- The four existing testnet validators continued to run
  `a5259969a7c5673ccdfc5dad97981d0a3607fcf0`.

## Live network — read-only checks

Two nodes per physical host, four public-node API responses; no direct SSH
access to the second host was used. Each node was checked independently for:

- Network ID `testnet`, shard `/testnet`, common genesis
  `c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26`.
- Block tip **218**, finalized height **214**, common finalized hash
  `8d973fd877d12663c2638121db48c08880bef579eb8fdf67bcee09d7dddd52c0`.
- Matching complete block-hash **and post-state-hash sets** at heights **190,
  200 and 210** across all four nodes (four blocks returned per height).

This proves four-node **snapshot agreement** at the captured point. It does not
establish current live-partition/rejoin acceptance, new finalized work, or an
upgrade of production binaries.

## Candidate source on a key-stripped copy of real persisted state

The copy excluded `node.key.pem` and `node.certificate.pem` and was
isolated from all four validator data directories. Its CLI pin came from the
independently checked full live genesis hash.

| Control | Outcome | Scope / limitation |
| --- | --- | --- |
| Start from populated DAG | PASS; Running, h=218, finalized hash matches | Local persisted replay; 0 peers at measurement |
| Stop and restart the same DAG | PASS; Running, h=218, finalized hash matches | Local restart; 0 peers at measurement |
| Wrong genesis pin on that DAG | PASS; exit 1 before Running with explicit mismatch | Negative control, not new network join |

These are **persistence and trust-boundary** controls. The zero-peer reading
makes it incorrect to call either positive arm a live network join. Logs were
retained on the authorized testnet host with SHA-256 hashes in the JSON witness.
The live validator tip and finalized hash did not change across these controls.

## Fresh join from an empty store: INCOMPLETE

An ephemeral, passive, non-validator process using the candidate binary received
the finalized fringe and StoreItems from the bootstrap and walked predecessor
blocks. Its log contains **221 BlockMessage arrivals**, down from block height
214 to 139. Ten 30-second requester idle warnings occurred; the **360-second**
observation limit expired before the old chain had been completely downloaded.
The process never entered Running. No `active-bonds leaf not found` appeared,
but this absence is not a PASS, because LFS never completed.

The original four-validator services were not stopped, replaced, restarted,
or modified; no production deploy or propose was initiated. Any temporary
observer firewall rules used during earlier trials were absent from UFW when
the host was rechecked.

## Remaining executable gates

1. Complete a **fresh empty-store join** on the exact new binary, with
   controlled inbound access to every validator, no helper deploy and no
   shared validator keys, then capture Running, a finalized anchor agreed
   with the original four nodes, full height recovery and no replay errors.
2. Restart that fully restored observer from its own persistent store; require
   the same finalized hash, no identity collision and no post-LFS
   `active-bonds` or BlockReceiver overflow.
3. Retain a wrong-pinned-genesis denial on that exact restored observer and
   verify that the original validators retain matching tip/finality.
4. For #435 and #339, separately establish current **live** N4/N8 and two
   epoch partition/rejoin finality/state acceptance. The previously successful
   disposable Docker runs are not substitutes for that operational gate.

**Do not close #457/#435/#339 or announce live deployment acceptance on this
record alone.**
