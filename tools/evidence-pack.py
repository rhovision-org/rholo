#!/usr/bin/env python3
"""evidence-pack — turn a run directory into a retained, digest-checked evidence record.

Why this exists. Acceptance evidence is written under gitignored `target/` and dies with the CI
artifact (14 days). Publishing it means copying it into `spec/audit/evidence/<slug>/` and then
hand-writing the manifest, the per-file digests, the provenance table and the run URL — the steps
that drift, 404, and cost an operator the most time per campaign.

What this does, and what it deliberately does not:

* it copies the run's files, hashes every one, records the source and binary pins, the run URL and
  its attempt, the verdict and the arms, and writes `manifest.json`, `SHA256SUMS.json` and a README
  **skeleton**;
* it does **not** write the boundary, the non-claims, the result reading or the close decision. Those
  are authored judgements and stay with the operator; the skeleton leaves explicit `<fill: …>`
  markers, and `verify` refuses to pass while any remain.
* `execution_proven` is always `false`. A digest chain proves the bytes are the bytes; it does not
  prove the run happened or that its verdict is sound.

Usage:
  python3 tools/evidence-pack.py pack --run-dir DIR --record spec/audit/evidence/<slug> \\
      --gate-id <id> --verdict PASS|FAIL [--source-sha SHA] [--binary-sha256 HEX] [--run-url URL] \\
      [--locality local-devnet|loopback-diagnostic|off-host-claim] [--copy|--move]
  python3 tools/evidence-pack.py verify --record spec/audit/evidence/<slug>

`pack` defaults to copying. `--move` exists for a scratch run directory and is refused when the
destination is inside the source.
"""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import re
import shutil
import sys
from datetime import datetime, timezone
from pathlib import Path

SCHEMA = 1
TOOL = "evidence-pack"
MAX_FILE_BYTES = 64 * 1024 * 1024
UPSTREAM = "rhovision-org/rholo"
RUN_URL = re.compile(
    rf"^https://github\.com/{UPSTREAM}/actions/runs/(\d+)(/attempts/(\d+))?/?$"
)
PLACEHOLDER = re.compile(r"<fill:[^>]*>")


def fail(message: str):
    print(f"FAIL  {message}", file=sys.stderr)
    raise SystemExit(1)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def pin_from(run_dir: Path, explicit, names, key=None) -> str | None:
    if explicit:
        return explicit
    for name in names:
        path = run_dir / name
        if path.is_file():
            return path.read_text().strip().split()[0]
    if key:
        for name in ("campaign.json", "result.json"):
            document = read_json(run_dir / name)
            value = document.get(key)
            if isinstance(value, str) and value:
                return value
    return None


def run_url(explicit) -> tuple[str | None, str | None]:
    if explicit:
        return explicit, None
    if os.environ.get("GITHUB_REPOSITORY") and os.environ["GITHUB_REPOSITORY"] != UPSTREAM:
        return None, (
            f"GITHUB_REPOSITORY is {os.environ['GITHUB_REPOSITORY']}, not {UPSTREAM}; "
            "a fork URL makes the only witness unopenable (#515)"
        )
    run_id = os.environ.get("GITHUB_RUN_ID")
    if not run_id:
        return None, None
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT")
    suffix = f"/attempts/{attempt}" if attempt else ""
    return f"{server}/{UPSTREAM}/actions/runs/{run_id}{suffix}", None


def verdict_from(value) -> str | None:
    if not value:
        return None
    text = str(value).upper()
    if text.startswith("PASS"):
        return "PASS"
    if text.startswith("FAIL"):
        return "FAIL"
    return None


def arms_from(run_dir: Path) -> list:
    campaign = read_json(run_dir / "campaign.json")
    if isinstance(campaign.get("cycles"), list):
        return campaign["cycles"]
    result = read_json(run_dir / "result.json")
    if isinstance(result.get("rounds"), list):
        return result["rounds"]
    return []


def arm_verdict(arm) -> str | None:
    if isinstance(arm, dict):
        return verdict_from(arm.get("result") or arm.get("verdict"))
    return None


