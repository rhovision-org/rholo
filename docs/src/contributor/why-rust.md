# Why Rust

The RChain node executes Rholang, a concurrent message-passing language formally modeled by the
ρ-calculus — a *reflective, higher-order extension of the π-calculus*. The original node was written
in Scala on the JVM, with a C++ actor VM ([Rosette](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/rosette/)) underneath. This
repository rewrites it in Rust.

Two reasons drive the rewrite.

## 1. Memory safety, and no collector in the runtime

The Scala/JVM node leaked memory and paused for garbage collection. These were not theoretical
concerns: the node shipped a `diagnostics` service that reported JVM `Memory`, `MemoryPool`, and
`GarbageCollector` metrics to its operators (see
[`legacy/docs/rnode-api/index.md`](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/docs/rnode-api/index.md)), and the build needed an
enlarged heap and thread stack just to run (see [`legacy/DEVELOPER.md`](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/DEVELOPER.md)):

```sh
export SBT_OPTS="-Xmx4g -Xss2m -Dsbt.supershell=false"
```

Rust addresses this, but the parts are worth keeping separate, because only one of them is the type
system.

**Memory safety is structural.** With `unsafe` forbidden across the crate graph, ownership and the
borrow checker make *undefined behaviour* — use-after-free, double free, out-of-bounds access, data
races — unwritable: resource lifetime becomes a compile-time, statically checked property rather
than a runtime, best-effort one.

**There is no collector to pause it — and that is a runtime property, not a type-system one.** The
binary ships no tracing garbage collector, so the stop-the-world pause the JVM imposed does not exist
here. But a tracing collector, including one that stops the world, is ordinary *safe* Rust; nothing
in the language forbids writing one. We simply do not ship one, and *that* is the claim.

**A leak is not a safety bug, and Rust does not make one unrepresentable.** Leaking memory is
defined as safe: `mem::forget`, a reference cycle through `Rc`/`Arc`, an unbounded cache, or simply
retaining live data are all things safe code does. The JVM node's leak was a resource-lifetime
defect, not a memory-safety one — Rust makes lifetime *visible in the types* rather than impossible to
get wrong. Footprint stays an engineering concern, and this node measures its own and records the
rate (see [Running a validator: hardware requirements](../node/validator-requirements.md)).

### The practical upshot — a validator on modest hardware

The payoff is operational, not just theoretical. Counted <!-- recount:date -->2026-10-30<!-- recount:end -->,
<!-- recount:rs-lines -->231,044<!-- recount:end --> lines of Rust across
<!-- recount:rs-files -->513<!-- recount:end --> source files compile to a single
**<!-- recount:binary-mib -->47.0<!-- recount:end --> MiB native binary**
(<!-- recount:binary-bytes -->49,317,360<!-- recount:end --> bytes) — no JVM to boot, no collector in
the process, no `-Xmx4g -Xss2m` to size. The collector-induced pauses and the heap sizing that made the
JVM node's runtime heavy and its latency unpredictable are gone, so a validator runs comfortably on any
reasonably modern desktop PC or high-performance laptop with an NVMe SSD. See
[Running a validator: hardware requirements](../node/validator-requirements.md).

*(Both figures are held in `tools/artifact-counts.json` and emitted by `tools/recount-artifact.sh`:
the line and file totals are `git ls-files '*.rs' | xargs wc -l` over the workspace —
<!-- recount:rs-lines -->231,044<!-- recount:end --> lines over
<!-- recount:rs-files -->513<!-- recount:end --> files — and the binary is the default release build
(unstripped, with no `[profile.release]` tuning),
<!-- recount:binary-bytes -->49,317,360<!-- recount:end --> bytes,
<!-- recount:binary-mib -->47.0<!-- recount:end --> MiB, sha256
`<!-- recount:binary-sha256 -->446dbaf13e24930fe4067ec97390959e328c62dc7a6e0e631dc83de846ad34bf<!-- recount:end -->`. `--check` refuses a page that disagrees with the
record, and the nightly `--verify-live` re-measures the tree. The record is refreshed when the
documentation changes, not on every code commit, which is why these figures are not in the law-count
gate.)*

