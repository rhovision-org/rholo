#!/usr/bin/env python3
"""Devnet fuzz harness — robustness + determinism, in modes.

Drives a running Docker devnet (started by tools/devnet.sh). **Nothing here gates a PR**: it needs
Docker, a built image and a live network, so it is a nightly/manual job rather than a PR check
(`.github/workflows/devnet-fuzz.yml`).

Modes (`--mode`, repeatable, `all` = every mode):

  valid      (default) robustness + determinism: random *syntactically-valid* terms through the
             read-only reducer (POST /api/v1/explore-deploy, which forks a throwaway runtime) must
             never 5xx, hang, or stop a node; signed deploys through the real block path
             (docker exec rnode deploy) must keep every validator in lock-step
             (latestBlockNumber within a small spread — the equal-stake devnet never reaches >2/3
             finality, so content-hash determinism is covered in-process).
  malformed  the R9/B-series *input* guards at process level: truncated terms, spliced delimiters,
             extreme nesting (the parser's depth guard), huge integers, non-UTF-8 bytes. Asserts
             each is a 4xx (never 5xx, never a hang) and that /api/v1/status still answers
             afterwards — a guard that wedged the node would pass a 5xx-only check.
  scheduler  the effect-scheduler modes over the wire: the devnet must be started with the mode
             under test (`tools/devnet.sh up --effect-scheduler dfs|gate`), every term in a corpus
             must reduce, and `relaxed` (unvalidated) must be **hard-rejected on the block path**
             while `relaxed-validated` is accepted — the in-process assertion is
             `casper/tests/scheduler.rs::block_paths_reject_relaxed_mode`.
  gateway    the single-shard surface: `/api/v1/shards` reports exactly one shard and the txn
             routes are 404 (the gateway needs a multi-shard node, which the Docker devnet cannot
             build — per-shard LFS sync is a missing product feature, see spec/TEST-COVERAGE.md).

Stdlib only (urllib + json + subprocess). Run against an already-up devnet:

    tools/devnet.sh up --validators 3
    python3 tools/devnet-fuzz.py --validators 3 --seed 1 --iterations 100 --mode all
"""

import argparse
import hashlib
import json
import os
import random
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

BOOTSTRAP = "devnet-bootstrap"
HTTP_BASE = 40403   # host HTTP port for validator 0; validator i is HTTP_BASE + 1000*i

# Max block-number spread (latestBlockNumber) that still counts as "in lock-step". The equal-stake
# devnet never reaches finality, so this convergence is the network-level determinism signal.
LOCKSTEP_TOLERANCE = 3

# Throwaway devnet deployer key (validator[0]'s private key, see tools/devnet.sh). Dev-only.
DEPLOYER_PRIV = "a68a6e6cca30f81bd24a719f3145d20e8424bd7b396309b0708a16c7d8000b76"


def http_json(url, method="GET", body=None, timeout=20):
    """Return (status, decoded_json_or_None). Raises on connection errors."""
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read().decode()
        return resp.status, json.loads(raw) if raw else None


def node_http(i):
    return f"http://localhost:{HTTP_BASE + 1000 * i}"


def fetch_status(i):
    _, body = http_json(f"{node_http(i)}/api/v1/status")
    return body


def all_nodes_running(validators):
    names = [BOOTSTRAP] + [f"devnet-validator-{i}" for i in range(1, validators)]
    for name in names:
        p = subprocess.run(
            ["docker", "inspect", "-f", "{{.State.Running}}", name],
            capture_output=True, text=True,
        )
        if p.returncode != 0 or p.stdout.strip() != "true":
            return False, name
    return True, ""


def dump_diagnostics(validators):
    subprocess.run(["docker", "ps"], check=False)
    names = [BOOTSTRAP] + [f"devnet-validator-{i}" for i in range(1, validators)]
    for name in names:
        subprocess.run(["docker", "logs", "--tail", "40", name], check=False)


def fail(msg, validators):
    print(f"FAIL: {msg}", file=sys.stderr)
    dump_diagnostics(validators)
    sys.exit(1)


