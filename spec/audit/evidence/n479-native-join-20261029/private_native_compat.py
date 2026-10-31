"""Bounded private compatibility proof using a closed historical observer snapshot."""
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import time
import urllib.request

HISTORICAL = Path('/home/aria/tmp/rholo-live-proof-20261027')
SNAPSHOT = HISTORICAL / 'observer-seeded-copy-r1'
PUBLIC = Path('/home/aria/tmp/rholo-recovery-probe-20261029/public-genesis')
TRUSTED = 'c596bef4bb678b3ac0cdb931b5b6968653209ca40bc35b6fec0050254d479b26'
LIVE_GENESIS = '133b1d307b1750129cf0cfb69c4c8f950da21c4e8db2a6a076464d0ccfd98d44'
OLD = Path('/usr/local/bin/rnode-a5259969')
NEW = HISTORICAL / 'rnode'
CURRENT = Path('/usr/local/bin/rnode-d1af45d1')
BINARIES = {
    str(OLD): ('a5259969a7c5673ccdfc5dad97981d0a3607fcf0', 'a5b9e9de4f3a75833da50aef67a845ddc350248ec3113040bc727dbcb9bdfa01'),
    str(NEW): ('3607cf9f7832054376ac33a4c1c56fd0b701e89d', 'b4b2c8c3b7ae12ab16fae4cfd31d9918e687f6c41f0c28befc4f05c9721b8fd2'),
    str(CURRENT): ('d1af45d1db3f5fe2fa6047ad6c8a72b654d041df', '76547fa988a4b2e216feb373cb17affa9cc4217ac9595ea97e77a2e1db3a8745'),
}
tag = dt.datetime.now(dt.timezone.utc).strftime('%H%M%S')
ROOT = Path('/home/aria/tmp') / ('rholo-native-compat-20261029-' + tag)
ROOT.mkdir(mode=0o700)
DOMAIN = 'private-n457-' + tag
PREFIX = 'rholo-private-n457-' + tag + '-'
units = []
report = {'scope': 'Private historical-chain compatibility; not a live-network acceptance',
          'network_id': DOMAIN, 'trusted_genesis': TRUSTED, 'cases': [], 'commands': [],
          'started_utc': dt.datetime.now(dt.timezone.utc).isoformat()}


def save():
    (ROOT / 'result.json').write_text(json.dumps(report, indent=2) + '\n')


def run(args, check=True):
    return subprocess.run(list(map(str, args)), capture_output=True, text=True, timeout=30, check=check)


def get(base, route):
    with urllib.request.urlopen(base + route, timeout=3) as response:
        return json.load(response)


def live_safety():
    urls = ['http://127.0.0.1:40403', 'http://127.0.0.1:41403',
            'http://167.99.43.214:40403', 'http://167.99.43.214:41403']
    def sample(base):
        status = get(base, '/api/status')
        genesis = get(base, '/api/blocks/0/0')[0]['blockHash']
        assert genesis == LIVE_GENESIS
        assert status.get('degraded') == 'normal'
        assert status.get('pool', {}).get('deploys', 0) == 0
        return {'api': base, 'status': status, 'genesis': genesis}
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        nodes = list(pool.map(sample, urls))
    services = run(['systemctl', 'show', 'rnode', 'rnode-d', '-p', 'Id', '-p', 'MainPID',
                    '-p', 'NRestarts', '-p', 'ExecMainStartTimestamp']).stdout
    firewall = run(['sudo', '-n', 'ufw', 'status', 'verbose']).stdout
    available = int(next(x for x in Path('/proc/meminfo').read_text().splitlines()
                         if x.startswith('MemAvailable:')).split()[1])
    assert available > 10 * 1024 * 1024, 'stop private proof to preserve live headroom'
    return {'nodes': nodes, 'services': services, 'firewall_sha256': hashlib.sha256(firewall.encode()).hexdigest(),
            'available_memory_kib': available}


def properties(unit):
    text = run(['systemctl', 'show', unit, '-p', 'ActiveState', '-p', 'Result',
                '-p', 'ExecMainStatus', '-p', 'MemoryCurrent', '-p', 'MemoryPeak',
                '-p', 'MainPID', '-p', 'CPUUsageNSec'], check=False).stdout
    return dict(line.split('=', 1) for line in text.splitlines() if '=' in line)


