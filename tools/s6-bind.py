#!/usr/bin/env python3
"""Run and bind the S6 gate tests.

`spec/audit/s6-test-plan.json` is the authored data: one entry per S6 gate, with a
real test witness and one production mutation that must make that test fail. This
tool does not invent evidence. It runs the test to PASS, applies the literal
mutation and runs it again to FAIL, packs both records with `evidence-pack.py`,
and emits a bindings fragment. `apply` merges the fragment into the register and
updates the met ratchet; it refuses if the resulting met set is not exactly the
ratchet.

Usage:
  python3 tools/s6-bind.py check --plan spec/audit/s6-test-plan.json --batch A
  python3 tools/s6-bind.py run   --plan spec/audit/s6-test-plan.json --batch A \
      --source-sha <40-hex> --date 20261118
  python3 tools/s6-bind.py apply --plan spec/audit/s6-test-plan.json --batch A \
      --evidence-commit <40-hex> --fragment target/s6-bindings-A.json
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGISTER = ROOT / "spec" / "audit" / "mainnet-gates.json"
BINDINGS = ROOT / "spec" / "audit" / "mainnet-gates.bindings.json"
EVIDENCE = ROOT / "spec" / "audit" / "evidence"
BUILDER = ROOT / "tools" / "build-mainnet-gates.py"
CHECKER = ROOT / "tools" / "check-mainnet-gates.py"
PACKER = ROOT / "tools" / "evidence-pack.py"
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
RUNNING = re.compile(r"Running\s+[^\n(]*\(([^)]+)\)")
STATUSES = {"owes", "reviewable", "external"}


class BindError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise BindError(message)


def load_json(path: Path):
    return json.loads(path.read_text())


def register() -> dict:
    return {row["id"]: row for row in load_json(REGISTER)["gates"]}


def plan_entries(document: dict, batch: str) -> list[dict]:
    require(document.get("schema") == 1, "plan schema 1 required")
    entries = document.get("entries")
    require(isinstance(entries, list), "plan entries must be a list")
    selected = [entry for entry in entries if entry.get("batch") == batch]
    require(selected, f"no entries for batch {batch!r}")
    return selected


def slug(entry: dict, date: str) -> str:
    return f"s6-{entry['gate_id'][3:]}-{date}"


def test_attribute(text: str, symbol: str) -> bool:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if re.search(rf"\bfn\s+{re.escape(symbol)}\b", line):
            window = "\n".join(lines[max(0, index - 3):index + 1])
            return "#[test]" in window or "#[tokio::test]" in window
    return False


def command_for(entry: dict) -> list[str]:
    path_text, _, symbol = entry["witness_ref"].partition("::")
    if entry["witness_kind"] == "python":
        return ["python3", path_text, "self-test"]
    require(entry["witness_kind"].startswith("rust"), "unknown witness kind")
    package = entry["package"]
    target = entry.get("cargo_target")
    if target == "lib":
        return ["cargo", "test", "-p", package, "--lib", symbol]
    require(isinstance(target, str) and target, "an integration target is required")
    return ["cargo", "test", "-p", package, "--test", target,
            "--", "--exact", symbol]


def check(document: dict, batch: str) -> int:
    rows = register()
    entries = plan_entries(document, batch)
    ids = [entry.get("gate_id") for entry in entries]
    require(len(ids) == len(set(ids)), f"batch {batch}: duplicate gate id")
    failures: list[str] = []
    for entry in entries:
        ident = entry.get("gate_id")
        try:
            require(isinstance(ident, str) and ident in rows,
                    f"{ident}: no register row")
            require(rows[ident]["status"] in STATUSES,
                    f"{ident}: status {rows[ident]['status']} is not promotable")
            path_text, _, symbol = entry["witness_ref"].partition("::")
            require(symbol, f"{ident}: witness_ref needs path::symbol")
            path = ROOT / path_text
            require(path.is_file(), f"{ident}: witness path {path_text} is absent")
            if entry["witness_kind"] == "python":
                require(path.suffix == ".py"
                        and re.search(rf"\bdef\s+{re.escape(symbol)}\b", path.read_text()),
                        f"{ident}: the python witness has no def {symbol}")
            else:
                require(path.suffix == ".rs"
                        and test_attribute(path.read_text(errors="ignore"), symbol),
                        f"{ident}: {entry['witness_ref']} is not a #[test]")
            mutation = entry.get("mutation") or {}
            file = ROOT / str(mutation.get("file") or "")
            require(file.is_file(), f"{ident}: mutation file is absent")
            before = mutation.get("before")
            after = mutation.get("after")
            require(isinstance(before, str) and before
                    and isinstance(after, str) and after and before != after,
                    f"{ident}: the mutation needs distinct before/after")
            require(file.read_text(errors="ignore").count(before) == 1,
                    f"{ident}: the mutation before text is not unique")
            require(command_for(entry), f"{ident}: no command")
            evidence_slug = slug(entry, document.get("date", "20261118"))
            require(not (EVIDENCE / evidence_slug).exists()
                    or entry.get("force"),
                    f"{ident}: {evidence_slug} already exists")
            require(not (EVIDENCE / f"{evidence_slug}-fail").exists()
                    or entry.get("force"),
                    f"{ident}: {evidence_slug}-fail already exists")
        except (BindError, KeyError, OSError) as error:
            failures.append(str(error))
    if failures:
        for failure in failures:
            print(f"CHECK FAIL: {failure}", file=sys.stderr)
        return 1
    print(json.dumps({"result": "PLAN_CHECKED", "batch": batch,
                      "entries": len(entries)}, sort_keys=True))
    return 0


def run(command: list[str], timeout: float) -> subprocess.CompletedProcess:
    environment = os.environ.copy()
    environment["CARGO_TERM_COLOR"] = "never"
    return subprocess.run(command, cwd=ROOT, text=True, capture_output=True,
                          timeout=timeout, env=environment)


def binary_pin(log: str) -> str:
    paths = {match.group(1) for match in RUNNING.finditer(log)}
    require(len(paths) == 1, f"the test log names {len(paths)} binaries, not one")
    path = Path(paths.pop())
    path = path if path.is_absolute() else ROOT / path
    require(path.is_file(), f"the test binary {path} is absent")
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_run(directory: Path, log: str, verdict: str, source: str,
              binary: str, command: list[str]) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "test.log").write_text(log)
    (directory / "result.json").write_text(json.dumps({"result": verdict}) + "\n")
    (directory / "source-sha.txt").write_text(source + "\n")
    (directory / "binary-sha256.txt").write_text(binary + "\n")
    (directory / "command.txt").write_text(" ".join(command) + "\n")


def pack(run_dir: Path, record: Path, gate_id: str, verdict: str,
         source: str, binary: str, locality: str) -> None:
    command = [sys.executable, str(PACKER), "pack", "--run-dir", str(run_dir),
               "--record", str(record), "--gate-id", gate_id, "--verdict", verdict,
               "--source-sha", source, "--binary-sha256", binary,
               "--locality", locality]
    completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True,
                               timeout=300)
    require(completed.returncode == 0,
            f"evidence-pack failed: {completed.stdout} {completed.stderr}")


def author_readme(record: Path, entry: dict, verdict: str, log: str) -> None:
    command = " ".join(command_for(entry))
    tail = "\n".join(log.strip().splitlines()[-12:])
    mutation = entry["mutation"]
    text = f"""# {record.name}

