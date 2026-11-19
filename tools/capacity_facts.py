#!/usr/bin/env python3
"""The capacity facts the resource arms record for the M1 typed record.

Both arms run against a live container, but the record is assembled later, when the devnet may be
gone. These helpers read the container once and write plain JSON: the effective command line (a
flag can hide from any one of `Config.Cmd`, the entrypoint or `/proc/1/cmdline`), the host CPU
count, and the node process's resident set during a replay, when the admin listener is down.

A HOCON override in a `--config-file` is not visible in any of these; the record says what it saw
rather than claiming the effective policy.
"""
from __future__ import annotations

import json
import subprocess

OVERRIDE_FLAGS = ("--allow-under-minimum", "--accept-under-minimum-risk")


def run(cmd: list[str], check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def override_words(words: list[str]) -> list[str]:
    return [word for word in words
            if any(word == flag or word.startswith(flag + "=") for flag in OVERRIDE_FLAGS)]


def container_runtime(container: str) -> dict:
    config = run(["docker", "inspect", "--format", "{{json .Config}}", container], check=False)
    try:
        document = json.loads(config.stdout)
    except json.JSONDecodeError:
        document = {}
    cmd = document.get("Cmd") if isinstance(document.get("Cmd"), list) else []
    entrypoint = (document.get("Entrypoint")
                  if isinstance(document.get("Entrypoint"), list) else [])
    env = document.get("Env") if isinstance(document.get("Env"), list) else []
    argv_text = run(["docker", "exec", container, "cat", "/proc/1/cmdline"], check=False).stdout
    argv = [word for word in argv_text.split("\0") if word]
    cores = run(["docker", "exec", container, "nproc", "--all"], check=False).stdout.strip()
    return {
        "cmd": cmd,
        "entrypoint": entrypoint,
        "env": env,
        "argv": argv,
        "host_cores": int(cores) if cores.isdigit() else None,
        "override_flags": sorted(set(override_words(cmd) + override_words(entrypoint)
                                     + override_words(argv))),
    }


def rss_mb(container: str) -> int | None:
    """The rnode process's resident set, in MiB, while the admin API may not answer.

    rnode is normally PID 1 in the devnet container; fall back to `pidof` when it is not.
    """
    text = run(["docker", "exec", container, "cat", "/proc/1/status"], check=False).stdout
    name = next((line.split(":", 1)[1].strip() for line in text.splitlines()
                 if line.startswith("Name:")), None)
    if name != "rnode":
        pids = run(["docker", "exec", container, "pidof", "rnode"], check=False).stdout.split()
        if not pids:
            return None
        text = run(["docker", "exec", container, "cat", f"/proc/{pids[0]}/status"],
                   check=False).stdout
    for line in text.splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1]) // 1024
    return None
