#!/usr/bin/env python3
"""Bounded two-by-two Docker partition with topology evidence and guaranteed cleanup.

This is a harness primitive for #447; it is not a claim of consensus recovery.
Usage:
  python3 tools/n447-partition.py --prefix n420fork --out /tmp/n447.json -- <workload-command> [args...]
The command runs with two validator pairs on separate Docker networks.
"""

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path


class DockerBackend:
    def call(self, *args, check=True):
        return subprocess.run(["docker", *args], check=check, capture_output=True, text=True)

    def members(self, container):
        result = self.call("inspect", "-f", "{{json .NetworkSettings.Networks}}", container)
        networks = json.loads(result.stdout)
        if not isinstance(networks, dict):
            raise RuntimeError(f"missing network inventory for {container}")
        return set(networks)


class SplitNetwork:
    """Preserve internal connectivity of two pairs; never conceal a partial cleanup."""

    def __init__(self, backend, prefix="n420fork", main="devnet"):
        self.backend = backend
        self.left = [f"{prefix}-bootstrap", f"{prefix}-validator-1"]
        self.right = [f"{prefix}-validator-2", f"{prefix}-validator-3"]
        self.nodes = self.left + self.right
        self.main = main
        self.bridge = f"{prefix}-n447-right"
        self.before = {}
        self.created = False

    def snapshot(self):
        return {node: sorted(self.backend.members(node)) for node in self.nodes}

    def __enter__(self):
        self.before = self.snapshot()
        if not all(self.main in self.before[node] for node in self.nodes):
            raise RuntimeError("all four validators must initially share the devnet bridge")
        if self.bridge == self.main:
            raise RuntimeError("temporary bridge cannot equal main bridge")
        if self.backend.call("network", "inspect", self.bridge, check=False).returncode == 0:
            raise RuntimeError(f"refusing to reuse existing Docker network: {self.bridge}")
        try:
            self.backend.call("network", "create", self.bridge)
            self.created = True
            for node in self.right:
                self.backend.call("network", "connect", "--alias", node, self.bridge, node)
            for node in self.right:
                self.backend.call("network", "disconnect", self.main, node)
            self.require_partition()
            return self
        except BaseException:
            self.restore()
            raise

    def require_partition(self):
        state = self.snapshot()
        left = [set(state[node]) for node in self.left]
        right = [set(state[node]) for node in self.right]
        if not all(self.main in group for group in left):
            raise RuntimeError("left-hand validators lost their common bridge")
        if not all(self.bridge in group and self.main not in group for group in right):
            raise RuntimeError("right-hand validators do not share only their separate bridge")
        if any(a.intersection(b) for a in left for b in right):
            raise RuntimeError("partition invalid: cross-side validators share a Docker network")
        return state

    def restore(self):
        errors = []
        # Restore the production-side bridge BEFORE removing the temporary one.
        for node in self.right:
            try:
                if self.main not in self.backend.members(node):
                    self.backend.call("network", "connect", self.main, node)
            except Exception as exc:
                errors.append(f"restore main network for {node}: {exc}")
        for node in self.right:
            try:
                if self.bridge in self.backend.members(node):
                    self.backend.call("network", "disconnect", self.bridge, node)
            except Exception as exc:
                errors.append(f"remove temporary attachment from {node}: {exc}")
        if self.created:
            try:
                self.backend.call("network", "rm", self.bridge)
            except Exception as exc:
                errors.append(f"remove temporary network: {exc}")
            self.created = False
        for node, original in self.before.items():
            try:
                if sorted(self.backend.members(node)) != original:
                    errors.append(f"network membership not restored for {node}")
            except Exception as exc:
                errors.append(f"inspect restored network for {node}: {exc}")
        if errors:
            raise RuntimeError("partition cleanup incomplete: " + "; ".join(errors))

    def __exit__(self, exc_type, exc, traceback):
        self.restore()
        return False


def main(argv=None, backend=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", default="n420fork")
    parser.add_argument("--network", default="devnet")
    parser.add_argument("--out", required=True, help="JSON evidence file, outside the repository")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a workload command is required after --")
    backend = backend or DockerBackend()
    scope = SplitNetwork(backend, prefix=args.prefix, main=args.network)
    report = {
        "kind": "bounded-two-by-two-topology",
        "boundary": "Docker network membership; not a consensus or finality verdict",
        "validators": scope.nodes,
        "workload_executable": Path(command[0]).name,
        "workload_argv_count": len(command),
        "started_unix": time.time(),
    }
    result_code = 1
    try:
        with scope:
            report["before"] = scope.before
            report["partition"] = scope.require_partition()
            result = subprocess.run(command, check=False)
            result_code = result.returncode
            report["workload_exit"] = result_code
        report["restored"] = scope.snapshot()
        report["topology_recovered"] = report["restored"] == scope.before
    except Exception as exc:
        report["error"] = f"{type(exc).__name__}: {exc}"
        try:
            report["restored"] = scope.snapshot()
            report["topology_recovered"] = bool(scope.before) and report["restored"] == scope.before
        except Exception as inspect_error:
            report["restoration_error"] = str(inspect_error)
            report["topology_recovered"] = False
    finally:
        report["finished_unix"] = time.time()
        dest = Path(args.out)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    if not report.get("topology_recovered", False):
        print(f"FAIL: partition setup or cleanup; evidence: {args.out}", file=sys.stderr)
        return 1
    print(f"Topology restored; workload exit={result_code}; evidence: {args.out}")
    return result_code


if __name__ == "__main__":
    sys.exit(main())
