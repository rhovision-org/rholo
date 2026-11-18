# g520-migration-store-drill-20261118

**Evidence locality.** `local-devnet`

## Scope

This run exercises the G520 acceptance: an exact-format populated-store **copy, restart, rollback
and replay qualification** on a disposable local devnet, with a fresh-genesis control. The question
is whether a populated store can be copied and started again as the same finalized state, and
whether a fresh genesis can be substituted for it.

The devnet is a **single-validator** local network (the validator is the bootstrap) with
`--epoch-length 6`, populated to finalized height 8. After the populated arm it is stopped; the
bootstrap store is copied byte-for-byte to a second volume (`lock.mdb` excluded), the copy's file
manifest is compared with the source, and the bootstrap is restarted from the copy. The chain is
then advanced to height 9, the H0 snapshot is restored and the node restarted, and finally a fresh
genesis is started as the negative arm.

## Provenance

| | |
|---|---|
| gate | `G520-MIGRATION` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `8e4596b71a107a2c0f85efd6db06990ee86f41515b48f9aa31c5bfafaebb5ad8` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:41:33+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`raw/raw/result.json` is the drill's reading; `raw/campaign.json` carries the per-arm records.

- **Populated**: height 8, finalized block
  `c90bd56f84d0f9c11f9d0e5469b576a6c4db9fb3b1587a2a503c05b9840d4294`, post-state
  `3384fb9374fe9ed3d6ba41e9b4dd11ba4679104867b8aaf565f10694f7020648`; the served certificate
  verifies.
- **Copy**: source and copy file manifests are identical (14 files, list digest
  `278834aa4435c0a0103656b2493198ab179fdb85f45acc405fd2c1b9e1019366`; `lock.mdb` is excluded on
  both sides).
- **Restart from the copy**: the node replays and reaches Running; it serves the **same** height 8,
  finalized block and post-state, and the node's own `POST /api/v1/checkpoint/verify` returns
  `verified: true`. `raw/raw/copied-bootstrap.log` contains `replay complete — starting listeners`
  and no replay-divergence marker.
- **Advance**: a proposed block moves the chain to height 9.
- **Rollback**: restoring the H0 store returns the node to height 8 with the same block and
  post-state, not height 9; the certificate verifies.
- **Fresh genesis**: height 0, a different post-state
  `3b5501dbddbd3f9fba57bc5a901bccbbf866222c4b5eece5422792317d75225e`; it is not a substitute for
  the populated store.

The checkpoint certificate digest is recorded and verifies, but it is **not compared across arms**:
it commits to the live fringe (fringe hashes, metadata/history roots), which can move between a
running node and a restart at the same anchor. The stable identity is the finalized block and its
post-state.

## What this does not establish

- It is a **single-validator** store. A 2→4 or 3→4 validator scale-up changes the genesis bond set
  and is an activation/genesis operation, not this store-migration arm; it is not claimed here.
- The store is copied within one host and one binary. It does not establish compatibility with a
  different store format, a future binary, or a populated archive from an operator.
- Docker is the harness. The acceptance is the store bytes, the restarted node's own certificate
  verification and the fresh-genesis control; Docker is not the witness.
- No live network, public deployment, key material or transfer is involved.
- The run proves this copy/restart/rollback at height 8 on this source and binary; it is not a
  universal replay theorem for every populated store.

## Reproduce

```sh
tools/devnet.sh build
python3 tools/populated-store-migration-drill.py --gate \
  --out target/g520-migration-store-drill-20261118 \
  --source-sha 8b4c6b21ca9c6eea266a73ce9bb8127f914ff510
python3 tools/evidence-pack.py verify \
  --record spec/audit/evidence/g520-migration-store-drill-20261118 --require-pins
```

The drill starts its own `DEVNET_PREFIX=g520drill` network on ports 42402/42403/42405 and removes
it with `down -v` unless `--keep` is given.