def gen_term(rng):
    """A syntactically-valid, closed (free-var-free) rholang term."""
    n = rng.randint(0, 1000)
    s = rng.choice(["foo", "bar", "hello", "world", "fuzz"])
    b = rng.choice(["true", "false"])
    templates = [
        "Nil",
        f'@"fuzz"!({n})',
        f'@"fuzz"!("{s}")',
        f'@"fuzz"!({b})',
        f'new x in {{ x!({n}) | for (@v <- x) {{ @"fuzzout"!(v) }} }}',
        f'new x in {{ x!({n}) | for (@v <= x) {{ @"fuzzout"!(v) }} }}',
        f'new x in {{ x!!({n}) | for (@v <= x) {{ @"fuzzout"!(v) }} }}',
        f'@"fuzz"!([{n}, {n + 1}, {n + 2}].nth(0))',
        f'@"fuzz"!( {{ "k": {n} }}.get("k") )',
        f'@"fuzz"!({n} + {rng.randint(0, 9)} * {rng.randint(0, 9)})',
        f'if ({b}) {{ @"fuzz"!("yes") }} else {{ @"fuzz"!("no") }}',
        f'match {n} {{ 0 => @"fuzz"!("zero") _ => @"fuzz"!("nonzero") }}',
        f'@"fuzz"!({n}) | @"fuzz2"!("{s}")',
        f'@"fuzz"!("{s}".length())',
        f'@"fuzz"!(({n}, "{s}").nth(1))',
    ]
    return rng.choice(templates)


def heights_in_lockstep(numbers):
    """Validators count as in lock-step when their block-number spread is small.

    The equal-stake devnet never reaches `> 2/3` finality (2/3 is not a supermajority), so there is
    no finalized fringe to compare; latest-block-number convergence is the network-level determinism
    signal (content-hash determinism is covered by casper/tests/determinism.rs + scheduler.rs).
    """
    return bool(numbers) and (max(numbers) - min(numbers)) <= LOCKSTEP_TOLERANCE


def wait_for_lockstep(validators, timeout=120):
    """Poll until every validator's latestBlockNumber is within LOCKSTEP_TOLERANCE."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            numbers = [fetch_status(i)["latestBlockNumber"] for i in range(validators)]
        except (urllib.error.HTTPError, urllib.error.URLError):
            time.sleep(2)
            continue
        if heights_in_lockstep(numbers):
            return max(numbers)
        time.sleep(2)
    return None


def assert_lockstep(validators):
    """Assert every validator's latestBlockNumber is within LOCKSTEP_TOLERANCE (no divergence)."""
    numbers = []
    for i in range(validators):
        try:
            numbers.append(fetch_status(i)["latestBlockNumber"])
        except (urllib.error.HTTPError, urllib.error.URLError) as e:
            fail(f"validator {i} status unreachable: {e}", validators)
    if not heights_in_lockstep(numbers):
        fail(f"validators diverged in latestBlockNumber: {numbers}", validators)
    return max(numbers)


def signed_deploy(node, term, height, shard_id):
    """Deploy a term through the real block path (docker exec rnode deploy).

    `shard_id` is passed in rather than hardcoded: the devnet's shard is `/root`, and this call used to
    send `root`, so every signed deploy failed the node's own shard check
    (`Deploy shardId 'root' is not a member of this node's shards: [/root]`) — found on the fuzzer's
    first real run. The caller reads it from the node's `/api/v1/status`, which is where the truth is.
    """
    write = subprocess.run(
        ["docker", "exec", "-i", node, "sh", "-c", "cat > /tmp/fuzz.rho"],
        input=term, text=True, capture_output=True,
    )
    if write.returncode != 0:
        return write
    return subprocess.run(
        ["docker", "exec", node, "rnode", "--grpc-host", "localhost", "deploy",
         "--phlo-limit", "1000000", "--phlo-price", "1",
         "--private-key", DEPLOYER_PRIV, "--shard-id", shard_id,
         "--valid-after-block-number", str(height), "/tmp/fuzz.rho"],
        text=True, capture_output=True,
    )


# --- mode: malformed -------------------------------------------------------------------------

class Malformed:
    """One malformed request: `kind` names the guard it targets, `body`/`raw` what to send.

    `raw=True` means the bytes are sent verbatim (no JSON encoding) — that is how a non-UTF-8 body
    reaches the server's decoder at all.
    """

    def __init__(self, kind, body, raw=False):
        self.kind = kind
        self.body = body
        self.raw = raw

    def describe(self):
        if self.raw:
            return f"{self.body!r}"
        text = self.body if isinstance(self.body, str) else repr(self.body)
        return text if len(text) <= 70 else text[:67] + "..."


