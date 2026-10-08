//! Byte Shannon entropy. SIMD run histograms preserve exact scalar counts.
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
    let (chunks, remainder) = data.as_chunks::<16>();
    for chunk in chunks {
        // SAFETY: chunks_exact guarantees 16 readable bytes; unaligned loads are valid.
        let equal = unsafe { vceqq_u8(vld1q_u8(chunk.as_ptr()), vdupq_n_u8(chunk[0])) };
        if vminvq_u8(equal) == 255 {
            hist[chunk[0] as usize] += 16;
        } else {
            scalar_hist(chunk, hist);
        }
    }
    scalar_hist(remainder, hist);
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
