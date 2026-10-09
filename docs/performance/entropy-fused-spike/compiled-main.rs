use std::{hint::black_box, time::Instant, sync::OnceLock};
#[path = "$WORKSPACE/libcram/crates/libcram-core/src/entropy/mod.rs"] mod baseline;
#[inline(never)]
fn fused(data: &[u8]) -> f64 {
    if data.len() < 64 || data.len() > 4096 || data[..64].iter().all(|b| *b == data[0]) {
        return baseline::shannon_entropy(data);
    }
    let n = data.len();
    static WEIGHTS: OnceLock<Vec<f64>> = OnceLock::new();
        let weights = WEIGHTS.get_or_init(|| {
            (0..=4096)
                .map(|v| {
                    if v == 0 {
                        0.0
                    } else {
                        v as f64 * (v as f64).log2()
                    }
                })
                .collect()
        });

        let mut counters = [[0u16; 256]; 4];
        let [h0, h1, h2, h3] = &mut counters;
        let (chunks, remainder) = data.as_chunks::<32>();
        for chunk in chunks {
            for bytes in chunk.as_chunks::<8>().0 {
                let word = u64::from_ne_bytes(*bytes);
                // Preserve the word load on AArch64 instead of eight byte loads.
                // SAFETY: register-only identity; no memory or flags are changed.
                #[cfg(target_arch = "aarch64")]
                let word = unsafe {
                    let mut word = word;
                    std::arch::asm!("/* {word} */", word = inout(reg) word,
                        options(pure, nomem, nostack, preserves_flags));
                    word
                };
                h0[(word & 255) as usize] += 1;
                h1[((word >> 8) & 255) as usize] += 1;
                h2[((word >> 16) & 255) as usize] += 1;
                h3[((word >> 24) & 255) as usize] += 1;
                h0[((word >> 32) & 255) as usize] += 1;
                h1[((word >> 40) & 255) as usize] += 1;
                h2[((word >> 48) & 255) as usize] += 1;
                h3[(word >> 56) as usize] += 1;
            }
        }

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
#[inline(never)]
fn fused_simd(data: &[u8]) -> f64 {
    if data.len() < 64 || data.len() > 4096 || data[..64].iter().all(|b| *b == data[0]) {
        return baseline::shannon_entropy(data);
    }
    let n = data.len();
    static WEIGHTS: OnceLock<Vec<f64>> = OnceLock::new();
        let weights = WEIGHTS.get_or_init(|| {
            (0..=4096)
                .map(|v| {
                    if v == 0 {
                        0.0
                    } else {
                        v as f64 * (v as f64).log2()
                    }
                })
                .collect()
        });

        let mut counters = [[0u16; 256]; 4];
        let [h0, h1, h2, h3] = &mut counters;
        let (chunks, remainder) = data.as_chunks::<32>();
        for chunk in chunks {
            for bytes in chunk.as_chunks::<8>().0 {
                let word = u64::from_ne_bytes(*bytes);
                // Preserve the word load on AArch64 instead of eight byte loads.
                // SAFETY: register-only identity; no memory or flags are changed.
                #[cfg(target_arch = "aarch64")]
                let word = unsafe {
                    let mut word = word;
                    std::arch::asm!("/* {word} */", word = inout(reg) word,
                        options(pure, nomem, nostack, preserves_flags));
                    word
                };
                h0[(word & 255) as usize] += 1;
                h1[((word >> 8) & 255) as usize] += 1;
                h2[((word >> 16) & 255) as usize] += 1;
                h3[((word >> 24) & 255) as usize] += 1;
                h0[((word >> 32) & 255) as usize] += 1;
                h1[((word >> 40) & 255) as usize] += 1;
                h2[((word >> 48) & 255) as usize] += 1;
                h3[(word >> 56) as usize] += 1;
            }
        }

    for &byte in remainder { h0[byte as usize] += 1; }
    let mut sums = [0.0; 4];
    for i in (0..256).step_by(8) {
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
    }
    ((weights[n] - ((sums[0] + sums[1]) + (sums[2] + sums[3]))) / n as f64).max(0.0)
}

#[inline(never)]
fn fused_simd8(data: &[u8]) -> f64 {
    if data.len() < 64 || data.len() > 4096 || data[..64].iter().all(|b| *b == data[0]) {
        return baseline::shannon_entropy(data);
    }
    let n = data.len();
    static WEIGHTS: OnceLock<Vec<f64>> = OnceLock::new();
        let weights = WEIGHTS.get_or_init(|| {
            (0..=4096)
                .map(|v| {
                    if v == 0 {
                        0.0
                    } else {
                        v as f64 * (v as f64).log2()
                    }
                })
                .collect()
        });

        let mut counters = [[0u16; 256]; 8];
        let [h0, h1, h2, h3, h4, h5, h6, h7] = &mut counters;
        let (chunks, remainder) = data.as_chunks::<32>();
        for chunk in chunks {
            for bytes in chunk.as_chunks::<8>().0 {
                let word = u64::from_ne_bytes(*bytes);
                // Preserve the word load on AArch64 instead of eight byte loads.
                // SAFETY: register-only identity; no memory or flags are changed.
                #[cfg(target_arch = "aarch64")]
                let word = unsafe {
                    let mut word = word;
                    std::arch::asm!("/* {word} */", word = inout(reg) word,
                        options(pure, nomem, nostack, preserves_flags));
                    word
                };
                h0[(word & 255) as usize] += 1;
                h1[((word >> 8) & 255) as usize] += 1;
                h2[((word >> 16) & 255) as usize] += 1;
                h3[((word >> 24) & 255) as usize] += 1;
                h4[((word >> 32) & 255) as usize] += 1;
                h5[((word >> 40) & 255) as usize] += 1;
                h6[((word >> 48) & 255) as usize] += 1;
                h7[(word >> 56) as usize] += 1;
            }
        }

    for &byte in remainder { h0[byte as usize] += 1; }
    let mut sums = [0.0; 4];
    for i in (0..256).step_by(8) {
        let mut counts = [0u16; 8];
        // SAFETY: eight counters fit in every bank; NEON is available on this owned Apple host.
        unsafe {
            use std::arch::aarch64::*;
            let a = vaddq_u16(vld1q_u16(h0.as_ptr().add(i)), vld1q_u16(h1.as_ptr().add(i)));
            let b = vaddq_u16(vld1q_u16(h2.as_ptr().add(i)), vld1q_u16(h3.as_ptr().add(i)));
            let c = vaddq_u16(vld1q_u16(h4.as_ptr().add(i)), vld1q_u16(h5.as_ptr().add(i)));
            let d = vaddq_u16(vld1q_u16(h6.as_ptr().add(i)), vld1q_u16(h7.as_ptr().add(i)));
            vst1q_u16(counts.as_mut_ptr(), vaddq_u16(vaddq_u16(a,b),vaddq_u16(c,d)));
        }
        for offset in [0,4] {
            for lane in 0..4 { sums[lane] += weights[counts[offset+lane] as usize]; }
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
                assert_eq!(fused_simd(input).to_bits(), baseline::scalar_entropy(input).to_bits());
                assert_eq!(fused_simd8(input).to_bits(), baseline::scalar_entropy(input).to_bits());
                cases += 1;
            }
        }
    }
    println!("EXACT_DIFFERENTIAL_PASS {cases}");
    let data = [vec![0;4096], samples[4][..4096].to_vec(), samples[2][..4096].to_vec()];
    let functions: [(&str, fn(&[u8])->f64);4] = [("baseline",baseline::shannon_entropy),("fused",fused),("fused_simd",fused_simd),("fused_simd8",fused_simd8)];
    for d in &data { for (_, f) in functions { for _ in 0..5000 { black_box(f(black_box(d))); } } }
    for round in 0..9 {
        for (kind,d) in ["zero","ascii","random"].iter().zip(&data) {
            for k in 0..4 {
                let (name,f) = functions[(k + round) % 4];
                let t=Instant::now();
                for _ in 0..50000 { black_box(f(black_box(d))); }
                println!("MEASURE {round} {kind} {name} {}", t.elapsed().as_nanos() as f64 / 50000.0);
            }
        }
    }
}