The consequence is structural: **validator operation genuinely decentralizes.** The requirements sit
within consumer-grade hardware, not a datacenter, so the barrier to running a validating node is a
commodity machine. The same native code buys throughput too — no collector pauses and no JVM startup
leave the CPU free for reduction itself, even while full ρ-calculus thread-level concurrency remains
work in progress (see [the concurrency model](../formal/concurrency.md)).

That decentralization is not an abstract ideal; it is the lesson of the original network's failure.
Running a validator meant an always-on, co-op-operated AWS instance. Operators who self-hosted —
including co-op members from their own homes — routinely hit technical difficulties and risked having
their stake slashed for downtime, so staking on anything but a co-op node carried too much risk. The
co-op ended up running the validators itself, and when the treasury ran dry as the token price fell,
it could no longer afford the instances that kept the network alive. Consumer-grade hardware removes
that centralizing pressure: the barrier to *being* a validator drops to a commodity machine that any
operator can keep online.

The **main strategic aim** of the port follows directly: to decentralize mainnet infrastructure to
the point where *anyone* can run a validator, so that whatever succeeds the co-op is no longer
responsible for keeping the network itself online. Its role becomes what does not scale down to an
individual operator — node software upgrades, research, technical expertise, standards and best
practices, and the mechanics of the REV token transition. A network that runs on commodity hardware is
self-healing and has no single point of failure, so the token earns a genuine price floor from the
fees paid on a network that keeps running — the same structural property as Ethereum or Bitcoin.
Whatever the market decides that price is — a tenth of a cent or a dollar — it is a price that exists
and persists.

## 2. Rust natively expresses the calculus hierarchy

The second reason is deeper, and it is about what the node *is*, not just how it runs.

The ρ-calculus sits at the top of a hierarchy of process calculi:

- The **λ-calculus** is the calculus of substitution — of functions and application.
- The **π-calculus** adds concurrency and mobility: processes communicate over *channels*, and a
  channel is itself a value that can be passed over another channel.
- The **ρ-calculus** is the **reflective** π-calculus: a *name* is a *quoted process* (`@P`), and a
  process can *evaluate* a name back into a process (`*x`). Reflection — quoting and dereferencing
  code — is built into the calculus rather than bolted on.

Rust expresses each rung natively:

- **λ** — closures (`fn`, `Fn`/`FnMut`/`FnOnce`) are exactly λ-abstraction and application.
  Higher-order functions are pervasive, e.g. `SyncVar::update(f: impl FnOnce(A) -> A)` in
  [`shared/src/sync_var.rs`](../../../shared/src/sync_var.rs).
- **π** — Rust's concurrency primitives — `std::sync::mpsc`/`tokio::sync::mpsc` channels,
  `Arc` + `Mutex`/`Condvar`, and the `Send`/`Sync` marker traits — are channels and name passing. A
  cell such as `SyncVar`/`MaybeCell` is a degenerate channel.
- **ρ** — reflection. In the port the `Par` AST is a first-class, sortable, hashable value (the
  `Par`/`GUnforgeable` types in `models`), so a *name* **is** a quoted process — expressed as data,
  exactly as in the calculus.
- **Calculus of Constructions** — the dependent-type systems of Lean 4 and Coq. The port's type
  discipline embeds ρ as the base sort of a Calculus of Constructions and proves its fundamentals.

The correspondence table below maps each Rust construct to the calculus concept it expresses and to
the file in [`spec/`](../../../spec/) where that concept is formalized.

## Correspondence

