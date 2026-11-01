# Evidence bundles — the manifest contract

Every claimed run lives in a directory under this one. The conventions are in
[`docs/src/methodology/agentic-assurance.md`](../../../docs/src/methodology/agentic-assurance.md)
§3.4; this page adds the **machine-readable manifest** that `tools/evidence-pack.py` writes and
verifies.

## The manifest

`manifest.json` (schema 1):

| field | meaning |
|---|---|
| `gate_id` | the gate the run speaks to (`n8-partition-rejoin`, …) |
| `verdict` | `PASS` or `FAIL` — the operator's reading, not the packer's |
| `arms` | the per-cycle/per-round results the run itself recorded |
| `source_sha` | the source the binary was built from |
| `binary_sha256` | the exact binary |
| `run.url`, `run.attempt` | the **upstream** actions run (`rhovision-org/rholo`), with the attempt |
| `locality` | `local-devnet`, `loopback-diagnostic` or `off-host-claim` |
| `files[]` | `{path, sha256, bytes, kind}` for every copied file |
| `redactions[]` | globs omitted from the bundle, recorded rather than silently dropped |
| `execution_proven` | **always `false`** |
| `packer` | tool, version and UTC packaging time |

`SHA256SUMS.json` is the flat `path → sha256` map of the same files.

## What the digests do and do not prove

A verified bundle proves the bytes are the bytes and that nothing was edited after packaging. It
does **not** prove the run happened, that the verdict is sound, or that the binary was the deployed
one. That is why `execution_proven` is `false` and why the README's **Scope**, **Result**, **What
this does not establish** and **Reproduce** sections are authored, not generated. `verify` refuses a
bundle whose README still contains a `<fill: …>` marker.

## Using it

```sh
# after a run has written target/<run>/:
python3 tools/evidence-pack.py pack --run-dir target/<run> \
    --record spec/audit/evidence/<slug> --gate-id <gate> --verdict PASS \
    --locality local-devnet
# author the README's four sections, then:
python3 tools/evidence-pack.py verify --record spec/audit/evidence/<slug>
```

`pack` refuses: a missing source or binary pin; a `PASS` beside a `FAIL` arm; a destination under
`target/`; a symlink; a fork run URL (the #515 404); and a non-empty destination. It copies by
default and never moves. The local-only `loopback-diagnostic` label must say in words that it is not
an off-host positive (`tools/check-evidence-locality.py`).
