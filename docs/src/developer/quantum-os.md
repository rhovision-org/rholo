# QuantumOS on Rholo — operator integration (#601)

**Scope:** install and operate the QuantumOS browser, signaling service and facilitator against a Rholo HTTP API. This is an **application deployment**: no browser is embedded in `rnode`, and no validator or consensus service is managed by the QuantumOS harness unless the operator explicitly chooses its **bundled disposable local node** mode.

**Owners:** [QuantumOS/browser/agent — @aixaria0](https://github.com/rhovision-org/quantum-os); [Rholo node/API/devnet — @PatrickMockridge](https://github.com/rhovision-org/rholo/issues/601). The required RhoGov handoff remains [quantum-os#291](https://github.com/rhovision-org/quantum-os/issues/291). The operational acceptance gate for the public four-validator network is [rholo#584](https://github.com/rhovision-org/rholo/issues/584); **#601 does not authorize its deployment, transfer, restart or store migration**.

## 1. Prerequisites and trust boundaries

- Linux x86-64 host; **Node.js 22+**, `pnpm`, **Python 3**, `git`, `openssl`, and Rust/`wasm-pack` for a clean browser WASM build. `bash` is required for the bundled node runner. Only the **bundled** path needs a runnable `quantum-os/bin/rnode`.
- Loopback ports: **40403** Rholo HTTP, **4444** Signaling, **5173** Vite HTTPS. The harness binds Signaling and Vite to `127.0.0.1` and refuses occupied service ports. If exposing browser access to a different device, use a trusted TLS reverse proxy / SSH forwarding with appropriate WebSocket upgrades, never publish the admin HTTP API.
- **40405 is administrative.** Keep it private/loopback. Do not proxy it through the browser or open it on a public firewall.
- The QuantumOS local TLS endpoint uses a self-signed development certificate. Accept it only on the explicitly trusted local device. Public deployment requires a trusted certificate, correct HTTPS/WSS origin and TURN for restrictive NATs.
- A `cap:room:…` URL is a **bearer secret**: the harness generates a fresh capability and prints it once on the operator terminal. It does not commit it, persist it in its JSON state, or pass it in the agent's argv. Avoid recording terminal stdout, terminal history, browser screenshots, process dumps or CI output containing the room URL.
- `--no-ai` is default. The optional `--ai` explicitly enables Claude CLI processing of room content; do not enable it without participants' consent and provider credentials. The deterministic facilitator FAQ runs without AI.
- The bundled localnet contains **public throwaway keys** and its `run-node.sh` may start a test faucet. These are never production credentials. Never re-use any bundled validator/deployer key on a chain holding real value.

## 2. One-command QOS harness (bundled disposable node)

From a fresh checkout of QuantumOS on the **same Linux host**:

```sh
git clone https://github.com/rhovision-org/quantum-os.git
cd quantum-os
bash scripts/localnet/qos-devnet.sh up --no-ai
```

**Build provenance:** QuantumOS's committed `bin/rnode` is a historical
disposable development binary. #601 GitHub CI reported
`bc80abbba42661f0252c04f931e6a4ac58bb6737`, **not** current Rholo `dev`.
For an isolated run on a freshly built pinned Rholo revision, set
`RNODE=/absolute/path/to/rnode` when launching the harness and record both
the binary's SHA-256 and its full reported source commit. This operates on the
harness's **separate** transient store only. It is never an installed public
validator qualification or a safe way to reopen a production store.

The harness builds the WASM package when missing, performs `pnpm install --frozen-lockfile`, builds the signaling server and browser, starts the bundled **local** Rholo node (no `--fresh`/data deletion), starts Signaling on port 4444, starts Vite HTTPS at 5173, and joins a deterministic facilitator agent to a fresh room. A freshly generated browser URL with the room capability is printed **once**.

The browser entry point is `https://127.0.0.1:5173/quantum-os/`. Follow the printed URL with `#room=…`; the harness does not retain that secret. In the UI set `/signal wss://127.0.0.1:5173/signal` if the browser is not using that local proxy already. Two independent browser contexts should enter the **same** capability room and exchange an ordinary chat message; the Signaling service at port 4444 is proxied as `/signal` by Vite.

```sh
bash scripts/localnet/qos-devnet.sh status
bash scripts/localnet/qos-devnet.sh down
```

`status` displays owned process PIDs, reported node version/network/shard, last-finalized block data and signaling readiness **without the room secret**. `down` checks the PID's Linux process-group identity, owner and start marker before signaling it, stops **only harness-owned groups**, and never deletes unrelated data or services. It intentionally retains its own temporary devnet data/log directory for explicit operator inspection/cleanup. A forced kill is not automatic.

To use a dedicated transient directory, set `QOS_RUN_DIR` to a private path; do not use a shared/world-writable data directory. Do not run two harnesses on the same ports.

## 3. Reuse an already running Rholo node (no lifecycle control)

For a Rholo HTTP API that is already reachable over local loopback, or through a trusted HTTPS endpoint:

```sh
bash scripts/localnet/qos-devnet.sh up --existing-node http://127.0.0.1:40403 --no-ai
```

This mode **never launches, stops or restarts the existing node**. It checks `/api/status` and requires a real `shardId`. Vite's local TLS `/rnode` reverse proxy targets the configured node URL, avoiding mixed-content and CORS failures. In the browser:

```text
/rholang rnode https://127.0.0.1:5173/rnode
/rholang status
/rholang eval 1 + 2
```

An exploratory evaluation is expected only if the node allows `explore-deploy` (e.g. a dev-mode or read-only-capable node). An explicit refusal is an **unsupported capability**, not proof of evaluation. A remote **plain HTTP** API is rejected by the harness; use SSH local forwarding to `127.0.0.1:40403` or a trusted `https://` API. The harness rejects admin port 40405.

To run a Rholo-managed disposable Devnet instead of the bundled node, follow [Rholo Devnet](../node/devnet.md):

```sh
# In an independent rholo checkout with Docker available:
tools/devnet.sh build
tools/devnet.sh up --validators 1
curl --fail http://127.0.0.1:40403/api/status
# Then from the quantum-os checkout:
bash scripts/localnet/qos-devnet.sh up --existing-node http://127.0.0.1:40403
```

The QOS harness must **not** bring this externally owned Devnet down; use the Rholo tools separately if you own that disposable network. Do not redirect these commands toward #584's public validators.

## 4. Interactive acceptance matrix

Record the **actual** QOS commit, Rholo commit **and installed node reported source**, exact commands, UTC times, HTTP status, sanitized raw outputs/digests, pass/fail/incomplete and reviewer. A successful devnet run is **not** a public four-validator finality verdict.

| Check | Required observation / gate |
|---|---|
| Node reachability | Public `GET /api/status`: version, network, `shardId` (never assume `/root`); a second block observation confirms the actual height/finality path. |
| QOS build | `pnpm` browser/Signaling builds exit zero; page loads at `/quantum-os/` with the expected revision. |
| Room + Signaling | Two independent browser contexts see each other and exchange encrypted chat; a fresh capability is not exposed in git or CI logs. |
| `/rholang status` | UI reports same node version and shard as the API. |
| `/rholang eval` | Node permits exploratory evaluation and returns the expected result; record a refusal as BLOCKED rather than PASS. |
| Signed deploy | On a **separately authorized throwaway funded devnet** only, sign one bounded deploy, capture deploy ID, accepted/included/finalized stages and final result. Never use the validator key. Included/executed is **not** settled; on the #584 four-validator gate only a separately approved transfer and G1–G6 procedure count. |
| `/goal` | Add/list/done in one browser identity; second identity must not see its private goals; explicit share is visible. |
| `/qos` | `/qos start example` → `/ps` → `/kill`; verify owner restrictions and digest consistency across peers. |
| Facilitator | `/facil ask what is /goal?` gives a deterministic answer without AI; errors are explicit. |
| Governance | `/gov chain install/push/pull` is PASS only if an actual chain result and status are captured; otherwise BLOCKED, never inferred from off-chain `/gov`. |
| Charter | `success-charter/1` canonical builder/validator exists; actual RhoGov submission/status remains [#291](https://github.com/rhovision-org/quantum-os/issues/291). |
| Shutdown | `down` removes only harness-owned processes and does **not** alter the reused Rholo node. |

The browser command reference is in [QuantumOS commands](https://github.com/rhovision-org/quantum-os/blob/main/docs/commands.md); identity-scoped goals and governance are in [success frameworks](https://github.com/rhovision-org/quantum-os/blob/main/docs/success-frameworks.md), and the canonical artifact is described in [success charter](https://github.com/rhovision-org/quantum-os/blob/main/docs/success-charter.md). The source-level localnet documentation is [QuantumOS localnet README](https://github.com/rhovision-org/quantum-os/blob/main/scripts/localnet/README.md).

## 5. Evidence status and non-claims (2026-11-17)

A read-only observation of **four real public-testnet services** is recorded at [Rholo PR #603](https://github.com/rhovision-org/rholo/pull/603), scoped to [#584](https://github.com/rhovision-org/rholo/issues/584). These services reported the older `0a1e0236…` binary, not the selected `70b8ea255…` candidate. That observation validates **existing-node reachability on the actual testnet**, **not** a QOS UI build, two-browser WebRTC exchange, signed deploy, Rholo `dev` source acceptance, or a G1–G6 live verdict.

This guide and its harness close the **documentation and orchestration** gap. Full #601 functional acceptance still requires a separately recorded genuine two-browser run, a bounded signed deploy on a permitted disposable network, and the combined Rholo/QuantumOS validation matrix. If any step fails, retain exact output and mark that cell **INCOMPLETE/BLOCKED**, not PASS.
