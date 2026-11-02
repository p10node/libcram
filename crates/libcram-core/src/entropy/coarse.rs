//! Store-free lower bound on byte entropy from coarse cell counts.
//!
//! Cells are `c32(x) = x >> 3` (bits 7..3) and `c16(x) = (x >> 3) & 15`
//! (bits 6..3), so every c16 cell is the union of the two c32 cells that differ
//! only in bit 7. Grouping never increases entropy: `H(bytes) >= H(c32(bytes))`.
//! Given exact c32 counts of a prefix and c16 counts of the rest, the completion
//! with the lowest `H(c32)` puts each c16 count on the larger of its two c32
//! cells, because `x * log2(x)` is convex. [`Cells::lowest`] is therefore the
//! exact minimum of `H(c32)` over all completions, and a lower bound on the
//! entropy of the bytes.
//!
//! On x86_64 with AVX2 the cells are counted without per-byte stores: `vpshufb`
//! turns each byte into a one-hot bit of an 8-bit plane, Harley-Seal carry-save
//! adders count the bits per position, and an 8x8 bit transpose plus `vpsadbw`
//! reduces the carry-save levels. Exact histograms are bounded by one counter
//! store per byte on that hardware; this bound is not.
//!
//! The bound itself is portable and tested on every target; only x86_64 has a
//! kernel that makes it cheaper than the exact verdict.
#![cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
use super::weights;

/// Shorter inputs leave too small a prefix for a decisive bound.
pub(super) const MIN_LEN: usize = 2048;
/// Largest input: the bound reads the cached `x * log2(x)` table.
pub(super) const MAX_LEN: usize = 4096;
const BLOCK: usize = 512;

/// c32 counts of the first `split(n)` bytes and c16 counts of the rest.
#[derive(Debug, PartialEq)]
pub(super) struct Cells {
    prefix: [u32; 32],
    rest: [u32; 16],
}

/// Prefix length: 3/8 of the input, rounded to whole 512-byte blocks.
pub(super) fn split(n: usize) -> usize {
    (3 * n / 8 + BLOCK / 2) & !(BLOCK - 1)
}

impl Cells {
    /// Reference counts of `data` (`MIN_LEN..=MAX_LEN` bytes).
    #[cfg(test)]
    pub(super) fn scalar(data: &[u8]) -> Self {
        let (prefix, rest) = data.split_at(split(data.len()));
        let mut cells = Self {
            prefix: [0; 32],
            rest: [0; 16],
        };
        prefix
            .iter()
            .for_each(|&b| cells.prefix[usize::from(b >> 3)] += 1);
        count_rest(rest, &mut cells.rest);
        cells
    }

    /// Prefix entropy of the c32 cells.
    pub(super) fn prefix_entropy(&self) -> f64 {
        entropy(self.prefix.iter().copied())
    }

    /// Lowest `H(c32)` of an `n`-byte input over every split of the c16 counts
    /// between their two c32 cells.
    pub(super) fn lowest(&self, n: usize) -> f64 {
        let table = weights();
        let mut sum = 0.0;
        for (cell, &rest) in self.rest.iter().enumerate() {
            let (low, high) = (self.prefix[cell], self.prefix[cell + 16]);
            let (small, large) = (low.min(high), low.max(high));
            sum += table[(large + rest) as usize] + table[small as usize];
        }
        (table[n] - sum) / n as f64
    }
}

fn entropy(counts: impl Iterator<Item = u32> + Clone) -> f64 {
    let table = weights();
    let n = counts.clone().sum::<u32>() as usize;
    let sum: f64 = counts.map(|c| table[c as usize]).sum();
    (table[n] - sum) / n as f64
}

fn count_rest(bytes: &[u8], rest: &mut [u32; 16]) {
    bytes
        .iter()
        .for_each(|&b| rest[usize::from((b >> 3) & 15)] += 1);
}

