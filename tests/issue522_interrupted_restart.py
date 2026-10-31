#!/usr/bin/env python3
"""Disposable N4 interruption witness; never operates on public validators."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from urllib.request import urlopen

PREFIX = os.environ.get('DEVNET_PREFIX')
if PREFIX != 'issue522interrupt':
    raise SystemExit('refusing to touch containers outside the dedicated issue522interrupt prefix')

OUT = Path(sys.argv[1] if len(sys.argv) > 1 else 'target/issue522-interrupted')
OUT.mkdir(parents=True, exist_ok=True)
EVENTS = OUT / 'events.jsonl'
VALIDATORS = 4
VICTIM = f'{PREFIX}-validator-1'


def record(**fields):
    with EVENTS.open('a') as f:
        f.write(json.dumps({'utc': time.time(), **fields}) + '\n')


def docker(*argv):
    p = subprocess.run(['docker', *argv], text=True, capture_output=True, timeout=35)
    if p.returncode:
        raise RuntimeError(f'docker {argv}: rc={p.returncode} stderr={p.stderr[:500]}')
    return p.stdout.strip()


def get(port, route):
    with urlopen(f'http://127.0.0.1:{port}{route}', timeout=3) as rsp:
        return json.load(rsp)


def snapshots():
    output = []
    for i in range(VALIDATORS):
        health = get(40403 + i * 1000, '/health')
        fin = get(40403 + i * 1000, '/api/last-finalized-block')['blockInfo']
        output.append({'node': i, 'health': health,
                       'final': [int(fin['blockNumber']), fin['blockHash']]})
    return output


def valid(samples, floor=0):
    if len(samples) != VALIDATORS or len({tuple(s['final']) for s in samples}) != 1:
        return False
    if samples[0]['final'][0] < floor:
        return False
    return all(s['health'].get('ready') is True and
               s['health'].get('orphanBodies') == 0 and
               s['health'].get('pipelineFaultsOutstanding') == 0 and
               s['health'].get('proposalHalted') is False
               for s in samples)


def wait_agreement(floor, deadline, label):
    latest = None
    while time.monotonic() < deadline:
        try:
            latest = snapshots()
            if valid(latest, floor):
                record(phase=label, agreed=latest)
                return latest
        except (OSError, ValueError, KeyError, TimeoutError) as exc:
            record(phase=label, transient=type(exc).__name__)
        time.sleep(2)
    record(phase=label, failure='agreement deadline', last=latest)
    raise RuntimeError(f'{label}: no unanimous healthy finalized identity above {floor}')


def proposal_count():
    p = subprocess.run(['docker', 'logs', VICTIM], text=True,
                       capture_output=True, timeout=35)
    if p.returncode:
        raise RuntimeError(f'cannot read {VICTIM} logs: {p.stderr[:300]}')
    return (p.stdout + p.stderr).count('proposed and added block')


def main():
    before = wait_agreement(2, time.monotonic() + 150, 'baseline')
    initial = before[0]['final']
    count_before = proposal_count()
    volume = f'{VICTIM}-data'
    docker('volume', 'inspect', volume)
    record(phase='baseline', hash=initial[1], height=initial[0], volume=volume,
           proposal_count=count_before)
    docker('kill', '--signal=KILL', VICTIM)
    record(phase='first_sigkill', running=docker('inspect', '-f', '{{.State.Running}}', VICTIM))
    docker('start', VICTIM)
    time.sleep(0.5)
    docker('kill', '--signal=KILL', VICTIM)
    is_running = docker('inspect', '-f', '{{.State.Running}}', VICTIM)
    record(phase='interrupted_start', running=is_running)
    if is_running != 'false':
        raise RuntimeError('interrupted restart incorrectly remained running')
    try:
        body = get(41403, '/health')
    except Exception as exc:
        record(phase='interrupted_start', refusal=repr(exc))
    else:
        record(phase='interrupted_start', unexpected_health=body)
        if body.get('ready') is True:
            raise RuntimeError('dead validator advertised ready after SIGKILL')
    docker('volume', 'inspect', volume)
    docker('start', VICTIM)
    after = wait_agreement(initial[0] + 3, time.monotonic() + 240, 'recovered')
    count_after = proposal_count()
    record(phase='post_restart_proposals', before=count_before, after=count_after)
    if count_after <= count_before:
        raise RuntimeError('victim has not proposed since interruption')
    if after[0]['final'][1] == initial[1]:
        raise RuntimeError('finalized identity did not advance after recovery')
    result = {'verdict': 'PASS', 'source': os.environ.get('SOURCE_SHA'),
              'initial': initial, 'final': after[0]['final'],
              'victim': VICTIM, 'before_proposals': count_before,
              'after_proposals': count_after, 'preserved_volume': volume}
    (OUT / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    try:
        main()
    except Exception as exc:
        record(verdict='FAIL', error=repr(exc))
        print(f'FAIL: {exc!r}', file=sys.stderr)
        sys.exit(1)
