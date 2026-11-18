# g520-migration-store-drill-20261118-fail

**Evidence locality.** `local-devnet`

## Scope

This is the negative control for the G520 populated-store acceptance. It applies the declared
mutation to `tools/populated-store-migration-drill.py` and runs the drill's hermetic `self-test`:

```text
-    if _fresh_is_substitute(fresh, pre):
+    if False and _fresh_is_substitute(fresh, pre):
```

With the fresh-genesis substitution check disabled, a fresh arm that is a substitute for the
populated store is no longer refused, and `self-test` fails. The mutation is recorded in
`raw/mutation.patch`; `raw/self-test.stderr` is the failure.

## Provenance

| | |
|---|---|
| gate | `G520-MIGRATION` |
| source | `8b4c6b21ca9c6eea266a73ce9bb8127f914ff510` |
| binary | `8e4596b71a107a2c0f85efd6db06990ee86f41515b48f9aa31c5bfafaebb5ad8` |
| run | local operator run (not a GitHub Actions run) |
| reference | none — local run |
| packaged | `2026-11-18T16:42:22+00:00` |
| verdict | **FAIL** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

`self-test` exited 1 with:

```text
SELF-TEST FAIL: the fresh-genesis substitution was not refused: PASS []
```

That is the control working: the unmutated drill refuses the substitution and exits 0
(`g520-migration-store-drill-20261118`), while the mutated drill accepts it and the self-test goes
red. The mutation is a predicate mutation, not a test edit; the test bytes are the witness.

## What this does not establish

- It is a negative control for the predicate, not a second populated-store campaign.
- It does not establish that every possible store-migration defect is caught; it exercises the
  fresh-genesis substitution arm and the drill's other hermetic controls.
- No live network, public deployment, key material or transfer is involved.
- `execution_proven` is false; the record proves the bytes, not that the run happened.

## Reproduce

```sh
# apply the mutation in raw/mutation.patch, then:
python3 tools/populated-store-migration-drill.py self-test   # exits 1
# restore the file and rerun:
python3 tools/populated-store-migration-drill.py self-test   # exits 0
```
