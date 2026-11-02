#![allow(dead_code, unsafe_op_in_unsafe_fn)]
#[path = "mod.rs"]
mod entropy;
use std::arch::aarch64::*;
use std::hint::black_box;
use std::time::Instant;

fn measure(name: &str, mut f: impl FnMut()) -> f64 {
    for _ in 0..20000 {
        f();
    }
    let mut s = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        for _ in 0..50000 {
            f();
        }
        s.push(t.elapsed().as_nanos() as f64 / 50000.0);
    }
    s.sort_by(f64::total_cmp);
    println!("{name:<34} median {:8.2} ns  min {:8.2}", s[4], s[0]);
    s[4]
}

/// 8x8 bit transpose per lane: x[r] becomes bit-plane r of the 128 bytes.
#[inline(always)]
unsafe fn planes(p: *const u8) -> [uint8x16_t; 8] {
    let a = vld1q_u8_x4(p);
    let b = vld1q_u8_x4(p.add(64));
    let mut x = [a.0, a.1, a.2, a.3, b.0, b.1, b.2, b.3];
    for v in 0..4 {
        let (a, b) = (x[v], x[v + 4]);
        x[v] = vsliq_n_u8::<4>(a, b);
        x[v + 4] = vsriq_n_u8::<4>(b, a);
    }
    let m2 = vdupq_n_u8(0xCC);
    for v in [0, 1, 4, 5] {
        let (a, b) = (x[v], x[v + 2]);
        x[v] = vbslq_u8(m2, vshlq_n_u8::<2>(b), a);
        x[v + 2] = vbslq_u8(m2, b, vshrq_n_u8::<2>(a));
    }
    let m1 = vdupq_n_u8(0xAA);
    for v in [0, 2, 4, 6] {
        let (a, b) = (x[v], x[v + 1]);
        x[v] = vbslq_u8(m1, vshlq_n_u8::<1>(b), a);
        x[v + 1] = vbslq_u8(m1, b, vshrq_n_u8::<1>(a));
    }
    x
}

#[inline(always)]
unsafe fn dec2(p0: uint8x16_t, p1: uint8x16_t) -> [uint8x16_t; 4] {
    [
        vmvnq_u8(vorrq_u8(p0, p1)),
        vbicq_u8(p0, p1),
        vbicq_u8(p1, p0),
        vandq_u8(p0, p1),
    ]
}

/// 16 counts (lanes l) for hi nibble h from the 2-bit decodes.
#[inline(always)]
unsafe fn count_h(
    hm: uint8x16_t,
    d: &[uint8x16_t; 4],
    e: &[uint8x16_t; 4],
) -> uint8x16_t {
    let mut c = [vdupq_n_u8(0); 16];
    for j in 0..4 {
        let he = vandq_u8(hm, e[j]);
        for i in 0..4 {
            c[4 * j + i] = vcntq_u8(vandq_u8(he, d[i]));
        }
    }
    let mut r8 = [vdupq_n_u8(0); 8];
    for k in 0..8 {
        r8[k] = vpaddq_u8(c[2 * k], c[2 * k + 1]);
    }
    let mut r4 = [vdupq_n_u8(0); 4];
    for k in 0..4 {
        r4[k] = vpaddq_u8(r8[2 * k], r8[2 * k + 1]);
    }
    let r2a = vpaddq_u8(r4[0], r4[1]);
    let r2b = vpaddq_u8(r4[2], r4[3]);
    vpaddq_u8(r2a, r2b)
}

#[inline(always)]
unsafe fn acc16(bank: *mut u16, v: uint8x16_t) {
    let lo = vld1q_u16(bank);
    let hi = vld1q_u16(bank.add(8));
    vst1q_u16(bank, vaddw_u8(lo, vget_low_u8(v)));
    vst1q_u16(bank.add(8), vaddw_high_u8(hi, v));
}

#[inline(always)]
fn word(b: &[u8; 8]) -> u64 {
    let w = u64::from_ne_bytes(*b);
    unsafe {
        let mut w = w;
        std::arch::asm!("/* {w} */", w = inout(reg) w, options(pure, nomem, nostack, preserves_flags));
        w
    }
}

#[inline(always)]
fn scalar8(w: u64, c: &mut [[u16; 256]; 5]) {
    c[0][(w & 255) as usize] += 1;
    c[1][((w >> 8) & 255) as usize] += 1;
    c[2][((w >> 16) & 255) as usize] += 1;
    c[3][((w >> 24) & 255) as usize] += 1;
    c[0][((w >> 32) & 255) as usize] += 1;
    c[1][((w >> 40) & 255) as usize] += 1;
    c[2][((w >> 48) & 255) as usize] += 1;
    c[3][(w >> 56) as usize] += 1;
}

