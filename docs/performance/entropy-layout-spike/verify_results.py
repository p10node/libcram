"""Offline integrity and summary checks. Does not execute Rust, SSH or timing."""
from pathlib import Path
import collections
import hashlib
import json
import statistics

root = Path(__file__).resolve().parent
for log, summary in [('measure.log', 'summary.json'), ('linux-measure.log', 'linux-summary.json')]:
    lines = (root / log).read_text().splitlines()
    if 'EXACT_ADDITIVE_AND_ENTROPY_PASS 665 cases per variant' not in lines:
        raise RuntimeError('Missing correctness receipt: ' + log)
    rows = collections.defaultdict(list)
    rounds = collections.defaultdict(list)
    for line in lines:
        if not line.startswith('MEASURE '):
            continue
        _, iteration, kind, name, ns = line.split()
        rows[(kind, name)].append(float(ns))
        rounds[(kind, name)].append(int(iteration))
    expected = {(k, n) for k in ['zero', 'ascii', 'random'] for n in ['baseline', 'packed4', 'interleaved4', 'wide4']}
    if set(rows) != expected or any(v != list(range(9)) for v in rounds.values()):
        raise RuntimeError('Missing, duplicate or unexpected measurement batches')
    rebuilt = [dict(kind=k, name=n, median_ns=statistics.median(v), min_ns=min(v), max_ns=max(v), batches=len(v)) for (k,n),v in rows.items()]
    if rebuilt != json.loads((root / summary).read_text()):
        raise RuntimeError('Summary differs from raw evidence: ' + summary)
manifest = json.loads((root / 'checksums.json').read_text())
actual = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(root.rglob('*')) if p.is_file() and p != root / 'checksums.json'}
if actual != manifest:
    raise RuntimeError('Artifact content or inventory changed')
print('LAYOUT_EVIDENCE_CONSISTENT: both raw datasets and summaries; no performance acceptance claimed')
