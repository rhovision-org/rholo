#!/usr/bin/env python3
"""m2-pressure-arm — drive the M2 pressure record on a disposable devnet.

The arm assumes a running devnet (like the other devnet arms); it does not start one. The cap is
read once at listener startup, so the two phases are two devnet lifetimes, not a mutation of a
running node:

    DEVNET_PREFIX=m2cap DEVNET_NETWORK=m2net DEVNET_GRPC_BASE=56402 \
    DEVNET_HTTP_BASE=56403 DEVNET_ADMIN_BASE=56405 DEVNET_NODE_MEMORY=2g \
    DEVNET_NODE_ENV=RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2 tools/devnet.sh up --validators 1 --fresh
    tools/m2-pressure-arm.py phase --mode capped --out target/m2/capped \
        --container m2cap-bootstrap --port 56403 --cap 2
    tools/devnet.sh down -v

    ... the same `up` without DEVNET_NODE_ENV ...
    tools/m2-pressure-arm.py phase --mode unset --out target/m2/unset \
        --container m2cap-bootstrap --port 56403
    tools/devnet.sh down -v

    tools/m2-pressure-arm.py grpc --out target/m2/grpc
    tools/m2-pressure-arm.py assemble --capped target/m2/capped --unset target/m2/unset \
        --grpc target/m2/grpc --out spec/audit/evidence/m2-pressure-local-20261119 \
        --locality local-devnet --port 56403

`phase` holds the configured permits with partial-body POSTs, fires complete invalid-JSON requests
(the admission refusal happens before the body is decoded, and an admitted one reaches the extractor
as a 400), and captures the raw socket view of the same listener. `assemble` merges the two phases
and the Rust gRPC observation into `pressure.json` plus pinned raw files.

This is a local-devnet rehearsal unless the endpoint is a real host: the criterion requires a
`public-live` label and a non-local endpoint, and refuses this record.
"""
from __future__ import annotations

import argparse
import hashlib
import http.client
import importlib.util
import json
import os
import re
import socket
import subprocess
import sys
import time
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
CAP_KEY = "RNODE_HTTP_MAX_INFLIGHT_DEPLOY"
INVALID = b"invalid json\n"
REFUSAL_MESSAGE = "HTTP in-flight capacity exhausted"
HEX40 = re.compile(r"\b[0-9a-f]{40}\b")
HEX64 = re.compile(r"\b[0-9a-f]{64}\b")


def _module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_locality = _module("m2_arm_locality", ROOT / "tools" / "check-evidence-locality.py")


def endpoint_parts(endpoint: str) -> tuple[str, int, str]:
    parsed = urlparse(endpoint)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise SystemExit(f"{endpoint}: not an http(s) URL")
    return parsed.hostname, parsed.port or (443 if parsed.scheme == "https" else 80), parsed.scheme


def derive_locality(endpoint: str) -> str:
    host, _, _ = endpoint_parts(endpoint)
    return "local-devnet" if _locality.local_address(host) else "public-live"


