from pathlib import Path
import subprocess,shlex,json,re,hashlib
r=Path(__file__).resolve().parent.parent
out=r/'target/pebs-address-user-diagnostic';out.mkdir(exist_ok=True)
def ssh(cmd,timeout=30):
 return subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10','s5',cmd],capture_output=True,text=True,timeout=timeout)
p=ssh('mktemp -d /tmp/libcram-address-diagnostic.XXXXXX');assert p.returncode==0,p.stderr
remote=p.stdout.strip();assert re.fullmatch('/tmp/libcram-address-diagnostic\\.[A-Za-z0-9]+',remote)
q=shlex.quote(remote)
rows=[]
try:
 subprocess.run(['scp',str(r/'target/pebs-address-probe.c'),'s5:'+remote+'/probe.c'],check=True,capture_output=True,timeout=30)
 p=ssh(f'cc -O2 -Wall -Wextra -Werror {q}/probe.c -o {q}/probe && sha256sum {q}/probe {q}/probe.c');assert p.returncode==0,p.stderr
 (out/'artifact-hashes.txt').write_text(p.stdout)
 for name,event in [('all-loads-p','cpu/event=0xd0,umask=0x81/up'),('all-loads-pp','cpu/event=0xd0,umask=0x81/upp'),('all-loads-ppp','cpu/event=0xd0,umask=0x81/uppp'),('l1-hit-pp','cpu/event=0xd1,umask=0x01/upp')]:
  command=f'timeout 12 taskset -c 12 perf record -o {q}/{name}.data -c 2000003 -d -e {shlex.quote(event)} -- {q}/probe'
  p=ssh(command);(out/(name+'.log')).write_text(p.stdout+p.stderr)
  row={'name':name,'command':command,'exit':p.returncode}
  if p.returncode==0:
   match=re.search(r'OWNED_READ_RANGE (0x[0-9a-f]+) (0x[0-9a-f]+)',p.stdout);assert match,p.stdout
   start,end=map(lambda x:int(x,16),match.groups());assert start<end
   data=ssh(f'perf script -i {q}/{name}.data -F addr');assert data.returncode==0,data.stderr
   (out/(name+'-addresses.txt')).write_text(data.stdout)
   addresses=[]
   for line in data.stdout.splitlines():
    if not line.strip():continue
    assert re.fullmatch(r'\s*(?:0x)?[a-fA-F0-9]+\s*',line),line
    addresses.append(int(line.strip(),16))
   row.update(samples=len(addresses),in_owned_read=sum(start<=a<end for a in addresses),zero=sum(a==0 for a in addresses),below_4096=sum(a<4096 for a in addresses),read_start=hex(start),read_end=hex(end))
  rows.append(row);print(row,flush=True)
finally:
 p=ssh(f'rm -rf -- {q}');assert p.returncode==0,p.stderr
 (out/'results.json').write_text(json.dumps({'scope':'owned C load-only perf diagnostic, not production BPF or CPU acceptance','remote_directory_removed':remote,'rows':rows},indent=2)+'\n')
