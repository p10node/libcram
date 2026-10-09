"""Exact histogram layout comparison; scratch outputs only, no production edits."""
from pathlib import Path
import argparse, hashlib, json, platform, subprocess

ap=argparse.ArgumentParser();ap.add_argument('--check-only',action='store_true');ap.add_argument('--stub',action='store_true');args=ap.parse_args()
if platform.machine() not in ('arm64','aarch64','x86_64'): raise RuntimeError('Unsupported diagnostic architecture')
root=next(p for p in Path(__file__).resolve().parents if (p/'crates/libcram-core/src/entropy/mod.rs').is_file())
expected={'mod.rs':'f8d8d7bd7a33fb1d165051bcaaaf57e838a97ee247a11dbd9264259270ba108c','periodic.rs':'4bd087b1553d377a4adb62187974c5b54e33c5450694410ec85e692226caf1c0'}
for n,h in expected.items():
 if hashlib.sha256((root/'crates/libcram-core/src/entropy'/n).read_bytes()).hexdigest()!=h: raise RuntimeError('Baseline changed: '+n)
if hashlib.sha256((root/'benches/entropy.rs').read_bytes()).hexdigest()!='ddb3052bc8219ebfdf648088920c4ff77cbcdf5200ce4c59c00d7e8544416ac5': raise RuntimeError('Benchmark changed')
out=root/'target/entropy-layout-spike';out.mkdir(exist_ok=True,parents=True)
text=(root/'crates/libcram-core/src/entropy/mod.rs').read_text()
start=text.index('        let mut counters =');end=text.index('        scalar_hist(remainder, hist);',start)
body=text[start:end]
packed=body.replace('let mut counters = [[0u16; 256]; 4];\n        let [h0, h1, h2, h3] = &mut counters;','let mut counters = [0u64; 256];')
import re
packed=re.sub(r'h([0-3])\[([^\n]+)\] \+= 1;',lambda m:'counters['+m[2]+'] += 1u64 << '+str(int(m[1])*16)+';',packed)
packed=packed.replace('(h0[i] + h1[i] + h2[i] + h3[i]) as usize','((counters[i] & 65535) + ((counters[i] >> 16) & 65535) + ((counters[i] >> 32) & 65535) + (counters[i] >> 48)) as usize')
inter=body.replace('let mut counters = [[0u16; 256]; 4];\n        let [h0, h1, h2, h3] = &mut counters;','let mut counters = [[0u16; 4]; 256];')
inter=re.sub(r'h([0-3])\[([^\n]+)\] \+= 1;',lambda m:'counters['+m[2]+']['+m[1]+'] += 1;',inter)
inter=inter.replace('(h0[i] + h1[i] + h2[i] + h3[i]) as usize','(counters[i][0] + counters[i][1] + counters[i][2] + counters[i][3]) as usize')
variants={'baseline':text,'packed4':text[:start]+packed+text[end:],'interleaved4':text[:start]+inter+text[end:],'wide4':text[:start]+body.replace('[[0u16; 256]; 4]','[[0u32; 256]; 4]')+text[end:]}
for name,source in variants.items():
 if args.stub and name!='baseline':
  a=source.index('fn banked_hist(');b=source.index('\nfn select()',a)
  source=source[:a]+'fn banked_hist(_data: &[u8], _hist: &mut [usize;256]) {}\n'+source[b:]
 d=out/name;d.mkdir(exist_ok=True)
 (d/'mod.rs').write_text(source+'\npub fn diagnostic_hist(data: &[u8], hist: &mut [usize;256]) { banked_hist(data,hist); }\n')
 (d/'periodic.rs').write_bytes((root/'crates/libcram-core/src/entropy/periodic.rs').read_bytes())
program=r'''
use std::{hint::black_box, time::Instant};
mod baseline; mod packed4; mod interleaved4; mod wide4;
fn random(n:usize,mut state:u64)->Vec<u8>{(0..n).map(|_|{state^=state<<13;state^=state>>7;state^=state<<17;state as u8}).collect()}
fn main(){
 let fs:[(&str,fn(&[u8])->f64,fn(&[u8],&mut[usize;256]));4]=[
  ("baseline",baseline::shannon_entropy,baseline::diagnostic_hist),
  ("packed4",packed4::shannon_entropy,packed4::diagnostic_hist),
  ("interleaved4",interleaved4::shannon_entropy,interleaved4::diagnostic_hist),
  ("wide4",wide4::shannon_entropy,wide4::diagnostic_hist)];
 let size=65664;
 let mut collision=vec![255;size]; for i in (0..size).step_by(8191){collision[i]=(i%251)as u8;}
 let ascii:Vec<u8>=b"libcram memory tier orchestration ".iter().copied().cycle().take(size).collect();
 let datasets=[vec![0;size],vec![173;size],random(size,0x9e3779b97f4a7c15),random(size,0x81227),ascii,collision,(0..size).map(|i|(i%7) as u8).collect()];
 let mut cases=0;
 for d in &datasets { for offset in [0,1,7,31,63] {for n in [0,1,31,63,64,65,127,128,255,512,4095,4096,4097,32767,32768,32769,65535,65536,65537] {
  let input=&d[offset..offset+n];
  let initial=std::array::from_fn(|i|i%3);let mut expected=initial;
  for &b in input {expected[b as usize]+=1;}
  let e=baseline::scalar_entropy(input);
  for (name,f,h) in fs {let mut hist=initial;h(input,&mut hist);assert_eq!(hist,expected,"hist {name} offset={offset} n={n}");assert_eq!(f(input).to_bits(),e.to_bits(),"entropy {name} offset={offset} n={n}");}
  cases+=1;
 }}}
 println!("EXACT_ADDITIVE_AND_ENTROPY_PASS {cases} cases per variant");
 if std::env::args().any(|a|a=="--check-only"){return;}
 let data=[("zero",vec![0;4096]),("ascii",datasets[4][..4096].to_vec()),("random",datasets[2][..4096].to_vec())];
 for (_,d) in &data {for (_,f,_) in fs{for _ in 0..5000{black_box(f(black_box(d)));}}}
 for round in 0..9 {for (kind,d) in &data {for k in 0..fs.len(){
  let (name,f,_)=fs[(round+k)%fs.len()];let t=Instant::now();
  for _ in 0..50000{black_box(f(black_box(d)));}
  println!("MEASURE {round} {kind} {name} {}",t.elapsed().as_nanos()as f64/50000.0);
 }}}
}
'''
(out/'main.rs').write_text(program)
subprocess.run(['rustc','--edition=2021','-O',str(out/'main.rs'),'-o',str(out/'probe')],check=True)
p=subprocess.run([str(out/'probe')]+(['--check-only'] if args.check_only or args.stub else []),capture_output=True,text=True)
name='stub-red.log' if args.stub else ('check-only.log' if args.check_only else 'measure.log')
(out/name).write_text(p.stdout+p.stderr);print(p.stdout,p.stderr)
(out/'source-hashes.json').write_text(json.dumps(expected,indent=2)+'\n')
raise SystemExit(p.returncode)
