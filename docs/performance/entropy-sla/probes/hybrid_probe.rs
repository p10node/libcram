//! x86_64 probe: store-free coarse lower bound for the entropy verdict.
//! Build: rustc --edition 2021 -C opt-level=3 -C codegen-units=1 hybrid_probe.rs
//! Run:   taskset -c 0 ./hybrid_probe
//! Cells: c32 = x >> 3 (b7..b3), c16 = (x >> 3) & 15 (b6..b3); each c16 cell is
//! the union of two c32 cells. 32-cell counts on a prefix and 16-cell counts on
//! the rest give the exact minimum of H(c32(page)) over all completions, which
//! is <= H(bytes). One-hot bits from vpshufb, counted with Harley-Seal.
//! Variants: per-block positional flush (`flush`), a second carry-save level
//! (`l2`), and l2 plus an 8x8 bit transpose and vpsadbw for the final reduction
//! (`tr`, adopted in production `coarse.rs`). `c32z` uses four shuffles with
//! vpshufb zeroing instead of a sign mask.
#![cfg(target_arch = "x86_64")]
#![allow(unsafe_op_in_unsafe_fn)]
use std::arch::x86_64::*;
use std::hint::black_box;
use std::time::Instant;

type V = __m256i;

#[inline(always)]
unsafe fn csa(a: V, b: V, c: V) -> (V, V) {
    let u = _mm256_xor_si256(a, b);
    (_mm256_or_si256(_mm256_and_si256(a, b), _mm256_and_si256(u, c)), _mm256_xor_si256(u, c))
}

#[inline(always)]
fn popcnt(x: u32) -> u32 {
    let r: u32;
    unsafe {
        std::arch::asm!("xor {r:e}, {r:e}", "popcnt {r:e}, {x:e}", r = out(reg) r, x = in(reg) x,
                        options(pure, nomem, nostack));
    }
    r
}

/// out[k] += (lanes with bit k set) << shift.
#[inline(always)]
unsafe fn positional(v: V, out: &mut [u32], shift: u32) {
    let mut s = v;
    for k in (0..8).rev() {
        out[k] += popcnt(_mm256_movemask_epi8(s) as u32) << shift;
        s = _mm256_add_epi8(s, s);
    }
}

/// Exchange the high `S` bits of each byte of `a` with the low `S` bits of `b`.
#[inline(always)]
unsafe fn swap<const S: i32>(a: &mut V, b: &mut V, mask: V) {
    let t = _mm256_and_si256(_mm256_xor_si256(_mm256_srli_epi16::<S>(*a), *b), mask);
    *b = _mm256_xor_si256(*b, t);
    *a = _mm256_xor_si256(*a, _mm256_slli_epi16::<S>(t));
}

/// out[k] += sum over lanes and levels L of bit k of levels[L] << L.
/// An 8x8 bit transpose per lane turns the levels into one byte per bit k,
/// whose lane sum (vpsadbw) is the weighted count; no vpmovmskb/popcnt.
#[inline(always)]
unsafe fn reduce(mut v: [V; 8], out: &mut [u32]) {
    let (m4, m2, m1) = (_mm256_set1_epi8(0x0f), _mm256_set1_epi8(0x33), _mm256_set1_epi8(0x55));
    let [v0, v1, v2, v3, v4, v5, v6, v7] = &mut v;
    swap::<4>(v0, v4, m4); swap::<4>(v1, v5, m4); swap::<4>(v2, v6, m4); swap::<4>(v3, v7, m4);
    swap::<2>(v0, v2, m2); swap::<2>(v1, v3, m2); swap::<2>(v4, v6, m2); swap::<2>(v5, v7, m2);
    swap::<1>(v0, v1, m1); swap::<1>(v2, v3, m1); swap::<1>(v4, v5, m1); swap::<1>(v6, v7, m1);
    let z = _mm256_setzero_si256();
    let s = v.map(|t| _mm256_sad_epu8(t, z));
    for half in 0..2 {
        let [a, b, c, d] = [s[4 * half], s[4 * half + 1], s[4 * half + 2], s[4 * half + 3]];
        let packed = _mm256_or_si256(_mm256_or_si256(a, _mm256_slli_epi64::<16>(b)),
                                     _mm256_or_si256(_mm256_slli_epi64::<32>(c), _mm256_slli_epi64::<48>(d)));
        let x = _mm_add_epi64(_mm256_castsi256_si128(packed), _mm256_extracti128_si256::<1>(packed));
        let x = _mm_add_epi64(x, _mm_unpackhi_epi64(x, x));
        let fields = _mm_cvtsi128_si64(x) as u64;
        for j in 0..4 {
            out[4 * half + j] += ((fields >> (16 * j)) & 0xffff) as u32;
        }
    }
}

