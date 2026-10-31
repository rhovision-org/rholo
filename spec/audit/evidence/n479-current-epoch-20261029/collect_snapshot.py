import urllib.request,json,concurrent.futures,datetime,pathlib,hashlib,os,shutil,subprocess
urls={'A':'http://127.0.0.1:40403','D':'http://127.0.0.1:41403','B':'http://167.99.43.214:40403','C':'http://167.99.43.214:41403'}
def get(url):
 with urllib.request.urlopen(url,timeout=8) as r:return json.load(r)
def sample(item):
 label,url=item;s=get(url+'/api/status');f=get(url+'/api/last-finalized-block')['blockInfo'];g=get(url+'/api/blocks/0/0')
 assert len(g)==1 and g[0]['blockNumber']==0
 return label,{'api_from_host_a':url,'status':{k:s[k] for k in ['version','networkId','shardId','latestBlockNumber','peers','degraded','acceptsDeploys']},'finalized':{k:f[k] for k in ['blockNumber','blockHash','postStateHash']},'genesis_hash':g[0]['blockHash'],'genesis_bonds':len(g[0]['bonds']),'genesis_stake_total':sum(x['stake'] for x in g[0]['bonds'])}
report={'sample_started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'mode':'Read-only APIs, public genesis inputs and unit/listener/resource readings; no process control or transaction'}
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:report['validators']=dict(pool.map(sample,urls.items()))
inputs={}
for name in ['bonds.txt','wallets.txt']:
 b=subprocess.check_output(['sudo','-n','cat','/var/lib/rnode/genesis/'+name])
 inputs[name]={'sha256':hashlib.sha256(b).hexdigest(),'noncomment_records':sum(bool(l.strip()) and not l.lstrip().startswith(b'#') for l in b.splitlines())}
report['host_a_public_inputs']=inputs
mem={l.split(':')[0]:int(l.split()[1]) for l in pathlib.Path('/proc/meminfo').read_text().splitlines() if l.startswith(('MemTotal:','MemAvailable:'))}
report['host_a_resources']={'logical_cpus':os.cpu_count(),'memory_kib':mem,'data_filesystem_bytes':shutil.disk_usage('/var/lib').total}
units={}
for name in ['rnode','rnode-d']:
 raw=subprocess.check_output(['systemctl','show',name,'-p','MainPID','-p','NRestarts','-p','ActiveState','-p','ExecMainStartTimestamp'],text=True)
 info=dict(l.split('=',1) for l in raw.splitlines() if '=' in l);pid=info['MainPID'];assert pid.isdigit() and int(pid)>1
 info['running_executable_sha256']=subprocess.check_output(['sudo','-n','sha256sum','/proc/'+pid+'/exe'],text=True).split()[0];units[name]=info
report['host_a_units']=units
listeners=[]
for l in subprocess.check_output(['ss','-Hlntu'],text=True).splitlines():
 a=l.split()
 if len(a)>4 and a[4].rsplit(':',1)[-1].isdigit() and int(a[4].rsplit(':',1)[-1]) in set(range(40400,40406))|set(range(41400,41406)):listeners.append({'transport':a[0],'local':a[4]})
report['host_a_validator_listeners']=listeners
report['sample_finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
print(json.dumps(report))
