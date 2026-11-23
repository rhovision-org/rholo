#!/usr/bin/env python3
"""RC-5 — overload refusal: a configured bound refuses explicitly and loses nothing.

Test: `python3 tools/release-check.py run --claim RC-5 --env local`

Two devnet lifetimes (capped and unset) drive the M2 pressure arm; the Rust gRPC observation is the
third input; the resulting record is judged by the existing `tools/m2-pressure.py::check_pressure`.
"""
from __future__ import annotations

import argparse
import hashlib
import http.client
import importlib.util
import json
import re
import socket
import socketserver
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location(
    "release_claim_common", ROOT / "tests" / "release_claim_common.py")
common = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(common)

CLAIM_ID = "RC-5"
TITLE = "Overload refusal"


def _fixture_baseline(arm: str = "local") -> dict:
    environment = "public" if arm == "public" else "local"
    return {
        "schema": 1, "claim_id": CLAIM_ID, "claim_title": TITLE, "arm": arm,
        "environment": environment,
        "locality": "public-live" if environment == "public" else "local-devnet",
        "provenance": {"source_sha": "0" * 40, "binary_sha256": "1" * 64},
        "preconditions": [], "verdict": "pass", "reasons": [], "errors": [],
        "harness": {"name": "m2-pressure", "exit_code": 0},
        "observations": {
            "check_exit": 0, "verdict": "PASS",
            "pressure": "record/pressure.json", "not_processed": [],
            "admission_traces": [{
                "kind": "admission", "site": "http-inflight", "surface": "http",
                "refused": True, "class": "inflight", "status": 429, "retry_after": 1,
                "inflight": 2, "inflight_limit": 2,
            }],
        },
    }


