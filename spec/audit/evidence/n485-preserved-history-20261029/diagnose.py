import gzip,json,hashlib,argparse
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument("--fixture",type=Path,default=Path(__file__).with_name("block-metadata.dump.gz"));parser.add_argument("--out",type=Path,default=Path(__file__).with_name("parent-diagnostic.json"));args=parser.parse_args();p=args.fixture
assert hashlib.sha256(p.read_bytes()).hexdigest()=="1f077932696f542f5c35ae6e6e40d4d51adae6461afdab05fb4b427983ea1b25"
raw=gzip.decompress(p.read_bytes())
def varint(b,i):
 v=s=0
 while True:
  x=b[i];i+=1;v|=(x&127)<<s
  if x<128:return v,i
  s+=7
def fields(b):
 out={};i=0
 while i<len(b):
  tag,i=varint(b,i);n,w=tag>>3,tag&7
  if w==0:v,i=varint(b,i)
  elif w==2:
   z,i=varint(b,i);v=b[i:i+z];i+=z
  else:raise ValueError(w)
  out.setdefault(n,[]).append(v)
 return out
lines=raw.splitlines();lo=lines.index(b"HEADER=END")+1;hi=lines.index(b"DATA=END")
rows={}
for i in range(lo,hi,2):
 h=bytes.fromhex(lines[i].strip().decode());f=fields(bytes.fromhex(lines[i+1].strip().decode()))
 assert f[1]==[h]
 bonds={}
 for b in f.get(6,[]):
  q=fields(b);bonds[q[1][0]]=q.get(2,[0])[0]
 rows[h]={"height":f.get(2,[0])[0],"sender":f[3][0],"seq":f.get(4,[0])[0],"parents":f.get(5,[]),"bonds":bonds,"failed":f.get(21,[0])[0]}
ordered=sorted(rows,key=lambda h:(rows[h]["height"],h));index={h:i for i,h in enumerate(ordered)}
latest={};boundary={};boundary_height=0
for h in ordered:
 r=rows[h];seen=1<<index[h]
 for parent in r["parents"]:
  assert parent in rows and rows[parent]["height"]<r["height"]
  seen|=rows[parent]["seen"]
 r["seen"]=seen
 s=r["sender"]
 if s not in latest or r["seq"]>rows[latest[s]]["seq"]:latest[s]=h
 tip=max(rows[x]["height"] for x in latest.values())
 if not boundary:
  boundary=dict(latest);boundary_height=tip;continue
 if tip<=boundary_height:continue
 bonds=rows[latest[sorted(latest)[0]]]["bonds"];total=sum(bonds.values());stake=0;support=0
 for height,weight in sorted(((rows[latest[s]]["height"],w) for s,w in bonds.items() if s in latest),reverse=True):
  stake+=weight
  if stake*3>total*2:support=height;break
 def progressed(s):
  if s in latest and s in boundary:
   return rows[latest[s]]["seq"]>rows[boundary[s]]["seq"] or support-rows[latest[s]]["height"]>5
  if s in latest:return True
  return support>5
 if all(progressed(s) for s in bonds):boundary=dict(latest);boundary_height=support
def describe(h):
 r=rows[h];return {"hash":h.hex(),"height":r["height"],"seq":r["seq"],"sender":r["sender"].hex(),"failed":r["failed"]}
reports=[]
for s,mine in latest.items():
 old=dict(boundary);old[s]=mine;new=dict(old)
 for prior in rows[mine]["parents"]:
  ps=rows[prior]["sender"];selected=new.get(ps)
  if selected is None or rows[prior]["seen"] & ~rows[selected]["seen"]:
   candidate=latest.get(ps)
   new[ps]=candidate if candidate is not None and not(rows[prior]["seen"] & ~rows[candidate]["seen"]) else prior
 def regressions(selected):
  bad=[]
  for ps,h in selected.items():
   for prior in rows[mine]["parents"]:
    if rows[prior]["sender"]==ps and rows[prior]["seen"] & ~rows[h]["seen"]:
     bad.append({"selected":describe(h),"previous_self_parent":describe(prior)})
  return bad
 reports.append({"self":describe(mine),"has_advanced":s not in boundary or rows[mine]["seq"]>rows[boundary[s]]["seq"],"before_parents":[describe(x) for x in old.values()],"after_parents":[describe(x) for x in new.values()],"before_regressions":regressions(old),"after_regressions":regressions(new)})
out={"fixture_sha256":hashlib.sha256(p.read_bytes()).hexdigest(),"metadata_records":len(rows),"max_height":max(r["height"] for r in rows.values()),"genesis":[h.hex() for h in ordered if rows[h]["height"]==0],"boundary_height":boundary_height,"boundary":[describe(h) for h in boundary.values()],"reports":reports,"scope":"Independent diagnostic reconstruction; not a Rust validator execution or authentication of droplet snapshot 252064726"}
args.out.write_text(json.dumps(out,indent=2))
assert out["metadata_records"]==9198 and out["max_height"]==3121
assert out["reports"][0]["self"]["hash"]=="0881c1a75e3f72e7e47dc050eb588ea06b4f186f0ee0afebd7625f1a05709a53"
assert len(out["reports"][0]["before_regressions"])==1 and not out["reports"][0]["after_regressions"]
print(json.dumps({"records":len(rows),"max_height":out["max_height"],"boundary_height":boundary_height,"original_self":out["reports"][0]["self"],"before_regressions":len(out["reports"][0]["before_regressions"]),"after_regressions":len(out["reports"][0]["after_regressions"])}))
