//! Exact threshold verdicts that can stop before reading the whole input.
use super::{
    coarse, count, count_banks, count_periodic, finish, merge_banks, periodic, scalar_hist, weights,
};

/// Decision margin in bits. For any length, `finish` and `Bounds` sum at most
/// 256 terms whose per-byte magnitude is at most log2(n) <= 64 bits. With f64
/// epsilon 1.1e-16, their rounding error stays below 1e-11 bits. A verdict
/// taken outside this margin therefore cannot disagree with the computed
/// entropy.
const MARGIN: f64 = 1e-9;
/// Shorter inputs are cheap enough to count completely.
const MIN_LEN: usize = 256;
/// Early verdicts use the cached `v * log2(v)` table of the hot-page domain.
/// Longer inputs are counted completely.
const MAX_LEN: usize = 4096;
/// Binary entropy of the 3/4 checkpoint, h(3/4) = 2 - (3/4)*log2(3).
const SPLIT_THREE_QUARTERS: f64 = 0.811_278_124_459_132_8;

/// Returns exactly `shannon_entropy(data) < threshold`, usually without
/// counting every byte.
///
/// Periodic inputs are counted from one period. Otherwise, on hardware with a
/// store-free coarse kernel (x86_64 AVX2), a lower bound from coarse cell counts
/// (see `coarse`) can certify `entropy >= threshold` without counting any byte
/// exactly. Failing that, the input is counted in prefixes (1/2, then 3/4). After each prefix, the
/// entropy of the whole input is bounded from both sides:
///
/// * lowest: every unread byte repeats the most frequent symbol so far. Since
///   `x * log2(x)` is convex, concentrating the remainder on the largest count
///   is the completion with the lowest entropy;
/// * highest: entropy of the mixture of the prefix and an arbitrary remainder,
///   `H <= q*H(prefix) + (1-q)*log2(min(256, rest)) + h(q)`.
///
/// When both bounds are on the same side of `threshold` by more than `MARGIN`,
/// that side is the verdict. Otherwise the whole input is counted and
/// finalized exactly as `shannon_entropy` does it.
pub fn entropy_below(data: &[u8], threshold: f64) -> bool {
    verdict(data, threshold).0
}

/// The verdict and the number of bytes counted to reach it.
fn verdict(data: &[u8], threshold: f64) -> (bool, usize) {
    let n = data.len();
    let mut hist = [0usize; 256];
    if !(MIN_LEN..=MAX_LEN).contains(&n) || threshold.is_nan() {
        count(data, &mut hist);
        return (finish(&hist, n) < threshold, n);
    }
    let period = periodic::period(data);
    if period != 0 {
        scalar_hist(count_periodic(data, period, &mut hist), &mut hist);
        return (finish(&hist, n) < threshold, n);
    }
    if n >= coarse::MIN_LEN && coarse::at_least(data, threshold + MARGIN) {
        return (false, 0);
    }
    // One set of u16 banks spans every checkpoint (n <= MAX_LEN, so no counter
    // wraps). Checkpoints are multiples of 32 bytes, so `count_banks` leaves no
    // uncounted tail before the final segment.
    let mut counters = [[0u16; 256]; 4];
    let mut seen = 0;
    for (index, stop) in [(n / 2) & !31, (n - n / 4) & !31].into_iter().enumerate() {
        let tail = count_banks(&data[seen..stop], &mut counters);
        debug_assert!(tail.is_empty());
        seen = stop;
        let prefix = Prefix::new(&counters, seen);
        if prefix.lowest(n) >= threshold + MARGIN {
            return (false, seen);
        }
        if prefix.highest(n) < threshold - MARGIN {
            return (true, seen);
        }
        // Performance only: skip the 3/4 checkpoint when, at the prefix's
        // current entropy, its bounds would still straddle the threshold.
        let lowest = 0.75 * prefix.entropy() + SPLIT_THREE_QUARTERS;
        if index == 0 && lowest < threshold && lowest + 2.0 >= threshold {
            break;
        }
    }
    let tail = count_banks(&data[seen..], &mut counters);
    merge_banks(&counters, &mut hist);
    scalar_hist(tail, &mut hist);
    (finish(&hist, n) < threshold, n)
}

/// Counts of the first `seen` bytes (0 < seen < n <= MAX_LEN) of an `n`-byte
/// input, reduced to what the entropy bounds need.
struct Prefix {
    seen: usize,
    /// Sum of `c * log2(c)` over the symbol counts.
    sum: f64,
    max: usize,
}

impl Prefix {
    fn new(counters: &[[u16; 256]; 4], seen: usize) -> Self {
        let [h0, h1, h2, h3] = counters;
        let mut merged = [0u16; 256];
        for i in 0..256 {
            merged[i] = h0[i] + h1[i] + h2[i] + h3[i];
        }
        let table = weights();
        // Independent sums keep the 256 additions off one latency chain.
        let mut sums = [0.0; 8];
        for counts in merged.as_chunks::<8>().0 {
            for lane in 0..8 {
                // Counts never exceed `seen` < MAX_LEN; `min` only lets the
                // compiler drop the bounds check.
                sums[lane] += table[usize::from(counts[lane]).min(MAX_LEN)];
            }
        }
        let max = merged.iter().copied().max().unwrap_or(0);
        Self {
            seen,
            sum: sums.iter().sum(),
            max: usize::from(max),
        }
    }

