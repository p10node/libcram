
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