/// Pure NEON bit-plane histogram (for throughput reference).
#[target_feature(enable = "neon")]
unsafe fn neon_only(data: &[u8], hist: &mut [usize; 256]) {
    let mut c = [[0u16; 256]; 5];
    let (blocks, rem) = data.as_chunks::<128>();
    for blk in blocks {
        let p = planes(blk.as_ptr());
        let d = dec2(p[0], p[1]);
        let e = dec2(p[2], p[3]);
        let f = dec2(p[4], p[5]);
        let g = dec2(p[6], p[7]);
        for h in 0..16 {
            let hm = vandq_u8(f[h & 3], g[h >> 2]);
            acc16(c[4].as_mut_ptr().add(16 * h), count_h(hm, &d, &e));
        }
    }
    for i in 0..256 {
        hist[i] += c[4][i] as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

/// Hybrid: per superblock, NEON counts `NB` 128-byte blocks while scalar
/// counts SW words, interleaved per hi nibble.
#[target_feature(enable = "neon")]
unsafe fn hybrid<const SUPER: usize, const NB: usize>(data: &[u8], hist: &mut [usize; 256]) {
    let mut c = [[0u16; 256]; 5];
    let (sbs, rem) = data.as_chunks::<SUPER>();
    for sb in sbs {
        let (nb, sc) = sb.split_at(128 * NB);
        let words = sc.as_chunks::<8>().0;
        let per_h = words.len() / 16;
        for b in 0..NB {
            let p = planes(nb.as_ptr().add(128 * b));
            let d = dec2(p[0], p[1]);
            let e = dec2(p[2], p[3]);
            let f = dec2(p[4], p[5]);
            let g = dec2(p[6], p[7]);
            for h in 0..16 {
                let hm = vandq_u8(f[h & 3], g[h >> 2]);
                acc16(c[4].as_mut_ptr().add(16 * h), count_h(hm, &d, &e));
                let base = (b * 16 + h) * per_h / NB;
                for w in &words[base..base + per_h / NB] {
                    scalar8(word(w), &mut c);
                }
            }
        }
        for w in &words[per_h * 16..] {
            scalar8(word(w), &mut c);
        }
    }
    let (rw, rem) = rem.as_chunks::<8>();
    for w in rw {
        scalar8(word(w), &mut c);
    }
    for i in 0..256 {
        hist[i] += (c[0][i] + c[1][i] + c[2][i] + c[3][i] + c[4][i]) as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

fn banked_core(data: &[u8], hist: &mut [usize; 256]) {
    let mut c = [[0u16; 256]; 5];
    let (chunks, rem) = data.as_chunks::<8>();
    for b in chunks {
        scalar8(word(b), &mut c);
    }
    for i in 0..256 {
        hist[i] += (c[0][i] + c[1][i] + c[2][i] + c[3][i]) as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

fn main() {
    let mut st = 0x9e3779b97f4a7c15_u64;
    let random: Vec<u8> = (0..4096)
        .map(|_| {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            st as u8
        })
        .collect();
    let ascii: Vec<u8> = b"libcram memory tier orchestration "
        .iter()
        .copied()
        .cycle()
        .take(4096)
        .collect();
    let zero = vec![0u8; 4096];
    let variants: [(&str, fn(&[u8], &mut [usize; 256])); 7] = [
        ("banked4", banked_core),
        ("neon_only", |d, h| unsafe { neon_only(d, h) }),
        ("hyb 512/1 (25% neon)", |d, h| unsafe { hybrid::<512, 1>(d, h) }),
        ("hyb 640/1 (20% neon)", |d, h| unsafe { hybrid::<640, 1>(d, h) }),
        ("hyb 384/1 (33% neon)", |d, h| unsafe { hybrid::<384, 1>(d, h) }),
        ("hyb 1024/1 (12.5%)", |d, h| unsafe { hybrid::<1024, 1>(d, h) }),
        ("hyb 1024/2 (25%)", |d, h| unsafe { hybrid::<1024, 2>(d, h) }),
    ];
    for (dn, d) in [("random", &random), ("ascii", &ascii), ("zero", &zero)] {
        let mut want = [0usize; 256];
        for &b in d.iter() {
            want[b as usize] += 1;
        }
        for (vn, f) in variants {
            let mut got = [0usize; 256];
            f(d, &mut got);
            assert_eq!(got, want, "{vn} {dn}");
            measure(&format!("{dn}/{vn}"), || {
                let mut h = [0usize; 256];
                f(black_box(d), &mut h);
                black_box(&h);
            });
        }
    }
    // exhaustive-ish correctness of neon_only on varied data
    let mut s = 1u64;
    for len in [128usize, 256, 640, 1000, 4096, 4097] {
        for pat in 0..4 {
            let data: Vec<u8> = (0..len)
                .map(|i| match pat {
                    0 => {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        s as u8
                    }
                    1 => 255,
                    2 => (i % 256) as u8,
                    _ => (i * 7 / 3) as u8,
                })
                .collect();
            let mut want = [0usize; 256];
            for &b in &data {
                want[b as usize] += 1;
            }
            for (vn, f) in variants {
                let mut got = [0usize; 256];
                f(&data, &mut got);
                assert_eq!(got, want, "{vn} len={len} pat={pat}");
            }
        }
    }
    println!("correctness ok");
}