**Evidence locality.** `single-host`

## Scope

{entry['scope_note']}

The witness is `{entry['witness_ref']}`. The mutation is:

```text
-{mutation['before']}
+{mutation['after']}
```

## Provenance

| | |
|---|---|
| gate | `{entry['gate_id']}` |
| source | `{entry['_source']}` |
| binary | `{entry['_binary']}` |
| run | local operator run (not a GitHub Actions run) |
| verdict | **{verdict}** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

{entry['result_note']}

{verdict} log tail:

```text
{tail}
```

## What this does not establish

{entry['does_not_claim']}

## Reproduce

```sh
{command}
```
"""
    (record / "README.md").write_text(text)


def run_batch(document: dict, batch: str, source: str, date: str,
              force: bool) -> int:
    require(HEX40.fullmatch(source), "--source-sha must be a 40-hex commit")
    head = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"],
                          text=True, capture_output=True, timeout=30).stdout.strip()
    require(head == source, f"HEAD is {head}, not the pinned source {source}")
    clean = subprocess.run(["git", "-C", str(ROOT), "diff", "--quiet"],
                           timeout=30).returncode
    require(clean == 0, "the tree is not clean")
    entries = plan_entries(document, batch)
    rows = register()
    fragment: list[dict] = []
    for entry in entries:
        ident = entry["gate_id"]
        evidence_slug = slug(entry, date)
        pass_record = EVIDENCE / evidence_slug
        fail_record = EVIDENCE / f"{evidence_slug}-fail"
        if force:
            shutil.rmtree(pass_record, ignore_errors=True)
            shutil.rmtree(fail_record, ignore_errors=True)
        if not force and pass_record.is_dir() and fail_record.is_dir():
            verified = all(
                subprocess.run([sys.executable, str(PACKER), "verify", "--record",
                                str(record), "--require-pins"], cwd=ROOT,
                               capture_output=True).returncode == 0
                for record in (pass_record, fail_record))
            if verified:
                fragment.append(binding_entry(entry, rows[ident], source, pass_record,
                                              fail_record, date))
                continue
        command = command_for(entry)
        timeout = float(entry.get("timeout_s", 600))
        run_dir = ROOT / "target" / "s6-runs" / batch / ident
        pass_dir = run_dir / "pass"
        fail_dir = run_dir / "fail"
        completed = run(command, timeout)
        log = completed.stdout + completed.stderr
        require(completed.returncode == 0, f"{ident}: PASS exited {completed.returncode}\n{log[-2000:]}")
        require("running 1 test" in log and "1 passed" in log,
                f"{ident}: the filter did not run exactly one test\n{log[-2000:]}")
        symbol = entry["witness_ref"].partition("::")[2]
        require(re.search(rf"test\s+\S*{re.escape(symbol)}\S*\s+\.\.\.\s+ok", log),
                f"{ident}: the named test is not in the PASS log")
        source_binary = binary_pin(log)
        write_run(pass_dir, log, "PASS", source, source_binary, command)
        pack(pass_dir, pass_record, ident, "PASS", source, source_binary, "single-host")
        entry["_source"] = source
        entry["_binary"] = source_binary
        author_readme(pass_record, entry, "PASS", log)

        mutation = entry["mutation"]
        file = ROOT / mutation["file"]
        original = file.read_text()
        require(original.count(mutation["before"]) == 1,
                f"{ident}: mutation before is not unique")
        mutated = original.replace(mutation["before"], mutation["after"], 1)
        file.write_text(mutated)
        try:
            diff = subprocess.run(["git", "-C", str(ROOT), "diff", "--no-color", "--",
                                   mutation["file"]], text=True, capture_output=True,
                                  timeout=60).stdout
            require(diff.strip(), f"{ident}: the mutation made no diff")
            completed = run(command, timeout)
            fail_log = completed.stdout + completed.stderr
            require(completed.returncode != 0,
                    f"{ident}: the mutated test passed")
            require("error[E" not in fail_log and "could not compile" not in fail_log,
                    f"{ident}: the mutation did not compile\n{fail_log[-2000:]}")
            require(re.search(rf"test\s+\S*{re.escape(symbol)}\S*\s+\.\.\.\s+FAILED",
                              fail_log) or "panicked" in fail_log,
                    f"{ident}: the named test did not fail\n{fail_log[-2000:]}")
            mutated_binary = binary_pin(fail_log)
        finally:
            file.write_text(original)
        require(hashlib.sha256(file.read_bytes()).hexdigest()
                == hashlib.sha256(original.encode()).hexdigest(),
                f"{ident}: the mutation was not restored")
        require(subprocess.run(["git", "-C", str(ROOT), "diff", "--quiet"],
                               timeout=30).returncode == 0,
                f"{ident}: the tree is dirty after the mutation")
        write_run(fail_dir, fail_log, "FAIL", source, mutated_binary, command)
        (fail_dir / "mutation.patch").write_text(diff)
        pack(fail_dir, fail_record, ident, "FAIL", source, mutated_binary, "single-host")
        entry["_source"] = source
        entry["_binary"] = mutated_binary
        author_readme(fail_record, entry, "FAIL", fail_log)
        fragment.append(binding_entry(entry, rows[ident], source, pass_record,
                                      fail_record, date))
    fragment_path = ROOT / "target" / f"s6-bindings-{batch}.json"
    fragment_path.write_text(json.dumps(fragment, indent=2) + "\n")
    print(json.dumps({"result": "BATCH_RAN", "batch": batch,
                      "entries": len(fragment),
                      "fragment": str(fragment_path)}, sort_keys=True))
    return 0


def binding_entry(entry: dict, row: dict, source: str, pass_record: Path,
                  fail_record: Path, date: str) -> dict:
    manifest_digest = hashlib.sha256((pass_record / "manifest.json").read_bytes()).hexdigest()
    pass_manifest = load_json(pass_record / "manifest.json")
    kind = entry["witness_kind"]
    scope = "unit" if kind == "rust-unit" else "integration"
    return {
        "gate_id": entry["gate_id"],
        "witness_kind": kind,
        "witness_ref": entry["witness_ref"],
        "witness_scope": scope,
        "locality": scope,
        "sole_evidence": "none",
        "docker_free": True,
        "negative_control": entry.get("negative_control") or entry["mutation"]["description"],
        "does_not_claim": entry["does_not_claim"],
        "witness_binding": {
            "asserts_sha256": hashlib.sha256(
                str(row["pass_condition"]).encode()).hexdigest(),
            "control": {
                "description": entry.get("negative_control")
                or entry["mutation"]["description"],
                "control_of": entry["witness_ref"],
                "mutation": {key: entry["mutation"][key]
                             for key in ("file", "before", "after")},
                "test": entry["witness_ref"],
                "expected": "FAIL",
                "run_record": str(fail_record.relative_to(ROOT)),
            },
        },
        "evidence": {
            "digest": manifest_digest,
            "source_sha": source,
            "binary_sha256": pass_manifest["binary_sha256"],
            "record": str(pass_record.relative_to(ROOT)),
        },
    }


def apply(document: dict, batch: str, evidence_commit: str,
          fragment_path: Path) -> int:
    require(HEX40.fullmatch(evidence_commit), "--evidence-commit must be a commit")
    fragment = load_json(fragment_path)
    require(isinstance(fragment, list) and fragment, "the fragment is empty")
    bindings = load_json(BINDINGS)
    existing = {binding["gate_id"] for binding in bindings["bindings"]}
    for entry in fragment:
        ident = entry["gate_id"]
        require(ident not in existing, f"{ident}: duplicate binding")
        for record in (ROOT / entry["evidence"]["record"],
                       ROOT / entry["witness_binding"]["control"]["run_record"]):
            completed = subprocess.run([sys.executable, str(PACKER), "verify",
                                        "--record", str(record), "--require-pins"],
                                       cwd=ROOT, capture_output=True, text=True)
            require(completed.returncode == 0,
                    f"{ident}: {record} does not verify: {completed.stdout} {completed.stderr}")
        entry["evidence"]["url"] = (
            f"https://github.com/rhovision-org/rholo/blob/{evidence_commit}/"
            f"{entry['evidence']['record']}/manifest.json")
        bindings["bindings"].append(entry)
        existing.add(ident)
    ratchet = bindings.setdefault("met_ratchet", {"minimum": 0, "required_ids": []})
    for entry in fragment:
        if entry["gate_id"] not in ratchet["required_ids"]:
            ratchet["required_ids"].append(entry["gate_id"])
    ratchet["minimum"] = len(ratchet["required_ids"])
    BINDINGS.write_text(json.dumps(bindings, indent=2) + "\n")
    built = subprocess.run([sys.executable, str(BUILDER)], cwd=ROOT,
                           capture_output=True, text=True)
    require(built.returncode == 0, f"builder failed: {built.stdout} {built.stderr}")
    checked = subprocess.run([sys.executable, str(CHECKER), "--check"], cwd=ROOT,
                             capture_output=True, text=True)
    require(checked.returncode == 0, f"checker failed: {checked.stdout} {checked.stderr}")
    print(json.dumps({"result": "BATCH_APPLIED", "batch": batch,
                      "entries": len(fragment),
                      "minimum": ratchet["minimum"]}, sort_keys=True))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("check", "run", "apply"):
        command = sub.add_parser(name)
        command.add_argument("--plan", required=True)
        command.add_argument("--batch", required=True)
        if name == "run":
            command.add_argument("--source-sha", required=True)
            command.add_argument("--date", default="20261118")
            command.add_argument("--force", action="store_true")
        if name == "apply":
            command.add_argument("--evidence-commit", required=True)
            command.add_argument("--fragment", required=True)
    args = parser.parse_args(argv)
    document = load_json(Path(args.plan))
    try:
        if args.command == "check":
            return check(document, args.batch)
        if args.command == "run":
            return run_batch(document, args.batch, args.source_sha, args.date, args.force)
        return apply(document, args.batch, args.evidence_commit, Path(args.fragment))
    except (BindError, OSError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"result": "BIND_FAILED", "reason": str(error)},
                         sort_keys=True))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
