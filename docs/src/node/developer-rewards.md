# Developer rewards: activating a fee-funded service after genesis

The protocol reserves a share of every deploy's burn (`developer-share`, in basis points) and pays it
to a genesis-created `MultiSigRevVault`. That much is a genesis value: no deploy can write a fee
parameter, so a share the genesis does not commit cannot be enabled later without a hard fork.

**What is deliberately *not* a genesis value is the service that spends the reserve.** Everything in
this page is an ordinary deploy — no new block type, no new deploy type, no new binary, no new
genesis. A network can run for months with the reserve accruing and the service inactive; switching it
on is a deploy.

## The moving parts

`examples/devrewards/` holds three contract fragments and one install template:

| file | role |
|---|---|
| `BeneficiaryRegistry.rho` | append-only `key → [(height, beneficiary)]`; a caller can only name a beneficiary for its **own** key |
| `Treasury.rho` | holds the reserve in a fresh unforgeable vault; refuses to move it until a validator-key quorum activates it |
| `Settlement.rho` | accepts one attested manifest per epoch and pays claims against its Merkle root |
| `install.rho` | the **one** deploy that wires them; rendered with the attestor keys and threshold |

Render and deploy the install:

```bash
tools/devrewards_install.py --attestor <hex65> [--attestor <hex65> ...] --threshold N > install.rho
rnode deploy --private-key <key> install.rho
```

The install publishes, on fixed string channels:

- `@"devrewards/registry"` — `bundle+{*registry}`
- `@"devrewards/treasury"` — `bundle+{*treasury}`
- `@"devrewards/settlement"` — `bundle+{*settlement}`
- `@"devrewards/treasury-address"` — the treasury's REV address

The treasury and the settlement are wired by a **private** `pay` name. The public treasury bundle
exposes only `treasuryAddress`, `status` and `activate`; there is no public way to move funds. The
install is one deploy precisely so that name never appears in a deploy term.

## Deploy order

1. **Install** (above). The treasury's vault starts empty.
2. **Fund the treasury**: the reserve's quorum transfers the reserve's REV to the published treasury
   address, through the existing `MultiSigRevVault` (`transfer`/`confirm`). The reserve contract is
   unchanged; the treasury is just a destination.
3. **Beneficiaries** (any time): a developer calls
   `registry("register", *deployerId, "<address>", *ret)`. The registry derives the key from the
   supplied `deployerId` via `rho:rchain:deployerId:ops "pubKeyBytes"`; the deployer id is the one the
   deploy system process produced, so the registration is self-scoped. A registration at height `h`
   applies at `h` and never before.
4. **Manifest**: build it off-chain from finalized blocks (below); the attestors sign the digest.
5. **Accept**: `settlement("accept", epoch, anchorHeight, anchorHash, root, total, attestations, *ret)`.
   A threshold of **distinct** install-time validator keys must have signed
   `blake2b256("devrewards-manifest-v1" || epoch_u64_le || anchorHash || root)`. A second accept for
   an epoch with a different digest is refused **and recorded** as a conflict.
6. **Activate** (when the network decides to pay): `treasury("activate", attestations, *ret)` with a
   threshold of distinct validator keys over
   `blake2b256("devrewards-activate-v1" || treasuryAddress)`. Idempotent.
7. **Claim**: `settlement("claim", epoch, deployerKey, amount, proof, *ret)`. The settlement
   recomputes the leaf, folds the proof against the stored root, resolves the beneficiary **at the
   manifest's anchor height**, and calls the private `pay`. The claim is marked paid only after the
   transfer succeeds.

**Inactive means held.** Before step 6 the treasury refuses `pay`; the REV stays in its vault. It is
never returned to validators or forwarded to the fee pot, and an unclaimed entitlement is never swept:
it carries forward to a later manifest.

## The manifest (`devrewards-usage-v1`)

```bash
tools/devrewards_manifest.py build  --input export.json --out manifest.json
tools/devrewards_manifest.py verify --manifest manifest.json
tools/devrewards_manifest.py self-test
```

Input: a finalized anchor and the deploys before it. Attribution links each invocation's
`CommEvent.consume.hash` to the **first** deploy that logged that consume hash (the installer). An
invocation's burned amount is split equally among the distinct installers it invoked; the integer
remainder goes to the lowest keys in sorted order. A deploy that invoked nothing prior pays for
itself. The encoding is inside `tools/devrewards_manifest.py` and is pinned by
`tests/fixtures/devrewards_manifest_golden.json`, which both the Python suite and
`casper/tests/devrewards_manifest_encoding.rs` read:

- leaf = `blake2b256(0x00 || deployer_key(65) || amount_i64_le(8))`
- node = `blake2b256(0x01 || left || right)`, an odd node promoted
- digest = `blake2b256("devrewards-manifest-v1" || epoch_u64_le || anchor_hash(32) || root(32))`

Ten thousand usages of one contract aggregate into one leaf and one claim.

## What this does not claim

- No mainnet-scale, timing or fee-market claim. The service is ordinary deploys; its cost is the
  deploy cost.
- The attestor set and threshold are fixed at install. There is no rotation. Losing the threshold
  keys strands the activation (the treasury stays inactive and the reserve stays held); losing the
  settlement's keys does not move funds, because acceptance needs the same quorum.
- An inactive reserve is **held**, not returned. That is a policy, not an economic-soundness claim.
- A manifest is only as good as the attribution export it was built from; the tool verifies its own
  encoding and the finalized anchor, not the exporter's honesty. The attestors sign what they read.
- Claiming is permissionless: anyone may submit a valid proof for a leaf. The beneficiary is resolved
  from the registry, not from the caller.
