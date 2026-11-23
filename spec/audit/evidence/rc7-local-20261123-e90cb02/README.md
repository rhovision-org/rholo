# rc7-local-20261123-e90cb02

**Evidence locality.** `local-devnet`

## Scope

The local mixed-store arm of RC-7, extended to measure HAZOP DE-18. Validator 3 runs the prior
image `rnode:pr567-old` (revision `ec104fb80`) while the other three run the candidate. The prior
binary finalises the first epoch boundary (height 10, epoch length 10), installs a persistent ρ
contract and writes a marker in one deploy at height 15, and its store is then kept while validator
3 is recreated on the candidate. The arm asks whether the candidate re-opens that store, reproduces
the pre-swap block's state triple, reads the marker, executes the contract the prior binary
installed, and reaches the same finalised head. It measures DE-18 (a mixed-store upgrade runs the
new node while a prior-format contract path stalls) in `docs/src/spec/stall-coverage.md`.

## Provenance

| | |
|---|---|
| gate | `RC-7` |
| source | `e90cb0211c36d1f37c0a5ce575079b04b4a870fe` |
| binary | `5c6946014a9869bc749d9ab436a8491319691c6effc33a21ba91db71430a44fd` |
| run | local Docker devnet; no GitHub Actions run |
| reference | the prior image `rnode:pr567-old` built from `ec104fb80`; HAZOP DE-18 in `docs/src/spec/stall-coverage.md` |
| packaged | `2026-11-23T16:46:25+00:00` |
| verdict | **PASS** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

The claim's own `check(record)` returned `pass`. The measured sequence:

- the mixed fleet finalised past the first epoch boundary at height **10**;
- validator 3 (the prior binary) submitted and finalised the install at height **15**; its marker is
  present on validator 3's own finalised head;
- at height **16** the two binaries derived the same state triple for the block
  (`preStateHash c32e3288…`, `postStateHash 80b57a50…`, no rejected deploys);
- validator 3 was recreated on the candidate, keeping its store; the common finalised head was
  height **15**, hash `653484365b807db21dd513ddffe0def0520cde2e2f38ea87dd20995659237598`, and every
  validator answered `is_finalized: true` for it;
- on the upgraded validator the pre-swap block (height 15) was **re-read**: its state triple matched
  the prior binary's derivation and its marker was present;
- the contract the prior binary installed was invoked through the candidate; the invocation settled
  `finalizedSuccess` at height **26**, and its result was readable on all four validators;
- the store format did not advance: validator 3's prior binary stamped build **4**, and the
  candidate opened the store with `decision: accept`, `present: "4"`.

`stall_episode`: `mixed-version` / `terminal` / `protocol-stall`, R3, Law 53b, deviations
`["DE-18"]`. The `exercise` block records `trigger_exercised: true` (the prior binary finalised the
boundary and wrote the contract and marker; the binaries are distinct),
`deviation_observed: false`, `safeguard_observed: true`, the install/invocation deploy ids and
heights, the pre-swap block re-read, and the store-format decision.

## What this does not establish

A finite local witness, not public evidence and not release readiness. **It does not cross a
store-format boundary**: both images write format 4, so this measures a prior *binary* over a
shared store format, not a v3→v4 migration. The migration half is pinned by
`casper/src/block_disposition.rs::v3_to_v4_preserves_fault_history_instead_of_adopting_it_as_pending`
(a v3 store opened by the v4 build preserves a terminal row instead of silently re-adopting its
body), the contract-state wire half by
`rholang/src/native_state.rs::params_wire_slots_are_append_only`, and the C402-shaped replay
consequence by
`casper/tests/determinism.rs::play_and_replay_agree_for_a_block_with_a_close_block_deploy`. The run
does not show that every prior-format contract shape resolves; it shows this contract, this store
and this candidate. DE-19 remains owed: it needs the public fleet after the upgrade window.

## Reproduce

```sh
RNODE_IMAGE=rnode:de18 tools/devnet.sh build
python3 tools/release-check.py run --claim RC-7 --env local \
  --old-image rnode:pr567-old --out target/release-claims/RC-7/local-de18
python3 tools/release-check.py record --claim RC-7 --arm local \
  --record target/release-claims/RC-7/local-de18
```

Source `e90cb0211c36d1f37c0a5ce575079b04b4a870fe`, binary
`5c6946014a9869bc749d9ab436a8491319691c6effc33a21ba91db71430a44fd`; the prior image is
`rnode:pr567-old` from `ec104fb80`.
