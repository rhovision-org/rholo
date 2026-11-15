#!/usr/bin/env python3
"""Measure C339's mature-chain RecoveryReplay: a wiped validator rejoining a chain of at least
`--target-height` blocks, the restart control, and the retained numbers.

Why this exists. `spec/audit/evidence/n479-native-join-20261029/` proves exact-source
`RecoveryReplay`, restart and wrong-pin controls on a 756-record chain, and names the one gap it
leaves: cold recovery of the longer 2,436-height history is unqualified. C339's `owes` cell closes
on that measurement. This tool stages the same shape on a disposable local devnet:

  1. `tools/devnet.sh up --validators 4 --fresh --autopropose --admin` and wait for the chain;
  2. `tools/devnet.sh reset 3` — validator 3's volume is emptied and it restarts, so it must take
     the LFS/`RecoveryReplay` path rather than rebuild a stored chain;
  3. time the restore to the bootstrap's finalized anchor and to `ready`;
  4. `stop 3` / `start 3` as the restart control (the store now exists, so no re-download);
  5. retain the validator's log, the bootstrap's health, the pins and the timings.

The tool decides nothing. It writes `raw/result.json` and the logs; the operator authors the
evidence README and chooses PASS/INCONCLUSIVE. `execution_proven` is whatever `evidence-pack.py`
says it is (always false).

Usage:
  python3 tools/c339_mature_replay.py --out target/c339-mature-replay --target-height 2436
  python3 tools/c339_mature_replay.py --skip-up --skip-chain --out target/c339-mature-replay
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

HTTP_BASE = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
ADMIN_BASE = int(os.environ.get("DEVNET_ADMIN_BASE", "40405"))
PREFIX = os.environ.get("DEVNET_PREFIX", "devnet")
BOOTSTRAP = f"{PREFIX}-bootstrap"


def utcnow() -> datetime.datetime:
    return datetime.datetime.now(datetime.timezone.utc)


def parse_started_at(raw: str) -> datetime.datetime:
    """Docker's `StartedAt` is UTC with a `Z` suffix; the clock origin is the container's start."""
    return datetime.datetime.strptime(raw[:19], "%Y-%m-%dT%H:%M:%S").replace(
        tzinfo=datetime.timezone.utc
    )


def restore_elapsed(started_at: datetime.datetime, ended_at: datetime.datetime) -> float:
    return max(0.0, (ended_at - started_at).total_seconds())


def measure_restore(started_at, readiness_probe, clock=utcnow) -> float:
    """**The clock stops after readiness, never before it.**

    The audit found the old code sampled the end time *before* waiting for `ready` and then waited,
    so a 20-second readiness delay was recorded as 0. The probe must complete before the end sample;
    the signature makes that ordering structural rather than a comment.
    """
    readiness_probe()
    return restore_elapsed(started_at, clock())


def http_post_json(port: int, path: str, payload, timeout: float = 15.0):
    url = f"http://127.0.0.1:{port}{path}"
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        body = response.read()
    if not body:
        return None
    try:
        return json.loads(body)
    except json.JSONDecodeError:
        return {"raw": body.decode(errors="replace")}


def try_http_json(port: int, path: str):
    try:
        return http_json(port, path)
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError) as exc:
        print(f"c339: GET {path} did not answer: {exc}", file=sys.stderr)
        return None


def measure_post_join(joiner_port: int, before: dict) -> dict:
    """**A join that stops at the anchor is not a join.**

    Submit a deploy to the bootstrap, force a block through the admin propose route, and require the
    joiner to observe the new height and then finalize it. Instrument failures are recorded as an
    error, which the acceptance treats as INCONCLUSIVE rather than PASS.
    """
    before_height = before.get("maxObservedHeight")
    before_finalized = before.get("finalizedHeight")
    if before_height is None or before_finalized is None:
        return {"error": "the joiner's pre-join height or finality is unknown"}
    try:
        deploy = http_post_json(
            HTTP_BASE, "/api/v1/explore-deploy", "new x in { x!(1) }"
        )
        propose = http_post_json(ADMIN_BASE, "/api/propose", None)
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError) as exc:
        return {"error": f"the post-join deploy/propose failed: {exc}"}
    try:
        advanced = wait_for(
            joiner_port,
            lambda body: body.get("maxObservedHeight", -1) > before_height,
            120.0,
            5.0,
            "post-join block observed",
        )
        finalized = wait_for(
            joiner_port,
            lambda body: body.get("finalizedHeight", -1) > before_finalized,
            180.0,
            5.0,
            "post-join finality",
        )
    except TimeoutError as exc:
        return {"error": str(exc), "deploy": deploy, "propose": propose}
    # **The proposed block itself must be proven finalized.** A height increase can come from any
    # block; the claim is that *this* block entered the finalized closure. The propose response names
    # it; `/api/is-finalized/{hash}` answers for it.
    proposed = _propose_hash(propose)
    if not proposed:
        return {
            "error": "the propose response did not name a block hash",
            "deploy": deploy,
            "propose": propose,
        }
    observed_false = False
    deadline = time.monotonic() + 180.0
    while time.monotonic() < deadline:
        verdict = _is_finalized(joiner_port, proposed)
        if verdict is True:
            return {
                "height_advanced": True,
                "finality_advanced": True,
                "observed_height": advanced.get("maxObservedHeight"),
                "finalized_height": finalized.get("finalizedHeight"),
                "proposed_block_hash": proposed,
                "proposed_block_finalized": True,
                "deploy": deploy,
                "propose": propose,
            }
        if verdict is False:
            observed_false = True
        time.sleep(5.0)
    if observed_false:
        return {
            "height_advanced": True,
            "finality_advanced": True,
            "observed_height": advanced.get("maxObservedHeight"),
            "finalized_height": finalized.get("finalizedHeight"),
            "proposed_block_hash": proposed,
            "proposed_block_finalized": False,
            "deploy": deploy,
            "propose": propose,
        }
    return {
        "error": f"the joiner's is-finalized route did not answer for {proposed}",
        "proposed_block_hash": proposed,
        "deploy": deploy,
        "propose": propose,
    }


def _propose_hash(propose) -> str | None:
    """The block hash the propose response names, from a string or `{"raw": ...}` answer."""
    if isinstance(propose, dict):
        text = propose.get("raw") or propose.get("result") or json.dumps(propose)
    else:
        text = "" if propose is None else str(propose)
    match = re.search(r"([0-9a-fA-F]{64})", text)
    return match.group(1).lower() if match else None


def _is_finalized(port: int, block_hash: str) -> bool | None:
    """`GET /api/is-finalized/{hash}`: True/False, or None when the route did not answer."""
    try:
        body = http_json(port, f"/api/is-finalized/{block_hash}")
    except (
        urllib.error.URLError,
        TimeoutError,
        json.JSONDecodeError,
        ConnectionError,
        ValueError,
    ):
        return None
    if isinstance(body, bool):
        return body
    if isinstance(body, dict):
        for key in ("finalized", "isFinalized", "value"):
            if key in body:
                return bool(body[key])
    return None


def _lp(value: bytes) -> bytes:
    return struct.pack(">I", len(value)) + value


def checkpoint_digest_hex(checkpoint: dict) -> str:
    """The statement digest, mirroring `rchain-casper::checkpoint::checkpoint_digest`.

    This is a second implementation on purpose: the benchmark compares the certificate a joiner
    serves with the one the bootstrap serves, and the golden test against the Rust fixture pins the
    two encodings together. A drift is a benchmark failure, not a silent disagreement.
    """
    buffer = _lp(b"rchain-checkpoint-v1")
    buffer += _lp(checkpoint["shardId"].encode())
    buffer += bytes.fromhex(checkpoint["anchor"])
    buffer += struct.pack(">q", checkpoint["anchorHeight"])
    buffer += bytes.fromhex(checkpoint["anchorPostState"])
    parent = checkpoint.get("parentDigest")
    buffer += bytes.fromhex(parent) if parent else bytes(32)
    buffer += struct.pack(">q", checkpoint["epoch"])
    fringe = sorted(bytes.fromhex(item) for item in checkpoint["fringeHashes"])
    buffer += struct.pack(">I", len(fringe)) + b"".join(fringe)
    metadata = checkpoint.get("metadataRoot")
    history = checkpoint.get("historyRoot")
    presence = (1 if metadata is not None else 0) | (2 if history is not None else 0)
    if presence:
        buffer += bytes([presence])
        if metadata is not None:
            buffer += _lp(b"rchain-checkpoint-metadata-root-v1") + bytes.fromhex(metadata)
        if history is not None:
            buffer += _lp(b"rchain-checkpoint-history-root-v1") + bytes.fromhex(history)
    return hashlib.blake2b(buffer, digest_size=32).hexdigest()


def certificate_fields(certificate) -> dict | None:
    if not isinstance(certificate, dict) or not isinstance(
        certificate.get("checkpoint"), dict
    ):
        return None
    checkpoint = certificate["checkpoint"]
    try:
        digest = checkpoint_digest_hex(checkpoint)
    except (KeyError, TypeError, ValueError):
        digest = None
    attestations = certificate.get("attestations") or []
    naming = (
        sum(
            1
            for attestation in attestations
            if isinstance(attestation, dict)
            and isinstance(attestation.get("checkpointDigest"), str)
            and digest is not None
            and attestation["checkpointDigest"].lower() == digest
        )
        if digest is not None
        else 0
    )
    return {
        "digest": digest,
        "anchor": checkpoint.get("anchor"),
        "anchor_height": checkpoint.get("anchorHeight"),
        "anchor_post_state": checkpoint.get("anchorPostState"),
        "metadata_root": checkpoint.get("metadataRoot"),
        "history_root": checkpoint.get("historyRoot"),
        "attestations": len(attestations),
        "attestations_naming_digest": naming,
    }


def checkpoint_acceptance(
    *,
    joiner_health: dict,
    bootstrap_finalized,
    joiner_certificate,
    bootstrap_certificate,
    bootstrap_post_state,
    quorum,
    restart: dict,
    post_join_progress,
    log_signals: dict,
) -> tuple[str, list[str]]:
    """**The checkpoint-mode verdict.** Height and `ready` alone are not a join.

    A boundary witness can report the requested height and `ready` while serving another root and
    a zero finalized height; the old predicate called that PASS. PASS now requires a certified
    anchor at or above the bootstrap's finalized height, a post-state that matches the bootstrap's
    block at that anchor, finalized values that match, a post-join block that finalizes, a restart
    that retains the certified anchor and finalized height, and a log that shows the certified path
    (`replayed none`) with no replay marker.
    """
    reasons: list[str] = []
    joiner = certificate_fields(joiner_certificate)
    if joiner is None:
        return "INCONCLUSIVE", ["the joiner serves no checkpoint certificate"]
    baseline = certificate_fields(bootstrap_certificate)
    if baseline is None:
        return "INCONCLUSIVE", ["the bootstrap's checkpoint certificate was not readable"]
    if bootstrap_post_state is None:
        return "INCONCLUSIVE", [
            "the bootstrap's post-state for the joiner's certified anchor was not readable"
        ]
    # **A certificate is evidence only with its commitments and a signature that names it.**
    if joiner["digest"] is None:
        return "INCONCLUSIVE", [
            "the joiner's certificate's canonical digest could not be recomputed"
        ]
    if not joiner["metadata_root"] or not joiner["history_root"]:
        return "INCONCLUSIVE", [
            "the joiner's certificate carries no metadata and validation-window commitment; an "
            "uncommitted certificate cannot be a PASS"
        ]
    if joiner["attestations_naming_digest"] == 0:
        return "INCONCLUSIVE", [
            "no attestation names the recomputed canonical digest; the certificate's signatures "
            "were not checked against this statement"
        ]
    if joiner["digest"] != baseline["digest"] or (
        joiner["anchor"],
        joiner["anchor_post_state"],
    ) != (baseline["anchor"], baseline["anchor_post_state"]):
        reasons.append(
            "the joiner's certificate disagrees with the bootstrap's (digest, anchor or post-state)"
        )
    if not isinstance(quorum, dict) or quorum.get("supporting") is None or quorum.get("total") is None:
        return "INCONCLUSIVE", [
            "the joiner's quorum line (supporting of total stake signed) was not found"
        ]
    if 3 * int(quorum["supporting"]) <= 2 * int(quorum["total"]):
        reasons.append(
            f"the certificate is not quorate: {quorum['supporting']} of {quorum['total']} stake"
        )

    if joiner_health.get("ready") is not True:
        reasons.append("the joiner is not ready")
    observed = joiner_health.get("maxObservedHeight")
    if observed is None or observed < bootstrap_finalized:
        reasons.append("the joiner's observed height is below the bootstrap's finalized anchor")

    anchor_height = joiner.get("anchor_height")
    if anchor_height is None or anchor_height < bootstrap_finalized:
        reasons.append("the certified anchor is below the bootstrap's finalized height")
    if joiner.get("anchor_post_state") != bootstrap_post_state:
        reasons.append(
            "the joiner's certified post-state differs from the bootstrap's post-state at the "
            "certified anchor"
        )

    finalized = joiner_health.get("finalizedHeight")
    if finalized is None or finalized < bootstrap_finalized:
        reasons.append("the joiner's finalized height is below the bootstrap's")

    if not isinstance(post_join_progress, dict) or post_join_progress.get("error"):
        return "INCONCLUSIVE", [
            f"post-join progress was not measured: {post_join_progress}"
        ]
    # **The proposed block itself must finalize**, not merely a height increase: a node can observe
    # a new height whose block never enters the finalized closure.
    proposed = post_join_progress.get("proposed_block_hash")
    if not proposed:
        return "INCONCLUSIVE", [
            "the propose response did not name the block, so its finality cannot be checked"
        ]
    if post_join_progress.get("proposed_block_finalized") is False:
        reasons.append(f"the proposed block {proposed} did not finalize")
    elif post_join_progress.get("proposed_block_finalized") is not True:
        return "INCONCLUSIVE", [
            f"the finality of the proposed block {proposed} was not measured"
        ]

    if restart.get("ready") is not True:
        reasons.append("the joiner is not ready after restart")
    if (
        restart.get("certified_anchor_height") is None
        or restart["certified_anchor_height"] < anchor_height
    ):
        reasons.append("the restart lost the certified anchor")
    if restart.get("finalized_height") is None or restart["finalized_height"] < finalized:
        reasons.append("the restart lost finalized height")

    if not log_signals.get("replayed_none"):
        reasons.append("the log does not show the certified path's 'replayed none'")
    if log_signals.get("replay_markers"):
        reasons.append("the log shows a replay marker")
    # **The observer's own collector is not the join.** A non-validator observer gathers
    # attestations for its current statement; while its state is still being hydrated it logs
    # "active-bonds leaf not found". The certified join's own evidence is the certificate, the
    # post-state and the finalized values above; that collector warning does not make the join
    # unattributed. A receiver overflow or an OOM is still a failure of the node.
    if log_signals.get("receiver_overflow"):
        reasons.append("the log carries a receiver-overflow signal")
    if log_signals.get("out_of_memory"):
        reasons.append("the log carries an out-of-memory signal")

    return ("FAIL", reasons) if reasons else ("PASS", [])


def http_json(port: int, path: str, timeout: float = 5.0) -> dict:
    url = f"http://127.0.0.1:{port}{path}"
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return json.loads(response.read())


def wait_for(port: int, predicate, timeout_s: float, poll_s: float, label: str) -> dict:
    deadline = time.monotonic() + timeout_s
    last: dict = {}
    while time.monotonic() < deadline:
        try:
            last = http_json(port, "/health")
            if predicate(last):
                return last
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, ConnectionError) as exc:
            last = {"error": str(exc)}
        time.sleep(poll_s)
    raise TimeoutError(f"{label}: no qualifying answer in {timeout_s:.0f}s; last={last}")


def run(argv: list[str], timeout: float | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(argv, check=True, text=True, capture_output=True, timeout=timeout)


def docker(argv: list[str]) -> str:
    # `docker logs` writes the container's stderr to the caller's stderr; merge it so a run's own
    # journal is retained rather than silently dropped.
    completed = subprocess.run(
        ["docker", *argv], check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
    )
    return completed.stdout


def capture(path: Path, text: str) -> None:
    path.write_text(text)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="run directory (gitignored target/... )")
    parser.add_argument("--target-height", type=int, default=2436,
                        help="minimum observed block height before the reset arm (default 2436)")
    parser.add_argument("--skip-up", action="store_true", help="use the devnet already running")
    parser.add_argument("--skip-chain", action="store_true",
                        help="skip the chain build and measure against the current chain")
    parser.add_argument("--bootstrap-volume", default=None,
                        help="start the bootstrap from this retained data volume (no chain build)")
    parser.add_argument("--no-autopropose", action="store_true",
                        help="start without autopropose (measure only, no competing production)")
    parser.add_argument("--no-propose-on-deploy", action="store_true",
                        help="disable propose-on-deploy for the measured run")
    parser.add_argument("--validators", type=int, default=4,
                        help="devnet validators (needs 4 for the default validator 3)")
    parser.add_argument("--observer", action="store_true",
                        help="measure an empty observer joining the retained chain instead of "
                             "wiping a bonded validator; requires the chain's genesis to match the "
                             "network this up starts")
    parser.add_argument("--chain-timeout", type=float, default=4 * 60 * 60,
                        help="seconds allowed for the chain to reach the target (default 4h)")
    parser.add_argument("--restore-timeout", type=float, default=45 * 60,
                        help="seconds allowed for the wiped validator to restore (default 45m)")
    parser.add_argument("--poll", type=float, default=10.0, help="poll interval in seconds")
    parser.add_argument("--validator", type=int, default=3, help="validator to wipe (default 3)")
    parser.add_argument(
        "--join-mode",
        default="checkpoint",
        help="the sync path measured; only `checkpoint` exists after #591 (use `rnode audit` "
        "for the full replay)",
    )
    args = parser.parse_args()

    if args.validator not in (1, 2, 3):
        print("c339: devnet.sh reset supports validators 1..3", file=sys.stderr)
        return 2
    if args.join_mode != "checkpoint":
        print(
            "c339: the replay sync path was removed in #591; measure the replay with "
            "`rnode audit`, not with a join",
            file=sys.stderr,
        )
        return 2

    out = Path(args.out)
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=False)

    source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    try:
        image_id = docker(["inspect", "-f", "{{.Id}}", os.environ.get("RNODE_IMAGE", "rnode:local")]).strip()
    except subprocess.CalledProcessError:
        image_id = None

    started = time.monotonic()
    if not args.skip_up:
        if args.observer:
            # The retained chain was built by a one-validator network; an observer joins the same
            # genesis, so the bootstrap's bonds file matches its stored chain. A second *validator*
            # would change the bonds file and the genesis post-state (AUDIT C46).
            up = ["tools/devnet.sh", "up", "--validators", "1", "--observers", "1",
                  "--fresh", "--admin"]
        else:
            up = ["tools/devnet.sh", "up", "--validators", str(args.validators), "--fresh", "--admin"]
        # Devnet defaults autopropose ON; omitting the flag does not disable it. The measured
        # network must not compete with its own restore, so pass the explicit negative.
        up.append("--no-autopropose" if args.no_autopropose else "--autopropose")
        if args.no_propose_on_deploy:
            up.append("--no-propose-on-deploy")
        if args.bootstrap_volume:
            # `--fresh` clears the standard volumes but deliberately leaves the named artifact
            # alone, so a retained chain is measured rather than rebuilt.
            up += ["--data-volume", args.bootstrap_volume]
        run(up)

    if not args.skip_chain:
        chain = wait_for(
            HTTP_BASE,
            lambda body: body.get("maxObservedHeight") is not None
            and body["maxObservedHeight"] >= args.target_height,
            args.chain_timeout,
            args.poll,
            f"chain to height {args.target_height}",
        )
        chain_seconds = time.monotonic() - started
    else:
        chain = http_json(HTTP_BASE, "/health")
        chain_seconds = None

    bootstrap_finalized = chain.get("finalizedHeight")
    if bootstrap_finalized is None:
        print("c339: the bootstrap reports no finalized height; refusing to measure a restore "
              "against an unknown anchor", file=sys.stderr)
        return 1
    capture(raw / "bootstrap-health.json", json.dumps(chain, indent=2, sort_keys=True) + "\n")

    if args.observer:
        # The observer starts empty and `devnet.sh up` waits until it serves; its container start is
        # the restore clock's origin. No reset is needed: the volume was created fresh by `--fresh`.
        joiner_name = f"{PREFIX}-observer-1"
        joiner_port = HTTP_BASE + 2 * 1000  # n=1 validators, observer i=1
        started_raw = docker(["inspect", "-f", "{{.State.StartedAt}}", joiner_name]).strip()
        # `devnet.sh up` returns only after the observer serves, so the restore's end is after the
        # readiness wait below; the clock starts at the container's own start, not at `up`'s return.
        try:
            started_dt = parse_started_at(started_raw)
        except ValueError:
            started_dt = None

        observed: dict = {}

        def probe() -> None:
            observed["restored"] = wait_for(
                joiner_port,
                lambda body: body.get("ready") is True
                and body.get("maxObservedHeight") is not None
                and body["maxObservedHeight"] >= bootstrap_finalized,
                args.restore_timeout,
                args.poll,
                f"observer restore to finalized {bootstrap_finalized}",
            )

        if started_dt is None:
            probe()
            restore_seconds = time.monotonic() - started
        else:
            # The probe runs inside `measure_restore`; the end sample cannot precede it.
            restore_seconds = measure_restore(started_dt, probe)
        restored = observed["restored"]
        restart_target = joiner_name
    else:
        joiner_name = f"{PREFIX}-validator-{args.validator}"
        joiner_port = HTTP_BASE + args.validator * 1000
        reset_started = time.monotonic()
        run(["tools/devnet.sh", "reset", str(args.validator)])
        restored = wait_for(
            joiner_port,
            lambda body: body.get("ready") is True
            and body.get("maxObservedHeight") is not None
            and body["maxObservedHeight"] >= bootstrap_finalized,
            args.restore_timeout,
            args.poll,
            f"validator {args.validator} restore to finalized {bootstrap_finalized}",
        )
        restore_seconds = time.monotonic() - reset_started
        restart_target = str(args.validator)

    # **The acceptance evidence, before the restart.** A certificate that survives the restart is
    # the control; the certificate and post-state measured here are the join's claim.
    joiner_certificate = try_http_json(joiner_port, "/api/v1/checkpoint")
    bootstrap_certificate = try_http_json(HTTP_BASE, "/api/v1/checkpoint")
    joiner_fields = certificate_fields(joiner_certificate)
    bootstrap_post_state = None
    if joiner_fields and joiner_fields.get("anchor"):
        block = try_http_json(HTTP_BASE, f"/api/v1/block/{joiner_fields['anchor']}")
        if isinstance(block, dict):
            bootstrap_post_state = block.get("postStateHash") or block.get(
                "post_state_hash"
            )
        if bootstrap_post_state is None:
            # Fall back to a certificate naming the same anchor; the block route may be pruned
            # while the certificate is still served.
            baseline = certificate_fields(bootstrap_certificate)
            if baseline and baseline.get("anchor") == joiner_fields.get("anchor"):
                bootstrap_post_state = baseline.get("anchor_post_state")
    post_join_progress = measure_post_join(joiner_port, restored)

    pre_restart_height = restored.get("maxObservedHeight", 0)
    if isinstance(post_join_progress, dict) and post_join_progress.get("observed_height"):
        pre_restart_height = max(pre_restart_height, post_join_progress["observed_height"])

    restart_started = time.monotonic()
    run(["tools/devnet.sh", "stop", restart_target])
    run(["tools/devnet.sh", "start", restart_target])
    restarted = wait_for(
        joiner_port,
        lambda body: body.get("ready") is True
        and body.get("maxObservedHeight") is not None
        and body["maxObservedHeight"] >= pre_restart_height,
        args.restore_timeout,
        args.poll,
        "restart control",
    )
    restart_seconds = time.monotonic() - restart_started
    restarted_certificate = try_http_json(joiner_port, "/api/v1/checkpoint")
    restarted_fields = certificate_fields(restarted_certificate) or {}
    restart = {
        "seconds": restart_seconds,
        "max_observed_height": restarted.get("maxObservedHeight"),
        "ready": restarted.get("ready"),
        "finalized_height": restarted.get("finalizedHeight"),
        "certified_anchor_height": restarted_fields.get("anchor_height"),
        "checkpoint_digest": restarted_fields.get("digest"),
    }

    capture(raw / "joiner-health.json",
            json.dumps(restored, indent=2, sort_keys=True) + "\n")
    capture(raw / "joiner-health-after-restart.json",
            json.dumps(restarted, indent=2, sort_keys=True) + "\n")
    capture(raw / "joiner.log", docker(["logs", "--timestamps", joiner_name]))
    capture(raw / "bootstrap.log", docker(["logs", "--timestamps", BOOTSTRAP]))

    log_text = (raw / "joiner.log").read_text(errors="replace")
    baseline_fields = certificate_fields(bootstrap_certificate) or {}
    # **The quorum line the joining node itself verified** (`<supporting> of <total> stake signed`).
    # Python cannot check secp256k1; this line plus attestations naming the recomputed digest is the
    # evidence class, and its absence is INCONCLUSIVE rather than a PASS.
    quorum = None
    quorum_match = re.search(r"(\d+) of (\d+) stake signed", log_text)
    if quorum_match:
        quorum = {
            "supporting": int(quorum_match.group(1)),
            "total": int(quorum_match.group(2)),
        }
    result = {
        "tool": "c339_mature_replay.py",
        "join_mode": args.join_mode,
        "source_sha": source_sha,
        "image_id": image_id,
        "target_height": args.target_height,
        "chain_seconds": chain_seconds,
        "bootstrap": {
            "max_observed_height": chain.get("maxObservedHeight"),
            "finalized_height": chain.get("finalizedHeight"),
            "finalized_lag": chain.get("finalizedLag"),
            "boot_id": chain.get("bootId"),
            "checkpoint_anchor_height": baseline_fields.get("anchor_height"),
            "checkpoint_digest": baseline_fields.get("digest"),
        },
        "joiner": {
            "mode": "observer" if args.observer else "validator",
            "name": joiner_name,
            "validator": None if args.observer else args.validator,
            "restore_seconds": restore_seconds,
            "max_observed_height": restored.get("maxObservedHeight"),
            "finalized_height": restored.get("finalizedHeight"),
            "ready": restored.get("ready"),
            "reached_bootstrap_finalized": restored.get("maxObservedHeight", -1) >= bootstrap_finalized,
            "checkpoint": joiner_fields,
            "bootstrap_post_state": bootstrap_post_state,
            "metadata_committed": bool(joiner_fields and joiner_fields.get("metadata_root")),
            "window_committed": bool(joiner_fields and joiner_fields.get("history_root")),
            "attestations_naming_digest": (
                None if joiner_fields is None else joiner_fields.get("attestations_naming_digest")
            ),
        },
        "quorum": quorum,
        "restart": restart,
        "post_join_progress": post_join_progress,
        "log_signals": {
            "lfs_restored": "LFS state is successfully restored" in log_text,
            "replayed_none": "replayed none" in log_text,
            "replay_markers": "RecoveryReplay" in log_text or "Replaying" in log_text,
            "certified_join": (
                "LFS state accepted under the checkpoint certificate" in log_text
                or "checkpoint certificate accepted" in log_text
            ),
            "running_transition": "Making a transition to Running state" in log_text,
            "active_bonds_error": "active-bonds leaf not found" in log_text,
            "receiver_overflow": "block receiver state full" in log_text,
            "out_of_memory": "Out of memory" in log_text or "OOM" in log_text,
        },
        "compatibility": {
            "arm": "current-source-to-current-source",
            "previous_binary_available": False,
            "note": "A mixed-version arm requires a retained prior image; none was supplied to "
                    "this run. The n479 record is the mixed-version control and is not extended.",
        },
        "wrong_pin_control": "covered by spec/audit/evidence/n479-native-join-20261029/",
        "execution_proven": False,
    }
    verdict, reasons = checkpoint_acceptance(
        joiner_health=restored,
        bootstrap_finalized=bootstrap_finalized,
        joiner_certificate=joiner_certificate,
        bootstrap_certificate=bootstrap_certificate,
        bootstrap_post_state=bootstrap_post_state,
        quorum=quorum,
        restart=restart,
        post_join_progress=post_join_progress,
        log_signals=result["log_signals"],
    )
    result["verdict_hint"] = verdict
    result["verdict_reasons"] = reasons
    capture(raw / "result.json", json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
