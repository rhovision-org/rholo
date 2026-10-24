#!/usr/bin/env python3
"""Bounded same-volume partition/rejoin under explicit supporting deploy workload."""
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

class WorkloadDrill(a.Drill):
    def wait(self, description, condition, drive=True):
        # This arm explicitly supplies work; it cannot establish quiet-chain liveness.
        return super().wait(description,condition,drive=True)

    def wait_for_healing(self, description, condition):
        # Online-partition progress is demonstrated while signed work is being
        # supplied. Once the victim rejoins, adding Nil deploys on every retry
        # changes the target faster than it can catch up. Certify retained
        # finalized identity against a finite, quiescent workload instead.
        # Preserve the original 150s bound and the strict all-node hash check.
        return super().wait(description, condition, drive=False)

    def finalized_marker_agreement(self, min_height):
        # A stale finalized height and a permanent divergence are different
        # failure classes. Record the gate's own health before every read, and
        # fail early if the node has abandoned an irrecoverable block.
        # Fixture-created drills have no live runner or events file. Do not touch
        # network/admin endpoints in that offline identity-only regression.
        # A real execution always passes through Drill.__init__ and must capture
        # the operator's irreversible-divergence verdict before its marker read.
        if getattr(self, "events", None) is not None:
            statuses = []
            for i in range(self.args.validators):
                operator = a.get(i, "/api/v1/status/operator", admin=True)
                statuses.append({
                    "node": i,
                    "latest": operator.get("latestBlockNumber"),
                    "peers": operator.get("peers"),
                    "finality_stall": operator.get("finalityStall"),
                    "unrestorable_records": operator.get("unrestorableRecords"),
                    "unrestorable_block": operator.get("unrestorableBlock"),
                    "undelivered_sends": operator.get("undeliveredSends"),
                })
            self.record(phase="healing_probe", target_min_height=min_height, nodes=statuses)
            bad = [x for x in statuses if (x["unrestorable_records"] or 0) > 0]
            if bad:
                details = [(x["node"], x["unrestorable_block"]) for x in bad]
                raise RuntimeError(
                    f"PERMANENT_DIVERGENCE: {details}; finalized agreement cannot be"
                    " certified by extending the wait. See healing_probe evidence."
                )
        # Retaining the marker at different finalized heads is not yet rejoin
        # agreement. Keep waiting inside the existing 150-second healing bound.
        identities = self.finalized_marker(min_height, required=True)
        if identities is None or len(identities) != self.args.validators:
            return None
        if {x["node"] for x in identities} != set(range(self.args.validators)):
            return None
        if len({(x["height"], x["hash"]) for x in identities}) != 1:
            return None
        return identities

def cmd(*args):
    return subprocess.check_output(args,text=True,stderr=subprocess.STDOUT,timeout=25).strip()