def _check_baseline(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    problems: list[str] = []
    observations = record.get("observations")
    if not isinstance(observations, dict):
        return "fail", ["observations: missing or not an object"]
    if observations.get("verdict") != "PASS":
        problems.append(f"check_pressure returned {observations.get('verdict')!r}")
    if observations.get("check_exit") != 0:
        problems.append(f"the pressure check exited {observations.get('check_exit')}")
    if observations.get("not_processed"):
        problems.append(f"accepted deploy(s) not resolved: {observations['not_processed']}")
    problems.extend(common.phase_space_reading(observations, bool(problems)))
    return common.evaluate(record, problems)


# --------------------------------------------------------------------------------------
# The transport arm. One signed deploy, four submission surfaces, four nodes.
#
# A node refuses a byte-identical deploy already in its pool, so the same signed bytes must be
# accepted once per node. The gRPC node signs once (RFC-6979); the other three receive the exact
# bytes reconstructed from the node's own PooledDeploy. The status and settlement stage are read
# on the node that received that surface's submission.
# --------------------------------------------------------------------------------------

TRANSPORT_SURFACES = ("grpc", "http", "direct-http", "proxy")


def _transport_devnet(prefix: str, http: int, admin: int, grpc: int) -> common.Devnet:
    # A lone validator's first block carries the deploy but no attestation; autopropose's tap is the
    # carrier that closes the round (`node/tests/deploy_lifespan.rs`, the one-flag control). The
    # deploy is signed with a future `validAfter`, so autopropose cannot consume it before the pool
    # read; propose-on-deploy stays off so it is not included at admission either.
    return common.Devnet(
        prefix, validators=1, http_base=http, admin_base=admin, grpc_base=grpc,
        memory="2g", extra_up=("--autopropose", "--no-propose-on-deploy"))


def _pooled_deploy(devnet: common.Devnet, deploy_id: str) -> dict:
    """Read the signed deploy back from the node's own pool.

    `PooledDeploy` is the recovery-tool view (#564): the term, timestamp, phlo, validity, shard,
    deployer, signature and attachments that were signed. Reconstructing the request from anything
    else would risk one field of drift and a different id.
    """
    url = devnet.nodes()[0]["http"] + "/api/v1/deploys"

    def fetch():
        status, body = common.http_json(url, timeout=5)
        if status != 200 or not isinstance(body, dict):
            return None
        for row in body.get("deploys") or []:
            if isinstance(row, dict) and row.get("deployId") == deploy_id:
                return row
        return None

    return common.wait_until("the signed deploy is pooled", fetch, timeout=60)


def _deploy_request(pooled: dict) -> dict:
    """The exact `DeployRequest` the signed bytes describe. No crypto is reimplemented here."""
    data = {
        "term": pooled["term"],
        "timestamp": pooled["timestamp"],
        "phloPrice": pooled["phloPrice"],
        "phloLimit": pooled["phloLimit"],
        "validAfterBlockNumber": pooled["validAfterBlockNumber"],
        "shardId": pooled["shardId"],
    }
    if pooled.get("attachments"):
        data["attachments"] = pooled["attachments"]
    return {
        "data": data,
        "deployer": pooled["deployer"],
        "signature": pooled["signature"],
        "sigAlgorithm": pooled["sigAlgorithm"],
    }


def _response_id(body) -> str:
    if isinstance(body, bytes):
        text = body.decode(errors="replace")
    elif isinstance(body, str):
        text = body
    else:
        text = json.dumps(body)
    try:
        parsed = json.loads(text)
        if isinstance(parsed, str):
            text = parsed
    except ValueError:
        pass
    match = re.search(r"DeployId is: ([0-9a-fA-F]+)", text)
    if not match:
        raise common.ClaimError(f"the deploy response names no id: {text[-400:]}")
    return match.group(1)


def _post_bytes(port: int, body: bytes) -> tuple[int, bytes]:
    """POST the exact bytes through an HTTP client; the body is never re-serialised."""
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
    try:
        connection.request(
            "POST", "/api/v1/deploy", body=body,
            headers={"Content-Type": "application/json", "Connection": "close"})
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def _raw_post_bytes(port: int, body: bytes) -> tuple[int, bytes]:
    """The same bytes over a raw TCP socket, with no HTTP client library in between."""
    head = (
        f"POST /api/v1/deploy HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
        "Content-Type: application/json\r\n"
        f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n"
    ).encode()
    with socket.create_connection(("127.0.0.1", port), timeout=60) as connection:
        connection.sendall(head + body)
        received = b""
        while b"\r\n\r\n" not in received:
            chunk = connection.recv(65536)
            if not chunk:
                break
            received += chunk
        header, _, rest = received.partition(b"\r\n\r\n")
        match = re.search(rb"(?i)content-length: *(\d+)", header)
        if match:
            length = int(match.group(1))
            while len(rest) < length:
                chunk = connection.recv(65536)
                if not chunk:
                    break
                rest += chunk
            rest = rest[:length]
        else:
            while True:
                chunk = connection.recv(65536)
                if not chunk:
                    break
                rest += chunk
    status_line = header.split(b"\r\n", 1)[0].decode(errors="replace")
    match = re.search(r"HTTP/1\.[01] (\d{3})", status_line)
    return (int(match.group(1)) if match else 0), rest


class _ProxyHandler(socketserver.BaseRequestHandler):
    """One connection, forwarded byte-for-byte, with both streams captured."""

    def handle(self) -> None:
        server = self.server
        captured = {"request": b"", "response": b""}
        upstream = socket.create_connection(("127.0.0.1", server.target_port), timeout=60)

        def pump(source, destination, key: str) -> None:
            try:
                while True:
                    data = source.recv(65536)
                    if not data:
                        break
                    captured[key] += data
                    destination.sendall(data)
            except OSError:
                pass
            finally:
                try:
                    destination.shutdown(socket.SHUT_WR)
                except OSError:
                    pass

        forward = threading.Thread(
            target=pump, args=(self.request, upstream, "request"), daemon=True)
        forward.start()
        try:
            pump(upstream, self.request, "response")
        finally:
            forward.join(timeout=10)
            upstream.close()
            server.captured = captured


class _TransportProxy(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, target_port: int):
        super().__init__(("127.0.0.1", 0), _ProxyHandler)
        self.target_port = target_port
        self.captured = None


def _start_proxy(target_port: int) -> tuple[_TransportProxy, int]:
    """A transparent TCP relay in front of the node's HTTP listener. It rewrites nothing.

    This is a TCP forwarder, not an HTTP proxy: it does not exercise CONNECT, absolute-form
    targets, chunked re-framing or proxy-generated errors. The record says so.
    """
    server = _TransportProxy(target_port)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, server.server_address[1]


def _submit_bytes(port: int, body: bytes, surface: str) -> str:
    status, response = _post_bytes(port, body)
    if status != 200:
        raise common.ClaimError(f"{surface} deploy returned {status}: {response[-400:]!r}")
    return _response_id(response)


def _submit_raw(port: int, body: bytes) -> str:
    status, response = _raw_post_bytes(port, body)
    if status != 200:
        raise common.ClaimError(f"direct-http deploy returned {status}: {response[-400:]!r}")
    return _response_id(response)


def _tampered(request: dict) -> dict:
    bad = json.loads(json.dumps(request))
    signature = bad["signature"]
    bad["signature"] = ("1" if signature[0] != "1" else "2") + signature[1:]
    return bad


def _captured_body(raw: bytes) -> bytes:
    """The body of a captured HTTP/1.1 request or response, by its Content-Length."""
    header, _, rest = raw.partition(b"\r\n\r\n")
    match = re.search(rb"(?i)content-length: *(\d+)", header)
    if match:
        return rest[:int(match.group(1))]
    return rest


def _settle(devnet: common.Devnet, deploy_id: str) -> dict:
    """Wait for autopropose to include the future deploy and its attestation to finalise it.

    The deploy is signed with a future `validAfter`, so it is pooled and unincludable until the
    chain reaches that height; autopropose then includes it and the next tick carries the
    attestation. `node/tests/deploy_lifespan.rs` is the one-flag control: a lone validator without
    autopropose never finalises.
    """
    url = devnet.nodes()[0]["http"] + f"/api/v1/deploy-settlement/{deploy_id}"

    def ready():
        status, body = common.http_json(url, timeout=5)
        if status != 200 or not isinstance(body, dict):
            return None
        if body.get("stage") in ("finalizedSuccess", "finalizedError", "notProcessed"):
            return body
        return None

    return common.wait_until("the deploy settles", ready, timeout=180)


def _environment(devnet: common.Devnet, valid_after: int) -> dict:
    """Assert the fresh devnet is the same deploy environment as the signing node."""
    status = devnet.target().status(0)
    shard = status.get("shardId")
    latest = status.get("latestBlockNumber")
    if shard != "/root":
        raise common.ClaimError(
            f"precondition: the node's shard is {shard!r}, not '/root'")
    if not isinstance(latest, int):
        raise common.ClaimError(f"precondition: the node reports no block height")
    if latest >= valid_after + 50:
        raise common.ClaimError(
            f"precondition: the deploy's validity window has expired "
            f"(latest {latest}, validAfter {valid_after})")
    return {"shard_id": shard, "latest_block_number": latest,
            "valid_after_block_number": valid_after,
            "future_at_start": latest < valid_after}


def _status_variant(devnet: common.Devnet, deploy_id: str) -> str:
    url = devnet.nodes()[0]["http"] + f"/api/v1/deploy-status/{deploy_id}"
    status, body = common.http_json(url, timeout=10)
    if status != 200:
        return f"HTTP {status}"
    if isinstance(body, dict):
        if len(body) == 1:
            return next(iter(body))
        return str(body.get("status") or body.get("deployStatus") or body)
    return str(body)


def _status_from_text(text: str) -> str | None:
    """The exec-status variant in the CLI's JSON, normalised to the HTTP spelling."""
    match = re.search(
        r'"(processedWithSuccess|processedWithError|notProcessed|'
        r'ProcessedWithSuccess|ProcessedWithError|NotProcessed)"', text)
    if not match:
        return None
    return {
        "processedwithsuccess": "ProcessedWithSuccess",
        "processedwitherror": "ProcessedWithError",
        "notprocessed": "NotProcessed",
    }[match.group(1).lower()]


def _grpc_status(devnet: common.Devnet, deploy_id: str) -> tuple[str, str]:
    """The gRPC `deployStatus` read through the CLI. A rendering failure falls back to HTTP."""
    result = subprocess.run(
        ["docker", "exec", devnet.node(0), "rnode", "--grpc-host", "localhost",
         "deploy-status", "--deploy-signature", deploy_id],
        text=True, capture_output=True, timeout=60)
    status = _status_from_text(result.stdout + result.stderr)
    if result.returncode == 0 and status is not None:
        return status, "grpc"
    return _status_variant(devnet, deploy_id), "http-fallback"


def _transport_row(surface: str, devnet: common.Devnet, deploy_id: str) -> dict:
    settlement = _settle(devnet, deploy_id)
    if surface == "grpc":
        status, status_source = _grpc_status(devnet, deploy_id)
    else:
        status, status_source = _status_variant(devnet, deploy_id), "http"
    return {
        "surface": surface,
        "deploy_id": deploy_id,
        "status": status,
        "settlement_stage": settlement.get("stage"),
        "settlement_confirmed": settlement.get("settlementConfirmed"),
        "error_class": None,
        "status_source": status_source,
        "block": {"blockNumber": settlement.get("blockNumber"),
                  "blockHash": settlement.get("blockHash")},
    }


def _run_transport(record: dict, out: Path, keep: bool) -> int:
    rows: list[dict] = []
    devnets: list[common.Devnet] = []
    proxy = None
    deploy_id = None
    request = None
    environment = {}
    negative = None
    try:
        stamp = str(time.time_ns())
        term = f'@{json.dumps("release-claim-transport:" + stamp)}!({common.MARKER_VALUE})'

        grpc = _transport_devnet("rc5t-grpc", 57403, 57405, 57402)
        devnets.append(grpc)
        grpc.up()
        record["provenance"] = grpc.provenance()
        version = subprocess.check_output(
            ["docker", "exec", grpc.node(0), "rnode", "--version"],
            text=True, stderr=subprocess.DEVNULL).strip()
        match = re.search(r"\(([0-9a-f]{40})\)", version)
        record["provenance"]["binary_source_sha"] = match.group(1) if match else None
        record["provenance"]["binary_version"] = version
        latest = int(grpc.target().status(0)["latestBlockNumber"])
        valid_after = latest + 5
        deploy_id = grpc.deploy(term, valid_after=valid_after)
        pooled = _pooled_deploy(grpc, deploy_id)
        request = _deploy_request(pooled)
        body = json.dumps(request, separators=(",", ":")).encode()
        environment = _environment(grpc, pooled["validAfterBlockNumber"])
        rows.append(_transport_row("grpc", grpc, deploy_id))

        http = _transport_devnet("rc5t-http", 58403, 58405, 58402)
        devnets.append(http)
        http.up()
        _environment(http, pooled["validAfterBlockNumber"])
        # A tampered signature must be refused, not echoed: the surface validates the deploy.
        bad_body = json.dumps(_tampered(request), separators=(",", ":")).encode()
        bad_status, bad_response = _post_bytes(http.http_base, bad_body)
        if bad_status != 400:
            raise common.ClaimError(
                f"the tampered deploy was not refused with 400: {bad_status} "
                f"{bad_response[-400:]!r}")
        negative = {"surface": "http", "status": bad_status,
                    "body": bad_response.decode(errors="replace")[-400:]}
        got = _submit_bytes(http.http_base, body, "http")
        rows.append(_transport_row("http", http, got))

        direct = _transport_devnet("rc5t-direct", 59403, 59405, 59402)
        devnets.append(direct)
        direct.up()
        _environment(direct, pooled["validAfterBlockNumber"])
        got = _submit_raw(direct.http_base, body)
        rows.append(_transport_row("direct-http", direct, got))

        proxied = _transport_devnet("rc5t-proxy", 60403, 60405, 60402)
        devnets.append(proxied)
        proxied.up()
        _environment(proxied, pooled["validAfterBlockNumber"])
        proxy, proxy_port = _start_proxy(proxied.http_base)
        got = _submit_bytes(proxy_port, body, "proxy")
        common.wait_until("the proxy transcript is captured",
                          lambda: proxy.captured is not None, timeout=15)
        transcript = proxy.captured or {"request": b"", "response": b""}
        forwarded = _captured_body(transcript["request"])
        if forwarded != body:
            raise common.ClaimError(
                "the proxy changed the request body: "
                f"{hashlib.sha256(forwarded).hexdigest()[:16]} != "
                f"{hashlib.sha256(body).hexdigest()[:16]}")
        rows.append(_transport_row("proxy", proxied, got))

        record["observations"]["transport"] = {
            "deploy_id": deploy_id,
            "request_body_sha256": hashlib.sha256(body).hexdigest(),
            "deploy_request": request,
            "environment_checks": environment,
            "negative_control": negative,
            "surfaces": rows,
            "proxy_transcript": {
                "request_sha256": hashlib.sha256(transcript["request"]).hexdigest(),
                "response_sha256": hashlib.sha256(transcript["response"]).hexdigest(),
            },
            "note": "four fresh single-node devnets; the invariant is the deploy id and the "
                    "node's status/stage, not block equality across chains",
        }
        record["observations"]["admission_traces"] = []
        record["harness"] = {"name": "rc5-transport", "out": str(out)}
        verdict, reasons = check(record)
        common.finish(record, verdict, reasons)
        return common.write_record(out, record)[1]
    except Exception as error:  # noqa: BLE001 - the failure is the observation
        record["observations"]["transport"] = {
            "deploy_id": deploy_id,
            "deploy_request": request,
            "environment_checks": environment,
            "negative_control": negative,
            "surfaces": rows,
        }
        record["observations"].setdefault("admission_traces", [])
        record["harness"] = {"name": "rc5-transport", "out": str(out)}
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if proxy is not None:
            try:
                proxy.shutdown()
                proxy.server_close()
            except Exception:  # noqa: BLE001
                pass
        if not keep:
            for devnet in reversed(devnets):
                devnet.down()


FIXTURES = {
    "local": lambda: _fixture_baseline("local"),
    "public": lambda: _fixture_baseline("public"),
    "transport": lambda: common.transport_fixture(CLAIM_ID, TITLE),
}
ARMS = {
    "local": _check_baseline,
    "public": _check_baseline,
    "transport": common.check_transport,
}


def fixture(arm: str = "local") -> dict:
    return FIXTURES[arm]()


def check(record: dict) -> tuple[str, list[str]]:
    if record.get("verdict") == "not run":
        return "not run", list(record.get("preconditions") or record.get("reasons") or [])
    return common.dispatch_arm(ARMS, record)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", choices=("local", "public"))
    parser.add_argument("--arm")
    parser.add_argument("--out", required=True)
    parser.add_argument("--public-manifest")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args(argv)
    arm = args.arm or args.env or "local"
    meta = common.claim_arm(CLAIM_ID, arm)
    environment = meta["environment"]
    record = common.new_record(CLAIM_ID, TITLE, environment, meta["locality"], arm=arm)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    failure = common.preflight_failure(CLAIM_ID, environment, out, record,
                                       getattr(args, "public_manifest", None))
    if failure is not None:
        return failure

    if arm == "transport":
        return _run_transport(record, out, args.keep)
    if arm not in ("local", "public"):
        return common.not_run(
            out, record,
            f"the {arm} arm's harness lands in {meta['next_pr']}")[1]

    if environment == "public":
        manifest = common.load_manifest(args.public_manifest)
        if not manifest:
            return common.not_run(out, record, "no --public-manifest for the public fleet")[1]
        record["provenance"] = manifest.get("provenance") or {}
        supplied = manifest.get("pressure_record")
        if not supplied:
            return common.not_run(
                out, record,
                "the public run needs the operator's capped/unset pressure record")[1]
        return _judge(Path(supplied), record, out, "public-live")

    capped = common.Devnet("m2cap", validators=1, http_base=56403, admin_base=56405,
                           grpc_base=56402, network="m2net", memory="2g",
                           node_env="RNODE_HTTP_MAX_INFLIGHT_DEPLOY=2")
    unset = common.Devnet("m2cap", validators=1, http_base=56403, admin_base=56405,
                          grpc_base=56402, network="m2net", memory="2g")
    arm = ROOT / "tools" / "m2-pressure-arm.py"
    try:
        record["provenance"] = {}
        capped.up()
        record["provenance"] = capped.provenance()
        # The criterion's candidate is the binary's own source pin, not the checkout's HEAD:
        # `rnode --version` names the commit the image was built from, and an evidence-only
        # commit can sit on top of it.
        version = subprocess.check_output(
            ["docker", "exec", "m2cap-bootstrap", "rnode", "--version"],
            text=True, stderr=subprocess.DEVNULL).strip()
        match = re.search(r"\(([0-9a-f]{40})\)", version)
        record["provenance"]["binary_source_sha"] = match.group(1) if match else None
        record["provenance"]["binary_version"] = version
        _run([sys.executable, str(arm), "phase", "--mode", "capped",
              "--out", str(out / "capped"), "--container", "m2cap-bootstrap",
              "--port", "56403", "--cap", "2"], record, out / "capped.log")
        _collect_admission_traces(capped, out / "traces" / "capped", record)
        capped.down()
        unset.up()
        _run([sys.executable, str(arm), "phase", "--mode", "unset",
              "--out", str(out / "unset"), "--container", "m2cap-bootstrap",
              "--port", "56403"], record, out / "unset.log")
        _collect_admission_traces(unset, out / "traces" / "unset", record)
        unset.down()
        _run([sys.executable, str(arm), "grpc", "--out", str(out / "grpc")],
             record, out / "grpc.log")
        _run([sys.executable, str(arm), "assemble",
              "--capped", str(out / "capped"), "--unset", str(out / "unset"),
              "--grpc", str(out / "grpc"), "--out", str(out / "record"),
              "--locality", "local-devnet", "--port", "56403",
              "--source-sha", record["provenance"].get("binary_source_sha")
              or record["provenance"].get("source_sha") or "",
              "--binary-sha256", record["provenance"].get("binary_sha256") or ""],
             record, out / "assemble.log")
        return _judge(out / "record", record, out, "local-devnet")
    except Exception as error:  # noqa: BLE001
        common.finish(record, "fail", [f"{type(error).__name__}: {error}"])
        return common.write_record(out, record)[1]
    finally:
        if not args.keep:
            capped.down()
            unset.down()


def _run(command: list[str], record: dict, log: Path) -> None:
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=1800)
    log.write_text(result.stdout + result.stderr)
    if result.returncode:
        raise common.ClaimError(
            f"{Path(command[1]).name} {' '.join(command[2:4])} failed ({result.returncode}): "
            f"{(result.stdout + result.stderr)[-1200:]}")


