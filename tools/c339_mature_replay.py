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
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

HTTP_BASE = int(os.environ.get("DEVNET_HTTP_BASE", "40403"))
PREFIX = os.environ.get("DEVNET_PREFIX", "devnet")
BOOTSTRAP = f"{PREFIX}-bootstrap"


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
    args = parser.parse_args()

    if args.validator not in (1, 2, 3):
        print("c339: devnet.sh reset supports validators 1..3", file=sys.stderr)
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
        if not args.no_autopropose:
            up.append("--autopropose")
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
        restored_at = http_json(joiner_port, "/health")
        # `devnet.sh up` returns only after the observer serves, so this is the restore's end; the
        # clock starts at the container's own start, not at `up`'s return.
        try:
            import datetime

            started_dt = datetime.datetime.strptime(started_raw[:19], "%Y-%m-%dT%H:%M:%S")
            now = datetime.datetime.utcnow()
            restore_seconds = max(0.0, (now - started_dt).total_seconds())
        except ValueError:
            # Fall back to the whole `up` window (the bootstrap's own start included).
            restore_seconds = time.monotonic() - started
        restored = wait_for(
            joiner_port,
            lambda body: body.get("ready") is True
            and body.get("maxObservedHeight") is not None
            and body["maxObservedHeight"] >= bootstrap_finalized,
            args.restore_timeout,
            args.poll,
            f"observer restore to finalized {bootstrap_finalized}",
        )

        restart_started = time.monotonic()
        run(["tools/devnet.sh", "stop", joiner_name])
        run(["tools/devnet.sh", "start", joiner_name])
        restarted = wait_for(
            joiner_port,
            lambda body: body.get("ready") is True
            and body.get("maxObservedHeight") is not None
            and body["maxObservedHeight"] >= restored["maxObservedHeight"],
            args.restore_timeout,
            args.poll,
            "observer restart control",
        )
        restart_seconds = time.monotonic() - restart_started
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

        restart_started = time.monotonic()
        run(["tools/devnet.sh", "stop", str(args.validator)])
        run(["tools/devnet.sh", "start", str(args.validator)])
        restarted = wait_for(
            joiner_port,
            lambda body: body.get("ready") is True
            and body.get("maxObservedHeight") is not None
            and body["maxObservedHeight"] >= restored["maxObservedHeight"],
            args.restore_timeout,
            args.poll,
            f"validator {args.validator} restart control",
        )
        restart_seconds = time.monotonic() - restart_started

    capture(raw / "joiner-health.json",
            json.dumps(restored, indent=2, sort_keys=True) + "\n")
    capture(raw / "joiner-health-after-restart.json",
            json.dumps(restarted, indent=2, sort_keys=True) + "\n")
    capture(raw / "joiner.log", docker(["logs", "--timestamps", joiner_name]))
    capture(raw / "bootstrap.log", docker(["logs", "--timestamps", BOOTSTRAP]))

    log_text = (raw / "joiner.log").read_text(errors="replace")
    result = {
        "tool": "c339_mature_replay.py",
        "source_sha": source_sha,
        "image_id": image_id,
        "target_height": args.target_height,
        "chain_seconds": chain_seconds,
        "bootstrap": {
            "max_observed_height": chain.get("maxObservedHeight"),
            "finalized_height": chain.get("finalizedHeight"),
            "finalized_lag": chain.get("finalizedLag"),
            "boot_id": chain.get("bootId"),
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
        },
        "restart": {
            "seconds": restart_seconds,
            "max_observed_height": restarted.get("maxObservedHeight"),
            "ready": restarted.get("ready"),
        },
        "log_signals": {
            "lfs_restored": "LFS state is successfully restored" in log_text,
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
    if not result["joiner"]["reached_bootstrap_finalized"]:
        result["verdict_hint"] = "INCONCLUSIVE"
    elif result["log_signals"]["active_bonds_error"] or result["log_signals"]["receiver_overflow"]:
        result["verdict_hint"] = "FAIL"
    else:
        result["verdict_hint"] = "PASS"
    capture(raw / "result.json", json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