def logs(unit):
    return run(['sudo', '-n', 'journalctl', '-u', unit, '--no-pager', '-o', 'cat'], check=False).stdout


def stop(unit):
    assert unit.startswith(PREFIX)
    run(['sudo', '-n', 'systemctl', 'stop', unit], check=False)
    text = logs(unit)
    (ROOT / (unit + '.log')).write_text(text)
    run(['sudo', '-n', 'systemctl', 'reset-failed', unit], check=False)
    return text


def start(binary, data, port, label, peer=None, standalone=False, expected=TRUSTED):
    unit = PREFIX + label
    args = [str(binary), '--profile', 'docker', 'run', '--config-file', str(ROOT / 'network.conf'), '--dev-mode', '--no-upnp',
            '--network-id', DOMAIN, '--shard-name', 'testnet', '--parent-shard-id', '/',
            '--host', '127.0.0.1', '--protocol-port', str(port), '--discovery-port', str(port + 4),
            '--api-port-grpc-external', str(port + 1), '--api-port-grpc-internal', str(port + 2),
            '--api-port-http', str(port + 3), '--api-port-admin-http', str(port + 5),
            '--api-host', '127.0.0.1', '--data-dir', str(data), '--epoch-length', '10',
            '--quarantine-length', '10', '--bond-minimum', '1', '--bond-maximum', '250',
            '--thread-pool-size', '2', *public_flags]
    if pin_support[str(binary)]:
        args += ['--expected-genesis-hash', expected]
    if standalone:
        args += ['--standalone']
    if peer:
        args += ['--bootstrap', peer]
    assert not any(x in args for x in ['--validator-private-key', '--validator-private-key-path',
                                     '--deployer-private-key', '--attest-on-new-blocks',
                                     '--propose-on-deploy', '--autopropose'])
    command = ['sudo', '-n', 'systemd-run', '--quiet', '--unit', unit, '--uid', 'aria',
               '--property', 'MemoryMax=' + ('768M' if standalone else '1024M'),
               '--property', 'CPUQuota=' + ('25%' if standalone else '50%'),
               '--property', 'CPUWeight=1', '--property', 'IOWeight=1', '--property', 'Nice=19',
               '--property', 'TasksMax=128', '--property', 'TimeoutStopSec=10',
               '--property', 'KillMode=control-group', '--property', 'RuntimeMaxSec=1200', *args]
    units.append(unit)
    report['commands'].append({'unit': unit, 'command': command})
    run(command)
    save()
    return unit


def wait_boot(unit, timeout=100):
    base = 'http://127.0.0.1:43403'
    started = time.monotonic()
    while time.monotonic() - started < timeout:
        state = properties(unit)
        if state.get('ActiveState') in ('failed', 'inactive'):
            report['bootstrap_exit'] = state
            selected = [line for line in logs(unit).splitlines()
                        if re.search(r'(?i)error|fatal|below.*floor|refus', line)]
            raise RuntimeError('private bootstrap exited: ' + '\n'.join(selected[:8]))
        try:
            status = get(base, '/api/status')
            final = get(base, '/api/last-finalized-block')['blockInfo']
            genesis = get(base, '/api/blocks/0/0')[0]['blockHash']
            if 'Making a transition to Running state.' in logs(unit):
                assert genesis == TRUSTED and status['networkId'] == DOMAIN
                return {'status': status, 'finalized': final, 'genesis': genesis}
        except (OSError, KeyError, ValueError):
            pass
        time.sleep(2)
    raise RuntimeError('private bootstrap did not reach Running within the bounded setup window')