/// Returns true only if the entropy of `data` (`MIN_LEN..=MAX_LEN` bytes) is
/// certainly at least `bound`. False means "not certified", never "below".
pub(super) fn at_least(data: &[u8], bound: f64) -> bool {
    if !(MIN_LEN..=MAX_LEN).contains(&data.len()) {
        return false;
    }
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: AVX2 was detected at runtime.
        return unsafe { avx2::at_least(data, bound) };
    }
    let _ = (data, bound);
    false
}

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::{count_rest, Cells, BLOCK};
    use std::arch::x86_64::*;
    type V = __m256i;

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn at_least(data: &[u8], bound: f64) -> bool {
        let n = data.len();
        let (prefix, rest) = data.split_at(super::split(n));
        // Performance only (the exact verdict follows either way): skip
        // inputs whose first 128 bytes touch too few cells for the bound, or
        // whose prefix is too concentrated.
        let head = prefix.as_chunks::<32>().0;
        if f64::from(present(&head[..4])) < bound.exp2() {
            return false;
        }
        let mut cells = Cells {
            prefix: [0; 32],
            rest: [0; 16],
        };
        count::<4>(prefix.as_chunks::<BLOCK>().0, &mut cells.prefix);
        if cells.prefix_entropy() < bound {
            return false;
        }
        let (blocks, tail) = rest.as_chunks::<BLOCK>();
        count::<2>(blocks, &mut cells.rest);
        count_rest(tail, &mut cells.rest);
        cells.lowest(n) >= bound
    }

    /// Full adder on every bit: returns (carry, sum).
    #[target_feature(enable = "avx2")]
    #[inline]
    fn csa(a: V, b: V, c: V) -> (V, V) {
        let u = _mm256_xor_si256(a, b);
        (
            _mm256_or_si256(_mm256_and_si256(a, b), _mm256_and_si256(u, c)),
            _mm256_xor_si256(u, c),
        )
    }

    /// One-hot planes of 32 bytes: bit `k` of plane `p` marks cell `8p + k`.
    /// `P == 2` gives the c16 cells, `P == 4` the c32 cells.
    #[target_feature(enable = "avx2")]
    #[inline]
    fn planes<const P: usize>(chunk: &[u8; 32]) -> [V; P] {
        // SAFETY: `chunk` is 32 readable bytes; the load is unaligned.
        let x = unsafe { _mm256_loadu_si256(chunk.as_ptr().cast()) };
        let low = _mm256_setr_epi8(
            1, 2, 4, 8, 16, 32, 64, -128, 0, 0, 0, 0, 0, 0, 0, 0, //
            1, 2, 4, 8, 16, 32, 64, -128, 0, 0, 0, 0, 0, 0, 0, 0,
        );
        let high = _mm256_setr_epi8(
            0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 64, -128, //
            0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 64, -128,
        );
        // c16 index (bits 6..3); 16-bit shifts leak only into masked bits.
        let index = _mm256_and_si256(_mm256_srli_epi16::<3>(x), _mm256_set1_epi8(0x0f));
        let c16 = [
            _mm256_shuffle_epi8(low, index),
            _mm256_shuffle_epi8(high, index),
        ];
        // Bit 7 set: signed byte below zero.
        let top = _mm256_cmpgt_epi8(_mm256_setzero_si256(), x);
        let c32 = [
            _mm256_andnot_si256(top, c16[0]),
            _mm256_andnot_si256(top, c16[1]),
            _mm256_and_si256(top, c16[0]),
            _mm256_and_si256(top, c16[1]),
        ];
        let mut out = [_mm256_setzero_si256(); P];
        for (plane, slot) in out.iter_mut().enumerate() {
            *slot = if P == 2 {
                c16[plane % 2]
            } else {
                c32[plane % 4]
            };
        }
        out
    }

    /// Number of distinct c32 cells in `chunks`.
    #[target_feature(enable = "avx2")]
    #[inline]
    fn present(chunks: &[[u8; 32]]) -> u32 {
        let mut any = [_mm256_setzero_si256(); 4];
        for chunk in chunks {
            let planes = planes::<4>(chunk);
            for (any, plane) in any.iter_mut().zip(planes) {
                *any = _mm256_or_si256(*any, plane);
            }
        }
        let mut cells = 0;
        for plane in any {
            let x = _mm_or_si128(
                _mm256_castsi256_si128(plane),
                _mm256_extracti128_si256::<1>(plane),
            );
            let x = _mm_or_si128(x, _mm_srli_si128::<8>(x));
            let x = _mm_or_si128(x, _mm_srli_si128::<4>(x));
            let x = _mm_or_si128(x, _mm_srli_si128::<2>(x));
            let x = _mm_or_si128(x, _mm_srli_si128::<1>(x));
            cells += (_mm_cvtsi128_si32(x) as u8).count_ones();
        }
        cells
    }

    /// Adds the cell counts of `blocks` to `out` (`8 * P` cells).
    #[target_feature(enable = "avx2")]
    fn count<const P: usize>(blocks: &[[u8; BLOCK]], out: &mut [u32]) {
        // Eight blocks hold at most 128 vectors: every per-lane, per-bit count
        // fits the byte that `reduce` sums.
        for group in blocks.chunks(8) {
            count_group::<P>(group, out);
        }
    }

    #[target_feature(enable = "avx2")]
    #[inline]
    fn count_group<const P: usize>(blocks: &[[u8; BLOCK]], out: &mut [u32]) {
        let zero = _mm256_setzero_si256();
        let (mut ones, mut twos, mut fours, mut eights) =
            ([zero; P], [zero; P], [zero; P], [zero; P]);
        let mut sixteens = [[zero; P]; 8];
        for (block, sixteen) in blocks.iter().zip(&mut sixteens) {
            let vectors = block.as_chunks::<32>().0;
            let mut eights_ab = [[zero; P]; 2];
            for (half, eight) in eights_ab.iter_mut().enumerate() {
                let mut fours_ab = [[zero; P]; 2];
                for (quad, four) in fours_ab.iter_mut().enumerate() {
                    let base = 8 * half + 4 * quad;
                    let (a, b) = (planes::<P>(&vectors[base]), planes::<P>(&vectors[base + 1]));
                    let (c, d) = (
                        planes::<P>(&vectors[base + 2]),
                        planes::<P>(&vectors[base + 3]),
                    );
                    for q in 0..P {
                        let (twos_a, sum) = csa(ones[q], a[q], b[q]);
                        let (twos_b, sum) = csa(sum, c[q], d[q]);
                        ones[q] = sum;
                        let (carry, sum) = csa(twos[q], twos_a, twos_b);
                        twos[q] = sum;
                        four[q] = carry;
                    }
                }
                for q in 0..P {
                    let (carry, sum) = csa(fours[q], fours_ab[0][q], fours_ab[1][q]);
                    fours[q] = sum;
                    eight[q] = carry;
                }
            }
            for q in 0..P {
                let (carry, sum) = csa(eights[q], eights_ab[0][q], eights_ab[1][q]);
                eights[q] = sum;
                sixteen[q] = carry;
            }
        }
        for q in 0..P {
            // Second carry-save level over at most eight sixteens: a 4-bit
            // ripple counter with weights 16, 32, 64 and 128.
            let (mut s16, mut s32, mut s64, mut s128) = (zero, zero, zero, zero);
            for pair in sixteens[..blocks.len()].chunks(2) {
                let second = pair.get(1).map_or(zero, |s| s[q]);
                let (carry, sum) = csa(s16, pair[0][q], second);
                s16 = sum;
                let (carry, sum) = csa(s32, carry, zero);
                s32 = sum;
                let (carry, sum) = csa(s64, carry, zero);
                s64 = sum;
                s128 = _mm256_or_si256(s128, carry);
            }
            reduce(
                [ones[q], twos[q], fours[q], eights[q], s16, s32, s64, s128],
                &mut out[8 * q..8 * q + 8],
            );
        }
    }

    /// Exchanges the high `S` bits of every byte of `a` with the low `S` bits
    /// of the same byte of `b`.
    #[target_feature(enable = "avx2")]
    #[inline]
    fn exchange<const S: i32>(a: &mut V, b: &mut V, mask: V) {
        let t = _mm256_and_si256(_mm256_xor_si256(_mm256_srli_epi16::<S>(*a), *b), mask);
        *b = _mm256_xor_si256(*b, t);
        *a = _mm256_xor_si256(*a, _mm256_slli_epi16::<S>(t));
    }

    /// `out[k] +=` the sum over lanes and levels `L` of bit `k` of
    /// `levels[L] << L`. An 8x8 bit transpose per byte makes byte `k` hold the
    /// weighted count of bit `k` for its lane; `vpsadbw` sums the lanes.
    #[target_feature(enable = "avx2")]
    #[inline]
    fn reduce(mut levels: [V; 8], out: &mut [u32]) {
        let (m4, m2, m1) = (
            _mm256_set1_epi8(0x0f),
            _mm256_set1_epi8(0x33),
            _mm256_set1_epi8(0x55),
        );
        let [v0, v1, v2, v3, v4, v5, v6, v7] = &mut levels;
        exchange::<4>(v0, v4, m4);
        exchange::<4>(v1, v5, m4);
        exchange::<4>(v2, v6, m4);
        exchange::<4>(v3, v7, m4);
        exchange::<2>(v0, v2, m2);
        exchange::<2>(v1, v3, m2);
        exchange::<2>(v4, v6, m2);
        exchange::<2>(v5, v7, m2);
        exchange::<1>(v0, v1, m1);
        exchange::<1>(v2, v3, m1);
        exchange::<1>(v4, v5, m1);
        exchange::<1>(v6, v7, m1);
        let zero = _mm256_setzero_si256();
        for (half, bits) in levels.as_chunks::<4>().0.iter().enumerate() {
            // Each lane-group sum is at most 8 * 255 and each total at most
            // 32 * 255, so four totals pack into 16-bit fields of one u64.
            let [a, b, c, d] = [
                _mm256_sad_epu8(bits[0], zero),
                _mm256_sad_epu8(bits[1], zero),
                _mm256_sad_epu8(bits[2], zero),
                _mm256_sad_epu8(bits[3], zero),
            ];
            let packed = _mm256_or_si256(
                _mm256_or_si256(a, _mm256_slli_epi64::<16>(b)),
                _mm256_or_si256(_mm256_slli_epi64::<32>(c), _mm256_slli_epi64::<48>(d)),
            );
            let sum = _mm_add_epi64(
                _mm256_castsi256_si128(packed),
                _mm256_extracti128_si256::<1>(packed),
            );
            let fields = _mm_cvtsi128_si64(_mm_add_epi64(sum, _mm_unpackhi_epi64(sum, sum))) as u64;
            for (field, slot) in out[4 * half..4 * half + 4].iter_mut().enumerate() {
                *slot += ((fields >> (16 * field)) & 0xffff) as u32;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn cells(data: &[u8]) -> Cells {
        let (prefix, rest) = data.split_at(super::split(data.len()));
        let mut cells = Cells {
            prefix: [0; 32],
            rest: [0; 16],
        };
        let (blocks, tail) = rest.as_chunks::<BLOCK>();
        // SAFETY: tests call this only after detecting AVX2.
        unsafe {
            count::<4>(prefix.as_chunks::<BLOCK>().0, &mut cells.prefix);
            count::<2>(blocks, &mut cells.rest);
        }
        count_rest(tail, &mut cells.rest);
        cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::shannon_entropy;

    fn pages() -> Vec<Vec<u8>> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut pages = Vec::new();
        for len in [MIN_LEN, 2049, 2560, 3000, 3413, 3414, 4000, 4095, MAX_LEN] {
            pages.push((0..len).map(|_| next() as u8).collect());
            pages.push((0..len).map(|i| (i % 256) as u8).collect());
            pages.push(vec![0x80; len]);
            pages.push(vec![0xff; len]);
            for alphabet in [2u64, 23, 100, 200] {
                pages.push(
                    (0..len)
                        .map(|_| ((next() % alphabet) as u8).wrapping_mul(3))
                        .collect(),
                );
            }
            // A random prefix whose remainder sits in one c32 cell: exactly
            // the lowest completion the bound considers.
            let cut = split(len);
            let mut page: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            page[cut..].fill(0x9c);
            pages.push(page);
            // Every byte with bit 7 set or clear, and random high bits only.
            pages.push((0..len).map(|_| next() as u8 | 0x80).collect());
            pages.push((0..len).map(|_| next() as u8 & 0xf8).collect());
        }
        pages
    }

    #[test]
    fn coarse_bound_never_exceeds_the_entropy() {
        for page in pages() {
            let n = page.len();
            let lowest = Cells::scalar(&page).lowest(n);
            let entropy = shannon_entropy(&page);
            assert!(lowest <= entropy + 1e-9, "n={n}: {lowest} > {entropy}");
            // The bound is the exact minimum over completions: moving every
            // remaining byte into one c32 cell of its c16 group reaches it.
            assert!(!at_least(&page, entropy + 1e-6), "n={n}");
        }
    }

    #[test]
    fn random_pages_are_certified_above_the_policy_threshold() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let page: Vec<u8> = (0..MAX_LEN)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let lowest = Cells::scalar(&page).lowest(MAX_LEN);
        assert!(lowest > 4.6, "{lowest}");
        #[cfg(target_arch = "x86_64")]
        assert_eq!(at_least(&page, 4.5), std::is_x86_feature_detected!("avx2"));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn avx2_cells_equal_scalar_cells() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        for page in pages() {
            assert_eq!(avx2::cells(&page), Cells::scalar(&page), "n={}", page.len());
        }
    }
}