def collect(run_dir: Path, redactions: list[str]) -> list[dict]:
    files = []
    for path in sorted(run_dir.rglob("*")):
        if path.is_symlink():
            fail(f"symlink in the run directory: {path.relative_to(run_dir)} (refused)")
        if not path.is_file():
            continue
        relative = path.relative_to(run_dir)
        text = relative.as_posix()
        if any(fnmatch.fnmatch(text, pattern) for pattern in redactions):
            continue
        size = path.stat().st_size
        if size > MAX_FILE_BYTES:
            fail(f"{text} is {size} bytes (cap {MAX_FILE_BYTES}); compress it or redact it")
        files.append(
            {
                "path": text,
                "sha256": sha256(path),
                "bytes": size,
                "kind": path.suffix.lstrip(".") or "text",
            }
        )
    if not files:
        fail("the run directory has no files to package")
    return files


README_TEMPLATE = """# {slug}

{locality_line}

## Scope

<fill: what this run is, and the exact question it answers.>

## Provenance

| | |
|---|---|
| gate | `{gate_id}` |
| source | `{source_sha}` |
| binary | `{binary_sha256}` |
| run | {run_url} |
| packaged | `{packaged_at}` |
| verdict | **{verdict}** |

Every file listed in `manifest.json` is hashed in `SHA256SUMS.json`. **`execution_proven` is
false**: the digests prove the bytes are the bytes, not that the run happened or that its verdict is
sound.

## Result

<fill: the reading. Which assertions passed, under which criterion, and what the arms show.>

## What this does not establish

<fill: the non-claims. A finite witness, not a universal safety theorem.>

## Reproduce

<fill: the exact command(s) and the source/binary pins above.>
"""


