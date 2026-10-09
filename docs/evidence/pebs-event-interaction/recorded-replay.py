from pathlib import Path
import hashlib,io,json,shlex,subprocess,tarfile
r=Path(__file__).resolve().parents[1];d=r/'target/pebs-interaction'
def ssh(cmd,data=None):
 p=subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10','s5',cmd],input=data,capture_output=True,timeout=40)
 if p.returncode: raise RuntimeError(p.stderr.decode())
 return p.stdout
remote=ssh('mktemp -d /tmp/libcram-pebs-replay.XXXXXXXX').decode().strip()
assert remote.startswith('/tmp/libcram-pebs-replay.') and '/' not in remote[len('/tmp/'):]
archive=(d/'remote-artifacts.tar.gz').read_bytes()
ssh('tar -xzf - -C '+shlex.quote(remote),archive)
results=json.loads((d/'results.json').read_text());receipt=[]
for row in results:
 name=row['case'];raw=ssh('cd '+shlex.quote(remote)+' && '+shlex.join(['perf','script','-i',name+'.data','-F','addr']))
 (d/(name+'-address-only.txt')).write_bytes(raw)
 addresses=[int(s,16) for s in raw.decode().splitlines() if s.strip()]
 hits=sum(row['lo']<=x<row['hi'] for x in addresses)
 assert len(addresses)==sum(v['samples'] for v in row['events'].values())
 assert hits==sum(v['in_owned_range'] for v in row['events'].values())
 receipt.append({'case':name,'samples':len(addresses),'owned_hits':hits,'matches_corrected_multifield':True})
 print(name,len(addresses),hits)
info="""from pathlib import Path
import json
addr=0xfeb80008
matches=[]
for line in Path('/proc/iomem').read_text().splitlines():
 try:
  span=line.strip().split(' : ',1)[0];lo,hi=(int(s,16) for s in span.split('-'))
  if lo<=addr<=hi: matches.append(line)
 except ValueError: pass
print(json.dumps({'address':hex(addr),'iomem_ranges':matches}))
"""
(d/'outside-address-resource.json').write_bytes(ssh('python3 -c '+shlex.quote(info)))
(d/'address-only-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz') as tar:
 for name in ['probe.c','probe','disassembly.txt']:
  member=tar.extractfile(name);assert member is not None
  raw=member.read();(d/name).write_bytes(raw)
assert hashlib.sha256((d/'probe.c').read_bytes()).hexdigest()==hashlib.sha256((r/'docs/evidence/pebs-address-diagnostic/probe.c').read_bytes()).hexdigest()
ssh('python3 -c '+shlex.quote('import shutil,pathlib;p=pathlib.Path('+repr(remote)+');shutil.rmtree(p);assert not p.exists()'))
(d/'replay-cleanup.json').write_text(json.dumps({'owned_directory':remote,'removed':True,'new_sampling':False})+'\n')
print('ADDRESS_ONLY_REPLAY_PASS; owned replay directory removed')
