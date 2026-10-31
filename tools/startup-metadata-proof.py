#!/usr/bin/env python3
"""Paired, process-isolated measurement of the production metadata constructor."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import time

BASELINE = "8cbee80374b58eb730425381c3fc4554608b93f6"
EVIDENCE = Path("spec/audit/evidence/n480-startup-metadata-20261029")
TEST = Path("casper/tests/startup_metadata_bench.rs")
PARITY = (
    "records", "parent_edges", "failed_records", "heights", "min_height",
    "max_height", "index_blake2b256", "persisted_metadata_blake2b256",
)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command(args, cwd, env=None, stdout=None, timeout=1800):
    print("+", " ".join(map(str, args)), flush=True)
    return subprocess.run(
        list(map(str, args)), cwd=cwd, env=env, check=True, timeout=timeout,
        stdout=stdout, text=True,
    )


def build(tree, name, out, env):
    log = out / f"build-{name}.jsonl"
    with log.open("w") as stream:
        command([
            "cargo", "test", "--locked", "--release", "-p", "rchain-casper",
            "--test", "startup_metadata_bench", "--no-run", "--message-format=json",
        ], tree, env, stream)
    executables = []
    for line in log.read_text().splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (item.get("reason") == "compiler-artifact"
                and item.get("target", {}).get("name") == "startup_metadata_bench"
                and item.get("executable")):
            executables.append(Path(item["executable"]))
    assert len(executables) == 1, executables
    binary = out / f"constructor-{name}"
    shutil.copy2(executables[0], binary)
    return binary


def run_trial(binary, name, number, db, out, env, cpu):
    result = out / f"{name}-{number}.json"
    trial_env = dict(env, N480_LMDB_DIR=str(db), N480_RESULT_JSON=str(result))
    args = [binary, "production_startup_from_persisted_metadata", "--ignored",
            "--exact", "--nocapture", "--test-threads=1"]
    if cpu is not None:
        args = ["taskset", "-c", str(cpu), *args]
    with (out / f"{name}-{number}.log").open("w") as log:
        command(args, out, trial_env, log, timeout=180)
    return json.loads(result.read_text())


def summarize(baseline, candidate):
    all_trials = baseline + candidate
    reference = {key: all_trials[0][key] for key in PARITY}
    for trial in all_trials:
        assert {key: trial[key] for key in PARITY} == reference, "index/persistence parity failed"
    metrics = ("constructor_peak_rss_kib", "constructor_elapsed_ns", "rss_after_kib")
    medians = {
        name: {key: statistics.median(t[key] for t in trials) for key in metrics}
        for name, trials in (("baseline", baseline), ("candidate", candidate))
    }
    old, new = medians["baseline"], medians["candidate"]
    gates = {
        "complete_index_and_persisted_metadata_parity": True,
        "lower_median_constructor_peak_rss": new[metrics[0]] < old[metrics[0]],
        # Predeclared noise allowance: 20% or 5 ms, whichever is larger.
        "no_material_constructor_time_regression": (
            new[metrics[1]] <= old[metrics[1]] + max(old[metrics[1]] * 0.20, 5_000_000)
        ),
    }
    return {
        "parity": reference, "medians": medians, "gates": gates,
        "peak_rss_reduction_percent": 100 * (1 - new[metrics[0]] / old[metrics[0]]),
        "elapsed_reduction_percent": 100 * (1 - new[metrics[1]] / old[metrics[1]]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--pairs", type=int, default=7)
    args = parser.parse_args()
    assert 7 <= args.pairs <= 21, "bounded repeated trials, at least seven pairs"
    root = Path(__file__).resolve().parents[1]
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, CARGO_TARGET_DIR=str(out / "target"), CARGO_INCREMENTAL="0")
    candidate_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    assert not subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip()
    manifest = json.loads((root / EVIDENCE / "fixture-manifest.json").read_text())
    compressed = root / EVIDENCE / "block-metadata.dump.gz"
    assert sha256(compressed) == manifest["compressed_sha256"], "compressed fixture checksum"
    dump_bytes = gzip.decompress(compressed.read_bytes())
    assert hashlib.sha256(dump_bytes).hexdigest() == manifest["uncompressed_sha256"], "fixture checksum"
    dump = out / "block-metadata.dump"
    dump.write_bytes(dump_bytes)
    db = out / "database"
    db.mkdir()
    command(["mdb_load", "-f", dump, db], root)

    baseline_tree = out / "baseline-source"
    command(["git", "fetch", "--no-tags", "origin", BASELINE], root)
    command(["git", "worktree", "add", "--detach", baseline_tree, BASELINE], root)
    # The only baseline overlay is this exact same measurement harness. Production code,
    # dependencies and lockfile are the recorded baseline revision's, without modification.
    shutil.copy2(root / TEST, baseline_tree / TEST)
    baseline_binary = build(baseline_tree, "baseline", out, env)
    candidate_binary = build(root, "candidate", out, env)
    cpu = min(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
    started = time.time()
    trials = {"baseline": [], "candidate": []}
    binaries = {"baseline": baseline_binary, "candidate": candidate_binary}
    warmups = {name: run_trial(binary, name, "warmup", db, out, env, cpu)
               for name, binary in binaries.items()}
    for number in range(args.pairs):
        order = ("baseline", "candidate") if number % 2 == 0 else ("candidate", "baseline")
        for name in order:
            trials[name].append(run_trial(binaries[name], name, number, db, out, env, cpu))
    # Warmups count for correctness; only the seven balanced pairs count for performance.
    for name in trials:
        assert {k: warmups[name][k] for k in PARITY} == {k: trials[name][0][k] for k in PARITY}
    summary = summarize(trials["baseline"], trials["candidate"])
    assert summary["parity"]["records"] == manifest["records"]
    report = {
        "schema": 1, "baseline_source_sha": BASELINE, "candidate_source_sha": candidate_sha,
        "harness_sha256": sha256(root / TEST),
        "binary_sha256": {name: sha256(binary) for name, binary in binaries.items()},
        "fixture": manifest, "pairs": args.pairs, "warmups_per_binary": 1,
        "order": "alternating AB/BA, independent OS process for every trial",
        "cpu_affinity": cpu, "kernel": platform.platform(), "machine": platform.machine(),
        "rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
        "measurement_seconds": time.time() - started,
        "scope": "Production BlockMetadataStore::create only; release build, jemalloc; warm filesystem, fresh processes. Not full-node replay, LFS, or steady-state RSS.",
        "workflow_run": os.environ.get("GITHUB_SERVER_URL", "https://github.com") + "/" + os.environ.get("GITHUB_REPOSITORY", "rhovision-org/rholo") + "/actions/runs/" + os.environ.get("GITHUB_RUN_ID", "local"),
        **summary, "trials": trials,
    }
    (out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: report[key] for key in ("candidate_source_sha", "parity", "medians", "gates", "peak_rss_reduction_percent")}, indent=2))
    assert all(summary["gates"].values()), "paired acceptance gate failed; retain results and logs"


if __name__ == "__main__":
    main()
