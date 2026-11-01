#!/usr/bin/env python3
"""Bounded same-volume partition/rejoin under explicit supporting deploy workload.

The healing verdict is a **fixed finalized anchor**, not equality of moving
heads: the drill picks a block one side has finalized and the other has not
(while both sides are observable), and requires the rejoining validator to
finalize it. Head heights may differ — a correct victim converges through the
anchor, and equality of last-finalized representatives is not a property of a
producing chain (Law 15's seen-set monotonicity is Reaches-scoped; the fringe
representative itself is not monotone).
"""
import argparse
from contextlib import contextmanager
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("acceptance",ROOT/"tools/consensus-acceptance-drill.py")
a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)

# The healing window is owned by a separate, measurement-led change; this arm
# keeps the pre-registered bound and records the gap so that change can be
# justified against the same evidence.
ANCHOR_ATTEMPTS = 5
ANCHOR_RETRY_SECONDS = 2.0
DWELL_MIN_SAMPLES = 3
CONSENSUS_PORTS = ("40401/tcp", "40402/tcp", "40404/tcp")
# **The healing window is measured, not wished.** The floor is the node's own block-request
# maintenance (`requested_blocks_timeout` 240 s plus the 30 s casper loop), not a measurement of
# this net; the rest is k x the gap the victim must close x the finality interval observed on the
# seven online nodes. `k = 2` is pre-registered. Never derive the rate from the victim: a blocked
# victim's own rate is the quantity under test.
HEALING_K = 2
HEALING_FLOOR_SECONDS = 270
HEALING_CEILING_SECONDS = 600


def cmd(*args):
    return subprocess.check_output(args,text=True,stderr=subprocess.STDOUT,timeout=25).strip()


def victim_indices(validators):
    """Two cycles must cut two different validators, never the bootstrap."""
    return [validators - 1, 1]


def online_validator_indices(validators, victim_index):
    """The live set must omit the actual rotated victim, not always the last node."""
    if not 1 <= victim_index < validators:
        raise ValueError("the partition victim must be a non-bootstrap validator")
    return [i for i in range(validators) if i != victim_index]


def verify_isolated_tip_dwell(victim, observations, *, max_extra_samples=30):
    """Prove an isolated tip is stable for three distinct polls.

    In-flight messages queued before network disconnect can finish processing
    after the cut. An immediate sample need not yet be frozen. Keep the original
    three-sample requirement and fail if a bounded settling interval cannot
    establish it; never accept an unobservable or still-moving victim.
    """
    def stable():
        last = observations[-DWELL_MIN_SAMPLES:]
        return (len(last) == DWELL_MIN_SAMPLES and
                all(type(x.get("tip")) is int for x in last) and
                len({x["tip"] for x in last}) == 1)

    for _ in range(max_extra_samples + 1):
        if stable():
            return True
        if _ == max_extra_samples:
            break
        time.sleep(2)
        try:
            sample = victim_sample(victim)
            if sample.get("autopropose") is not False:
                raise AssertionError("isolated victim began proposing")
            observations.append({"tip": sample["tip"], "peers": sample["peers"],
                                 "finalized_height": sample["finalized_height"]})
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            observations.append({"error": repr(error)})
    return False


def finalized_fence_is_frozen(victim_before, observations):
    """The isolated victim may hold a rising tip; it may not advance its finalized fence.

    `victim_before` is read *after* the cut, so any later finalization would need a quorum the
    isolated validator does not have. This is the sound invariant behind the tip dwell: a locally
    proposed block raises `latest` without raising `finalized_height` (run 41219535778 arm 2 read
    34, 34, 35 while finality stayed at 25).
    """
    baseline = victim_before["finalized_height"]
    heights = [x["finalized_height"] for x in observations if "finalized_height" in x]
    return len(heights) >= DWELL_MIN_SAMPLES and all(h == baseline for h in heights)