#[derive(Clone, Copy)]
struct K { a: V, b: V, m0f: V }

#[target_feature(enable = "avx2")]
unsafe fn k() -> K {
    K {
        a: _mm256_setr_epi8(1, 2, 4, 8, 16, 32, 64, -128, 0, 0, 0, 0, 0, 0, 0, 0,
                            1, 2, 4, 8, 16, 32, 64, -128, 0, 0, 0, 0, 0, 0, 0, 0),
        b: _mm256_setr_epi8(0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 64, -128,
                            0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 64, -128),
        m0f: _mm256_set1_epi8(0x0f),
    }
}

/// 16 cells (b6..b3): plane 0 = cells 0..8, plane 1 = cells 8..16.
#[inline(always)]
unsafe fn p16(x: V, k: K) -> [V; 2] {
    let t = _mm256_and_si256(_mm256_srli_epi16(x, 3), k.m0f);
    [_mm256_shuffle_epi8(k.a, t), _mm256_shuffle_epi8(k.b, t)]
}

/// 32 cells (b7..b3) via the b7 sign mask: 2 shuffles instead of 4.
#[inline(always)]
unsafe fn p32(x: V, k: K) -> [V; 4] {
    let [a, b] = p16(x, k);
    let m7 = _mm256_cmpgt_epi8(_mm256_setzero_si256(), x);
    [_mm256_andnot_si256(m7, a), _mm256_andnot_si256(m7, b), _mm256_and_si256(m7, a), _mm256_and_si256(m7, b)]
}

/// 32 cells via vpshufb's zeroing of indices with bit 7 set: 4 shuffles.
#[inline(always)]
unsafe fn p32z(x: V, k: K) -> [V; 4] {
    let m80 = _mm256_set1_epi8(-128);
    let t = _mm256_and_si256(_mm256_srli_epi16(x, 3), k.m0f);
    let i0 = _mm256_or_si256(t, _mm256_and_si256(x, m80));
    let i1 = _mm256_xor_si256(i0, m80);
    [_mm256_shuffle_epi8(k.a, i0), _mm256_shuffle_epi8(k.b, i0),
     _mm256_shuffle_epi8(k.a, i1), _mm256_shuffle_epi8(k.b, i1)]
}

