"""Launch a bounded passive observer using retained public genesis inputs."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--name")
p.add_argument("--port", type=int)
p.add_argument("--timeout", type=int, default=3600)
p.add_argument("--report-only", type=Path)
cli = p.parse_args()

def outcome(report):
    cases = report.get("cases", [])
    complete = (
        len(cases) == 3
        and [c.get("label") for c in cases] == ["fresh-1", "restart", "wrong-genesis"]
        and all(c.get("pass") is True for c in cases)
    )
    return 0 if complete and report.get("diagnostic_pass") is True and report.get("local_services_and_firewall_unchanged") is True else 1

if cli.report_only:
    saved = json.loads(cli.report_only.read_text())
    print(json.dumps({k: saved.get(k) for k in ("source_sha", "diagnostic_pass", "error", "local_services_and_firewall_unchanged")}))
    raise SystemExit(outcome(saved))
if not cli.name or cli.port is None:
    p.error("--name and --port are required for a new experiment")
root = Path("/home/aria/tmp/rholo-recovery-probe-20261029")
cfg = json.loads((root / "public-genesis/manifest.json").read_text())
spec = importlib.util.spec_from_file_location("proof", root / "live_acceptance_with_genesis.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
a = argparse.Namespace(
    binary=root / "rnode", expected_source="6ff5947bea886ff3ed882a5af4ee5d408c5b6692",
    trusted_genesis="c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26",
    bootstrap_host="127.0.0.1", network_api=["http://127.0.0.1:40403"],
    out=root / cli.name, timeout=cli.timeout, genesis_flags=cfg["public_flags"],
)
assert a.out.parent == root
assert hashlib.sha256(a.binary.read_bytes()).hexdigest() == "484c382cec31121466f314c1f1e7f8511337dfa2a3eadc71ba12bd4f654ddce1"
a.out.mkdir(mode=0o700)
r = dict(source_sha=a.expected_source, binary_sha256=hashlib.sha256(a.binary.read_bytes()).hexdigest(),
         scope="Configured fresh-store diagnostic; four-validator acceptance BLOCKED",
         public_genesis_sha256=cfg["sha256"], cases=[], safety_before=m.safety_readings())
try:
    ok = m.case(a, r, "fresh-1", a.out / "empty-store", cli.port)
    if ok:
        ok = m.case(a, r, "restart", a.out / "empty-store", cli.port + 10)
    if ok:
        ok = m.case(a, r, "wrong-genesis", a.out / "empty-store", cli.port + 20, wrong=True)
    r["diagnostic_pass"] = ok and len(r["cases"]) == 3
except Exception as e:
    r["error"] = repr(e)
    print("ERROR", repr(e), flush=True)
finally:
    r["safety_after"] = m.safety_readings()
    r["local_services_and_firewall_unchanged"] = r["safety_before"] == r["safety_after"]
    (a.out / "acceptance.json").write_text(json.dumps(r, indent=2) + "\n")
    print(json.dumps({k: v for k, v in r.items() if k not in ("cases", "safety_before", "safety_after")}), flush=True)
raise SystemExit(outcome(r))
