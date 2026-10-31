#!/usr/bin/env python3
"""Fail-closed, same-volume proposer restart witness on disposable N8 GitHub Actions.

This proves active proposals after an independently stopped validator is restarted,
with exactly one signed deploy directed at that validator per cycle. A chain with
--no-autopropose is not expected to make progress without an input trigger.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('acceptance', ROOT/'tools/consensus-acceptance-drill.py')
a = importlib.util.module_from_spec(spec)
spec.loader.exec_module(a)
PROPOSAL_RE = re.compile(r'proposed and added block #(\d+) \(seq (\d+)\)')


def command(*args, timeout=45):
    return subprocess.run(args, text=True, capture_output=True, check=True, timeout=timeout).stdout


def docker_logs(name):
    result = subprocess.run(['docker', 'logs', name], text=True, capture_output=True, check=True, timeout=45)
    return result.stdout + result.stderr


def inspect(name):
    return json.loads(command('docker', 'inspect', name))[0]


def durable_volume(node):
    mounts = [x for x in node.get('Mounts', []) if x.get('Destination') == '/var/lib/rnode']
    if len(mounts) != 1 or mounts[0].get('Type') != 'volume':
        raise AssertionError('Expected exactly one persistent Docker state volume')
    m = mounts[0]
    return (m['Name'], m['Source'], m['Destination'])


def proposals(log):
    rows = [{'height': int(h), 'seq': int(seq)} for h, seq in PROPOSAL_RE.findall(log)]
    if any(b['seq'] <= a['seq'] for a, b in zip(rows, rows[1:])):
        raise AssertionError('Repeated or regressed local proposal sequence')
    return rows


def post_restart_proposals(before, after):
    if not before:
        raise AssertionError('Missing pre-restart local proposal baseline')
    if not after or after[0]['seq'] <= before[-1]['seq']:
        raise AssertionError('No new successful local proposal after restart')
    if any(b['seq'] <= a['seq'] for a, b in zip(after, after[1:])):
        raise AssertionError('Post-restart proposal sequence regressed')
    return after


def healthy(validator):
    health = a.get(validator, '/health')
    operator = a.get(validator, '/api/v1/status/operator', admin=True)
    # Missing fields are not evidence of readiness or non-divergence.
    return (health.get('ready') is True
        and type(health.get('orphanBodies')) is int and health['orphanBodies'] == 0
        and type(health.get('pipelineFaults')) is int and health['pipelineFaults'] == 0
        and health.get('proposalHalted') is False
        and type(operator.get('unrestorableRecords')) is int
        and operator['unrestorableRecords'] == 0)


def bounded_wait(check, deadline, label):
    last_error = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
            last_error = repr(error)
        time.sleep(2)
    raise AssertionError(f'{label} not demonstrated before deadline: {last_error}')


def signed_deploy_to_restarted_validator(index, name, cycle):
    status = a.get(index, '/api/v1/status')
    if not isinstance(status.get('latestBlockNumber'), int) or not status.get('shardId'):
        raise AssertionError('Cannot construct valid-after deploy without node status')
    marker = f'post-restart:{uuid.uuid4().hex}:{cycle}'
    term = f'@{json.dumps(marker)}!({cycle})'
    write = subprocess.run(['docker', 'exec', '-i', name, 'sh', '-c', 'cat > /tmp/post-restart.rho'],
                           input=term, text=True, capture_output=True, timeout=20)
    if write.returncode != 0:
        raise AssertionError(f'Could not write signed-deploy term: {write.stderr[-200:]}')
    result = subprocess.run(['docker', 'exec', name, 'rnode', '--grpc-host', 'localhost', 'deploy',
        '--phlo-limit', '1000000', '--phlo-price', '1', '--private-key', a.fuzz.DEPLOYER_PRIV,
        '--shard-id', status['shardId'], '--valid-after-block-number', str(status['latestBlockNumber']),
        '/tmp/post-restart.rho'], capture_output=True, text=True, timeout=45)
    if result.returncode != 0:
        raise AssertionError('Direct signed deploy was not accepted: ' + (result.stderr+result.stdout)[-350:])
    return {'marker':marker, 'valid_after_block_number':status['latestBlockNumber'],
            'deploy_ack':result.stdout[-250:]}


def check_retained_snapshots(out):
    original = json.loads((out/'result.json').read_text())
    events = (out/'observations.jsonl').read_text().splitlines()
    first = json.loads(events[0])
    marker, value = first['marker'], first['expected']
    for identity in original['initial_finalized']:
        node = identity['node']
        if not a.marker_present(a.probe(node,marker,identity['hash']),value):
            raise AssertionError(f'Node {node} lost previously finalized snapshot')


def run(out, validators, cycles, window):
    victim = validators - 1
    name = a.node_name(victim)
    before = inspect(name)
    if before.get('Name') != '/'+name or not before['State']['Running']:
        raise AssertionError('Unexpected/nonrunning validator container')
    if before['HostConfig'].get('NetworkMode') != 'acceptance':
        raise AssertionError('Unexpected Docker network')
    if '--validator-private-key-path' not in (before['Config'].get('Cmd') or []):
        raise AssertionError('Restart target has no configured validator key path')
    volume = durable_volume(before)
    if volume[0] != name+'-data':
        raise AssertionError('Unexpected persistent volume; refusing restart')
    checkpoint = {'source_sha':command('git','rev-parse','HEAD'), 'result':'INCOMPLETE',
        'target':name, 'state_volume':volume[0], 'cycles':[],
        'scope':'N8 active proposer after same-volume restart with one direct signed deploy per cycle; not quiet-chain liveness'}
    destination = out/'post-restart-result.json'
    def save():
        destination.write_text(json.dumps(checkpoint,indent=2)+'\n')
    save()
    try:
        for cycle in range(1,cycles+1):
            row = {'cycle':cycle, 'phase':'pre-restart'}
            checkpoint['cycles'].append(row)
            save()
            old = proposals(docker_logs(name))
            if not old:
                raise AssertionError('Missing pre-restart proposal baseline')
            row['pre_restart_seq']=old[-1]['seq']
            baseline=inspect(name)
            if durable_volume(baseline)!=volume:
                raise AssertionError('Validator volume changed before restart')
            row['phase']='stop-start'
            command('docker','kill','--signal','KILL',name)
            command('docker','start',name)
            restarted=inspect(name)
            if restarted['Id']!=baseline['Id'] or durable_volume(restarted)!=volume:
                raise AssertionError('Restart changed container identity or persistent state volume')
            if not restarted['State']['Running'] or restarted['State'].get('OOMKilled'):
                raise AssertionError('Restarted validator is stopped or OOM-killed')
            if restarted['Config'].get('Cmd') != baseline['Config'].get('Cmd'):
                raise AssertionError('Validator key/configuration changed across restart')
            row['same_container_and_volume']=True
            row['phase']='recovery-ready'
            bounded_wait(lambda:healthy(victim),time.monotonic()+window,'post-restart readiness')
            row['phase']='direct-deploy'
            # After readiness: exactly one direct signed deploy. No d.drive loop.
            row['signed_deploy']=signed_deploy_to_restarted_validator(victim,name,cycle)
            row['phase']='local-proposal'
            def fresh():
                all_rows=proposals(docker_logs(name))
                new_rows=[x for x in all_rows if x['seq']>old[-1]['seq']]
                return post_restart_proposals(old,new_rows) if new_rows else None
            new_rows=bounded_wait(fresh,time.monotonic()+window,'new successful local proposal')
            row['post_restart_sequences']=new_rows
            row['phase']='retained-finality'
            bounded_wait(lambda:all(healthy(i) for i in range(validators)),
                         time.monotonic()+window,'all validators ready after direct proposal')
            check_retained_snapshots(out)
            row['retained_original_finalized_snapshots']=True
            row['result']='PASS';row['phase']='complete'
            save()
        checkpoint['result']='PASS_ACTIVE_POST_RESTART_PROPOSAL'
    except Exception as error:
        checkpoint['result']='FAIL_ACTIVE_POST_RESTART_PROPOSAL'
        checkpoint['error']={'type':type(error).__name__,'message':str(error)}
        save()
        raise
    save()
    print(json.dumps({'result':checkpoint['result'], 'target':name,
        'cycles':[{'cycle':r['cycle'],'pre':r['pre_restart_seq'],
                   'post':[x['seq'] for x in r['post_restart_sequences']]} for r in checkpoint['cycles']]}),flush=True)


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--validators',type=int,choices=[8],required=True)
    ap.add_argument('--cycles',type=int,choices=[2],default=2)
    ap.add_argument('--window',type=int,default=150)
    ap.add_argument('--out',type=Path,required=True)
    args=ap.parse_args()
    if os.environ.get('GITHUB_ACTIONS')!='true' or os.environ.get('RUNNER_ENVIRONMENT')!='github-hosted':
        ap.error('Disposable GitHub-hosted Actions runner required')
    if os.environ.get('DEVNET_PREFIX')!='acceptance' or os.environ.get('DEVNET_NETWORK')!='acceptance':
        ap.error('Expected isolated acceptance namespace')
    if not 60<=args.window<=300:
        ap.error('Bounded recovery window must be 60–300 seconds')
    args.out.mkdir(parents=True, exist_ok=True)
    run(args.out,args.validators,args.cycles,args.window)


if __name__=='__main__':
    main()