/// Harley-Seal over 16-vector blocks (data.len() % 512 == 0, <= 8 blocks).
/// FLUSH: positional count of each block's sixteens, else keep them and run a
/// second carry-save level at the end.
#[inline(always)]
unsafe fn hs<const P: usize, const FLUSH: bool, const TR: bool>(
    data: &[u8], k: K, planes: impl Fn(V, K) -> [V; P], out: &mut [u32]) {
    let z = _mm256_setzero_si256();
    let (mut ones, mut twos, mut fours, mut eights) = ([z; P], [z; P], [z; P], [z; P]);
    let mut sixteens = [[z; P]; 8];
    let blocks = data.as_chunks::<512>().0;
    for (index, block) in blocks.iter().enumerate() {
        let p = block.as_ptr();
        let v = |i: usize| planes(_mm256_loadu_si256(p.add(32 * i).cast()), k);
        let mut eights_ab = [[z; P]; 2];
        for half in 0..2 {
            let mut fours_ab = [[z; P]; 2];
            for quad in 0..2 {
                let base = 8 * half + 4 * quad;
                let (a, b) = (v(base), v(base + 1));
                let mut ta = [z; P];
                for q in 0..P { let (t, o) = csa(ones[q], a[q], b[q]); ta[q] = t; ones[q] = o; }
                let (c, d) = (v(base + 2), v(base + 3));
                for q in 0..P {
                    let (tb, o) = csa(ones[q], c[q], d[q]);
                    ones[q] = o;
                    let (f, t) = csa(twos[q], ta[q], tb);
                    twos[q] = t;
                    fours_ab[quad][q] = f;
                }
            }
            for q in 0..P {
                let (e, f) = csa(fours[q], fours_ab[0][q], fours_ab[1][q]);
                fours[q] = f;
                eights_ab[half][q] = e;
            }
        }
        for q in 0..P {
            let (s, e) = csa(eights[q], eights_ab[0][q], eights_ab[1][q]);
            eights[q] = e;
            if FLUSH { positional(s, &mut out[8 * q..8 * q + 8], 4); } else { sixteens[index][q] = s; }
        }
    }
    if TR && !FLUSH {
        for q in 0..P {
            let (mut s1, mut s2, mut s4, mut s8) = (z, z, z, z);
            for pair in 0..blocks.len().div_ceil(2) {
                let (a, b) = (sixteens[2 * pair][q], if 2 * pair + 1 < blocks.len() { sixteens[2 * pair + 1][q] } else { z });
                let (t2, o) = csa(s1, a, b);
                s1 = o;
                let (t4, t) = csa(s2, t2, z);
                s2 = t;
                let (t8, f) = csa(s4, t4, z);
                s4 = f;
                s8 = _mm256_or_si256(s8, t8);
            }
            reduce([ones[q], twos[q], fours[q], eights[q], s1, s2, s4, s8], &mut out[8 * q..8 * q + 8]);
        }
        return;
    }
    for q in 0..P {
        let o = &mut out[8 * q..8 * q + 8];
        for (level, v) in [ones[q], twos[q], fours[q], eights[q]].into_iter().enumerate() {
            positional(v, o, level as u32);
        }
        if !FLUSH {
            let (mut s1, mut s2, mut s4, mut s8) = (z, z, z, z);
            for pair in 0..blocks.len().div_ceil(2) {
                let (a, b) = (sixteens[2 * pair][q], if 2 * pair + 1 < blocks.len() { sixteens[2 * pair + 1][q] } else { z });
                let (t2, o) = csa(s1, a, b);
                s1 = o;
                let (t4, t) = csa(s2, t2, z);
                s2 = t;
                let (t8, f) = csa(s4, t4, z);
                s4 = f;
                s8 = _mm256_or_si256(s8, t8);
            }
            for (level, v) in [s1, s2, s4, s8].into_iter().enumerate() {
                positional(v, o, 4 + level as u32);
            }
        }
    }
}

