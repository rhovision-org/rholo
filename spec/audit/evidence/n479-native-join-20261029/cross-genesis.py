import subprocess,gzip,json,hashlib
from pathlib import Path
import argparse
parser=argparse.ArgumentParser();parser.add_argument('--fixture-repo',required=True);args=parser.parse_args()
repo=args.fixture_repo;rev='15e2e16240424fe4a5a8aa9131e5462bdbe56f2a';prefix='spec/audit/evidence/n480-startup-metadata-20261029/'
z=subprocess.check_output(['git','-C',repo,'show',rev+':'+prefix+'block-metadata.dump.gz']);raw=gzip.decompress(z)
manifest=json.loads(subprocess.check_output(['git','-C',repo,'show',rev+':'+prefix+'fixture-manifest.json']))
assert hashlib.sha256(z).hexdigest()==manifest['compressed_sha256'];assert hashlib.sha256(raw).hexdigest()==manifest['uncompressed_sha256']
def varint(b,i):
 v=0;s=0
 while True:
  x=b[i];i+=1;v|=(x&127)<<s
  if x<128:return v,i
  s+=7;assert s<70
def fields(b):
 i=0;out={}
 while i<len(b):
  tag,i=varint(b,i);n,w=tag>>3,tag&7
  if w==0:v,i=varint(b,i)
  elif w==2:
   length,i=varint(b,i);v=b[i:i+length];i+=length
  else:raise ValueError(w)
  out.setdefault(n,[]).append(v)
 return out
a=raw.splitlines();start=a.index(b'HEADER=END')+1;end=a.index(b'DATA=END');payload=a[start:end];assert len(payload)%2==0
rows={}
for i in range(0,len(payload),2):
 key=bytes.fromhex(payload[i].strip().decode());value=bytes.fromhex(payload[i+1].strip().decode());f=fields(value);assert f[1]==[key]
 rows[key]={'height':f.get(2,[0])[0],'parents':f.get(5,[]),'encoded_sha256':hashlib.sha256(value).hexdigest()}
assert len(rows)==manifest['records']
target=bytes.fromhex('c20645c3f68aad1717bed1290e7097d12ac1d5f14cd393263273fdbe38828c80');seen=set();roots=set();todo=[target]
while todo:
 h=todo.pop()
 if h in seen:continue
 seen.add(h);r=rows[h]
 if not r['parents']:roots.add(h)
 for p in r['parents']:assert rows[p]['height']<r['height'];todo.append(p)
assert roots=={bytes.fromhex('c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26')}
out={'fixture_source':rev,'fixture_sha256':manifest['uncompressed_sha256'],'metadata_proto':'models/proto/casper.proto:300','observed_first_overflow_hash':target.hex(),'old_chain_height':rows[target]['height'],'metadata_value_sha256':rows[target]['encoded_sha256'],'ancestry_records':len(seen),'ancestry_genesis':[x.hex() for x in roots],'current_live_genesis':'133b1d307b1750129cf0cfb69c4c8f950da21c4e8db2a6a076464d0ccfd98d44','sender_attribution':'not established','scope':'Hash correlation and full metadata ancestry, not authentication of the sending peer'}
expected=json.loads((Path(__file__).resolve().parent/'cross-genesis-result.json').read_text());assert out==expected
print(json.dumps(out,indent=2))