    /// Entropy of the counted prefix itself.
    fn entropy(&self) -> f64 {
        (weights()[self.seen] - self.sum) / self.seen as f64
    }

    /// Lowest entropy of any completion: every unread byte repeats the most
    /// frequent symbol so far.
    fn lowest(&self, n: usize) -> f64 {
        let table = weights();
        let rest = n - self.seen;
        (table[n] - (self.sum - table[self.max] + table[self.max + rest])) / n as f64
    }

    /// Highest entropy of any completion: the mixture of the prefix and an
    /// arbitrary remainder.
    fn highest(&self, n: usize) -> f64 {
        let rest = n - self.seen;
        let q = self.seen as f64 / n as f64;
        let split = -(q * q.log2() + (1.0 - q) * (1.0 - q).log2());
        q * self.entropy() + (1.0 - q) * (rest.min(256) as f64).log2() + split
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::shannon_entropy;

    fn xorshift(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    fn inputs(len: usize) -> Vec<Vec<u8>> {
        let mut next = xorshift(0x9e37_79b9_7f4a_7c15 ^ len as u64);
        let mut sets = vec![vec![0u8; len], (0..len).map(|i| (i % 256) as u8).collect()];
        sets.push(
            b"libcram memory tier orchestration "
                .iter()
                .copied()
                .cycle()
                .take(len)
                .collect(),
        );
        for alphabet in [2u64, 3, 5, 16, 22, 23, 24, 32, 45, 64, 128, 256] {
            sets.push((0..len).map(|_| (next() % alphabet) as u8).collect());
        }
        // Geometric-like skew: entropy near 2 bits.
        sets.push((0..len).map(|_| next().leading_zeros() as u8).collect());
        let random: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        for cut in [len / 4, len / 2, len - len / 4, len.saturating_sub(1)] {
            // Worst cases for the bounds: a random prefix followed by the most
            // frequent prefix symbol (exactly the lowest completion), or a
            // constant prefix followed by random bytes.
            let mut low = random.clone();
            let mut hist = [0usize; 256];
            low[..cut].iter().for_each(|&b| hist[b as usize] += 1);
            let top = (0..256).max_by_key(|&b| hist[b]).unwrap_or(0) as u8;
            low[cut..].fill(top);
            sets.push(low);
            let mut high = random.clone();
            high[..cut].fill(7);
            sets.push(high);
        }
        sets.push(random);
        sets
    }

    fn thresholds(entropy: f64) -> Vec<f64> {
        let mut values: Vec<f64> = (0..=17).map(|i| i as f64 * 0.5).collect();
        values.extend([
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            1e-12,
            4.5,
            8.5,
            entropy,
            entropy.next_up(),
            entropy.next_down(),
        ]);
        for delta in [1e-12, 1e-10, 1e-9, 2e-9, 1e-8, 1e-6, 1e-3, 0.1] {
            values.extend([entropy + delta, entropy - delta]);
        }
        values
    }

    #[test]
    fn verdict_equals_full_entropy_comparison() {
        for len in [
            0, 1, 63, 255, 256, 257, 511, 1000, 2047, 2048, 2560, 3413, 3414, 4095, 4096, 4097,
            10007, 65537,
        ] {
            for data in inputs(len) {
                let entropy = shannon_entropy(&data);
                for threshold in thresholds(entropy) {
                    assert_eq!(
                        entropy_below(&data, threshold),
                        entropy < threshold,
                        "len={len} entropy={entropy} threshold={threshold}"
                    );
                }
            }
        }
    }

    #[test]
    fn bounds_contain_every_completion_of_the_prefix() {
        let mut next = xorshift(0x51ed_270b);
        for alphabet in [1u64, 4, 23, 256] {
            let data: Vec<u8> = (0..4096).map(|_| (next() % alphabet) as u8).collect();
            for seen in [1, 100, 2048, 3072, 4095] {
                let mut counters = [[0u16; 256]; 4];
                for (i, &b) in data[..seen].iter().enumerate() {
                    counters[i & 3][b as usize] += 1;
                }
                let prefix = Prefix::new(&counters, seen);
                let (lowest, highest) = (prefix.lowest(data.len()), prefix.highest(data.len()));
                for fill in [0u8, 1, 200] {
                    for random_tail in [false, true] {
                        let mut full = data.clone();
                        for byte in &mut full[seen..] {
                            *byte = if random_tail { next() as u8 } else { fill };
                        }
                        let entropy = shannon_entropy(&full);
                        assert!(
                            lowest - MARGIN <= entropy && entropy <= highest + MARGIN,
                            "alphabet={alphabet} seen={seen}: {lowest} <= {entropy} <= {highest}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn random_page_is_decided_without_counting_it_all() {
        let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
        let random: Vec<u8> = (0..4096).map(|_| next() as u8).collect();
        // The coarse bound decides it without exact counts where it runs;
        // otherwise the first checkpoint does.
        #[cfg(target_arch = "x86_64")]
        let expected = if std::is_x86_feature_detected!("avx2") {
            0
        } else {
            2048
        };
        #[cfg(not(target_arch = "x86_64"))]
        let expected = 2048;
        assert_eq!(verdict(&random, 4.5), (false, expected));
        // Uniform pages are periodic; the kernel counts them from one byte.
        assert_eq!(verdict(&[0u8; 4096], 4.5), (true, 4096));
    }
}