def _judge(record_dir: Path, record: dict, out: Path, locality: str) -> int:
    check_command = [sys.executable, str(ROOT / "tools" / "m2-pressure.py"),
                     "check", "--record", str(record_dir), "--locality", locality]
    result = subprocess.run(check_command, cwd=ROOT, text=True, capture_output=True, timeout=600)
    (out / "check.log").write_text(result.stdout + result.stderr)
    pressure = _read_json(record_dir / "pressure.json") or {}
    observations = record["observations"]
    observations.update({
        "check_exit": result.returncode,
        "verdict": (pressure.get("verdict") if isinstance(pressure, dict) else None)
        or ("PASS" if result.returncode == 0 else "FAIL"),
        "pressure": str(record_dir / "pressure.json"),
        "not_processed": [row for row in (pressure.get("accepted") or [])
                          if isinstance(row, dict) and row.get("status") == "NotProcessed"],
        "check_output": (result.stdout + result.stderr)[-2000:],
    })
    record["harness"] = {"name": "m2-pressure", "command": check_command,
                         "exit_code": result.returncode, "out": str(record_dir)}
    verdict, reasons = check(record)
    common.finish(record, verdict, reasons)
    return common.write_record(out, record)[1]


def _collect_admission_traces(devnet, dest: Path, record: dict) -> None:
    """Copy the node's admission traces before its container is torn down."""
    try:
        devnet.copy_traces(0, dest)
    except Exception:  # noqa: BLE001 - a missing trace is evidence-insufficient, not a crash
        return
    rows = record["observations"].setdefault("admission_traces", [])
    for path in sorted(dest.glob("admission-*.json")):
        try:
            row = json.loads(path.read_text())
        except ValueError:
            continue
        rows.append({"file": path.name, **row})


def _read_json(path: Path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
