from pathlib import Path
import hashlib, json, subprocess
root = Path(__file__).resolve().parents[2]
source = root / 'crates/libcram-core/src/entropy/mod.rs'
text = source.read_text()
loop = text[text.index('        let mut counters ='):text.index('        for i in 0..256 {')]
loop = loop.replace('batch.as_chunks', 'data.as_chunks')
finish = text[text.index('        let weights ='):text.index('        let mut sums =')]
program = '''use std::{hint::black_box, time::Instant, sync::OnceLock};
#[path = "BASELINE"] mod baseline;
#[inline(never)]
fn fused(data: &[u8]) -> f64 {
    if data.len() < 64 || data.len() > 4096 || data[..64].iter().all(|b| *b == data[0]) {
        return baseline::shannon_entropy(data);
    }
    let n = data.len();
    static WEIGHTS: OnceLock<Vec<f64>> = OnceLock::new();
FINISH
LOOP
    for &byte in remainder { h0[byte as usize] += 1; }
    let mut sums = [0.0; 4];
    for i in (0..256).step_by(4) {
        for lane in 0..4 {
            let j = i + lane;
            sums[lane] += weights[(h0[j] + h1[j] + h2[j] + h3[j]) as usize];
        }
    }
    ((weights[n] - ((sums[0] + sums[1]) + (sums[2] + sums[3]))) / n as f64).max(0.0)
}
fn random(n: usize, mut state: u64) -> Vec<u8> {
    (0..n).map(|_| { state ^= state << 13; state ^= state >> 7; state ^= state << 17; state as u8 }).collect()
}
fn main() {
    let samples = [vec![0; 4224], vec![255; 4224], random(4224, 0x9e3779b97f4a7c15),
        (0..4224).map(|i| (i % 256) as u8).collect(),
        b"libcram memory tier orchestration ".iter().copied().cycle().take(4224).collect()];
    let mut cases = 0;
    for data in &samples {
        for offset in 0..32 {
            for n in [0,1,31,32,63,64,65,95,127,128,255,512,1023,4095,4096,4097] {
                let input = &data[offset..offset+n];
                assert_eq!(fused(input).to_bits(), baseline::scalar_entropy(input).to_bits(), "offset={offset} n={n}");
                cases += 1;
            }
        }
    }
    println!("EXACT_DIFFERENTIAL_PASS {cases}");
    let data = [vec![0;4096], samples[4][..4096].to_vec(), samples[2][..4096].to_vec()];
    let functions: [(&str, fn(&[u8])->f64);2] = [("baseline",baseline::shannon_entropy),("fused",fused)];
    for d in &data { for (_, f) in functions { for _ in 0..5000 { black_box(f(black_box(d))); } } }
    for round in 0..9 {
        for (kind,d) in ["zero","ascii","random"].iter().zip(&data) {
            for k in 0..2 {
                let (name,f) = functions[(k + round) % 2];
                let t=Instant::now();
                for _ in 0..50000 { black_box(f(black_box(d))); }
                println!("MEASURE {round} {kind} {name} {}", t.elapsed().as_nanos() as f64 / 50000.0);
            }
        }
    }
}
'''.replace('BASELINE',str(source)).replace('FINISH',finish).replace('LOOP',loop)
scalar_loop = '''    for i in (0..256).step_by(4) {
        for lane in 0..4 {
            let j = i + lane;
            sums[lane] += weights[(h0[j] + h1[j] + h2[j] + h3[j]) as usize];
        }
    }'''
simd_loop = '''    for i in (0..256).step_by(8) {
        let mut counts = [0u16; 8];
        // SAFETY: eight counters fit in every bank; NEON is available on this owned Apple host.
        unsafe {
            use std::arch::aarch64::*;
            let a = vaddq_u16(vld1q_u16(h0.as_ptr().add(i)), vld1q_u16(h1.as_ptr().add(i)));
            let b = vaddq_u16(vld1q_u16(h2.as_ptr().add(i)), vld1q_u16(h3.as_ptr().add(i)));
            vst1q_u16(counts.as_mut_ptr(), vaddq_u16(a,b));
        }
        for offset in [0,4] {
            for lane in 0..4 { sums[lane] += weights[counts[offset+lane] as usize]; }
        }
    }'''
fn_text = program[program.index('#[inline(never)]'):program.index('fn random(')]
assert scalar_loop in fn_text
simd_fn = fn_text.replace('fn fused(', 'fn fused_simd(').replace(scalar_loop, simd_loop)
program = program.replace('fn random(', simd_fn + '\nfn random(', 1)
program = program.replace('                cases += 1;', '                assert_eq!(fused_simd(input).to_bits(), baseline::scalar_entropy(input).to_bits());\n                cases += 1;')
program = program.replace('f64);2]', 'f64);3]').replace('("fused",fused)]', '("fused",fused),("fused_simd",fused_simd)]').replace('for k in 0..2', 'for k in 0..3').replace('(k + round) % 2', '(k + round) % 3')
simd8 = simd_fn.replace('fn fused_simd(', 'fn fused_simd8(').replace('[[0u16; 256]; 4]', '[[0u16; 256]; 8]').replace('[h0, h1, h2, h3]', '[h0, h1, h2, h3, h4, h5, h6, h7]')
for old,new in [('h0[((word >> 32)', 'h4[((word >> 32)'), ('h1[((word >> 40)', 'h5[((word >> 40)'), ('h2[((word >> 48)', 'h6[((word >> 48)'), ('h3[(word >> 56)', 'h7[(word >> 56)')]:
    assert old in simd8
    simd8 = simd8.replace(old,new)
simd8 = simd8.replace('vst1q_u16(counts.as_mut_ptr(), vaddq_u16(a,b));', '''let c = vaddq_u16(vld1q_u16(h4.as_ptr().add(i)), vld1q_u16(h5.as_ptr().add(i)));
            let d = vaddq_u16(vld1q_u16(h6.as_ptr().add(i)), vld1q_u16(h7.as_ptr().add(i)));
            vst1q_u16(counts.as_mut_ptr(), vaddq_u16(vaddq_u16(a,b),vaddq_u16(c,d)));''')
program = program.replace('fn random(',simd8+'\nfn random(',1).replace('                cases += 1;', '                assert_eq!(fused_simd8(input).to_bits(), baseline::scalar_entropy(input).to_bits());\n                cases += 1;')
program = program.replace('f64);3]', 'f64);4]').replace('("fused_simd",fused_simd)]', '("fused_simd",fused_simd),("fused_simd8",fused_simd8)]').replace('for k in 0..3', 'for k in 0..4').replace('(k + round) % 3', '(k + round) % 4')
dest = Path(__file__).parent
(dest/'main.rs').write_text(program)
subprocess.run(['rustc','--edition=2021','-O',str(dest/'main.rs'),'-o',str(dest/'probe')],check=True)
r = subprocess.run([str(dest/'probe')],capture_output=True,text=True)
(dest/'output.log').write_text(r.stdout+r.stderr)
print(r.stdout, r.stderr)
assert r.returncode == 0
print('SOURCE_SHA256',hashlib.sha256(source.read_bytes()).hexdigest())