def cmd_pack(args) -> int:
    run_dir = Path(args.run_dir).resolve()
    record = Path(args.record).resolve()
    if not run_dir.is_dir():
        fail(f"no run directory at {run_dir}")
    if "target" in record.parts:
        fail("the record may not live under target/ — it dies with the CI artifact")
    if record == run_dir or run_dir in record.parents:
        fail("the record may not be the run directory or inside it")
    if record.exists() and any(record.iterdir()):
        fail(f"{record} is not empty; packaging into it would overwrite evidence")

    source_sha = pin_from(run_dir, args.source_sha, ("source-sha.txt",), "source_sha")
    binary_sha256 = pin_from(run_dir, args.binary_sha256, ("binary-sha256.txt",), "binary_sha256")
    if not source_sha:
        fail("no source SHA: pass --source-sha or put source-sha.txt / campaign.json in the run")
    if not binary_sha256:
        fail("no binary SHA-256: pass --binary-sha256 or put binary-sha256.txt / campaign.json in the run")
    url, url_error = run_url(args.run_url)
    if url_error:
        fail(url_error)
    if url and not RUN_URL.match(url):
        fail(f"run URL is not an {UPSTREAM} actions run: {url}")

    verdict = verdict_from(args.verdict) or verdict_from(
        read_json(run_dir / "campaign.json").get("aggregate")
        or read_json(run_dir / "result.json").get("result")
    )
    if not verdict:
        fail("no verdict: pass --verdict PASS|FAIL or write campaign.json/result.json")
    arms = arms_from(run_dir)
    failures = [arm for arm in arms if arm_verdict(arm) == "FAIL"]
    if verdict == "PASS" and failures:
        fail(f"the run says PASS but {len(failures)} arm(s) say FAIL; a green aggregate cannot cover them")

    files = collect(run_dir, args.redact)
    record.mkdir(parents=True, exist_ok=True)
    raw = record / "raw"
    raw.mkdir(exist_ok=True)
    for entry in files:
        source = run_dir / entry["path"]
        destination = raw / entry["path"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        if args.move:
            shutil.move(str(source), str(destination))
        else:
            shutil.copy2(source, destination)
    # The manifest's paths are relative to the record, not to the run directory.
    for entry in files:
        entry["path"] = f"raw/{entry['path']}"

    packaged_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    manifest = {
        "schema": SCHEMA,
        "gate_id": args.gate_id,
        "verdict": verdict,
        "arms": arms,
        "source_sha": source_sha,
        "binary_sha256": binary_sha256,
        "run": {"url": url, "attempt": (RUN_URL.match(url).group(3) if url else None)},
        "locality": args.locality,
        "files": files,
        "redactions": args.redact,
        "execution_proven": False,
        "packer": {"tool": TOOL, "version": 1, "packaged_at": packaged_at},
    }
    (record / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    (record / "SHA256SUMS.json").write_text(
        json.dumps({entry["path"]: entry["sha256"] for entry in files}, indent=2, sort_keys=True)
        + "\n"
    )
    locality_line = (
        f"**Evidence locality.** `{args.locality}`" if args.locality else
        "**Evidence locality.** <fill: local-devnet | loopback-diagnostic | off-host-claim>"
    )
    (record / "README.md").write_text(
        README_TEMPLATE.format(
            slug=record.name,
            locality_line=locality_line,
            gate_id=args.gate_id,
            source_sha=source_sha,
            binary_sha256=binary_sha256,
            run_url=url or "<fill: the actions run URL>",
            packaged_at=packaged_at,
            verdict=verdict,
        )
    )
    print(f"packed {len(files)} file(s) into {record} (execution_proven=false)")
    return 0


def cmd_verify(args) -> int:
    record = Path(args.record).resolve()
    manifest_path = record / "manifest.json"
    if not manifest_path.is_file():
        fail(f"no manifest.json under {record}")
    manifest = read_json(manifest_path)
    if manifest.get("schema") != SCHEMA:
        fail(f"manifest schema {manifest.get('schema')!r} is not {SCHEMA}")
    if manifest.get("execution_proven") is not False:
        fail("execution_proven must be false: a digest chain does not prove execution")
    url = (manifest.get("run") or {}).get("url")
    if url and not RUN_URL.match(url):
        fail(f"the recorded run URL is not an {UPSTREAM} actions run: {url}")
    sums = read_json(record / "SHA256SUMS.json")
    listed = {entry["path"]: entry for entry in manifest.get("files", [])}
    if set(sums) != set(listed):
        fail("SHA256SUMS.json and manifest.json disagree on the file set")
    for relative, entry in listed.items():
        path = record / relative
        if not path.is_file():
            fail(f"missing file: {relative}")
        digest = sha256(path)
        if digest != entry["sha256"] or sums.get(relative) != digest:
            fail(f"digest mismatch: {relative}")
        if path.stat().st_size != entry["bytes"]:
            fail(f"byte count mismatch: {relative}")
    actual = {
        p.relative_to(record).as_posix()
        for p in (record / "raw").rglob("*")
        if p.is_file()
    }
    extra = actual - set(listed)
    if extra:
        fail(f"unlisted file(s) under raw/: {sorted(extra)[:5]}")
    readme = record / "README.md"
    if not readme.is_file():
        fail("no README.md")
    placeholders = PLACEHOLDER.findall(readme.read_text())
    if placeholders:
        fail(f"README.md has {len(placeholders)} unresolved placeholder(s); author it first")
    verdict = verdict_from(manifest.get("verdict"))
    if not verdict:
        fail("manifest verdict is absent or not PASS/FAIL")
    failures = [arm for arm in manifest.get("arms", []) if arm_verdict(arm) == "FAIL"]
    if verdict == "PASS" and failures:
        fail("PASS verdict beside a FAIL arm")
    print(f"ok    {record.name}: {len(listed)} file(s), digest-verified, authored README")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    pack = sub.add_parser("pack")
    pack.add_argument("--run-dir", required=True)
    pack.add_argument("--record", required=True)
    pack.add_argument("--gate-id", required=True)
    pack.add_argument("--verdict", choices=["PASS", "FAIL"])
    pack.add_argument("--source-sha")
    pack.add_argument("--binary-sha256")
    pack.add_argument("--run-url")
    pack.add_argument("--locality",
                      choices=["local-devnet", "loopback-diagnostic", "off-host-claim"])
    pack.add_argument("--redact", action="append", default=[])
    pack.add_argument("--move", action="store_true")
    pack.set_defaults(func=cmd_pack)
    verify = sub.add_parser("verify")
    verify.add_argument("--record", required=True)
    verify.set_defaults(func=cmd_verify)
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