| Rust construct | Calculus concept | Formal home |
|---|---|---|
| `fn` / `impl Fn` / closures | λ-abstraction and application | — |
| `std::sync::mpsc` / `tokio::sync::mpsc` channel | π-calculus channel (name) | — |
| `Arc` + `Mutex`/`Condvar`, `Send`/`Sync` | π name mobility (passing a channel) | — |
| `Par` / `GUnforgeable` value (sorted, hashed) | ρ quoted process / name | [`spec/Rchain/Par.lean`](../../../spec/Rchain/Par.lean), [`Sort.lean`](../../../spec/Rchain/Sort.lean) |
| `classify : Par → PSort`, `HasSort` | ρ base sort (process vs name) | [`spec/Rchain/Ty.lean`](../../../spec/Rchain/Ty.lean) |
| `Closed`, `Subst`, `Reduce` | α-equivalence, substitution, COMM (Laws 2–6) | [`spec/Rchain/Rho.lean`](../../../spec/Rchain/Rho.lean), [`Ty.lean`](../../../spec/Rchain/Ty.lean) |
| `TotalOn f := ∀ p, Closed p → Closed (f p)` | "no `.unwrap()`" totality | [`spec/TYPE-SYSTEM.md`](../../../spec/TYPE-SYSTEM.md) (F6) |
| Lean CIC / Coq CIC | Calculus of Constructions | [`spec/lakefile.toml`](../../../spec/lakefile.toml), [`spec/coq/_CoqProject`](../../../spec/coq/_CoqProject) |

## From calculus to proof

Because the hierarchy bottoms out in the Calculus of Constructions, the port's invariants can be
*constructed and proven* rather than merely asserted. This is what [`spec/`](../../../spec/) does:

- [`spec/TYPE-SYSTEM.md`](../../../spec/TYPE-SYSTEM.md) embeds the ρ-calculus as the base sort of a
  Calculus of Constructions and proves six fundamentals (F1–F6) in Lean 4 — sort classification is
  functional and decidable, structural congruence is an equivalence, substitution preserves sort,
  reduction preserves sort and closedness, canonicalization commutes with typing, and totality is
  compositional.
- [`spec/INVENTORY.md`](../../../spec/INVENTORY.md) is the **invariant catalog** — one law per
  Rholang / RSpace / Rosette / Casper / Storage / Crypto invariant, each with a Scala source-of-truth
  pointer and a Lean/Coq formalization target.
- [`spec/Rchain/`](../../../spec/Rchain/) (Lean 4) and [`spec/coq/`](../../../spec/coq/) (Coq) hold the
  machine-checked definitions and theorems.

The claim "every fundamental property of expressing the node is contained within Rust *as-is*" is the
intuition; `spec/` is its machine-checked realization.

## Lineage

- Meredith & Radestock, *A Reflective Higher-Order Calculus* (2005) — the ρ-calculus.
- Meredith, *Higher Category Models of the π-Calculus* — the categorical semantics.
- The in-repo Rholang reference ([`legacy/rholang/reference_doc/`](https://github.com/rhovision-org/rholo/blob/1b7583649/legacy/rholang/reference_doc/))
  documents the tuplespace model, quoting of processes into names, normalization (de Bruijn
  α-equivalence and the canonical `|` sort), and the ρ/λ/π relationship.

## A corollary: implement the calculus, don't reproduce the JVM

The port is complete; the node is now a *faithful implementation of the ρ-calculus*. The motivation is
memory safety and calculus-native expression, **not** a correctness repair of consensus behavior. The
binding constraint is stated in [`AGENTS.md`](../../../AGENTS.md): the laws in
[`spec/INVENTORY.md`](../../../spec/INVENTORY.md) and the ρ→CoC type discipline in
[`spec/TYPE-SYSTEM.md`](../../../spec/TYPE-SYSTEM.md) are the oracle. Rust carries those invariants
structurally (refinement types, no silent partiality) rather than reproducing the JVM's patterns —
including its latent bugs.