#[target_feature(enable = "avx2,popcnt")]
unsafe fn c16_flush(d: &[u8]) -> [u32; 16] { let mut o = [0; 16]; hs::<2, true, false>(d, k(), |x, k| unsafe { p16(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c16_l2(d: &[u8]) -> [u32; 16] { let mut o = [0; 16]; hs::<2, false, false>(d, k(), |x, k| unsafe { p16(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c32_l2(d: &[u8]) -> [u32; 32] { let mut o = [0; 32]; hs::<4, false, false>(d, k(), |x, k| unsafe { p32(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c32z_l2(d: &[u8]) -> [u32; 32] { let mut o = [0; 32]; hs::<4, false, false>(d, k(), |x, k| unsafe { p32z(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c16_tr(d: &[u8]) -> [u32; 16] { let mut o = [0; 16]; hs::<2, false, true>(d, k(), |x, k| unsafe { p16(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c32_tr(d: &[u8]) -> [u32; 32] { let mut o = [0; 32]; hs::<4, false, true>(d, k(), |x, k| unsafe { p32(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c32z_tr(d: &[u8]) -> [u32; 32] { let mut o = [0; 32]; hs::<4, false, true>(d, k(), |x, k| unsafe { p32z(x, k) }, &mut o); o }
#[target_feature(enable = "avx2,popcnt")]
unsafe fn c32_flush(d: &[u8]) -> [u32; 32] { let mut o = [0; 32]; hs::<4, true, false>(d, k(), |x, k| unsafe { p32(x, k) }, &mut o); o }

fn s16(d: &[u8]) -> [u32; 16] { let mut c = [0; 16]; d.iter().for_each(|&x| c[((x >> 3) & 15) as usize] += 1); c }
fn s32(d: &[u8]) -> [u32; 32] { let mut c = [0; 32]; d.iter().for_each(|&x| c[(x >> 3) as usize] += 1); c }

/// Minimum H(c32(page)) given exact c32 prefix counts and c16 suffix counts.
fn lowest(w: &[f64; 4097], c32: &[u32; 32], c16: &[u32; 16], n: usize) -> f64 {
    let mut sum = 0.0;
    for g in 0..16 {
        let (a, b) = (c32[g], c32[g + 16]);
        sum += w[(a.max(b) + c16[g]) as usize] + w[a.min(b) as usize];
    }
    (w[n] - sum) / n as f64
}

#[target_feature(enable = "avx2,popcnt")]
unsafe fn hybrid(w: &[f64; 4097], d: &[u8], m: usize, c32f: unsafe fn(&[u8]) -> [u32; 32],
                 c16f: unsafe fn(&[u8]) -> [u32; 16]) -> f64 {
    lowest(w, &c32f(&d[..m]), &c16f(&d[m..]), d.len())
}

fn measure(name: &str, mut f: impl FnMut()) -> f64 {
    for _ in 0..20000 { f(); }
    let mut s = Vec::new();
    for _ in 0..15 {
        let t = Instant::now();
        for _ in 0..20000 { f(); }
        s.push(t.elapsed().as_nanos() as f64 / 20000.0);
    }
    s.sort_by(f64::total_cmp);
    println!("{name:<30} median {:8.1} ns  min {:8.1}", s[7], s[0]);
    s[7]
}

/// Effective clock (1-cycle dependent add chain) and vector logic throughput.
fn machine() {
    let n: u64 = 200_000_000;
    let t = Instant::now();
    unsafe {
        std::arch::asm!("2:", "add {a}, 1", "add {a}, 1", "add {a}, 1", "add {a}, 1",
             "add {a}, 1", "add {a}, 1", "add {a}, 1", "add {a}, 1",
             "dec {n}", "jnz 2b", a = inout(reg) 0u64 => _, n = inout(reg) n / 8 => _);
    }
    let ghz = n as f64 / t.elapsed().as_nanos() as f64;
    let iters: u64 = 20_000_000;
    let t = Instant::now();
    unsafe {
        std::arch::asm!("2:",
             "vpxor ymm0, ymm0, ymm6", "vpand ymm1, ymm1, ymm6", "vpor ymm2, ymm2, ymm6",
             "vpxor ymm3, ymm3, ymm6", "vpand ymm4, ymm4, ymm6", "vpor ymm5, ymm5, ymm6",
             "vpxor ymm0, ymm0, ymm6", "vpand ymm1, ymm1, ymm6", "vpor ymm2, ymm2, ymm6",
             "vpxor ymm3, ymm3, ymm6", "vpand ymm4, ymm4, ymm6", "vpor ymm5, ymm5, ymm6",
             "dec {n}", "jnz 2b", n = inout(reg) iters => _,
             out("ymm0") _, out("ymm1") _, out("ymm2") _, out("ymm3") _, out("ymm4") _,
             out("ymm5") _, out("ymm6") _);
    }
    let ops = (iters * 12) as f64 / t.elapsed().as_nanos() as f64;
    println!("dependent add chain {ghz:.3} GHz, ymm logic ops {ops:.2}/ns");
}

fn main() {
    assert!(is_x86_feature_detected!("avx2") && is_x86_feature_detected!("popcnt"));
    machine();
    machine();
    const N: usize = 4096;
    let w: Box<[f64; 4097]> = Box::new(std::array::from_fn(|v| if v == 0 { 0.0 } else { v as f64 * (v as f64).log2() }));
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let random: Vec<u8> = (0..N).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x as u8 }).collect();
    let seq: Vec<u8> = (0..N).map(|i| i as u8).collect();
    let text: Vec<u8> = (0..N).map(|i| b"the quick brown fox jumps over the lazy dog "[(i * 7) % 44]).collect();
    let skew: Vec<u8> = (0..N).map(|i| if i % 3 == 0 { 255 } else { (i * 37 % 251) as u8 }).collect();
    for d in [&random, &seq, &text, &skew, &vec![0u8; N], &vec![255u8; N]] {
        for len in [512, 1536, 2560, 4096] {
            let s = &d[..len];
            unsafe {
                assert_eq!(c16_flush(s), s16(s));
                assert_eq!(c16_l2(s), s16(s));
                assert_eq!(c32_l2(s), s32(s));
                assert_eq!(c32z_l2(s), s32(s));
                assert_eq!(c32_flush(s), s32(s));
                assert_eq!(c16_tr(s), s16(s));
                assert_eq!(c32_tr(s), s32(s));
                assert_eq!(c32z_tr(s), s32(s));
            }
        }
    }
    println!("correctness ok");
    for m in [1536, 2048] {
        println!("random m={m} lowest {:.4}", unsafe { hybrid(&w, &random, m, c32_l2, c16_l2) });
    }
    let r = &random;
    measure("c16_flush/4096", || { black_box(unsafe { c16_flush(black_box(r)) }); });
    measure("c16_l2/4096", || { black_box(unsafe { c16_l2(black_box(r)) }); });
    measure("c32_l2/4096", || { black_box(unsafe { c32_l2(black_box(r)) }); });
    measure("c32z_l2/4096", || { black_box(unsafe { c32z_l2(black_box(r)) }); });
    measure("c32_flush/4096", || { black_box(unsafe { c32_flush(black_box(r)) }); });
    measure("c16_tr/4096", || { black_box(unsafe { c16_tr(black_box(r)) }); });
    measure("c32_tr/4096", || { black_box(unsafe { c32_tr(black_box(r)) }); });
    measure("c32z_tr/4096", || { black_box(unsafe { c32z_tr(black_box(r)) }); });
    measure("c16_tr/2560", || { black_box(unsafe { c16_tr(black_box(&r[1536..])) }); });
    measure("c32_tr/1536", || { black_box(unsafe { c32_tr(black_box(&r[..1536])) }); });
    measure("c32z_tr/1536", || { black_box(unsafe { c32z_tr(black_box(&r[..1536])) }); });
    for m in [1536, 2048] {
        measure(&format!("hybrid m={m} (tr/tr)"), || { black_box(unsafe { hybrid(&w, black_box(r), m, c32_tr, c16_tr) }); });
        measure(&format!("hybrid m={m} (ztr/tr)"), || { black_box(unsafe { hybrid(&w, black_box(r), m, c32z_tr, c16_tr) }); });
        measure(&format!("hybrid m={m} (l2/l2)"), || { black_box(unsafe { hybrid(&w, black_box(r), m, c32_l2, c16_l2) }); });
        measure(&format!("hybrid m={m} (l2/flush)"), || { black_box(unsafe { hybrid(&w, black_box(r), m, c32_l2, c16_flush) }); });
    }
}
