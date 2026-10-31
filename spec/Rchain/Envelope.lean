import Rchain.Par

/-!
# Law 43 — the envelope schema is the envelope, and it is checked against both parties

A response's *envelope* is its top-level shape: which keys a client finds, and — for a tagged union —
which tags. AUDIT C16 is what happens when that shape is pinned by nothing: `DeployExecStatus`'s fields
were snake_case in a camelCase response, so a client reading `deployResult` **got nothing and nothing
errored**.

The shape has two parties in the tree, and they can disagree silently:

- the DTOs (`node/src/api/dto.rs`, `models/src/casper/protocol/deploy_service.rs`), whose `serde`
  attributes *are* the serialization;
- the served OpenAPI document (`node/src/web/http.rs`'s `OPENAPI_JSON`), a hand-written string that
  `GET /api/v1/openapi` hands to clients — nothing checked it against the DTOs, and it was **stale**
  when this law was written (AUDIT C29: `ApiStatus` declared 8 properties where the endpoint returns
  13; `LightBlockInfo` 15 where the type has 16).

This module is the catalog both parties are checked against, and `Corpus.envelopeCases` +
`node/tests/lean_envelope_corpus.rs` are the checks: each row's keys are compared to the DTO's
serialization *and* to the served schema, so a shape that drifts on either side fails a test that names
the key. The `decide`d checks on the table itself are the C16 rule — **no key contains an underscore**,
no key repeats within a row, every tag is capitalized (a client switches on the tag), and the two row
shapes (a struct's keys, a union's tags) are exclusive.

**Boundary, stated:** the law pins the envelope's *keys and tags*, not the *types* of their values
(`errored` being a boolean, `cost` an integer). A schema whose types drift is still unchecked, and
`spec/API-SCHEMA.md` remains the register for the reply *values* (law 39's catalog covers those).
-/

namespace Rchain

/-- A response envelope: the type's name (as the DTO and the served schema name it), the endpoint the
reader should think of, and either a struct's keys or a tagged union's `tag → keys`. -/
structure EnvelopeRow where
  name : String
  endpoint : String
  keys : List String
  variants : List (String × List String) := []

/-- Does any key contain an underscore? C16's defect in one predicate: a camelCase envelope with a
snake_case key renders the field a client asked for as absent. -/
def hasUnderscore (k : String) : Bool := k.toList.any (fun c => c == '_')

/-- A tag is capitalized (a client switches on `ProcessedWithSuccess`, not `processedWithSuccess`). -/
def tagCapitalized (t : String) : Bool :=
  match t.toList with
  | [] => false
  | c :: _ => c.isUpper

/-- The row's keys are non-empty, snake-free, and distinct. -/
def keysWellFormed (ks : List String) : Bool :=
  !ks.isEmpty && ks.all (fun k => !hasUnderscore k && !k.isEmpty)
    && ks.eraseDups.length == ks.length

/-- The row's shape: a struct's keys, or a tagged union's variants — not both, not neither. -/
def shapeWellFormed (r : EnvelopeRow) : Bool :=
  if r.variants.isEmpty then keysWellFormed r.keys
  else r.keys.isEmpty && r.variants.all (fun v => tagCapitalized v.1 && keysWellFormed v.2)

/-- The catalog. Each row is checked twice by the corpus — against the DTO's own serialization and
against the served OpenAPI schema — so a row is not an opinion about a shape but the shape both parties
are held to. `endpoint` is for the reader; the check is on the type, which both parties name. -/
def envelopeCatalog : List EnvelopeRow :=
  [ { name := "ApiStatus", endpoint := "GET /api/v1/status",
      keys := ["version", "address", "networkId", "shardId", "peers", "nodes", "minPhloPrice",
        "latestBlockNumber", "autopropose", "proposeOnDeploy", "manualPropose", "devMode",
        "proposer", "degraded", "acceptsDeploys", "pool"] }
  -- **The operator half, on its own endpoint** (W-N3-01). The row above used to carry every key
  -- below, and served them on a public listener; the split *is* the finding, so the catalog splits
  -- with it. A superset of the row above by construction: `OperatorStatus` flattens `PublicStatus`
  -- and adds these, which is why every public key appears again here.
  , { name := "OperatorStatus", endpoint := "GET /api/v1/status/operator",
      keys := ["version", "address", "networkId", "shardId", "peers", "nodes", "minPhloPrice",
        "latestBlockNumber", "autopropose", "proposeOnDeploy", "manualPropose", "devMode",
        "proposer", "degraded", "acceptsDeploys", "pool",
        "adminHttp",
        "consecutiveSelfValidationFailures", "autoproposeTimerHalted",
        "autoproposeTimerHaltEpisodes", "autoproposeHalt", "autoproposeTapSuppressed",
        "staleSnapshotSelfEquivocations", "finalityStall", "finalityStalled", "finalityStallEpisodes",
        "finalityCountersDurable",
        "resetMarker",
        "nonQuietMergeReports", "sync", "poisonRecoveries", "poisonRecoveriesScope",
        "unrestorableRecords", "unrestorableBlock",
        "tasksPanicked", "tasksPanickedScope", "tasksExited", "tasksExitedScope",
        "undeliveredSends", "undeliveredSendsScope",
        "shards", "resources"] }
  , { name := "NodeCapabilities", endpoint := "GET /api/v1/capabilities",
      keys := ["autopropose", "proposeOnDeploy", "manualPropose", "adminHttp", "devMode",
        "proposer", "nodeList", "faucet", "faucetRemaining"] }
  -- **The deploy hostlist** (Law 90): the response a client spreads deploys over. Declared here so
  -- the shape a wallet parses is the shape this node serves, not a coincidence of two writers.
  , { name := "NodeListResponse", endpoint := "GET /api/v1/nodes",
      keys := ["networkId", "primaryShard", "generatedAt", "ownLatestBlockNumber", "nodes"] }
  , { name := "NodeListEntry", endpoint := "the `nodes` entries of GET /api/v1/nodes",
      keys := ["nodeId", "httpUrl", "networkId", "shardId", "nodeVersion", "latestBlockNumber",
        "proposer", "acceptsDeploys", "self"] }
  , { name := "LightBlockInfo", endpoint := "the `block` field of most responses",
      keys := ["version", "shardId", "blockHash", "blockNumber", "sender", "seqNum", "preStateHash",
        "postStateHash", "justifications", "bonds", "sigAlgorithm", "sig", "blockSize", "deployCount",
        "rejectedDeploys", "timestamp"] }
  , { name := "DeployInfo", endpoint := "the `deploys` entries of a block response",
      keys := ["deployer", "term", "timestamp", "sig", "sigAlgorithm", "phloPrice", "phloLimit",
        "validAfterBlockNumber", "cost", "errored", "systemDeployError"] }
  , { name := "RhoDataResponse", endpoint := "POST /api/v1/data-at-name",
      keys := ["expr", "block"] }
  -- The exploratory response names the *channel* it read the reply from, so an empty `expr` is a
  -- fact rather than a guess (AUDIT C38).
  , { name := "ExploratoryDeployResponse", endpoint := "POST /api/v1/explore-deploy",
      keys := ["expr", "block", "replySource"] }
  -- The two rows AUDIT C29 found missing: the document `$ref`s both, neither had a row, so neither
  -- was checked — and a response a client can call with no declaration is the shape this law exists
  -- to refuse.
  , { name := "PooledDeploys", endpoint := "GET /api/v1/deploys",
      keys := ["deploys"] }
  , { name := "FaucetResponse", endpoint := "POST /api/v1/faucet",
      keys := ["deployId", "amount", "to", "status", "deployError"] }
  , { name := "DeployExecStatus", endpoint := "the `deployResult` body of a deploy response",
      keys := [],
      variants :=
        [ ("ProcessedWithSuccess", ["deployResult", "block"])
        , ("ProcessedWithError", ["deployError", "block"])
        , ("NotProcessed", ["status"]) ] }
  -- The two rows AUDIT C261's review added: the recovery tool needs artefacts it can *verify*, and
  -- neither existed. `LightBlockInfo` is a summary a client cannot recompute a block's hash from, so
  -- every field in it is the node's word; `RawBlock` is the preimage the hash *is* taken over, so the
  -- client can hash it and read the fields off what it verified. `FinalityAttestation` replaces a
  -- node-local claim with a statement a validator signs, which is what lets a report carry weight.
  , { name := "RawBlock", endpoint := "GET /api/v1/block/{hash}/raw",
      keys := ["blockHash", "encoded", "sender", "sigAlgorithm", "sig"] }
  , { name := "FinalityAttestation", endpoint := "GET /api/v1/finality-attestation",
      keys := ["blockHash", "nonce", "sender", "sigAlgorithm", "sig"] }
  ]

/-- The catalog is well-formed, `decide`d: names unique, endpoints named, and every row's keys or
tags conforming (non-empty, no underscore — C16's rule — distinct, and tags capitalized). -/
theorem envelopeCatalog_decide :
    (envelopeCatalog.all (fun r =>
        !r.name.isEmpty && !r.endpoint.isEmpty && shapeWellFormed r)
      && (envelopeCatalog.map EnvelopeRow.name).eraseDups.length
        == envelopeCatalog.length) = true := by
  decide

/-- The count the Rust consumer asserts it read. -/
def envelopeCaseCount : Nat := 14

/-- The catalog carries exactly `envelopeCaseCount` rows. -/
theorem envelopeCatalog_length : envelopeCatalog.length = envelopeCaseCount := by decide

end Rchain
