#!/usr/bin/env python3
"""classify-run — name the cause of a failed acceptance run from its own artifacts.

Reads a run directory and returns one of the closed classes, or `UNDETERMINED` with the datum that
would have decided it. It never guesses: a class without evidence is worse than the string it
replaces, because it sends the operator to the wrong place with confidence.

Sources: `failure.json` (the harness's own envelope) is authoritative for the class; the log tail is
always read for `ORPHAN_WITNESS` lines. An orphan-class failure is only confirmed when a witness
names the hash and its disposition stage; otherwise the result is
`UNDETERMINED/missing_orphan_witness` (the runtime emitter is deferred, C2). Without an envelope,
the class comes from the result/campaign reasons, the observations and the log signatures.

Usage: python3 tools/classify-run.py --run-dir target/isolated-16gb/n8-partition
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("failure_envelope", ROOT / "tools/failure_envelope.py")
failure_envelope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(failure_envelope)

WITNESS = re.compile(r"ORPHAN_WITNESS\s+hash=([0-9a-fA-F]+)\s+disposition=(\S+)")


def _tail(path: Path, limit: int = 20_000) -> str:
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return ""
    return text[-limit:]


def _texts(directory: Path) -> list[str]:
    texts = []
    for name in ("result.json", "campaign.json"):
        path = directory / name
        if not path.is_file():
            continue
        try:
            document = json.loads(path.read_text())
        except ValueError:
            continue
        for key in ("error", "reason", "aggregate", "failure_class"):
            if document.get(key):
                texts.append(str(document[key]))
        for round_ in document.get("rounds") or document.get("cycles") or []:
            if not isinstance(round_, dict):
                continue
            for key in ("error", "reason", "result", "failure_class"):
                if round_.get(key):
                    texts.append(str(round_[key]))
    observations = directory / "observations.jsonl"
    if observations.is_file():
        texts.extend(_tail(observations).splitlines()[-20:])
    for log in list(directory.glob("*.log"))[:10]:
        texts.append(_tail(log))
    return texts


def classify_run(run_dir) -> dict:
    directory = Path(run_dir)
    text = " \n ".join(_texts(directory))
    witnesses = [{"hash": h, "disposition": d} for h, d in WITNESS.findall(text)]
    report = {
        "run_dir": str(directory),
        "class": "UNDETERMINED",
        "source": None,
        "evidence": None,
        "missing": [],
        "orphans": witnesses,
    }

    failure = directory / "failure.json"
    if failure.is_file():
        try:
            document = json.loads(failure.read_text())
        except ValueError:
            document = {}
        body = document.get("failure") or {}
        report["class"] = body.get("class") or "UNDETERMINED"
        report["source"] = "failure.json"
        report["evidence"] = body.get("first_error")
        report["missing"] = list(body.get("missing") or [])
        if body.get("capture_error"):
            report["capture_error"] = body["capture_error"]
    else:
        failure_class, missing = failure_envelope.classify(text)
        report["class"] = failure_class
        report["source"] = "result/observations/logs"
        report["evidence"] = text.strip().splitlines()[0][:500] if text.strip() else None
        report["missing"] = missing

    if report["class"] == "orphan_body" and not witnesses:
        # The class is visible; the hashes and stages are not. C2 (deferred) is the emitter.
        report["class"] = "UNDETERMINED"
        report["missing"] = list(report["missing"]) + ["orphan_witness"]
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-dir", required=True)
    args = parser.parse_args()
    print(json.dumps(classify_run(args.run_dir), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
