"""Owned-process PEBS interaction diagnostic. Explicit --run required for SSH."""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess

ALL = 'cpu/event=0xd0,umask=0x81/upp'
HIT = 'cpu/event=0xd1,umask=0x01/upp'
STORE = 'cpu/event=0xd0,umask=0x82/upp'
CASES = [('alone12',12,ALL), ('hit12',12,HIT),
         ('with-store12',12,'{'+ALL+','+STORE+'}'),
         ('with-hit12',12,'{'+ALL+','+HIT+'}'),
         ('hit-first12',12,'{'+HIT+','+ALL+'}'),
         ('alone28',28,ALL), ('hit28',28,HIT)]
LINE = re.compile(r'^\s*(.+?):\s+((?:0x)?[0-9a-fA-F]+)\s+((?:0x)?[0-9a-fA-F]+)\s*$')

def parse(text, lo, hi):
    result={}
    for line in text.splitlines():
        if not line.strip(): continue
        m=LINE.fullmatch(line)
        if m is None: raise ValueError('Unrecognized perf sample: '+line)
        event,addr,ip=m.groups(); addr=int(addr,16); ip=int(ip,16)
        row=result.setdefault(event, {'samples':0,'in_owned_range':0,'zero':0,'ips':collections.Counter(),'outside_addresses':collections.Counter()})
        row['samples']+=1; row['in_owned_range']+=int(lo<=addr<hi); row['zero']+=int(addr==0)
        row['ips'][hex(ip)]+=1
        if not lo<=addr<hi: row['outside_addresses'][hex(addr)]+=1
    for row in result.values():
        row['ips']=row['ips'].most_common(8)
        row['outside_addresses']=row['outside_addresses'].most_common(8)
    return result

def tests():
    # perf selects fields with -F but emits event, addr, ip in canonical order.
    sample=f'{ALL}: 1000 401010\n{ALL}: 1fff 401011\n{ALL}: 2000 401012\n{HIT}: 0 401013\n'
    rows=parse(sample,0x1000,0x2000)
    assert rows[ALL]['samples']==3 and rows[ALL]['in_owned_range']==2
    assert rows[HIT]['zero']==1 and rows[HIT]['in_owned_range']==0
    assert parse('',0x1000,0x2000)=={}
    try: parse('malformed sample',0x1000,0x2000)
    except ValueError: pass
    else: raise AssertionError('Malformed sample accepted')

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--run',action='store_true');ap.add_argument('--output',type=Path,required=True)
    args=ap.parse_args();tests()
    if not args.run: print('PARSER_FIXTURES_PASS; no SSH executed');return
    root=Path(__file__).resolve().parents[1]
    source=root/'docs/evidence/pebs-address-diagnostic/probe.c'
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    def ssh(command, data=None):
        p=subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10','s5',command],input=data,capture_output=True,timeout=45)
        if p.returncode: raise RuntimeError(f'SSH failed {p.returncode}: {p.stderr.decode()}')
        return p.stdout
    remote=ssh('mktemp -d /tmp/libcram-pebs-interaction.XXXXXXXX').decode().strip()
    assert re.fullmatch(r'/tmp/libcram-pebs-interaction\.[A-Za-z0-9]+',remote)
    (out/'remote-path.txt').write_text(remote+'\n')
    ssh('python3 -c '+shlex.quote('import sys,pathlib;pathlib.Path('+repr(remote+'/probe.c')+').write_bytes(sys.stdin.buffer.read())'),source.read_bytes())
    def run(command):
        wrapped='cd '+shlex.quote(remote)+' && '+command
        raw=ssh(wrapped);return raw
    run('cc -O2 -g -fno-pie -no-pie -Wall -Wextra -Werror probe.c -o probe && objdump -d probe > disassembly.txt')
    results=[]
    for name,cpu,event in CASES:
        cmd=['perf','record','-q','-d','-c','2000003','-e',event,'-o',name+'.data','--','taskset','-c',str(cpu),'./probe']
        shell=shlex.join(cmd)+f' > {name}.log 2>&1; code=$?; printf "%s\\n" "$code" > {name}.exit'
        run(shell)
        code=int(run('python3 -c '+shlex.quote(f'from pathlib import Path;print(Path("{name}.exit").read_text())')))
        log=run('python3 -c '+shlex.quote(f'from pathlib import Path;print(Path("{name}.log").read_text(),end="")')).decode()
        (out/(name+'.log')).write_text(log)
        if code: raise RuntimeError(f'{name}: perf record exit {code}: {log}')
        assert 'OWNED_DONE ' in log
        m=re.search(r'OWNED_READ_RANGE (0x[0-9a-f]+) (0x[0-9a-f]+) PID (\d+)',log)
        assert m is not None
        lo,hi,pid=(int(m[1],16),int(m[2],16),int(m[3]))
        text=run(shlex.join(['perf','script','-i',name+'.data','-F','event,ip,addr'])).decode()
        (out/(name+'-samples.txt')).write_text(text)
        rows=parse(text,lo,hi)
        expected=[ALL] if name.startswith('alone') or name.startswith('with-store') else ([HIT] if name in ('hit12','hit28') else [ALL,HIT])
        assert all(rows.get(e,{}).get('samples',0)>100 for e in expected),rows
        result={'case':name,'command':cmd,'record_exit':code,'pid':pid,'lo':lo,'hi':hi,'events':rows}
        results.append(result);(out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
        print(name, {e:(v['samples'],v['in_owned_range']) for e,v in rows.items()},flush=True)
    (out/'remote-artifacts.tar.gz').write_bytes(run('tar -czf - probe probe.c disassembly.txt *.data *.exit *.log'))
    inventory=ssh("python3 -c "+shlex.quote("import json,pathlib,platform;print(json.dumps({'kernel':platform.release(),'cpuinfo':pathlib.Path('/proc/cpuinfo').read_text(),'paranoid':pathlib.Path('/proc/sys/kernel/perf_event_paranoid').read_text(),'max_precise':pathlib.Path('/sys/bus/event_source/devices/cpu/caps/max_precise').read_text()}))"))
    (out/'host-info.json').write_bytes(inventory)
    (out/'source.sha256').write_text(hashlib.sha256(source.read_bytes()).hexdigest()+'  probe.c\n')
    ssh('python3 -c '+shlex.quote('import shutil,pathlib;p=pathlib.Path('+repr(remote)+');shutil.rmtree(p);assert not p.exists();print("OWNED_PROBE_REMOVED")'))
    (out/'cleanup.json').write_text(json.dumps({'owned_directory':remote,'removed':True})+'\n')
if __name__=='__main__': main()
