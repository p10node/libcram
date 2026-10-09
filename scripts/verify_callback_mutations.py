from pathlib import Path
import subprocess, tempfile, tarfile, io, shutil, os, json, hashlib
if not __debug__:
 raise SystemExit('Assertions are required; do not run with Python -O')
root=Path(__file__).resolve().parent.parent
out=root/'target/callback-mutation-evidence';out.mkdir(exist_ok=True)
relative='crates/libcram-core/src/allocator/pressure.rs'
test='crates/libcram-core/src/allocator/pressure/tests.rs'
original=(root/relative).read_text()
hashes={p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in [relative,test]}
mutations=[
 ('skip-inflight-wait','while slot.in_flight {','while false {'),
 ('ignore-generation','slot.generation == generation && !slot.quiescing','!slot.quiescing'),
 ('allow-registration-during-wait','if slot.quiescing {','if false {'),
 ('allow-competing-wait','slot.quiescing || (slot.in_flight && slot.worker == Some(thread::current().id()))','slot.in_flight && slot.worker == Some(thread::current().id())'),
 ('lose-worker-on-panic','std::mem::forget(payload);','std::mem::forget(payload); break;'),
]
results=[]
with tempfile.TemporaryDirectory(prefix='libcram-callback-mutations-') as temporary:
 d=Path(temporary)
 archive=subprocess.check_output(['git','archive','HEAD'],cwd=root)
 with tarfile.open(fileobj=io.BytesIO(archive)) as tar: tar.extractall(d,filter='data')
 (d/test).parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(root/test,d/test)
 env=dict(os.environ,CARGO_TARGET_DIR=str(root/'target/callback-mutation-build'))
 for name,old,new in [('baseline',None,None)]+mutations+[('restored',None,None)]:
  candidate = original
  if old is not None:
   assert new is not None
   assert original.count(old)==1,(name,original.count(old))
   candidate = original.replace(old,new,1)
  (d/relative).write_text(candidate)
  build=subprocess.run(['cargo','test','--locked','--offline','-p','libcram-core','--lib','--no-run','--message-format=json'],cwd=d,env=env,text=True,capture_output=True,timeout=120)
  (out/(name+'-build.log')).write_text(build.stdout+build.stderr)
  assert build.returncode==0,(name,'not a behavioral failure',build.stderr)
  binaries=[j['executable'] for line in build.stdout.splitlines() if (j:=json.loads(line)).get('reason')=='compiler-artifact' and j.get('executable')]
  assert len(binaries)==1
  try:
   run=subprocess.run([binaries[0],'allocator::pressure::tests','--nocapture'],text=True,capture_output=True,timeout=8)
   code=run.returncode;log=run.stdout+run.stderr
  except subprocess.TimeoutExpired as e:
   code='timeout';log=(e.stdout or b'').decode()+(e.stderr or b'').decode()
  (out/(name+'.log')).write_text(log)
  if old is None: assert code==0,(name,code,log)
  elif name=='lose-worker-on-panic': assert code=='timeout' and 'deliberate callback capture destructor panic' in log,(name,code,log)
  else: assert code==101 and 'FAILED' in log,(name,code,log)
  results.append({'variant':name,'exit':code});print(results[-1],flush=True)
assert hashes=={p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in hashes}
(out/'results.json').write_text(json.dumps({'results':results,'canonical_hashes_unchanged':hashes},indent=2)+'\n')
print('MUTATION_CONTROLS_PASS; disposable tree removed; canonical hashes unchanged')
