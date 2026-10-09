import json
from pathlib import Path
import subprocess
import time

out = Path('target/final-gates')
out.mkdir(parents=True, exist_ok=True)
command = ['cargo', 'test', '-p', 'libcram-core', '--locked', '--offline', '--features', 'ebpf', '--test', 'stress', '--test', 'runtime', '--test', 'runtime_bridge', '--test', 'device_adapter_reader', '--test', 'pressure_lifecycle']
start = time.monotonic()
runs = 0
with (out / 'short-soak.log').open('w') as log:
    while time.monotonic() - start < 60:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=30)
        if result.returncode:
            raise SystemExit(result.returncode)
        runs += 1
report = {'source_commit': '3edfa98d95e039cdaca20e9906de3d10db301f2a', 'command': command, 'successful_runs': runs, 'elapsed_seconds': time.monotonic() - start, 'scope': 'owned fixture/stress tests; not real CRAM or long-duration production soak'}
(out / 'short-soak.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))
