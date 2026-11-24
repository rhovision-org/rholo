"""Compare actual Lean executable output to the Python reference model."""
import sys
from dataclasses import asdict
from assurance import Policy, Entitlement, reconcile, digest, vested, release

alloc, schedules = set(), set()
end = 0
for line in open(sys.argv[1], encoding="utf-8"):
    fields = line.split()
    if not fields:
        continue
    if fields[0] == "ALLOC":
        a,t,l,v = map(int, fields[1:])
        assert (a,t) not in alloc
        alloc.add((a,t))
        p = Policy("synthetic",t,1,0,"external")
        m = reconcile([Entitlement("position","owner",a,"fixture",True)],p,digest(asdict(p)))
        assert m["payload"]["accounts"]["owner"] == {"liquid":l,"vesting":v}
    elif fields[0] == "VEST":
        a,d,h,v = map(int,fields[1:])
        assert (a,d,h) not in schedules
        schedules.add((a,d,h))
        assert vested(a,d,h)==v
        assert release(a,d,h,v)==0
    elif fields[0] == "FORMAL_WITNESS_END":
        end += 1
assert alloc == {(a,t) for a in range(33) for t in range(33)}
assert schedules == {(a,d,h) for a in range(33) for d in range(1,17) for h in range(d+3)}
assert end == 1
print(f"PASS Lean/Python agreement: {len(alloc)} allocations, {len(schedules)} vesting states")