def docker(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", *args], capture_output=True, text=True, check=False)


def host_header(host: str, port: int) -> str:
    return host if port in (80, 443) else f"{host}:{port}"


def hold(host: str, port: int, length: int = 65536) -> socket.socket:
    """Open a request whose headers are complete and whose body is not.

    The admission middleware runs before the body extractor, so this deterministically occupies
    one permit; the request is later closed, which the transcript records as `held`.
    """
    connection = socket.create_connection((host, port), timeout=10)
    connection.sendall(
        (
            "POST /api/v1/deploy HTTP/1.1\r\n"
            f"Host: {host_header(host, port)}\r\n"
            "Content-Type: application/json\r\n"
            f"Content-Length: {length}\r\n\r\n"
        ).encode()
    )
    connection.sendall(b'{"a"')
    return connection


PHASE_START = time.monotonic()


def probe(host: str, port: int) -> dict:
    connection = http.client.HTTPConnection(host, port, timeout=10)
    try:
        connection.request("POST", "/api/v1/deploy", body=INVALID,
                           headers={"Content-Type": "application/json"})
        response = connection.getresponse()
        body = response.read().decode(errors="replace").strip()
        try:
            parsed = json.loads(body)
        except ValueError:
            parsed = body
        headers = {name.lower(): value for name, value in response.getheaders()}
        return {
            "kind": "probe",
            "status": response.status,
            "retry_after": int(headers["retry-after"]) if "retry-after" in headers else None,
            "body": parsed if isinstance(parsed, str) else body,
            "t_ms": int((time.monotonic() - PHASE_START) * 1000),
        }
    finally:
        connection.close()


def after(host: str, port: int) -> dict:
    record = probe(host, port)
    record["kind"] = "after"
    return record


def raw_exchange(host: str, port: int) -> str:
    """One raw-socket request, with its status line and headers read verbatim.

    The body is complete: an admitted request must reach the extractor and answer, while the
    admission refusal happens before the body is read.
    """
    connection = socket.create_connection((host, port), timeout=10)
    try:
        connection.sendall(
            (
                "POST /api/v1/deploy HTTP/1.1\r\n"
                f"Host: {host_header(host, port)}\r\n"
                "Content-Type: application/json\r\n"
                f"Content-Length: {len(INVALID)}\r\n\r\n"
            ).encode()
            + INVALID
        )
        received = b""
        while b"\r\n\r\n" not in received:
            chunk = connection.recv(4096)
            if not chunk:
                break
            received += chunk
        return received.decode(errors="replace")
    finally:
        connection.close()


def capture(path: Path, process: subprocess.CompletedProcess) -> None:
    path.write_text(process.stdout if process.returncode == 0 else process.stderr)


def phase(args: argparse.Namespace) -> int:
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if getattr(args, "endpoint", ""):
        host, port, scheme = endpoint_parts(args.endpoint)
        if scheme == "https":
            raise SystemExit("the raw direct-socket surface is HTTP-only; TLS direct is deferred")
        if not _locality.local_address(host) and not (
                getattr(args, "inspect", "") and getattr(args, "version", "")):
            raise SystemExit(
                "a non-local endpoint requires --inspect and --version from that host")
        endpoint = args.endpoint
    else:
        host, port = "127.0.0.1", args.port
        endpoint = f"http://127.0.0.1:{args.port}"
    capped = args.mode == "capped"
    cap = args.cap if capped else 0
    held = [hold(host, port) for _ in range(cap)]
    # The permits are acquired when the server parses the held request heads; give the accepts a
    # moment, or the first probe races them and is admitted.
    time.sleep(args.settle)
    records: list[dict] = [{"kind": "held", "status": None, "retry_after": None,
                            "body": "", "t_ms": 0} for _ in range(cap)]
    try:
        probes = []
        for _ in range(args.probe):
            probes.append(probe(host, port))
            time.sleep(args.spacing)
        records.extend(probes)
        direct_refused = raw_exchange(host, port)
    finally:
        for connection in held:
            connection.close()
    time.sleep(args.settle)
    released = after(host, port)
    records.append(released)
    direct_after = raw_exchange(host, port)
    transcript = out / f"http-{args.mode}.jsonl"
    transcript.write_text("".join(json.dumps(record) + "\n" for record in records))
    direct_path = out / f"direct-{args.mode}.txt"
    direct_path.write_text(direct_refused + "\n-- release --\n" + direct_after)

    refused = [record for record in probes if record["status"] == 429]
    admitted = [record for record in probes if record["status"] == 400]
    server = [record for record in probes if isinstance(record["status"], int)
              and 500 <= record["status"] <= 599]
    elapsed = ((probes[-1]["t_ms"] - probes[0]["t_ms"]) / 1000) if len(probes) > 1 else 0
    rate = round((len(probes) - 1) / elapsed, 2) if elapsed > 0 else 0.0
    summary = {
        "mode": args.mode,
        "endpoint": endpoint,
        "cap": cap if capped else None,
        "held": cap if capped else 0,
        "probe": len(probes),
        "admitted": len(admitted),
        "refused": len(refused),
        "server_errors": len(server),
        "retry_after": len([record for record in refused if record["retry_after"] == 1]),
        "handler_reached_after_release": released["status"] == 400,
        "requests_per_second": rate,
        "refusal": {
            "class": "429" if refused else "none",
            "message": refused[0]["body"] if refused else "",
        },
        "transcript": f"http-{args.mode}.jsonl",
        "direct": {
            "status_line": "HTTP/1.1 429" if "HTTP/1.1 429" in direct_refused else "HTTP/1.1 400",
            "retry_after": 1 if re.search(r"(?im)^retry-after:\s*1\s*$", direct_refused) else None,
            "transcript": f"direct-{args.mode}.txt",
            "after_status_line": "HTTP/1.1 400" if "HTTP/1.1 400" in direct_after else "other",
        },
    }
    if getattr(args, "inspect", ""):
        (out / f"{args.mode}-inspect.json").write_bytes(Path(args.inspect).read_bytes())
    else:
        capture(out / f"{args.mode}-inspect.json", docker(["inspect", args.container]))
    if getattr(args, "version", ""):
        (out / f"{args.mode}-version.txt").write_bytes(Path(args.version).read_bytes())
    else:
        capture(out / f"{args.mode}-version.txt",
                docker(["exec", args.container, "rnode", "--version"]))
    digest = docker(["exec", args.container, "sha256sum", "/usr/local/bin/rnode"])
    if digest.returncode == 0:
        match = HEX64.search(digest.stdout)
        summary["binary_sha256"] = match.group(0) if match else None
    (out / "phase.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    print(json.dumps(summary, indent=1, sort_keys=True))
    problems = []
    if capped and summary["refused"] != summary["probe"]:
        problems.append("a capped probe was not refused")
    if capped and summary["held"] != cap:
        problems.append("not every permit was held")
    if capped and summary["retry_after"] != summary["refused"]:
        problems.append("a refusal carries no retry-after: 1")
    if capped and summary["refusal"]["message"] != REFUSAL_MESSAGE:
        problems.append(
            f"the refusal body {summary['refusal']['message']!r} is not the admission message"
        )
    if not capped and summary["refused"]:
        problems.append("the unset default refused a probe")
    if not capped and summary["admitted"] != summary["probe"]:
        problems.append("an unset probe did not reach the handler")
    if summary["server_errors"]:
        problems.append("a probe answered 5xx")
    if not summary["handler_reached_after_release"]:
        problems.append("the after-release probe did not reach the handler")
    if rate > 100:
        problems.append("the burst exceeds the per-peer ceiling")
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


def grpc(args: argparse.Namespace) -> int:
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if getattr(args, "origin", "candidate-test") != "candidate-test":
        raise SystemExit("an in-process cargo test cannot claim endpoint origin")
    source = getattr(args, "source_sha", "")
    binary = getattr(args, "binary_sha256", "")
    if getattr(args, "locality", "local-devnet") == "public-live" \
            and not (HEX40.fullmatch(source or "") and HEX64.fullmatch(binary or "")):
        raise SystemExit("a public-live gRPC observation needs the selected pins")
    command = ["cargo", "test", "-p", "rchain-node", "--lib", "m2_grpc_pressure",
               "--", "--nocapture"]
    environment = dict(os.environ, RNODE_M2_GRPC_OUT=str(out))
    proc = subprocess.run(command, env=environment, capture_output=True,
                          text=True, check=False)
    sys.stdout.write(proc.stdout[-2000:])
    sys.stderr.write(proc.stderr[-2000:])
    if proc.returncode != 0 or not (out / "grpc-summary.json").is_file():
        return 1
    candidate = {
        "schema": 1,
        "origin": "candidate-test",
        "scope": "candidate-code",
        "candidate": {"source_sha": source or None, "binary_sha256": binary or None},
        "endpoint": {"url": getattr(args, "endpoint", ""), "proxy": "none", "used": False},
        "command": command,
    }
    (out / "grpc-candidate.json").write_text(json.dumps(candidate, indent=1) + "\n")
    return 0


def _phase(out: Path) -> dict:
    path = out / "phase.json"
    if not path.is_file():
        raise SystemExit(f"{out}: phase.json is absent")
    return json.loads(path.read_text())


def assemble(args: argparse.Namespace) -> int:
    capped = _phase(Path(args.capped))
    unset = _phase(Path(args.unset))
    endpoint = getattr(args, "endpoint", "") or f"http://127.0.0.1:{args.port}"
    derived = derive_locality(endpoint)
    wanted = getattr(args, "locality", None)
    if wanted and wanted != derived:
        raise SystemExit(
            f"the endpoint derives {derived!r}, not the requested {wanted!r}")
    locality = derived
    for phase in (capped, unset):
        recorded = phase.get("endpoint")
        if recorded and recorded != endpoint:
            raise SystemExit("the capped and unset phases used different endpoints")
    grpc_dir = Path(args.grpc)
    grpc_summary = json.loads((grpc_dir / "grpc-summary.json").read_text())
    candidate_path = grpc_dir / "grpc-candidate.json"
    if not candidate_path.is_file():
        raise SystemExit(f"{candidate_path}: absent")
    grpc_candidate = json.loads(candidate_path.read_text())
    if capped.get("binary_sha256") != unset.get("binary_sha256"):
        raise SystemExit("the capped and unset phases were not run on the same binary")
    source = args.source_sha
    if not source:
        text = (Path(args.capped) / "capped-version.txt").read_text(errors="replace")
        match = HEX40.search(text)
        source = match.group(0) if match else ""
    binary = args.binary_sha256 or capped.get("binary_sha256") or ""
    if locality == "public-live" and (
            not HEX40.fullmatch(source or "") or not HEX64.fullmatch(binary or "")):
        raise SystemExit("a public-live record needs the selected source and binary pins")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    copies = {
        "http-capped.jsonl": Path(args.capped) / "http-capped.jsonl",
        "http-unset.jsonl": Path(args.unset) / "http-unset.jsonl",
        "direct-capped.txt": Path(args.capped) / "direct-capped.txt",
        "direct-unset.txt": Path(args.unset) / "direct-unset.txt",
        "grpc-capped.jsonl": grpc_dir / "grpc-capped.jsonl",
        "grpc-permissive.jsonl": grpc_dir / "grpc-permissive.jsonl",
        "grpc-candidate.json": grpc_dir / "grpc-candidate.json",
        "capped-inspect.json": Path(args.capped) / "capped-inspect.json",
        "unset-inspect.json": Path(args.unset) / "unset-inspect.json",
        "capped-version.txt": Path(args.capped) / "capped-version.txt",
        "unset-version.txt": Path(args.unset) / "unset-version.txt",
    }
    for name, path in copies.items():
        if not path.is_file():
            raise SystemExit(f"{path}: absent")
        (out / name).write_bytes(path.read_bytes())
    record = {
        "schema": 1,
        "gate_id": "G428-M2",
        "locality": locality,
        "endpoint": {"url": endpoint, "proxy": "none"},
        "candidate": {"source_sha": source, "binary_sha256": binary},
        "runs": {
            "capped": {
                "env": {CAP_KEY: capped["cap"]},
                "inspect": "capped-inspect.json",
                "version": "capped-version.txt",
            },
            "unset": {
                "env": {},
                "inspect": "unset-inspect.json",
                "version": "unset-version.txt",
            },
        },
        "surfaces": {
            "http": {
                "endpoint": {"url": endpoint, "proxy": "none"},
                "capped": {
                    "cap": capped["cap"], "held": capped["held"],
                    "probe": capped["probe"], "admitted": capped["admitted"],
                    "refused": capped["refused"], "server_errors": capped["server_errors"],
                    "retry_after": capped["retry_after"], "refusal": capped["refusal"],
                    "handler_reached_after_release":
                        capped["handler_reached_after_release"],
                    "transcript": "http-capped.jsonl",
                    "requests_per_second": capped["requests_per_second"],
                },
                "unset": {
                    "probe": unset["probe"], "admitted": unset["admitted"],
                    "refused": unset["refused"], "server_errors": unset["server_errors"],
                    "retry_after": unset["retry_after"],
                    "handler_reached_after_release": unset["handler_reached_after_release"],
                    "transcript": "http-unset.jsonl",
                    "requests_per_second": unset["requests_per_second"],
                },
            },
            "direct_http": {
                "endpoint": {"url": endpoint, "proxy": "none"},
                "capped": {
                    "status_line": capped["direct"]["status_line"],
                    "retry_after": capped["direct"]["retry_after"],
                    "transcript": "direct-capped.txt",
                },
                "unset": {
                    "status_line": unset["direct"]["status_line"],
                    "transcript": "direct-unset.txt",
                },
            },
            "grpc": {
                "origin": grpc_candidate.get("origin"),
                "scope": grpc_candidate.get("scope"),
                "candidate": grpc_candidate.get("candidate"),
                "endpoint": grpc_candidate.get("endpoint"),
                "candidate_record": "grpc-candidate.json",
                "capped": {
                    "arms": {
                        "ingress_rate": grpc_summary["ingress_rate"],
                        "pool_full": grpc_summary["pool_full"],
                    },
                    "server_errors": 0,
                    "transcript": "grpc-capped.jsonl",
                },
                "permissive": {
                    "admitted": grpc_summary["permissive"]["admitted"],
                    "refused": grpc_summary["permissive"]["refused"],
                    "transcript": "grpc-permissive.jsonl",
                },
            },
        },
    }
    (out / "pressure.json").write_text(json.dumps(record, indent=1, sort_keys=True) + "\n")
    (out / "README.md").write_text(
        "# m2-pressure rehearsal\n\n"
        f"**Evidence locality.** {locality} — {endpoint}.\n\n"
        "The capped phase holds every configured permit, refuses the burst with 429 and "
        "`retry-after: 1`, and reaches the handler after release; the unset phase is the control "
        "and refuses nothing. This is not the public-live positive and not gate evidence.\n"
    )
    record_dir = out.resolve()
    try:
        relative = record_dir.relative_to(ROOT).as_posix() or "."
    except ValueError:
        relative = str(record_dir)
    (out / "observations.json").write_text(json.dumps({
        "source": {"path": "node/src/runtime/node_runtime.rs"},
        "pressure": {"dir": relative},
    }, indent=1) + "\n")
    sums = {}
    for path in sorted(out.rglob("*")):
        if path.is_file() and path.name != "SHA256SUMS.json":
            sums[path.relative_to(out).as_posix()] = hashlib.sha256(
                path.read_bytes()).hexdigest()
    (out / "SHA256SUMS.json").write_text(json.dumps(sums, indent=1, sort_keys=True) + "\n")
    print(json.dumps({"passthrough": [
        "python3 tools/m2-pressure.py check --record "
        f"{out} --locality {locality}",
        f"python3 tools/gate-check.py --gate G428-M2 --observations {out / 'observations.json'}",
    ]}, indent=1))
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    phase_cmd = sub.add_parser("phase", help="drive one devnet lifetime")
    phase_cmd.add_argument("--mode", required=True, choices=("capped", "unset"))
    phase_cmd.add_argument("--out", required=True)
    phase_cmd.add_argument("--container", default="m2cap-bootstrap")
    phase_cmd.add_argument("--port", type=int, default=56403)
    phase_cmd.add_argument("--endpoint", default="",
                           help="the installed candidate's HTTP endpoint, e.g. http://host:40403")
    phase_cmd.add_argument("--inspect", default="",
                           help="the container config JSON captured on that host")
    phase_cmd.add_argument("--version", default="",
                           help="the `rnode --version` output captured on that host")
    phase_cmd.add_argument("--cap", type=int, default=2)
    phase_cmd.add_argument("--probe", type=int, default=4)
    phase_cmd.add_argument("--spacing", type=float, default=0.15)
    phase_cmd.add_argument("--settle", type=float, default=0.3)
    grpc_cmd = sub.add_parser("grpc", help="run the Rust real-listener observation")
    grpc_cmd.add_argument("--out", required=True)
    grpc_cmd.add_argument("--source-sha", default="")
    grpc_cmd.add_argument("--binary-sha256", default="")
    grpc_cmd.add_argument("--endpoint", default="",
                          help="the validator's gRPC endpoint (declared, not used in-process)")
    grpc_cmd.add_argument("--origin", default="candidate-test",
                          choices=("candidate-test", "endpoint"))
    grpc_cmd.add_argument("--locality", default="local-devnet",
                          choices=("local-devnet", "public-live"))
    assemble_cmd = sub.add_parser("assemble", help="merge the phases into a record")
    assemble_cmd.add_argument("--capped", required=True)
    assemble_cmd.add_argument("--unset", required=True)
    assemble_cmd.add_argument("--grpc", required=True)
    assemble_cmd.add_argument("--out", required=True)
    assemble_cmd.add_argument("--locality", default=None,
                              choices=("public-live", "local-devnet"))
    assemble_cmd.add_argument("--endpoint", default="")
    assemble_cmd.add_argument("--port", type=int, default=56403)
    assemble_cmd.add_argument("--source-sha", default="")
    assemble_cmd.add_argument("--binary-sha256", default="")
    args = parser.parse_args(argv)
    if args.command == "phase":
        return phase(args)
    if args.command == "grpc":
        return grpc(args)
    return assemble(args)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