def epoch_boundary_crossed(low, high, epoch):
    """Does the missing range (low, high] contain a multiple of the epoch length?"""
    if epoch <= 0 or high <= low:
        return False
    return any(low < epoch * k <= high
               for k in range(low // epoch, high // epoch + 2))


def container_get(node, path, admin=False, timeout=10):
    """Read a node's own API through its container loopback.

    The victim is network-isolated from its peers, not from itself: `rnode run`
    binds 0.0.0.0, so the container's loopback answers while the partition is
    installed. This is the only way to witness that the isolation held and that
    the victim kept a consistent local view.
    """
    port = 40405 if admin else 40403
    raw = subprocess.check_output(
        ["docker", "exec", node, "curl", "-fsS", f"http://127.0.0.1:{port}{path}"],
        text=True, stderr=subprocess.STDOUT, timeout=timeout)
    return json.loads(raw)


def container_is_finalized(node, block_hash):
    answer = container_get(node, f"/api/is-finalized/{block_hash}")
    if type(answer) is not bool:
        raise ValueError(f"{node}: is-finalized did not answer a JSON boolean")
    return answer


def victim_sample(node):
    status = container_get(node, "/api/v1/status")
    health = container_get(node, "/health")
    operator = container_get(node, "/api/v1/status/operator", admin=True)
    height, block_hash = a.finalized_identity(container_get(node, "/api/last-finalized-block"))
    return {
        # The contract's tip: `maxObservedHeight` (`height - 1`), not `latestBlockNumber`, which is
        # one past the highest observed height (`node-diagnostics.md` §4.1).
        "tip": health.get("maxObservedHeight"),
        "height": health.get("height"),
        "autopropose": status.get("autopropose"),
        "peers": operator.get("peers"),
        "finalized_height": height,
        "finalized_hash": block_hash,
        "ready": health.get("ready"),
        "ready_reasons": health.get("readyReasons"),
        "finalized_lag": health.get("finalizedLag"),
        "finality_stalled": health.get("finalityStalled"),
        "unrestorable_records": operator.get("unrestorableRecords"),
        "cgroup_oom_kills": (operator.get("resources") or {}).get("cgroupOomKills"),
    }


def container_lifecycle(node):
    return cmd("docker", "inspect", "-f",
               "{{.Id}} {{.State.StartedAt}} {{.RestartCount}} {{.State.OOMKilled}} {{json .Config.Cmd}}",
               node)


def published_ports(node):
    return json.loads(cmd("docker", "inspect", "-f", "{{json .NetworkSettings.Ports}}", node))


def durable_state_volume(container):
    mounts=[m for m in container["Mounts"] if m["Destination"]=="/var/lib/rnode"]
    if len(mounts)!=1 or mounts[0]["Type"]!="volume":
        raise AssertionError("exactly one durable state volume required")
    m=mounts[0]
    return (m["Type"],m["Name"],m["Source"],m["Destination"],m["RW"])


class WorkloadDrill(a.Drill):
    def __init__(self, args):
        super().__init__(args)
        self.online_finality_samples = []
        self.healing_samples = []
        self.healing_failure_class = None
        self.anchor_true_samples = 0
        self.online_false_samples = {}
        self.healing_stall_samples = 0
        self.healing_stall_signals = []
        self.healing_poll_signals = []

    def wait(self, description, condition, drive=True, window=None):
        # This arm explicitly supplies work; it cannot establish quiet-chain liveness.
        return super().wait(description,condition,drive=True,window=window)

    def wait_for_healing(self, description, condition, window=None):
        # Online-partition progress is demonstrated while signed work is being
        # supplied. Once the victim rejoins, adding Nil deploys on every retry
        # changes the target faster than it can catch up, so healing certifies
        # the anchor without injecting further work.
        self.healing_samples = []
        self.healing_failure_class = None
        self.anchor_true_samples = 0
        self.online_false_samples = {}
        self.healing_stall_samples = 0
        self.healing_stall_signals = []
        self.healing_poll_signals = []
        try:
            return super().wait(description, condition, drive=False, window=window)
        except a.WaitTimeout as error:
            self.healing_failure_class = self.classify_healing_timeout()
            if getattr(self, "events", None) is not None:
                heights = [s["victim_height"] for s in self.healing_samples
                           if s.get("victim_height") is not None]
                self.record(phase="healing_verdict",
                            failure_class=self.healing_failure_class,
                            window_seconds=window if window is not None else self.args.window,
                            samples=len(self.healing_samples),
                            first_victim_height=heights[0] if heights else None,
                            last_victim_height=heights[-1] if heights else None)
            raise RuntimeError(f"{self.healing_failure_class}: {error}") from error

    def classify_healing_timeout(self):
        """Separate a stalled victim from a slow one, using progress other than finality.

        Finalized height alone is the quantity under test, so a victim blocked on
        its peers can look 'stalled' there while its tip, peer count or
        undelivered-send count still moves. A stall therefore needs every signal
        flat across the dwell; monotone progress is inconclusive, not a defect.
        """
        samples = [s for s in getattr(self, "healing_samples", [])]
        heights = [s.get("victim_height") for s in samples
                   if s.get("victim_height") is not None]
        if not heights:
            return "INCONCLUSIVE_MEASUREMENT"
        # One entry per poll: `healing_samples` takes two entries per poll, so its three-entry
        # tail spans one and a half polls and cannot certify a dwell.
        polls = [p for p in getattr(self, "healing_poll_signals", [])]
        tail = polls[-DWELL_MIN_SAMPLES:]
        if len(tail) >= DWELL_MIN_SAMPLES and len(set(tail)) == 1:
            return "FAIL_STALLED"
        return "INCONCLUSIVE_SLOW_PROGRESS"

    def record_online_finality(self, identities, at):
        self.online_finality_samples.append(
            (at, max(x["height"] for x in identities)))

    def measured_finality_interval(self):
        """Median seconds per finalized height on the seven, if measured enough.

        The samples must span at least two intervals and contain at least three
        advances; otherwise the measurement is not used and the floor stands.
        """
        series = getattr(self, "online_finality_samples", [])
        changes = []
        last_height, last_at = None, None
        for at, height in series:
            if last_height is not None and height > last_height:
                changes.append((at - last_at) / (height - last_height))
            if last_height is None or height > last_height:
                last_height, last_at = height, at
        if len(changes) < 3:
            return None
        interval = sorted(changes)[len(changes) // 2]
        if interval <= 0 or series[-1][0] - series[0][0] < 2 * interval:
            return None
        return interval

    def healing_window(self, anchor, victim_height):
        gap = max(0, anchor["height"] - victim_height) if victim_height is not None else 0
        interval = self.measured_finality_interval()
        if interval is None or gap == 0:
            return HEALING_FLOOR_SECONDS
        scaled = HEALING_K * gap * interval
        return int(min(HEALING_CEILING_SECONDS,
                       max(HEALING_FLOOR_SECONDS, scaled)))

    def note_healing_sample(self, sample):
        if getattr(self, "healing_samples", None) is not None:
            self.healing_samples.append(sample)

    def anchor_agreement(self, anchor, victim=None):
        return anchor_agreement(self, anchor, victim)


def establish_healing_anchor(d, online, victim, cycle, victim_index=None):
    """Pick a block one side has finalized and the other has not.

    The anchor is a **finalization** boundary, not a storage boundary: the
    victim may hold the block in its DAG; what it must not have done is
    finalize it. If the victim's own fence is ahead of the online group, the
    roles swap and the seven must later finalize the victim's block.
    """
    last = None
    for attempt in range(1, ANCHOR_ATTEMPTS + 1):
        try:
            fences = []
            for i in online:
                height, block_hash = d.finalized_head(i)
                fences.append({"node": i, "height": height, "hash": block_hash})
            victim_view = victim_sample(victim)
            low = min(f["height"] for f in fences)
            pick = next(f for f in fences if f["height"] == low)
            if victim_view["finalized_height"] == low and victim_view["finalized_hash"] != pick["hash"]:
                raise RuntimeError(
                    "FAIL_FINALIZED_DISAGREEMENT: victim and online group finalized "
                    f"different blocks at height {low} "
                    f"({victim_view['finalized_hash']} vs {pick['hash']})")
            if victim_view["finalized_height"] > low:
                anchor = {"height": victim_view["finalized_height"],
                          "hash": victim_view["finalized_hash"],
                          "node": d.args.validators - 1 if victim_index is None else victim_index,
                          "case": "victim-ahead"}
                answers = [{"node": i, "answer": d.is_finalized(i, anchor["hash"])}
                           for i in online]
                victim_answer = container_is_finalized(victim, anchor["hash"])
                if any(row["answer"] is not False for row in answers) or victim_answer is not True:
                    raise RuntimeError(
                        "INCONCLUSIVE_ANCHOR: the victim is ahead but the sides are not separated "
                        "on its block")
            else:
                anchor = {"height": low, "hash": pick["hash"], "node": pick["node"],
                          "case": "seven-ahead"}
                answers = [{"node": i, "answer": d.is_finalized(i, anchor["hash"])}
                           for i in online]
                victim_answer = container_is_finalized(victim, anchor["hash"])
                if not all(row["answer"] is True for row in answers):
                    last = "online nodes disagree on the anchor"
                    raise a.WaitTimeout(last)
                if victim_answer is not False:
                    raise RuntimeError(
                        "INCONCLUSIVE_ANCHOR: the victim has already finalized the online "
                        "group's common anchor; the partition is not separating the sides")
            d.record(phase="healing_anchor", cycle=cycle, attempt=attempt, anchor=anchor,
                     online=fences, anchor_finalized=answers,
                     victim_view=victim_view, victim_anchor_finalized=victim_answer)
            return anchor
        except a.WaitTimeout as error:
            d.record(phase="healing_anchor", cycle=cycle, attempt=attempt,
                     online=fences if "fences" in locals() else None, error=str(error))
            time.sleep(ANCHOR_RETRY_SECONDS)
        except (OSError, ValueError) as error:
            last = repr(error)
            d.record(phase="healing_anchor", cycle=cycle, attempt=attempt, error=last)
            time.sleep(ANCHOR_RETRY_SECONDS)
    raise RuntimeError(
        f"FAIL_FORK: no separated finalized anchor after {ANCHOR_ATTEMPTS} attempts ({last})")


def anchor_agreement(d, anchor, victim=None):
    """One poll of the healing predicate; returns identities only when settled.

    Returns ``None`` while the predicate is not yet satisfied (the caller waits),
    raises a named verdict when it can never be satisfied, and returns the
    per-node heads once the anchor is retained on a dwell and the fork-sensitive
    cross-checks pass.
    """
    victim = d.args.validators - 1 if victim is None else victim
    statuses = [d.status_row(i) for i in range(d.args.validators)]
    victim_row = statuses[victim]
    # One entry per *poll*: the healing sample list above takes two entries per poll, so a
    # three-entry tail is only one and a half polls.
    d.healing_poll_signals = getattr(d, "healing_poll_signals", [])
    d.healing_poll_signals.append((victim_row["tip"], victim_row["peers"],
                                   victim_row["undelivered_sends"]))
    d.note_healing_sample({"victim_height": victim_row["tip"],
                           "victim_peers": victim_row["peers"],
                           "victim_undelivered": victim_row["undelivered_sends"],
                           "victim_finalized": None,
                           "anchor_finalized": None})
    # A latched fault is terminal for the process, not a transient reading.
    pipeline_faults = victim_row["pipeline_faults_outstanding"]
    if type(pipeline_faults) is not int:
        d.record(phase="healing_probe", anchor=anchor, nodes=statuses,
                 victim_finalized=None, anchor_finalized=None)
        return None
    if pipeline_faults != 0:
        raise RuntimeError(
            "FAIL_PRECONDITION_pipelineFaultsOutstanding: node {} outstanding={} "
            "cumulative={} lastPipelineFault={!r}".format(
                victim, pipeline_faults, victim_row["pipeline_faults"],
                victim_row["last_pipeline_fault"]))
    if victim_row["proposal_halted"] is True:
        raise RuntimeError(
            "FAIL_PRECONDITION_proposalHalted: node {} reason={!r}"
            .format(victim, victim_row["proposal_halt_reason"]))
    if type(victim_row["unrestorable_records"]) is not int:
        d.record(phase="healing_probe", anchor=anchor, nodes=statuses,
                 victim_finalized=None, anchor_finalized=None)
        return None
    if victim_row["unrestorable_records"] > 0:
        raise RuntimeError(
            f"FAIL_PERMANENT_DIVERGENCE: node {victim} unrestorableRecords="
            f"{victim_row['unrestorable_records']} block={victim_row['unrestorable_block']}")
    # **`finalityStalled` is a diagnosis, not a verdict** (Law 54a). A rejoining victim holds the
    # latch while its derivation catches up, so the flag alone turns a recovering node into a coin
    # flip — run 41225136080 arm 1 failed exactly here, while arm 2 passed the same code. A stall
    # needs the alarm *and* every progress signal flat across the dwell, the same rule
    # `classify_healing_timeout` uses: finalized height, tip, peers, undelivered sends.
    if victim_row["finality_stalled"] is True:
        d.healing_stall_samples = getattr(d, "healing_stall_samples", 0) + 1
        d.healing_stall_signals = getattr(d, "healing_stall_signals", [])
        d.healing_stall_signals.append((victim_row["tip"], victim_row["peers"],
                                        victim_row["undelivered_sends"]))
    else:
        d.healing_stall_samples = 0
        d.healing_stall_signals = []
    stall_tail = getattr(d, "healing_stall_signals", [])[-DWELL_MIN_SAMPLES:]
    if (getattr(d, "healing_stall_samples", 0) >= DWELL_MIN_SAMPLES
            and len(stall_tail) == DWELL_MIN_SAMPLES and len(set(stall_tail)) == 1):
        raise RuntimeError(
            f"FAIL_STALLED: node {victim} reports a sustained finality stall with no progress "
            f"(tip {stall_tail[0][0]}, peers {stall_tail[0][1]}, "
            f"undelivered {stall_tail[0][2]})")

    answers = []
    for i in range(d.args.validators):
        answers.append({"node": i, "answer": d.is_finalized(i, anchor["hash"])})
    try:
        victim_height, victim_hash = d.finalized_head(victim)
    except (OSError, ValueError):
        victim_height, victim_hash = None, None
    d.note_healing_sample({"victim_height": victim_row["tip"],
                           "victim_peers": victim_row["peers"],
                           "victim_undelivered": victim_row["undelivered_sends"],
                           "victim_finalized": victim_height,
                           "anchor_finalized": all(x["answer"] is True for x in answers)})
    d.record(phase="healing_probe", anchor=anchor, nodes=statuses,
             victim_finalized={"height": victim_height, "hash": victim_hash},
             anchor_finalized=answers)
    if any(x["answer"] is not True for x in answers):
        online_false = [x["node"] for x in answers if x["node"] != victim and x["answer"] is not True]
        if online_false:
            d.online_false_samples = getattr(d, "online_false_samples", {})
            for node in online_false:
                d.online_false_samples[node] = d.online_false_samples.get(node, 0) + 1
            stable = [n for n, count in d.online_false_samples.items()
                      if count >= DWELL_MIN_SAMPLES]
            if stable:
                raise RuntimeError(
                    f"FAIL_FORK: online validator(s) {stable} do not report the anchor {anchor['hash']} finalized")
        else:
            d.online_false_samples = {}
        d.anchor_true_samples = 0
        return None
    d.online_false_samples = {}
    d.anchor_true_samples = getattr(d, "anchor_true_samples", 0) + 1
    if d.anchor_true_samples < DWELL_MIN_SAMPLES:
        return None

    # The dwell holds. Now the cross-node state commitments.
    height = anchor["height"]
    for i in range(d.args.validators):
        blocks = a.get(i, f"/api/blocks/{height}/{height}")
        rows = [row for row in blocks if isinstance(row, dict) and row.get("blockNumber") == height] \
            if isinstance(blocks, list) else []
        # A Casper height holds one block per sender, so the anchor is *among* the
        # blocks at its height, not the only one. The pairwise finalization check
        # below is what excludes a different finalized block at the same height.
        if anchor["hash"] not in {row.get("blockHash") for row in rows}:
            raise RuntimeError(
                f"FAIL_FINALIZED_DISAGREEMENT: validator {i} serves {len(rows)} block(s) "
                f"at height {height} and none is the anchor {anchor['hash']}")
        answer = a.probe(i, d.marker, anchor["hash"])
        d.record(node=i, anchor_marker_at=anchor["hash"], response=answer)
        if not a.marker_present(answer, d.value):
            raise RuntimeError(f"FAIL_ANCHOR_STATE: validator {i} cannot read the marker at the anchor")

    heads = []
    for i in range(d.args.validators):
        ident_height, ident_hash = d.finalized_head(i)
        heads.append({"node": i, "height": ident_height, "hash": ident_hash})
    for row in heads:
        for other in range(d.args.validators):
            if other == row["node"]:
                continue
            other_height = next(h["height"] for h in heads if h["node"] == other)
            if other_height >= row["height"] and d.is_finalized(other, row["hash"]) is not True:
                raise RuntimeError(
                    "FAIL_FINALIZED_DISAGREEMENT: node {} at height {} does not have node {}'s "
                    "finalized block {} (heights {} >= {})".format(
                        other, other_height, row["node"], row["hash"],
                        other_height, row["height"]))
    return heads


@contextmanager
def record_cycle(out, rounds, record, capture_phase):
    """Persist a failed cycle before cleanup, without replacing its primary error."""
    rounds.append(record)
    try:
        yield record
    except Exception as error:
        record["result"] = "FAIL"
        record["error"] = {"type": type(error).__name__, "message": str(error)}
        # Capture the failure state before the outer cleanup reconnects the victim.
        try:
            capture_phase(f"partition-{record['cycle']}-{record['phase']}-failed")
        except Exception as capture_error:
            record["capture_error"] = str(capture_error)
        # **The structured envelope** (M): the class and phase beside the legacy result string.
        a.failure_envelope.write(
            out, "FAIL", error, phase=str(record.get("phase")), arm="partition-rejoin",
            node=record.get("isolated_validator"),
            artifact_refs=["partition-result.json", "campaign.json"])
        document = {
            "result": "FAIL_BOUNDED_WORKLOAD_PARTITION_REJOIN",
            "partition_cycles_completed": sum(r.get("result") == "PASS" for r in rounds),
            "failed_cycle": record["cycle"], "failed_phase": record["phase"],
            "rounds": rounds,
        }
        if record.get("healing_failure_class"):
            document["failure_class"] = record["healing_failure_class"]
        (out/"partition-result.json").write_text(json.dumps(document, indent=2)+"\n")
        try:
            source_sha = cmd("git", "rev-parse", "HEAD")
            binary_sha256 = cmd(
                "docker", "exec", a.node_name(0), "sha256sum", "/usr/local/bin/rnode").split()[0]
        except (OSError, subprocess.SubprocessError):
            source_sha, binary_sha256 = None, None
        (out/"campaign.json").write_text(json.dumps({
            "gate_id": "n8-partition-rejoin",
            "gate_version": 2,
            "source_sha": source_sha,
            "binary_sha256": binary_sha256,
            "run_id": os.environ.get("GITHUB_RUN_ID"),
            "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
            "aggregate": "FAIL",
            "cycles": [{"cycle": r.get("cycle"), "result": r.get("result"),
                        "failure_class": r.get("healing_failure_class")} for r in rounds],
        }, indent=2)+"\n")
        raise
    else:
        record["result"] = "PASS"
    finally:
        (out/"partition-observations.json").write_text(json.dumps(rounds, indent=2)+"\n")


def record_attestations(d, anchor):
    """Record each validator's signed finality claim at the PASS sample.

    Evidence, not a gate: the attestation binds a validator key to the block the
    node says it finalized, which a summary route cannot do. A refusal is
    recorded as a refusal — this layer never turns a missing signature into a
    pass or a fail of the predicate.
    """
    nonce = os.urandom(16).hex()
    rows = []
    for i in range(d.args.validators):
        try:
            attestation = a.get(i, f"/api/finality-attestation?nonce={nonce}")
            _, head_hash = d.finalized_head(i)
            rows.append({
                "node": i,
                "block_hash": attestation.get("blockHash"),
                "matches_head": attestation.get("blockHash") == head_hash,
                "nonce_echoed": attestation.get("nonce") == nonce,
                "sig_algorithm": attestation.get("sigAlgorithm"),
                "has_signature": bool(attestation.get("sig")),
            })
        except (OSError, ValueError) as error:
            rows.append({"node": i, "error": repr(error)})
    d.record(phase="finality_attestation", nonce=nonce, anchor=anchor, rows=rows)


def write_campaign(d, args, result, source_sha, binary_sha256):
    """One aggregate record, so a green arm cannot be quoted beside a red one."""
    rounds = result.get("rounds", [])
    campaign = {
        "gate_id": "n8-partition-rejoin",
        "gate_version": 2,
        "source_sha": source_sha,
        "binary_sha256": binary_sha256,
        "run_id": os.environ.get("GITHUB_RUN_ID"),
        "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "validators": args.validators,
        "aggregate": result["result"],
        "cycles": [{"cycle": r.get("cycle"), "result": r.get("result"),
                    "failure_class": r.get("healing_failure_class")} for r in rounds],
    }
    (d.out/"campaign.json").write_text(json.dumps(campaign, indent=2)+"\n")


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--validators",type=int,choices=[4,8],required=True)
    ap.add_argument("--epoch-length",type=int,default=10,
                    help="the devnet's epoch length, for missing-range coverage")
    ap.add_argument("--out",required=True);args=ap.parse_args();args.window=150
    if os.environ.get("GITHUB_ACTIONS")!="true" or os.environ.get("RUNNER_ENVIRONMENT")!="github-hosted":
        raise SystemExit("disposable hosted runner only")
    if os.environ.get("DEVNET_PREFIX")!="acceptance":
        raise SystemExit("unexpected namespace")
    d=WorkloadDrill(args);d.run()
    original=json.loads((d.out/"workload-result.json").read_text())
    # **Rotate the victim.** Two cycles that cut the same node look like two witnesses and are one;
    # cycle 1 cuts the last validator, cycle 2 a non-bootstrap one.
    victims = victim_indices(args.validators)
    rounds=[]
    def capture_phase(label):
        # Raw accepted blocks, operator gate verdicts and binary identity; read-only.
        previous=d.out
        try:
            d.out=previous/label;d.out.mkdir(exist_ok=True)
            a.causal_trace.capture(d,a.get,a.node_name)
        finally:
            d.out=previous
    capture_phase("before-partition")
    try:
        for cycle in range(2):
            victim_index = victims[cycle]
            victim=a.node_name(victim_index)
            online = online_validator_indices(args.validators, victim_index)
            record={"cycle":cycle+1,"isolated_validator":victim,"phase":"partition-install"}
            with record_cycle(d.out,rounds,record,capture_phase):
                d.health_precondition("partition-install")
                volume_before=durable_state_volume(json.loads(cmd("docker","inspect",victim))[0])
                before=a.finalized_identity(a.get(0,"/api/last-finalized-block"))
                cmd("docker","network","disconnect","acceptance",victim)
                networks=json.loads(cmd("docker","inspect","-f","{{json .NetworkSettings.Networks}}",victim))
                if networks:raise AssertionError("partition was not installed: networks remain")
                leaked=[p for p in published_ports(victim) if p in CONSENSUS_PORTS]
                if leaked:raise AssertionError(f"partition leaked: consensus port(s) {leaked} published")
                lifecycle=container_lifecycle(victim)
                victim_before=victim_sample(victim)
                if victim_before.get("autopropose") is not False:
                    raise AssertionError("victim is not on a no-autopropose rig")
                record["victim_view_at_install"]=victim_before
                record["container_lifecycle_at_install"]=lifecycle
                # Two 16-second escape/cadence cycles per N=8 sender plus margin:
                # a pre-registered 300-second window, not an unbounded retry.
                record["phase"]="online-progress"
                d.online_finality_samples=[]
                deadline=time.monotonic()+300;advanced=False;observed=[];frozen=[]
                record["before_height"]=before[0]
                record["online_observations"]=observed
                while time.monotonic()<deadline:
                    d.drive()
                    identities=[]
                    for i in online:
                        height,hash_=a.finalized_identity(a.get(i,"/api/last-finalized-block"))
                        if not a.marker_present(a.probe(i,d.marker,hash_),d.value):
                            raise AssertionError("online validator lost finalized marker")
                        identities.append({"node":i,"height":height,"hash":hash_})
                    observed.append(identities)
                    d.record_online_finality(identities, time.monotonic())
                    try:
                        sample=victim_sample(victim)
                        frozen.append({"tip": sample["tip"], "peers": sample["peers"],
                                       "finalized_height": sample["finalized_height"]})
                    except (OSError, ValueError, subprocess.SubprocessError) as error:
                        frozen.append({"error": repr(error)})
                    record["isolation_samples"]=frozen
                    (d.out/f"partition-{cycle+1}-live.json").write_text(json.dumps({
                        "cycle":cycle+1,"before_height":before[0],"observations":observed,
                        "isolation_samples":frozen,
                        "remaining_seconds":max(0,deadline-time.monotonic())},indent=2)+"\n")
                    if all(x["height"]>=before[0]+3 for x in identities):
                        advanced=True;break
                    time.sleep(2)
                record["partition_progress_observed"]=advanced
                if not advanced:
                    raise AssertionError("online supermajority did not advance in 300s")
                # Drain already-queued pre-cut messages before judging the frozen
                # state. The three distinct stable polls are still mandatory.
                if not verify_isolated_tip_dwell(victim, frozen):
                    raise AssertionError("partition leaked or the victim view was unobservable")
                # The tip dwell is the practical settling check; the finalized fence is the
                # sound one — finality cannot advance without a quorum the isolated node lacks.
                if not finalized_fence_is_frozen(victim_before, frozen):
                    raise AssertionError(
                        "partition leaked: the victim advanced its finalized fence while "
                        f"isolated (baseline {victim_before['finalized_height']})")
                record["isolation_samples"]=frozen
                (d.out/f"partition-{cycle+1}-live.json").write_text(json.dumps({
                    "cycle":cycle+1,"before_height":before[0],"observations":observed,
                    "isolation_samples":frozen},indent=2)+"\n")
                # Prove the same physical network cut still holds *after* the
                # settling dwell; a stable tip alone cannot certify isolation.
                if json.loads(cmd("docker","inspect","-f",
                                  "{{json .NetworkSettings.Networks}}",victim)):
                    raise AssertionError("partition was reconnected before the healing phase")
                if container_lifecycle(victim)!=lifecycle:
                    raise AssertionError("VICTIM_LIFECYCLE_CHANGED during the partition")
                capture_phase(f"partition-{cycle+1}-before-healing")
                record["phase"]="healing"
                anchor=establish_healing_anchor(
                    d, online, victim, cycle+1, victim_index=victim_index)
                record["healing_anchor"]=anchor
                record["phase"]="rejoin"
                cmd("docker","network","connect","acceptance",victim)
                try:
                    victim_head=d.finalized_head(victims[cycle])
                    record["victim_finalized_at_rejoin"]={"height": victim_head[0], "hash": victim_head[1]}
                except (OSError, ValueError) as error:
                    record["victim_finalized_at_rejoin"]=None
                    d.record(phase="healing_rejoin_read", error=repr(error))
                victim_height=(record["victim_finalized_at_rejoin"] or {}).get("height")
                window=d.healing_window(anchor, victim_height)
                record["healing_window_seconds"]=window
                record["healing_gap"]=max(0, anchor["height"]-victim_height) if victim_height is not None else None
                record["finality_interval_seconds"]=d.measured_finality_interval()
                record["healing_k"]=HEALING_K
                try:
                    recovered=d.wait_for_healing(
                        "all validators retain the healing anchor in finalized state after partition healing",
                        lambda:anchor_agreement(d,anchor,victims[cycle]), window=window)
                except RuntimeError:
                    failure_class=getattr(d,"healing_failure_class",None)
                    record["healing_failure_class"]=(
                        failure_class if isinstance(failure_class,str) else "HEALING_FAILURE")
                    raise
                record["rejoined"]=recovered
                record["healing_failure_class"]=getattr(d,"healing_failure_class",None)
                # The victim's missing range must cross an epoch boundary, or two runs at
                # different production rates are not the same code path (D-CH-8).
                if victim_height is not None:
                    crosses=epoch_boundary_crossed(victim_height, anchor["height"],
                                                   args.epoch_length)
                    record["epoch_boundary_crossed"]=crosses
                    if not crosses:
                        raise RuntimeError(
                            "INCONCLUSIVE_EPOCH_COVERAGE: the victim's missing range "
                            f"({victim_height}, {anchor['height']}] does not cross an epoch boundary")
                record_attestations(d, anchor)
                record["phase"]="retention"
                d.fixed_snapshots(original["initial_finalized"])
                record["original_finalized_snapshots_retained"]=True
                capture_phase(f"partition-{cycle+1}-healed")
                if durable_state_volume(json.loads(cmd("docker","inspect",victim))[0])!=volume_before:
                    raise AssertionError("rejoin replaced the durable mounts")
                if container_lifecycle(victim)!=lifecycle:
                    raise AssertionError("VICTIM_LIFECYCLE_CHANGED across the heal")
                record["same_volume"]=True
    finally:
        for victim_index in victims:
            try:
                subprocess.run(["docker","network","connect","acceptance",a.node_name(victim_index)],
                               capture_output=True,timeout=25)
            except (OSError,subprocess.TimeoutExpired) as cleanup_error:
                (d.out/"partition-cleanup-error.txt").write_text(str(cleanup_error)+"\n")
        (d.out/"partition-observations.json").write_text(json.dumps(rounds,indent=2)+"\n")
    memory=cmd("docker","stats","--no-stream","--format","{{.Name}} {{.MemUsage}}")
    result={"result":"PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN","source_sha":cmd("git","rev-parse","HEAD"),
        "validators":args.validators,"partition_cycles":len(rounds),"rounds":rounds,
        "resource_sample":memory,"quiet_chain_liveness":"NOT_ESTABLISHED",
        "scope":"anchor retention and finality progression in these executions; not universal CBC safety"}
    (d.out/"partition-result.json").write_text(json.dumps(result,indent=2)+"\n")
    binary_sha256=cmd("docker","exec",a.node_name(0),"sha256sum","/usr/local/bin/rnode").split()[0]
    write_campaign(d,args,result,result["source_sha"],binary_sha256)
    print(json.dumps(result),flush=True)

if __name__=="__main__":main()
