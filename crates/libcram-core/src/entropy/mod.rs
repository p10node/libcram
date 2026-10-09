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
/// Four disjoint banks remove same-symbol dependencies between adjacent bytes.
/// A batch contributes at most 8192 to any u16 counter, and at most 32768
/// to their sum. Merge before the next batch to retain arbitrary-length inputs.
#[inline]
fn banked_hist(data: &[u8], hist: &mut [usize; 256]) {
    if data.len() < 64 {
        scalar_hist(data, hist);
        return;
    }
    for batch in data.chunks(32768) {
        let mut counters = [[0u16; 256]; 4];
        let [h0, h1, h2, h3] = &mut counters;
        let (chunks, remainder) = batch.as_chunks::<32>();
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
        for i in 0..256 {
            hist[i] += (h0[i] + h1[i] + h2[i] + h3[i]) as usize;
        }
        scalar_hist(remainder, hist);
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
    banked_hist(&data[offset..], hist);
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2(data: &[u8], hist: &mut [usize; 256]) {
    use std::arch::x86_64::*;
    let mut offset = 0;
    if let Some(&first) = data.first() {
        let expected = _mm256_set1_epi8(first as i8);
        while data.len() - offset >= 64 {
            // SAFETY: both unaligned vector loads fit in this cache line.
            let equal = unsafe {
                let p = data.as_ptr().add(offset);
                _mm256_and_si256(
                    _mm256_cmpeq_epi8(_mm256_loadu_si256(p.cast()), expected),
                    _mm256_cmpeq_epi8(_mm256_loadu_si256(p.add(32).cast()), expected),
                )
            };
            if _mm256_movemask_epi8(equal) != -1 {
                break;
            }
            offset += 64;
        }
        hist[first as usize] += offset;
    }
    banked_hist(&data[offset..], hist);
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512bw")]
unsafe fn avx512(data: &[u8], hist: &mut [usize; 256]) {
    use std::arch::x86_64::*;
    let mut offset = 0;
    if let Some(&first) = data.first() {
        let expected = _mm512_set1_epi8(first as i8);
        while data.len() - offset >= 64 {
            // SAFETY: the loop bounds this unaligned 64-byte vector load.
            let equal = unsafe {
                _mm512_cmpeq_epi8_mask(
                    _mm512_loadu_si512(data.as_ptr().add(offset).cast()),
                    expected,
                )
            };
            if equal != u64::MAX {
                break;
            }
            offset += 64;
        }
        hist[first as usize] += offset;
    }
    banked_hist(&data[offset..], hist);
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
