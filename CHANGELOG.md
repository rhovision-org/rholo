# Changelog

## v0.1.0 — 2026-10-25 (pre-release)

The first tagged release of rholo, the Rust RChain node rewritten from the Scala rnode. It is the build
for the public testnet (`testnet.rhobot.net`, validators in nyc3 and ams3) and the Rholang playground
(`playground.rhobot.net`). **This is a testnet release, not a mainnet release:** the mainnet gate
[#428](https://github.com/rhovision-org/rholo/issues/428) is still open.

Tag `v0.1.0` → `dev` at `a5259969a`. This release covers 2,185 commits, from the first Rust commit
(13 Aug 2026) to that head. Work before the move to the community org is dated, and work after it
cites PR numbers.

### Highlights

- **Any validator can propose, with no coordination.** With `--propose-on-deploy` and attestation on by
  default, whichever bonded validator receives a deploy proposes it. The others attest, and the deploy
  finalises. No round-robin script, designated proposer or `--autopropose` heartbeat is needed.
  (#80, #215, #219, #379, #395)
- **Validators can go down and come back.** When one of four equal validators stops, the others keep
  finalising at 75% of stake. A restarted validator rejoins the tip in under 20 s with no operator
  action. An eight-validator net with one validator partitioned now finalises, and it reconverges
  after the partition heals. (#213/#216, #223/#230, #392, #402, #408, #410, #411, #444, #450)
- **Dynamic validator set.** The whole lifecycle runs on chain: observer → trusted → bonded → active
  → withdrawing → paid out. The active set is drawn at random, weighted by stake, at each epoch
  boundary. Slashing is graded by the offence, and delegated stake is supported. Testnet acceptance
  rows A2.1–A2.5 cover this. (#36, #182–#186, #193, #198, #201, #206)
- **Consensus across continents.** The testnet runs four equal validators on two hosts, one in New
  York and one in Amsterdam, so every 3-of-4 quorum crosses the Atlantic.

### The Rust port itself (13–22 Aug 2026, before the move)

- The whole node is rewritten in Rust as a Cargo workspace, one crate per original sbt module. The
  Scala tree is preserved at `1b7583649`.
- A formal specification in Lean 4 and Coq comes first. The spec is the reference, not the Scala
  node. The CBC-Casper finality rule is reconstructed in TLA+.
- The registry, PoS and REV vault system contracts run natively in Rust, including the bond and
  withdraw handlers.
- The type system enforces the calculus: a compile-time Name/Proc sort, `Sorted<Par>` canonical
  forms, `Closed`/`WellScoped`/`FreeCount` refinements, and non-negative stake and balance types.
- No production code path can panic. Every store, DAG, lock and deserializer call is fallible, and
  untrusted input is validated where it enters.
- Gas is charged in proportion to size. Phlo limits are enforced and the matcher is bounded.
- Two red-team audits and a full-system HAZOP were run (R1–R12), with a test-coverage gate.
- A full OpenAPI v1 schema, LMDB storage, the REPL, and a Docker multi-node pipeline.

### Consensus and liveness

- Finality counts the live weight set, and a validator outside the live partition no longer voids a
  finality candidate. (#108, #216)
- The round gate escapes a stall on an elapsed monotonic clock. (#215, #379, #395)
- A quorum-supported liveness clock, bounded hash announcements and a missing-parent lease. (#408)
- A block waiting on a missing parent is held until the parent arrives instead of being dropped.
  (#230, #231, #238)
- Block merging (RCHIP-02) is deterministic. The fringe record is a function of its key (C215),
  concurrent native writes conflict cleanly, and the native merge relation is recorded on chain.
  (#101, #280, #299, #304, #331)
- Merge search and the ingress pipeline are bounded. (#121, #141, #147, #151)
- A deploy that can never be included is refused, and a silent block says why. (#365)
- Same-channel state survives a four-validator rejoin, and each channel is diffed once.
  (#434, #444, #448–#451)

### Resource safety (#420)

- Host resource sampling with a degraded/headroom status. The node refuses to start below the
  resource floor. (#439, #440)
- The deploy pool is bounded in bytes, and work below the admission floor is refused. (#442, #443)
- Failure and fallback states are durable, and the safety case is in the book. (#436, #446)

### Proof of stake and validator economics

- The epoch model adds a staking vault, a reward split, committed rewards and staged withdrawal
  with quarantine.
- Deploy fees are the only validator reward. Nothing is minted. The signer takes `executor-share`
  (25% by default), and the rest is split by stake each epoch.
- `fundPot` lets anyone add to the reward pot without a fork. (#373)
- Equivocation is slashable, slashes are bounded by the offence, and absence costs income.
  (#182, #184, #185, #186, #198)
- A multi-signature REV vault is installed at genesis.

### Genesis and fork readiness

- An activation schedule stages block versions by height. (#373)
- The minimum phlo price and the maximum number of parents are committed into genesis state and
  checked against every block. (#373, #376)
- A developer allocation that vests linearly over 36 months. Its address is set per network.
  (#388)
- The rooted-name master dictionary and the RhoGOV governance class contracts are installed at
  genesis. (#194)

### Rholang

- RCHIP #12: bit operations. (#279)
- RCHIP #37: string functions. (#47)
- RCHIP #39: binary deploy attachments, read with `rho:attachment:N`. (#53)
- RCHIP #51: exact integer arithmetic. Overflow promotes to BigInt. (#49)
- RCHIP #54: `rho:io:http`, a deterministic HTTP-result oracle. (#48)
- An exact decimal value type, with its Lean tie. (#389, #390)
- Block timestamps in the header, exposed through `rho:block:data`. (#44, #45)
- Parser and matcher fixes found by running 165 legacy `.rho` programs, plus a bounded AST depth.

### Interoperability

- OCapN over Noise, websocket and Unix-socket transports, passing 24/24 conformance tests. A peer
  can hold an ERTP issuer, with REV as an ERTP brand. (#248, #251, #268, #269, #271, #272)
- Multi-shard nodes, cross-shard invoke and a two-phase-commit transaction gateway (laws 26–29).
- The Rholang reducer compiles to wasm32. (#187, #188)
- A deployer index, a faucet that reports each drip's outcome, an OpenAPI document and `/health`.
  (#246, #277, #306, #355)

### Recovery and operations

- A reconciliation tool and `--restore-from-master` for a diverged chain. (#301, #312, #321)
- `--sync-anchor` restores to a named block, and its ancestry travels with it. (#303)
- Untrusted state-sync data is refused, and restore fails closed. (#383, #402, #410, #411)
- A failed reset stops the node, and an interrupted reset can be repaired. (#325, #330, #343)
- Builds name the commit they came from. CI builds a static musl `rnode` with a checksum. (#32)

### Security and assurance

- There is no `unsafe` code. 80 laws are machine-checked in Lean 4 and Coq, and CI checks the law
  register against the code.
- Red-team, type-safety, HAZOP and production-readiness wargame passes were run, and every finding
  is closed in the register.
- Critical fixes cover two ways a block could create REV, peer identity spoofing, Kademlia over
  mutual TLS, revocation that took a peer's stake, and propose being open to the network.
  (C109–C117)
- CI actions are pinned and a dependency policy is enforced. (#404, #415, #433)

### Performance

- About 203,000 lines of Rust build into a single 43.6 MB native binary with no JVM.
- A read is 58% faster after the reset was removed from the read path. (#369)
- Rebuilding the DAG index is no longer quadratic, and the DAG is not copied per block.

### Known issues

- **#457:** a brand-new node with an empty data directory cannot sync from a chain that has
  history. Adding validators still works: an observer already following the chain can be trusted
  and bonded (A2.4). Workaround: seed the new node from a synced node's data directory, leaving
  out `node.key.pem` and `node.certificate.pem` (see `docs/src/node/testnet.md`).
- **#456:** the genesis hash depends on which validator created it. A joiner must receive genesis
  from the network and cannot recompute it.
- This release needs a **new genesis** and cannot replay older chains.
- Post-quantum signatures and key exchange are planned (#274) and not shipped.

### Deferred RCHIPs

Each row on the hard-fork tracker #51 has a named owner. #43 (genesis-contract upgrade, design in
PR #437) and #40 (storage rent) still need decisions. The economics and staking rows (#4, #7/#8, #30,
#31, #32, #38, #46, #47) are owned by Patrick.

### Contributors

Patrick Mockridge, Aria Fani (AixAria), Jim Whitescarver, Shplarggle and nutzipper, with dependabot.
The node descends from the RChain Cooperative's Scala rnode.