def case(binary, label, data, peer, anchor, fresh=False, wrong=False, timeout=420):
    if fresh:
        data.mkdir()
        assert not list(data.iterdir())
    item = {'label': label, 'source_sha': BINARIES[str(binary)][0],
            'binary_sha256': BINARIES[str(binary)][1], 'state_at_start': 'empty' if fresh else 'persistent',
            'empty_listing_before_start': [] if fresh else None, 'pass': False, 'samples': []}
    report['cases'].append(item)
    unit = start(binary, data, 44400, label, peer=peer, expected='0' * 64 if wrong else TRUSTED)
    started = time.monotonic()
    next_sample = 0
    try:
        while time.monotonic() - started < timeout:
            text = logs(unit)
            running = 'Making a transition to Running state.' in text
            state = properties(unit)
            item['resource_sample'] = state
            if 'WrongNetwork(' in text:
                item['terminal_failure'] = 'network identity mismatch before fringe admission'
                break
            if wrong:
                if state.get('ActiveState') in ('failed', 'inactive'):
                    item['pass'] = (not running and TRUSTED in text and '0' * 64 in text
                                    and bool(re.search(r'genesis.*(match|expected|refus)', text, re.I)))
                    item['exit'] = state
                    break
            else:
                try:
                    base = 'http://127.0.0.1:44403'
                    status = get(base, '/api/status')
                    final = get(base, '/api/last-finalized-block')['blockInfo']
                    genesis = get(base, '/api/blocks/0/0')[0]['blockHash']
                    restored = 'LFS state is successfully restored' in text
                    item['last'] = {'status': status, 'finalized': final, 'genesis': genesis,
                                    'running_log': running, 'restored_log': restored}
                    no_errors = 'active-bonds leaf not found' not in text and 'receiver state full' not in text
                    if restored and not no_errors:
                        item['terminal_failure'] = 'post-restore required state or receiver-capacity failure'
                        break
                    if (running and (restored or not fresh) and no_errors and genesis == TRUSTED
                            and status['latestBlockNumber'] >= anchor['status']['latestBlockNumber']
                            and final['blockHash'] == anchor['finalized']['blockHash']):
                        item['pass'] = True
                        break
                except (OSError, KeyError, ValueError):
                    pass
                if state.get('ActiveState') in ('failed', 'inactive'):
                    item['exit'] = state
                    break
            if time.monotonic() - started >= next_sample:
                print(json.dumps({'case': label, 'seconds': round(time.monotonic() - started),
                                  'height': item.get('last', {}).get('status', {}).get('latestBlockNumber'),
                                  'running': running, 'unit': state.get('ActiveState')}), flush=True)
                item['samples'].append(item.get('last', {}))
                private_pids = [properties(u).get('MainPID') for u in units]
                connections = run(['sudo', '-n', 'ss', '-Htnp', 'state', 'established']).stdout
                private_rows = [line for line in connections.splitlines()
                                if any(pid and pid != '0' and ('pid=' + pid + ',') in line
                                       for pid in private_pids)]
                assert all('127.0.0.1:' in line and not re.search(r'(?<![\d.])(?:164\.90|167\.99)\.', line)
                           for line in private_rows), 'non-loopback private proof connection'
                item.setdefault('tcp_samples', []).append(private_rows)
                live_safety()
                next_sample += 30
                save()
            time.sleep(2)
    finally:
        item['duration_seconds'] = round(time.monotonic() - started, 3)
        text = stop(unit)
        item['log_file'] = unit + '.log'
        item['errors'] = {word: text.lower().count(word) for word in
                          ['active-bonds leaf not found', 'receiver state full', 'out of memory']}
        save()
        print(json.dumps({'case': label, 'pass': item['pass'], 'seconds': item['duration_seconds'],
                          'errors': item['errors']}), flush=True)
    return item['pass']


def source_fingerprints():
    result = {}
    for name in ['blockstorage', 'dagstorage', 'rspace/history', 'rspace/cold']:
        for file in (SNAPSHOT / name).glob('*.mdb'):
            if file.name != 'lock.mdb':
                result[str(file.relative_to(HISTORICAL))] = hashlib.sha256(file.read_bytes()).hexdigest()
    return result


