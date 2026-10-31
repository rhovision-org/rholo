# Startup metadata reconstruction proof (#480)

The fixture contains 7,866 public `block-metadata` records from the existing testnet.
The [manifest](fixture-manifest.json) pins the exported dump and describes the read-only
snapshot method. It contains no validator keys, wallet, tuple-space state or deploy payloads.
The live validator processes, data and firewall configuration were not changed.

The [paired runner](../../../../tools/startup-metadata-proof.py) builds the actual production
`BlockMetadataStore::create` at baseline `8cbee80374b58eb730425381c3fc4554608b93f6`
and at the checked-out candidate. The only baseline overlay is the identical
[measurement harness](../../../../casper/tests/startup_metadata_bench.rs).
Both use locked dependencies, release optimization and the node's Linux jemalloc allocator.
No production source or lockfile is rewritten for either arm.

Each arm has one uncounted warmup, then seven alternating AB/BA pairs in fresh OS processes,
pinned to the same allowed CPU. The filesystem cache is warm. The measurement reads Linux
`VmHWM` immediately after the production constructor returns, before parity checks allocate
their own buffers. Elapsed time covers that constructor only.

Every trial checks all stored metadata and parent edges, the block set and height membership.
The runner requires complete canonical index and encoded metadata fingerprints to agree across
every trial, including warmups. It refuses fixture checksum mismatches. The predeclared
performance gates require a lower median peak RSS and no time regression larger than 20%
or 5 ms (whichever is larger); raw results are retained even on gate failure.

Run on Linux with Rust 1.95.0, the repository's native build dependencies and `lmdb-utils`:

```sh
python3 tools/startup-metadata-proof.py --out /tmp/startup-metadata-proof --pairs 7
```

The [workflow](../../../../.github/workflows/startup-metadata-proof.yml) checks out the exact PR
head and retains raw trial JSON, logs, build messages and both binary hashes. The final
source-pinned result and run link are recorded in the PR after execution.

This proves constructor projection parity and measures its transient memory cost. Full-node
replay, live fresh join/LFS, ancestry-set residency, steady-state RSS and validator hardware
capacity are outside this measurement. No live deployment is included.