def durable_state_volume(container):
    mounts=[m for m in container["Mounts"] if m["Destination"]=="/var/lib/rnode"]
    if len(mounts)!=1 or mounts[0]["Type"]!="volume":
        raise AssertionError("exactly one durable state volume required")
    m=mounts[0]
    return (m["Type"],m["Name"],m["Source"],m["Destination"],m["RW"])


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
        (out/"partition-result.json").write_text(json.dumps({
            "result": "FAIL_BOUNDED_WORKLOAD_PARTITION_REJOIN",
            "partition_cycles_completed": sum(r.get("result") == "PASS" for r in rounds),
            "failed_cycle": record["cycle"], "failed_phase": record["phase"],
            "rounds": rounds,
        }, indent=2)+"\n")
        raise
    else:
        record["result"] = "PASS"
    finally:
        (out/"partition-observations.json").write_text(json.dumps(rounds, indent=2)+"\n")


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--validators",type=int,choices=[4,8],required=True)
    ap.add_argument("--out",required=True);args=ap.parse_args();args.window=150
    if os.environ.get("GITHUB_ACTIONS")!="true" or os.environ.get("RUNNER_ENVIRONMENT")!="github-hosted":
        raise SystemExit("disposable hosted runner only")
    if os.environ.get("DEVNET_PREFIX")!="acceptance":
        raise SystemExit("unexpected namespace")
    d=WorkloadDrill(args);d.run()
    original=json.loads((d.out/"result.json").read_text())
    victim=a.node_name(args.validators-1)
    volume_before=durable_state_volume(json.loads(cmd("docker","inspect",victim))[0])
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
            record={"cycle":cycle+1,"isolated_validator":victim,"phase":"partition-install"}
            with record_cycle(d.out,rounds,record,capture_phase):
                before=a.finalized_identity(a.get(0,"/api/last-finalized-block"))
                cmd("docker","network","disconnect","acceptance",victim)
                networks=json.loads(cmd("docker","inspect","-f","{{json .NetworkSettings.Networks}}",victim))
                if "acceptance" in networks:raise AssertionError("partition was not installed")
                # Two 16-second escape/cadence cycles per N=8 sender plus margin:
                # a pre-registered 300-second window, not an unbounded retry.
                record["phase"]="online-progress"
                deadline=time.monotonic()+300;advanced=False;observed=[]
                record["before_height"]=before[0]
                record["online_observations"]=observed
                while time.monotonic()<deadline:
                    d.drive()
                    identities=[]
                    for i in range(args.validators-1):
                        height,hash_=a.finalized_identity(a.get(i,"/api/last-finalized-block"))
                        if not a.marker_present(a.probe(i,d.marker,hash_),d.value):
                            raise AssertionError("online validator lost finalized marker")
                        identities.append({"node":i,"height":height,"hash":hash_})
                    observed.append(identities)
                    (d.out/f"partition-{cycle+1}-live.json").write_text(json.dumps({
                        "cycle":cycle+1,"before_height":before[0],"observations":observed,
                        "remaining_seconds":max(0,deadline-time.monotonic())},indent=2)+"\n")
                    if all(x["height"]>=before[0]+3 for x in identities):
                        advanced=True;break
                    time.sleep(2)
                record["partition_progress_observed"]=advanced
                capture_phase(f"partition-{cycle+1}-before-healing")
                record["phase"]="healing"
                cmd("docker","network","connect","acceptance",victim)
                target=max(x["height"] for x in identities)
                recovered=d.wait_for_healing("all validators agree on finalized height/hash and retain marker after partition healing",
                                            lambda:d.finalized_marker_agreement(target))
                record["rejoined"]=recovered
                record["phase"]="retention"
                d.fixed_snapshots(original["initial_finalized"])
                record["original_finalized_snapshots_retained"]=True
                capture_phase(f"partition-{cycle+1}-healed")
                if durable_state_volume(json.loads(cmd("docker","inspect",victim))[0])!=volume_before:
                    raise AssertionError("rejoin replaced the durable mounts")
                record["same_volume"]=True
                record["phase"]="online-progress-verdict"
                if not advanced:
                    raise AssertionError("online supermajority did not advance in 300s; healed recovery was checked separately")
    finally:
        try:
            subprocess.run(["docker","network","connect","acceptance",victim],capture_output=True,timeout=25)
        except (OSError,subprocess.TimeoutExpired) as cleanup_error:
            (d.out/"partition-cleanup-error.txt").write_text(str(cleanup_error)+"\n")
        (d.out/"partition-observations.json").write_text(json.dumps(rounds,indent=2)+"\n")
    memory=cmd("docker","stats","--no-stream","--format","{{.Name}} {{.MemUsage}}")
    result={"result":"PASS_BOUNDED_WORKLOAD_PARTITION_REJOIN","source_sha":cmd("git","rev-parse","HEAD"),
        "validators":args.validators,"partition_cycles":len(rounds),"rounds":rounds,
        "resource_sample":memory,"quiet_chain_liveness":"NOT_ESTABLISHED",
        "scope":"retention and finality progression in these executions; not universal CBC safety"}
    (d.out/"partition-result.json").write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps(result),flush=True)

if __name__=="__main__":main()