try:
    report['safety_before'] = live_safety()
    seeded = json.loads((HISTORICAL / 'seeded-proof.json').read_text())
    assert seeded['controls'][0]['label'] == 'seeded-first' and seeded['controls'][0]['pass'] is True
    report['historical_snapshot'] = str(SNAPSHOT)
    report['source_before'] = source_fingerprints()
    assert report['source_before']
    pin_support = {}
    for name, (source, digest) in BINARIES.items():
        assert hashlib.sha256(Path(name).read_bytes()).hexdigest() == digest
        help_text = run([name, '--profile', 'docker', 'run', '--help']).stdout
        pin_support[name] = '--expected-genesis-hash' in help_text
    assert pin_support[str(NEW)]
    report['runtime_trust_pin_support'] = pin_support
    # Defaults are resolved before CLI merging in these revisions; set both ends explicitly.
    (ROOT / 'network.conf').write_text(
        'protocol-server.network-id = "' + DOMAIN + '"\n'
        'protocol-client.network-id = "' + DOMAIN + '"\n')
    cfg = json.loads((PUBLIC / 'manifest.json').read_text())
    for filename, digest in cfg['sha256'].items():
        assert filename in ('bonds.txt', 'wallets.txt')
        assert hashlib.sha256((PUBLIC / filename).read_bytes()).hexdigest() == digest
    public_flags = list(cfg['public_flags'])
    (ROOT / 'public-genesis').mkdir()
    for flag, filename in [('--bonds-file', 'bonds.txt'), ('--wallets-file', 'wallets.txt')]:
        destination = ROOT / 'public-genesis' / filename
        shutil.copy2(PUBLIC / filename, destination)
        public_flags[public_flags.index(flag) + 1] = str(destination)
    report['public_genesis_sha256'] = cfg['sha256']
    for port in [*range(43400, 43406), *range(44400, 44406)]:
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', port))
    data = ROOT / 'bootstrap'
    data.mkdir()
    for name in ['blockstorage', 'dagstorage', 'rspace/history', 'rspace/cold', 'transaction']:
        source = SNAPSHOT / name
        if source.exists():
            shutil.copytree(source, data / name, ignore=shutil.ignore_patterns('lock.mdb'))
    for name in ['store-format', 'shard-id']:
        shutil.copy2(SNAPSHOT / name, data / name)
    assert not any('key' in p.name or 'certificate' in p.name for p in data.iterdir())
    boot = start(OLD, data, 43400, 'bootstrap-1', standalone=True)
    anchor = wait_boot(boot)
    report['bootstrap_baseline'] = anchor
    peer = f"rnode://{anchor['status']['address']}@127.0.0.1?protocol=43400&discovery=43404"
    case(OLD, 'old-old-empty', ROOT / 'old-empty', peer, anchor, fresh=True, timeout=360)
    stop(boot)
    boot = start(OLD, data, 43400, 'bootstrap-2', standalone=True)
    anchor = wait_boot(boot)
    fresh = case(NEW, 'old-new-empty', ROOT / 'new-empty', peer, anchor, fresh=True)
    if fresh:
        restarted = case(NEW, 'old-new-restart', ROOT / 'new-empty', peer, anchor, timeout=120)
        if restarted:
            case(NEW, 'wrong-genesis', ROOT / 'new-empty', peer, anchor, wrong=True, timeout=90)
    stop(boot)
    boot = start(OLD, data, 43400, 'bootstrap-3', standalone=True)
    anchor = wait_boot(boot)
    current = case(CURRENT, 'old-current-empty', ROOT / 'current-empty', peer, anchor, fresh=True, timeout=600)
    if current:
        restarted = case(CURRENT, 'old-current-restart', ROOT / 'current-empty', peer, anchor, timeout=120)
        if restarted:
            case(CURRENT, 'current-wrong-genesis', ROOT / 'current-empty', peer, anchor, wrong=True, timeout=90)
except Exception as error:
    report['error'] = repr(error)
    print('LAB_ERROR', repr(error), flush=True)
finally:
    for unit in reversed(units):
        stop(unit)
    report['source_after'] = source_fingerprints()
    report['source_evidence_unchanged'] = report.get('source_before') == report['source_after']
    try:
        report['safety_after'] = live_safety()
        report['live_services_and_firewall_unchanged'] = all(
            report.get('safety_before', {}).get(k) == report['safety_after'].get(k)
            for k in ('services', 'firewall_sha256'))
    except Exception as error:
        report['safety_error'] = repr(error)
    report['finished_utc'] = dt.datetime.now(dt.timezone.utc).isoformat()
    save()
    print('EVIDENCE', str(ROOT), flush=True)
    print(json.dumps({k: v for k, v in report.items() if k in
                      ['error', 'source_evidence_unchanged', 'live_services_and_firewall_unchanged']}), flush=True)

required = ['old-old-empty', 'old-new-empty', 'old-new-restart', 'wrong-genesis',
            'old-current-empty', 'old-current-restart', 'current-wrong-genesis']
accepted = (not report.get('error') and report.get('source_evidence_unchanged') is True
            and report.get('live_services_and_firewall_unchanged') is True
            and all(any(case['label'] == label and case['pass'] is True
                        for case in report['cases']) for label in required))
raise SystemExit(0 if accepted else 1)
