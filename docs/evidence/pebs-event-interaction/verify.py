"""Offline replay of this diagnostic's raw evidence; never opens SSH/perf."""
import gzip
import hashlib
import json
from pathlib import Path
import runpy
import tarfile

if not __debug__:
    raise RuntimeError('Run without Python -O: evidence assertions are mandatory')

root = Path(__file__).resolve().parent
module = runpy.run_path(str(root / 'recorded-runner.py'))
module['tests']()
results = json.loads((root / 'results.json').read_text())
assert [r['case'] for r in results] == [r[0] for r in module['CASES']]
assert len(results) == 7
for row in results:
    name = row['case']
    text = gzip.decompress((root / (name + '-samples.txt.gz')).read_bytes()).decode()
    parsed = module['parse'](text, row['lo'], row['hi'])
    # JSON converts Counter.most_common tuples into lists.
    assert json.loads(json.dumps(parsed)) == row['events'], name
    addresses = [int(s, 16) for s in gzip.decompress((root / (name + '-address-only.txt.gz')).read_bytes()).decode().splitlines() if s.strip()]
    assert len(addresses) == sum(v['samples'] for v in parsed.values()), name
    assert sum(row['lo'] <= a < row['hi'] for a in addresses) == sum(v['in_owned_range'] for v in parsed.values()), name
    assert row['record_exit'] == 0
    assert all(v['samples'] > 100 for v in parsed.values())
    for event, value in parsed.items():
        assert value['ips'] == [('0x401208', value['samples'])]
        if event == module['ALL']:
            assert value['in_owned_range'] == 0
            assert {a for a, _ in value['outside_addresses']} == {'0xfeb80008', '0xfeb80040', '0xfeb80048'}
        elif event == module['HIT']:
            assert value['in_owned_range'] == value['samples']
        else:
            raise AssertionError('Unexpected sampled event: ' + event)
with tarfile.open(root / 'remote-artifacts.tar.gz', 'r:gz') as archive:
    member = archive.extractfile('probe.c')
    assert member is not None
    assert member.read() == (root / 'probe.c').read_bytes()
    member = archive.extractfile('disassembly.txt')
    assert member is not None
    disassembly = member.read()
    assert disassembly == gzip.decompress((root / 'disassembly.txt.gz').read_bytes())
    assert b'401208:\t48 8b 08' in disassembly
    assert b'mov    (%rax),%rcx' in disassembly
for name, digest in json.loads((root / 'checksums.json').read_text()).items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
print('OFFLINE_EVIDENCE_CONSISTENT: seven captures, independent address-only replay, positive L1 controls; ALL_LOADS acceptance still FAIL')
