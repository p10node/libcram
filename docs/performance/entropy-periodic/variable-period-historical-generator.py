from pathlib import Path
import subprocess
r=Path(__file__).resolve().parent.parent
d=r/'target/repeated-block-spike';d.mkdir(exist_ok=True)
s=(r/'crates/libcram-core/src/entropy/mod.rs').read_text()
c=s.replace('fn banked_hist(data: &[u8], hist:', 'fn banked_hist(mut data: &[u8], hist:')
needle='    if data.len() < 64 {\n        scalar_hist(data, hist);'
assert c.count(needle)==1
c=c.replace(needle,'''    if data.len() >= 128 {
        if let Some(index) = data[1..65].iter().position(|byte| *byte == data[0]) {
            let period = index + 1;
            if data[period..] == data[..data.len()-period] {
                let repeats = data.len() / period;
                for &byte in &data[..period] { hist[byte as usize] += repeats; }
                data = &data[repeats * period..];
            }
        }
    }
'''+needle)
(d/'candidate.rs').write_text(c)
main=r'''#[path="BASELINE"] mod baseline;
mod candidate;
use std::{hint::black_box,time::Instant};
fn random(n: usize) -> Vec<u8> {
 let mut x=0x9e3779b97f4a7c15u64;
 (0..n).map(|_| {x^=x<<13;x^=x>>7;x^=x<<17;x as u8}).collect()
}
fn main() {
 let tile=random(64);
 let mut data:Vec<u8>=(0..131200).map(|i|tile[i%64]).collect();
 let mut cases=0;
 for offset in 0..32 {
  for len in [0,1,63,64,65,127,128,129,255,256,257,4095,4096,4097,65535,65536,65537,131073] {
   let input=&data[offset..offset+len];
   assert_eq!(candidate::shannon_entropy(input).to_bits(),baseline::scalar_entropy(input).to_bits());cases+=1;
  }
 }
 for pos in 0..4097 {
  data[pos]^=0x81;
  assert_eq!(candidate::shannon_entropy(&data[..4097]).to_bits(),baseline::scalar_entropy(&data[..4097]).to_bits());cases+=1;
  data[pos]^=0x81;
 }
 let corpus=[vec![0;4096],b"libcram memory tier orchestration ".iter().copied().cycle().take(4096).collect(),random(4096)];
 for input in &corpus {assert_eq!(candidate::shannon_entropy(input).to_bits(),baseline::scalar_entropy(input).to_bits());cases+=1;}
 println!("DIFFERENTIAL_PASS {cases}");
 let fs:[(&str,fn(&[u8])->f64);2]=[("baseline",baseline::shannon_entropy),("repeated",candidate::shannon_entropy)];
 for input in &corpus {for (_,f) in fs {for _ in 0..5000 {black_box(f(black_box(input)));}}}
 for round in 0..9 {
  for (kind,input) in ["zero","ascii","random"].iter().zip(&corpus) {
   for i in 0..2 {
    let (name,f)=fs[(i+round)%2];let t=Instant::now();
    for _ in 0..50000 {black_box(f(black_box(input)));}
    println!("MEASURE {round} {kind} {name} {}",t.elapsed().as_nanos() as f64/50000.0);
   }
  }
 }
}'''.replace('BASELINE',str(r/'crates/libcram-core/src/entropy/mod.rs'))
(d/'main.rs').write_text(main)
subprocess.run(['rustc','--edition=2021','-O',str(d/'main.rs'),'-o',str(d/'probe')],check=True)
p=subprocess.run([str(d/'probe')],capture_output=True,text=True,check=True)
(d/'output.log').write_text(p.stdout+p.stderr);print(p.stdout)
