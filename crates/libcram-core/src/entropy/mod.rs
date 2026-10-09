//! Byte Shannon entropy. SIMD-assisted histograms preserve exact scalar counts.
#![deny(clippy::unwrap_used)]
use std::sync::OnceLock;
fn finish(hist: &[usize; 256], n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    // Cache n*log2(n) only for the bounded hot-page domain.
    static WEIGHTS: OnceLock<Vec<f64>> = OnceLock::new();
    if n <= 4096 {
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
        let mut sums = [0.0; 4];
        for counts in hist.as_chunks::<4>().0 {
            sums[0] += weights[counts[0]];
            sums[1] += weights[counts[1]];
            sums[2] += weights[counts[2]];
            sums[3] += weights[counts[3]];
        }
        ((weights[n] - ((sums[0] + sums[1]) + (sums[2] + sums[3]))) / n as f64).max(0.0)
    } else {
        let sum: f64 = hist
            .iter()
            .filter(|&&v| v != 0)
            .map(|&v| v as f64 * (v as f64).log2())
            .sum();
        ((n as f64).log2() - sum / n as f64).max(0.0)
    }
}
pub fn scalar_entropy(data: &[u8]) -> f64 {
    let mut hist = [0usize; 256];
    for &b in data {
        hist[b as usize] += 1;
    }
    finish(&hist, data.len())
}
type Kernel = unsafe fn(&[u8], &mut [usize; 256]);
fn scalar_hist(data: &[u8], hist: &mut [usize; 256]) {
    for &b in data {
        hist[b as usize] += 1;
    }
}
fn select() -> Kernel {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx512bw") {
            return avx512;
        }
        if std::is_x86_feature_detected!("avx2") {
            return avx2;
        }
    }
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("neon") {
        return neon;
    }
    scalar_hist
}
pub fn shannon_entropy(data: &[u8]) -> f64 {
    static KERNEL: OnceLock<Kernel> = OnceLock::new();
    let mut hist = [0usize; 256];
    // SAFETY: select checks the runtime ISA and kernels bound every vector load.
    unsafe {
        KERNEL.get_or_init(select)(data, &mut hist);
    }
    finish(&hist, data.len())
}
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn neon(data: &[u8], hist: &mut [usize; 256]) {
    use std::arch::aarch64::*;
    // Consume a uniform prefix a cache line at a time. Stop checking after the
    // first mixed line: ordinary input should not pay a SIMD reduction per 16 bytes.
    let mut offset = 0;
    if let Some(&first) = data.first() {
        let expected = vdupq_n_u8(first);
        while data.len() - offset >= 64 {
            // SAFETY: the loop bounds all four loads; NEON accepts unaligned input.
            let equal = unsafe {
                let p = data.as_ptr().add(offset);
                vandq_u8(
                    vandq_u8(
                        vceqq_u8(vld1q_u8(p), expected),
                        vceqq_u8(vld1q_u8(p.add(16)), expected),
                    ),
                    vandq_u8(
                        vceqq_u8(vld1q_u8(p.add(32)), expected),
                        vceqq_u8(vld1q_u8(p.add(48)), expected),
                    ),
                )
            };
            if vminvq_u8(equal) != 255 {
                break;
            }
            offset += 64;
        }
        hist[first as usize] += offset;
    }
    let data = &data[offset..];
    if data.len() < 64 {
        scalar_hist(data, hist);
        return;
    }
    // Eight independent counters break dependency chains for repeated symbols.
    // Each lane receives at most 8192 bytes per batch, so both u16 increments
    // and the four-lane partial sums (at most 32768) are exact without overflow.
    // Merge into usize before processing another batch to support arbitrary sizes.
    for batch in data.chunks(65536) {
        let mut counters = [[0u16; 256]; 8];
        let [h0, h1, h2, h3, h4, h5, h6, h7] = &mut counters;
        let (chunks, remainder) = batch.as_chunks::<64>();
        for chunk in chunks {
            for j in 0..8 {
                // SAFETY: each eight-byte unaligned load is inside this chunk.
                let mut word = unsafe { chunk.as_ptr().add(j * 8).cast::<u64>().read_unaligned() };
                // Keep the word intact without imposing a memory-ordering barrier.
                // SAFETY: this empty register-only assembly preserves its input.
                unsafe {
                    std::arch::asm!("/* {word} */", word = inout(reg) word, options(pure, nomem, nostack, preserves_flags));
                }
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
        for i in 0..256 {
            hist[i] +=
                (h0[i] + h1[i] + h2[i] + h3[i]) as usize + (h4[i] + h5[i] + h6[i] + h7[i]) as usize;
        }
        scalar_hist(remainder, hist);
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2(data: &[u8], hist: &mut [usize; 256]) {
    use std::arch::x86_64::*;
    let (chunks, remainder) = data.as_chunks::<32>();
    for chunk in chunks {
        // SAFETY: the chunk contains 32 readable bytes; loadu permits unaligned input.
        let equal = unsafe {
            _mm256_cmpeq_epi8(
                _mm256_loadu_si256(chunk.as_ptr().cast()),
                _mm256_set1_epi8(chunk[0] as i8),
            )
        };
        if _mm256_movemask_epi8(equal) == -1 {
            hist[chunk[0] as usize] += 32;
        } else {
            scalar_hist(chunk, hist);
        }
    }
    scalar_hist(remainder, hist);
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512bw")]
unsafe fn avx512(data: &[u8], hist: &mut [usize; 256]) {
    use std::arch::x86_64::*;
    let (chunks, remainder) = data.as_chunks::<64>();
    for chunk in chunks {
        // SAFETY: the chunk contains 64 readable bytes; loadu permits unaligned input.
        let equal = unsafe {
            _mm512_cmpeq_epi8_mask(
                _mm512_loadu_si512(chunk.as_ptr().cast()),
                _mm512_set1_epi8(chunk[0] as i8),
            )
        };
        if equal == u64::MAX {
            hist[chunk[0] as usize] += 64;
        } else {
            scalar_hist(chunk, hist);
        }
    }
    scalar_hist(remainder, hist);
}
#[cfg(test)]
mod differential;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhaustive_lengths_alignments_and_uniform_transitions() {
        let kernel = select();
        let mut seed = 0x87162dff_u64;
        for pattern in 0..4 {
            let data: Vec<u8> = (0..131200)
                .map(|i| match pattern {
                    0 => {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        seed as u8
                    }
                    1 => 173,
                    2 => (i / 97) as u8,
                    _ => (i % 7) as u8,
                })
                .collect();
            for offset in 0..64 {
                let mut expected = [0usize; 256];
                for len in 0..=4097 {
                    if len != 0 {
                        expected[data[offset + len - 1] as usize] += 1;
                    }
                    let mut actual = [0usize; 256];
                    // SAFETY: select checks the runtime ISA.
                    unsafe {
                        kernel(&data[offset..offset + len], &mut actual);
                    }
                    assert_eq!(
                        actual, expected,
                        "pattern={pattern}, offset={offset}, len={len}"
                    );
                }
                for len in [8191, 8192, 16384, 65535, 65536, 65537, 131073] {
                    let bytes = &data[offset..offset + len];
                    let mut expected = [0usize; 256];
                    scalar_hist(bytes, &mut expected);
                    let mut actual = [0usize; 256];
                    // Verify additive kernel semantics as well as large batches.
                    actual[19] = 11;
                    expected[19] += 11;
                    // SAFETY: select checks the runtime ISA.
                    unsafe {
                        kernel(bytes, &mut actual);
                    }
                    assert_eq!(actual, expected);
                    assert_eq!(shannon_entropy(bytes), scalar_entropy(bytes));
                }
            }
        }
        // A single different byte at every position must end uniform-prefix
        // handling without losing bytes from the mixed block or the tail.
        let mut data = [173u8; 4160];
        for position in 0..4097 {
            data[position] = 91;
            for offset in [0, 1, 15, 31, 63] {
                let bytes = &data[offset..offset + 4097];
                assert_eq!(shannon_entropy(bytes), scalar_entropy(bytes));
            }
            data[position] = 173;
        }
    }

    #[test]
    fn reference_and_dispatch() {
        assert_eq!(scalar_entropy(&[]), 0.0);
        assert_eq!(shannon_entropy(&[17; 4096]), 0.0);
        let bytes: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        assert_eq!(scalar_entropy(&bytes), 8.0);
        for offset in 0..64 {
            for len in [0, 1, 15, 16, 31, 32, 63, 64, 255, 4096] {
                let data = &bytes[offset..(offset + len).min(bytes.len())];
                assert_eq!(shannon_entropy(data), scalar_entropy(data));
            }
        }
    }
}
