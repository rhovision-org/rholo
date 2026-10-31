"""Check retained private-chain observations without starting a node."""
import gzip
import hashlib
import ipaddress
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
report = json.loads((ROOT / 'result.json').read_text())
anchor = report['bootstrap_baseline']
trusted = report['trusted_genesis']
expected = {
    'old-old-empty': 'a5259969a7c5673ccdfc5dad97981d0a3607fcf0',
    'old-new-empty': '3607cf9f7832054376ac33a4c1c56fd0b701e89d',
    'old-new-restart': '3607cf9f7832054376ac33a4c1c56fd0b701e89d',
    'wrong-genesis': '3607cf9f7832054376ac33a4c1c56fd0b701e89d',
    'old-current-empty': 'd1af45d1db3f5fe2fa6047ad6c8a72b654d041df',
    'old-current-restart': 'd1af45d1db3f5fe2fa6047ad6c8a72b654d041df',
    'current-wrong-genesis': 'd1af45d1db3f5fe2fa6047ad6c8a72b654d041df',
}
binary_hashes = {
    expected['old-old-empty']: 'a5b9e9de4f3a75833da50aef67a845ddc350248ec3113040bc727dbcb9bdfa01',
    expected['old-new-empty']: 'b4b2c8c3b7ae12ab16fae4cfd31d9918e687f6c41f0c28befc4f05c9721b8fd2',
    expected['old-current-empty']: '76547fa988a4b2e216feb373cb17affa9cc4217ac9595ea97e77a2e1db3a8745',
}
manifest = json.loads((ROOT / 'SHA256SUMS.json').read_text())
for name, digest in manifest.items():
    assert Path(name).name == name
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == digest, name
assert len(report['cases']) == len(expected)
assert {c['label'] for c in report['cases']} == set(expected)
assert report['source_evidence_unchanged'] is True
assert report['source_before'] == report['source_after']
assert report['live_services_and_firewall_unchanged'] is True
assert not report.get('error') and not report.get('safety_error')
assert anchor['genesis'] == trusted
assert anchor['status']['latestBlockNumber'] >= anchor['finalized']['blockNumber']
assert hashlib.sha256((ROOT / 'private_native_compat.py').read_bytes()).hexdigest() == (
    '17ffd8aacdaff931eb1207c60224e21108b6e0a4a53a48f419aa4d0e47abf7f2')

results = []
for case in report['cases']:
    label = case['label']
    assert case['source_sha'] == expected[label]
    assert case['binary_sha256'] == binary_hashes[case['source_sha']]
    assert case['pass'] is True, label
    log = gzip.decompress((ROOT / (case['log_file'] + '.gz')).read_bytes()).decode()
    assert 'WrongNetwork(' not in log
    for text in ['active-bonds leaf not found', 'receiver state full', 'out of memory']:
        assert text not in log.lower(), (label, text)
    if 'wrong-genesis' in label or label == 'wrong-genesis':
        assert case['exit']['ActiveState'] in ('failed', 'inactive')
        assert case['exit']['ExecMainStatus'] == '1'
        assert 'Making a transition to Running state.' not in log
        assert trusted in log and '0' * 64 in log
        assert 'genesis' in log.lower()
    else:
        last = case['last']
        assert last['genesis'] == trusted
        assert last['status']['latestBlockNumber'] >= anchor['status']['latestBlockNumber']
        assert last['finalized']['blockHash'] == anchor['finalized']['blockHash']
        assert last['finalized']['blockNumber'] == anchor['finalized']['blockNumber']
        assert expected[label] in last['status']['version']['node']
        assert last['running_log'] is True
        assert 'Making a transition to Running state.' in log
        if label.endswith('-empty'):
            assert case['state_at_start'] == 'empty'
            assert case['empty_listing_before_start'] == []
            assert last['restored_log'] is True
            assert log.index('LFS state is successfully restored') < log.index(
                'Making a transition to Running state.')
        else:
            assert case['state_at_start'] == 'persistent'
            assert 'Reconnecting to existing network' in log
    for rows in case.get('tcp_samples', []):
        for row in rows:
            for endpoint in row.split()[2:4]:
                assert ipaddress.ip_address(endpoint.rsplit(':', 1)[0]).is_loopback, row
    results.append({'case': label, 'result': 'PASS', 'seconds': case['duration_seconds']})

negative = json.loads((ROOT / 'network-id-negative-result.json').read_text())
assert len(negative['cases']) == 3 and all(c['pass'] is False for c in negative['cases'])
assert negative['source_evidence_unchanged'] is True
assert negative['live_services_and_firewall_unchanged'] is True

follow_up = json.loads((ROOT / 'live-follow-up.json').read_text())
for node in follow_up['nodes']:
    for claimed in follow_up['nodes']:
        final = claimed['final']
        assert final['blockHash'] in node['finalized_height_sets'][str(final['blockNumber'])]
live_log = json.loads((ROOT / 'live-log-follow-up.json').read_text())
assert live_log['logs']['/var/log/rnode-d.log']['counts_in_tail']['receiver state full'] > 0
timeline = json.loads((ROOT / 'live-failure-timeline.json').read_text())
for log in timeline.values():
    onset = log['first_overflow'][0].split()[0]
    assert onset < report['started_utc']
print(json.dumps({'scope': report['scope'], 'retained_private_cases': results,
                  'live_acceptance': 'NOT ESTABLISHED',
                  'live_receiver_overflow': 'OBSERVED',
                  'same_height_finalized_block_sets': 'MATCH',
                  'independent_repeat': 'NOT TESTED'}, indent=2))
