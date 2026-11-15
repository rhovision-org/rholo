# Storage

The node persists two kinds of state: the **block DAG** (the consensus structure) and the **tuple-space
history** (the rholang state). Both are content-addressed and both are stored in LMDB (an embedded
transactional key-value store), so a node can be restarted and continue from the same state.

## The block store

Blocks are stored keyed by their **block hash** — which, because the hash is computed over the block
*minus* its hash and signature, **is** the block's identity (Law 16). Storing and retrieving a block is
therefore content-addressed: you can only look a block up by the hash that *is* its contents. The store
also keeps the **approved block** (the genesis) and the **DAG representation** — the latest messages,
the fringe, and the height map.

## The height map

To serve "give me the block at height N" efficiently, the node maintains a **height map**: block height
→ block hash. Law 18 requires it to be **contiguous** — no holes — so a range query walks a complete
sequence, and the *lowest* and *highest* heights are meaningful bounds for the DAG.

## The checkpoint store

A node that joins from a checkpoint keeps the certificates it has seen. The store separates
**evidence** from **service**:

- **evidence** — one row per certificate digest, plus an index from the anchor it names. Recording a
  certificate never changes what the node serves, and a second statement at the same anchor is
  reported as a conflict rather than overwriting the first;
- **service** — the *head*: the certificate whose anchor the node is currently serving. Adoption is
  monotonic: the head never moves backwards, across shards, or between two different statements at
  one anchor. The store also records the history floor (`history_from`), the lowest height below
  which this node holds headers but not bodies.

The head is re-verified at boot. A populated DAG above genesis with no verified certificate has no
head to serve and is refused rather than run.

## The tuple-space history

The rholang state is stored as a **history** of the tuple space: a persistent, content-addressed radix
trie (the Merkle structure of [The tuple space (RSpace)](rspace.md)). Each block commits to a **state
hash** — the trie root — and the history lets the node move between states (checkpoints, rollbacks, and
replay) by root hash alone.

## Content addressing end to end

The storage layer's one idea is **content addressing**:

- a block's identity is its hash;
- a state's identity is its trie root;
- a deploy's identity is its signature.

Every pointer in the node — a block's parents, a state reference, a fringe member — is a hash. There
are no mutable, in-place edits: each new block and each new state is an *addition* keyed by its own
content. That is what makes the node's storage reproducible — two nodes that ingest the same blocks
reach byte-identical stores, because the keys are determined by the contents, not by insertion order.

## Atomicity

Each state change is a single LMDB transaction: either the whole change (a new block, a new trie root)
is written, or none of it is. A crash mid-write leaves the previous state intact, so the node always
restarts from a consistent checkpoint.

> **Formal.** Content addressing and the bonds cache are Law 16; the contiguous height map and
> order-independent fringe identity are Law 18. See [The laws](../formal/laws.md).