def _nested_chain(depth, chain):
    """`depth` nested parens, each holding a flat chain of `chain` additions.

    The shape the two parser guards do not bound: a chain is built by a loop, so it costs a bounded
    number of parser frames and produces an **AST of depth `n`**, and nesting composes it. Written as a
    helper rather than inlined because the three cases below differ only in which guard they stay
    inside.
    """
    inner = "1"
    link = " + 1" * chain
    for _ in range(depth):
        inner = f"({inner}{link})"
    return inner


def gen_malformed(rng, n):
    """The malformed corpus: one case per input guard, generated rather than hard-coded.

    Each entry names the guard it exercises so a failure report says *which* one let something
    through (a bare "500" is not actionable).
    """
    valid = gen_term(rng)
    out = []

    # Truncation: cut a valid term at a random point. The parser must report a syntax error.
    for _ in range(max(1, n // 6)):
        cut = rng.randint(1, max(1, len(valid) - 1))
        out.append(Malformed("truncated", valid[:cut]))

    # Delimiter splicing: unmatched/closed-out-of-order brackets and a lone closing delimiter.
    #
    # The lone-closing case is *doubled*: `@"f"!()` is **valid** — a send with one empty group — so the
    # single-`)` payload this generator used to emit was accepted (200) on the fuzzer's first real run.
    # A generator that emits a well-formed term cannot test an input guard.
    out.append(Malformed("spliced-delimiter", '@"f"!())'))
    for delim in ["}", "]", "(", "{", "["]:
        out.append(Malformed("spliced-delimiter", f'@"f"!({delim}'))
    out.append(Malformed("spliced-delimiter", 'new x in { x!(1)'))
    out.append(Malformed("spliced-delimiter", 'for (@v <- x) { }'))

    # Extreme nesting: past the parser's **recursion** guard — `MAX_PARSE_DEPTH = 128` in
    # `rholang/src/parser.rs` (this line used to say 512, which is `MAX_CHAIN_LENGTH`: the depth guard
    # was under-stated by 4x and the chain guard was named as the depth one, so a reader measuring the
    # fuzzer's coverage from this comment would have had both backwards).
    for depth in [200, 5000]:
        out.append(Malformed("deep-nesting", "new x in " * depth + "Nil"))
    out.append(Malformed("deep-nesting", "@" * 2000 + "Nil"))
    out.append(Malformed("deep-nesting", "not " * 5000 + "Nil"))

    # The **product** of the two guards, which neither of them bounds (AUDIT C99): `d` levels of `c`
    # chained operators are an AST of depth `d x c`, and `new x in`/`not` nesting cannot generate the
    # shape — the chain is what makes it deep, and those cases have no chain at all. Both guards are
    # satisfied in the case below (`depth < 128`, `chain < 512`) while the AST is ~50,000 deep, which
    # before the fix was a *process abort* on the node's own 32 MiB worker; the two over-bounds cases
    # are the controls, so a run says which guard fired.
    out.append(Malformed("nested-chain", _nested_chain(100, 500)))
    out.append(Malformed("nested-chain-over-depth", _nested_chain(200, 500)))
    out.append(Malformed("nested-chain-over-chain", _nested_chain(100, 600)))

    # Oversized integers: past i64 and past any plausible bignum budget.
    for digits in [20, 100, 5000]:
        out.append(Malformed("huge-integer", f'@"f"!({"9" * digits})'))
    out.append(Malformed("huge-integer", f'@"f"!({"9" * 100}.toInt())'))

    # Non-UTF-8 bytes inside a string literal, sent raw so the decoder sees them.
    out.append(Malformed("invalid-utf8", b'"\xff\xfe\x80"', raw=True))
    out.append(Malformed("invalid-utf8", b'@"f"!("\xc3")', raw=True))
    # …and a body that is not JSON at all.
    out.append(Malformed("invalid-utf8", b"\x00\x01\x02", raw=True))

    # A JSON body that is not a string (the API takes the term as a JSON string).
    out.append(Malformed("wrong-shape", {"term": valid}))
    out.append(Malformed("wrong-shape", [valid]))
    out.append(Malformed("wrong-shape", None))

    return out[:n] if n and n < len(out) else out


def run_malformed(args, rng):
    """Every malformed input is a 4xx; every node stays up; the status endpoint still answers."""
    cases = gen_malformed(rng, 0)
    print(f"==> malformed: {len(cases)} cases")
    for i, case in enumerate(cases):
        node = i % args.validators
        url = f"{node_http(node)}/api/v1/explore-deploy"
        try:
            if case.raw:
                req = urllib.request.Request(url, data=case.body, method="POST")
                try:
                    with urllib.request.urlopen(req, timeout=20) as resp:
                        status = resp.status
                except urllib.error.HTTPError as e:
                    status = e.code
            else:
                status, _ = http_json(url, method="POST", body=case.body)
        except urllib.error.HTTPError as e:
            status = e.code
        except (urllib.error.URLError, OSError) as e:
            fail(f"malformed[{case.kind}] on validator {node}: connection error {e}", args.validators)

        if status >= 500:
            fail(
                f"malformed[{case.kind}] on validator {node} returned {status} for "
                f"{case.describe()}",
                args.validators,
            )
        if status == 200:
            # Not a failure for every kind (a wrapped-but-valid payload may reduce), but worth
            # printing: a case that is *supposed* to be rejected and is not means a guard is gone.
            print(f"    note: malformed[{case.kind}] was accepted (200): {case.describe()}")

    # The node must still answer after the whole corpus — a guard that wedged or killed a node would
    # pass a 5xx-only check.
    for i in range(args.validators):
        try:
            fetch_status(i)
        except (urllib.error.HTTPError, urllib.error.URLError, OSError) as e:
            fail(f"validator {i} did not answer /api/v1/status after the malformed corpus: {e}",
                 args.validators)
    running, bad = all_nodes_running(args.validators)
    if not running:
        fail(f"node '{bad}' stopped during the malformed corpus", args.validators)
    print("==> malformed: all inputs rejected without a 5xx, every node still healthy")


# --- mode: scheduler -------------------------------------------------------------------------

# Terms whose reduction is order-sensitive enough to tell the scheduler modes apart. All are closed,
# bounded, and must reduce cleanly in every mode. Persistence is deliberately one-sided: pairing a
# persistent send (`!!`) with a persistent receive (`<=`) is a live process, not a finite reducer
# probe, so it correctly exhausts the explore-deploy step budget instead of terminating.
SCHEDULER_CORPUS = [
    'new c in { c!(1) | for (@x <- c) { @"out"!(x) } }',
    'new c in { c!(1) | c!(2) | for (@x <- c) { @"out"!(x) } }',
    'new c, d in { c!(1) | for (@x <- c) { d!(x) } | for (@y <- d) { @"out"!(y) } }',
    'new c in { c!!(1) | for (@x <- c) { @"out"!(x) } }',
    'new c in { c!(1) | for (@x <= c) { @"out"!(x) } }',
    'new c in { c!(1) | for (@x <- c) { @"out"!(x) } } | new e in { e!(2) | for (@y <- e) { @"out"!(y) } }',
]


def explicit_explore_anchor(node):
    """Select a real block on this node; never silently substitute an unfinalized tip."""
    _, blocks = http_json(f"{node_http(node)}/api/blocks")
    if not isinstance(blocks, list) or not blocks:
        fail(f"validator {node} has no blocks to anchor scheduler exploration", 3)
    genesis = next((b for b in blocks if b.get("blockNumber") == 0), blocks[0])
    block_hash = genesis.get("blockHash")
    if not isinstance(block_hash, str) or not block_hash:
        fail(f"validator {node} block listing lacks blockHash: {genesis!r}", 3)
    return block_hash


def run_scheduler(args, rng):
    """Reduce against a named block: the no-hash route requires finality."""
    print(f"==> scheduler: {len(SCHEDULER_CORPUS)} corpus terms")
    for i, term in enumerate(SCHEDULER_CORPUS):
        node = i % args.validators
        block_hash = explicit_explore_anchor(node)
        url = f"{node_http(node)}/api/v1/explore-deploy-by-block-hash"
        payload = {"term": term, "blockHash": block_hash, "usePreStateHash": False}
        print(f"scheduler case={i} validator={node} blockHash={block_hash} "
              f"route=/api/v1/explore-deploy-by-block-hash", flush=True)
        try:
            status, _ = http_json(url, method="POST", body=payload)
        except urllib.error.HTTPError as e:
            status = e.code
            # Preserve the server's rejection reason; a bare 400 cannot
            # distinguish invalid Rholang from scheduler/runtime failures.
            error_body = e.read(4096).decode("utf-8", errors="replace")
            print(
                f"scheduler corpus term {i} validator {node} HTTP {status} "
                f"response: {error_body!r}", file=sys.stderr,
            )
        except (urllib.error.URLError, OSError) as e:
            fail(f"scheduler corpus term {i} on validator {node}: connection error {e}",
                 args.validators)
        if status >= 500:
            fail(f"scheduler corpus term {i} on validator {node} returned {status}: {term}",
                 args.validators)
        if status != 200:
            fail(
                f"scheduler corpus term {i} on validator {node} returned {status} (a clean term "
                f"must reduce in every scheduler mode): {term}",
                args.validators,
            )
    print("==> scheduler: the corpus reduced (start the devnet with --effect-scheduler to sweep "
          "the modes)")


# --- mode: gateway ---------------------------------------------------------------------------

def run_gateway(args, rng):
    """The single-shard surface: one shard reported, txn routes absent."""
    del rng
    _, shards = http_json(f"{node_http(0)}/api/v1/shards")
    listed = shards if isinstance(shards, list) else shards.get("shards", [])
    if len(listed) != 1:
        fail(
            f"a single-shard devnet must report exactly one shard, got {listed!r} "
            "(see spec/TEST-COVERAGE.md: the multi-shard devnet is blocked on per-shard LFS sync)",
            args.validators,
        )
    # The txn API is a multi-shard feature: on a single-shard node the route is absent.
    try:
        status, _ = http_json(
            f"{node_http(0)}/api/v1/txn/status?txnId={0:064x}", method="GET"
        )
    except urllib.error.HTTPError as e:
        status = e.code
    if status not in (404, 405):
        fail(
            f"a single-shard node must not serve the txn API, got {status} "
            "(the surface must be unchanged when the gateway is disabled)",
            args.validators,
        )
    print("==> gateway: one shard reported, txn routes absent (single-shard surface unchanged)")


def run_acceptance_campaign(trace=False):
    """Opt-in CI-only campaign; preserve the ordinary fuzz modes and their network."""
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("acceptance mode requires an isolated GitHub Actions runner")
    # The existing workflow started this default devnet in this same disposable
    # job. Stop it to free its ports; no public node or user data is reachable.
    subprocess.run(["tools/devnet.sh", "down"], check=True)
    env = {**os.environ, "DEVNET_PREFIX": "acceptance", "DEVNET_NETWORK": "acceptance",
           "DEVNET_NODE_MEMORY": "2g", "DEVNET_THREAD_POOL_SIZE": "2"}
    if trace:
        # A diagnostic arm, not a replacement for the N=4/N=8 acceptance gate.
        # Full inbound traces distinguish propagation from validation rejection.
        env["DEVNET_LOG_LEVEL"] = "debug"
    outcomes = []
    for validators in ((5, 8) if trace else (4, 8)):
        out = Path("target/consensus-acceptance") / f"n{validators}"
        out.mkdir(parents=True, exist_ok=True)
        code = 1
        try:
            subprocess.run(["tools/devnet.sh", "up", "--validators", str(validators),
                            "--fresh", "--no-autopropose", "--epoch-length", "10",
                            "--effect-scheduler", env.get("EFFECT_SCHEDULER", "dfs")],
                           env=env, check=True)
            code = subprocess.run([sys.executable, "tools/consensus-acceptance-drill.py",
                                   "--validators", str(validators), "--out", str(out)], env=env).returncode
        except (OSError, subprocess.CalledProcessError) as error:
            (out / "workload-result.json").write_text(
                json.dumps({"result": "fail", "reason": str(error)}) + "\n")
        finally:
            for i in range(validators):
                node = "acceptance-bootstrap" if i == 0 else f"acceptance-validator-{i}"
                print(f"=== n={validators} node={i} resource status ===", flush=True)
                subprocess.run(["docker", "inspect", "--format", "{{json .State}}", node], check=False)
                print(f"=== n={validators} node={i} log tail ===", flush=True)
                subprocess.run(["docker", "logs", "--tail", "80", node], check=False)
            # The existing workflow has no upload step. Put machine-readable
            # observations in its retained log; refuse to silently truncate.
            for path in (out / "workload-result.json", out / "observations.jsonl"):
                if path.exists():
                    print(f"=== BEGIN {path} ===", flush=True)
                    if env.get("ACCEPTANCE_ARTIFACTS") == "1" and path.name == "observations.jsonl":
                        data = path.read_bytes()
                        print("EVIDENCE_ARTIFACT " + json.dumps({
                            "path": str(path), "bytes": len(data),
                            "sha256": hashlib.sha256(data).hexdigest()}), flush=True)
                    else:
                        print(path.read_text(), flush=True)
                    print(f"=== END {path} ===", flush=True)
            teardown = subprocess.run(["tools/devnet.sh", "down", "-v"], env=env, check=False)
            if teardown.returncode:
                print(f"TEARDOWN_WARNING n={validators} exit={teardown.returncode}", flush=True)
        outcomes.append({"validators": validators, "exit_code": code})
    print("ACCEPTANCE_OUTCOMES " + json.dumps(outcomes), flush=True)
    if any(x["exit_code"] for x in outcomes):
        raise SystemExit(1)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--validators", type=int, default=3, help="bonded validators (default 3)")
    ap.add_argument("--iterations", type=int, default=100, help="explore-deploy fuzz iterations")
    ap.add_argument("--deploy-burst", type=int, default=10, help="signed deploys through the block path")
    ap.add_argument("--seed", type=int, default=None, help="RNG seed (reproducible)")
    ap.add_argument("--sleep", type=float, default=0.05, help="pause between explore-deploys (s)")
    ap.add_argument(
        "--mode",
        action="append",
        choices=["valid", "malformed", "scheduler", "gateway", "all", "lfs-identity", "lfs-adversarial", "lfs-resilience", "lfs-history", "lfs-history-controlled", "lfs-history-anchored"],
        default=None,
        help="which mode(s) to run (repeatable; default: valid)",
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="print the payloads the mode would send and exit (no node needed)",
    )
    args = ap.parse_args()

    modes = args.mode or ["valid"]
    if "all" in modes:
        modes = ["valid", "malformed", "scheduler", "gateway"]

    if "lfs-history-anchored" in modes:
        if modes != ["lfs-history-anchored"]:
            ap.error("explicit LFS anchor is a standalone isolated acceptance arm")
        if args.dry_run:
            print("isolated: explicit finality anchor, full justification closure, state hashes, restart persistence")
            return
        subprocess.run([sys.executable, "tests/lfs-anchored-checkpoint.py"], check=True)
        return

    if "lfs-history-controlled" in modes:
        if modes != ["lfs-history-controlled"]:
            ap.error("controlled finalized state restoration must be standalone")
        if args.dry_run:
            print("isolated controlled source: manual proposals, weighted stake, frozen prefix, LFS persistence")
            return
        os.environ["LFS_HISTORY_MANUAL_PROPOSE"] = "1"
        subprocess.run([sys.executable, "tests/lfs-finalized-history.py"], check=True)
        return

    if "lfs-history" in modes:
        if modes != ["lfs-history"]:
            ap.error("finalized-state restoration is a standalone acceptance arm")
        if args.dry_run:
            print("isolated: weighted-stake finalized prefix, real state transition, LFS, restart checkpoint")
            return
        subprocess.run([sys.executable, "tests/lfs-finalized-history.py"], check=True)
        return

    if "lfs-resilience" in modes:
        if modes != ["lfs-resilience"]:
            ap.error("LFS resilience requires a standalone disposable-network arm")
        if args.dry_run:
            print("isolated CI: 25s network partition, repeated joins, durable restart, netem delay/loss")
            return
        subprocess.run([sys.executable, "tests/lfs-resilience.py"], check=True)
        return

    if "lfs-adversarial" in modes:
        if modes != ["lfs-adversarial"]:
            ap.error("LFS adversarial test requires a standalone isolated network")
        if args.dry_run:
            print("isolated CI: reject wrong bootstrap certificate identity, recover, and reconnect")
            return
        subprocess.run([sys.executable, "tests/lfs-adversarial-bootstrap.py"], check=True)
        return

    if "lfs-identity" in modes:
        if modes != ["lfs-identity"]:
            ap.error("LFS identity is a standalone disposable-network arm")
        if args.dry_run:
            print("LFS bootstrap hostname-to-IP rejoin test on isolated CI devnet")
            return
        subprocess.run([sys.executable, "tests/lfs-ip-bootstrap.py"], check=True)
        return

    if args.dry_run:
        # The generator half of `malformed` is checkable without a node: print what a run would send
        # so the shapes can be inspected (and so a broken generator is visible in CI-less review).
        rng = random.Random(args.seed)
        for payload in gen_malformed(rng, 0):
            print(f"{payload.kind}: {payload.describe()}")
        return

    rng = random.Random(args.seed)

    # Pre-flight: every validator must be up and healthy.
    running, bad = all_nodes_running(args.validators)
    if not running:
        fail(f"node '{bad}' is not running (start it with tools/devnet.sh up)", args.validators)

    non_valid = [m for m in modes if m != "valid"]
    if non_valid:
        # The non-`valid` modes still need a live devnet, but not the pre-fuzz lock-step wait: they
        # are about input guards and the single-shard surface, not about convergence.
        for mode in non_valid:
            if mode == "malformed":
                run_malformed(args, rng)
            elif mode == "scheduler":
                run_scheduler(args, rng)
            elif mode == "gateway":
                run_gateway(args, rng)

    if "valid" not in modes:
        print("==> done (no `valid` mode requested)")
        return

    print(f"==> fuzz: {args.iterations} explore-deploys + {args.deploy_burst} signed deploys "
          f"across {args.validators} validators (seed={args.seed})")

    print("==> waiting for cross-validator lock-step ...")
    common_height = wait_for_lockstep(args.validators)
    if common_height is None:
        fail("timed out waiting for validators to reach lock-step", args.validators)
    print(f"==> pre-fuzz lock-step height: {common_height}")

    # --- robustness: read-only reducer fuzz ------------------------------------------------
    for it in range(args.iterations):
        node = it % args.validators
        term = gen_term(rng)
        url = f"{node_http(node)}/api/v1/explore-deploy"
        try:
            status, _ = http_json(url, method="POST", body=term)
        except urllib.error.HTTPError as e:
            status = e.code
        except (urllib.error.URLError, OSError) as e:
            fail(f"explore-deploy #{it} on validator {node} connection error: {e}", args.validators)

        if status >= 500:
            fail(f"explore-deploy #{it} on validator {node} returned {status} for term: {term}",
                 args.validators)
        # 200 = reduced, 400 = reduce error (fine — exercises error paths), 429 = rate limit (warn).
        if status == 429 and (it + 1) % 50 == 0:
            print(f"    (rate-limited at iteration {it})")
        time.sleep(args.sleep)

    # --- state-mutating fuzz through the block path ---------------------------------------
    status = fetch_status(0)
    height = status["latestBlockNumber"]
    # The shard id comes from the running node, not a literal: a hardcoded `root` sent every signed
    # deploy into the node's shard check and failed there.
    shard_id = status["shardId"]
    for j in range(args.deploy_burst):
        term = f'@"fuzzstate{j}"!({rng.randint(0, 10 ** 6)})'
        p = signed_deploy(BOOTSTRAP, term, height, shard_id)
        if p.returncode != 0:
            fail(f"signed deploy #{j} failed: {p.stderr.strip() or p.stdout.strip()}", args.validators)
        height += 1

    # Wait for the deploy-driven blocks to propagate across validators.
    time.sleep(5)

    # --- determinism re-check --------------------------------------------------------------
    running, bad = all_nodes_running(args.validators)
    if not running:
        fail(f"node '{bad}' stopped during the fuzz run", args.validators)
    final_height = assert_lockstep(args.validators)

    print(f"==> post-fuzz: {args.validators} validators in lock-step at height {final_height}")
    print("==> robustness + determinism OK")


if __name__ == "__main__":
    main()
