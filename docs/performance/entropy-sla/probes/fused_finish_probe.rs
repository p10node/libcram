#![allow(dead_code, unsafe_op_in_unsafe_fn)]
#[path = "mod.rs"]
mod entropy;
use std::arch::aarch64::*;
use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Instant;
fn measure(name: &str, mut f: impl FnMut()) -> f64 {
    for _ in 0..20000 { f(); }
    let mut s = Vec::new();
    for _ in 0..15 { let t = Instant::now(); for _ in 0..50000 { f(); } s.push(t.elapsed().as_nanos() as f64 / 50000.0); }
    s.sort_by(f64::total_cmp);
    println!("{name:<30} median {:8.2} ns  min {:8.2}", s[7], s[0]);
    s[7]
}
fn weights() -> &'static [f64] {
    static W: OnceLock<Vec<f64>> = OnceLock::new();
    W.get_or_init(|| (0..=4096).map(|v| if v == 0 { 0.0 } else { v as f64 * (v as f64).log2() }).collect())
}
#[inline(always)]
fn word(b: &[u8; 8]) -> u64 { let mut w = u64::from_ne_bytes(*b); unsafe { std::arch::asm!("/* {w} */", w = inout(reg) w, options(pure, nomem, nostack, preserves_flags)); } w }
#[inline(always)]
fn banks(data: &[u8], c: &mut [[u16; 256]; 4]) {
    for b in data.as_chunks::<8>().0 {
        let w = word(b);
        c[0][(w & 255) as usize] += 1; c[1][((w >> 8) & 255) as usize] += 1;
        c[2][((w >> 16) & 255) as usize] += 1; c[3][((w >> 24) & 255) as usize] += 1;
        c[0][((w >> 32) & 255) as usize] += 1; c[1][((w >> 40) & 255) as usize] += 1;
        c[2][((w >> 48) & 255) as usize] += 1; c[3][(w >> 56) as usize] += 1;
    }
}
/// 8-chain finish over usize hist (reference association).
fn finish8(hist: &[usize; 256], n: usize) -> f64 {
    let w = weights();
    let mut s = [0.0f64; 8];
    for c in hist.as_chunks::<8>().0 { for k in 0..8 { s[k] += w[c[k]]; } }
    ((w[n] - (((s[0] + s[1]) + (s[2] + s[3])) + ((s[4] + s[5]) + (s[6] + s[7])))) / n as f64).max(0.0)
}
/// Fused: NEON-sum banks to u16x8, extract lanes, 8 chains (same association as finish8).
#[target_feature(enable = "neon")]
unsafe fn fused8(c: &[[u16; 256]; 4], n: usize) -> f64 {
    let w = weights();
    let mut s = [0.0f64; 8];
    for g in 0..32 {
        let v = vaddq_u16(vaddq_u16(vld1q_u16(c[0].as_ptr().add(8 * g)), vld1q_u16(c[1].as_ptr().add(8 * g))),
                          vaddq_u16(vld1q_u16(c[2].as_ptr().add(8 * g)), vld1q_u16(c[3].as_ptr().add(8 * g))));
        s[0] += w[vgetq_lane_u16::<0>(v) as usize]; s[1] += w[vgetq_lane_u16::<1>(v) as usize];
        s[2] += w[vgetq_lane_u16::<2>(v) as usize]; s[3] += w[vgetq_lane_u16::<3>(v) as usize];
        s[4] += w[vgetq_lane_u16::<4>(v) as usize]; s[5] += w[vgetq_lane_u16::<5>(v) as usize];
        s[6] += w[vgetq_lane_u16::<6>(v) as usize]; s[7] += w[vgetq_lane_u16::<7>(v) as usize];
    }
    ((w[n] - (((s[0] + s[1]) + (s[2] + s[3])) + ((s[4] + s[5]) + (s[6] + s[7])))) / n as f64).max(0.0)
}
/// Fused, 4 chains (current production association).
#[target_feature(enable = "neon")]
unsafe fn fused4(c: &[[u16; 256]; 4], n: usize) -> f64 {
    let w = weights();
    let mut s = [0.0f64; 4];
    for g in 0..32 {
        let v = vaddq_u16(vaddq_u16(vld1q_u16(c[0].as_ptr().add(8 * g)), vld1q_u16(c[1].as_ptr().add(8 * g))),
                          vaddq_u16(vld1q_u16(c[2].as_ptr().add(8 * g)), vld1q_u16(c[3].as_ptr().add(8 * g))));
        s[0] += w[vgetq_lane_u16::<0>(v) as usize]; s[1] += w[vgetq_lane_u16::<1>(v) as usize];
        s[2] += w[vgetq_lane_u16::<2>(v) as usize]; s[3] += w[vgetq_lane_u16::<3>(v) as usize];
        s[0] += w[vgetq_lane_u16::<4>(v) as usize]; s[1] += w[vgetq_lane_u16::<5>(v) as usize];
        s[2] += w[vgetq_lane_u16::<6>(v) as usize]; s[3] += w[vgetq_lane_u16::<7>(v) as usize];
    }
    ((w[n] - ((s[0] + s[1]) + (s[2] + s[3]))) / n as f64).max(0.0)
}
/// Portable fused: same association as production finish.
#[inline(never)]
fn fused_portable(c: &[[u16; 256]; 4], n: usize) -> f64 {
    let w = weights();
    let mut s = [0.0f64; 4];
    let (b0, b1, b2, b3) = (c[0].as_chunks::<8>().0, c[1].as_chunks::<8>().0, c[2].as_chunks::<8>().0, c[3].as_chunks::<8>().0);
    for g in 0..32 {
        let mut v = [0u16; 8];
        for k in 0..8 { v[k] = b0[g][k] + b1[g][k] + b2[g][k] + b3[g][k]; }
        for k in 0..8 { s[k & 3] += w[v[k] as usize]; }
    }
    ((w[n] - ((s[0] + s[1]) + (s[2] + s[3]))) / n as f64).max(0.0)
}
fn main() {
    let mut st = 0x9e3779b97f4a7c15_u64;
    let r: Vec<u8> = (0..4096).map(|_| { st ^= st << 13; st ^= st >> 7; st ^= st << 17; st as u8 }).collect();
    let mut hist = [0usize; 256]; for &b in &r { hist[b as usize] += 1; }
    let mut c = [[0u16; 256]; 4]; banks(&r, &mut c);
    assert_eq!(finish8(&hist, 4096), unsafe { fused8(&c, 4096) });
    assert_eq!(entropy::scalar_entropy(&r), unsafe { fused4(&c, 4096) });
    assert_eq!(entropy::scalar_entropy(&r), fused_portable(&c, 4096));
    println!("prod {:.17} f8 {:.17}", entropy::scalar_entropy(&r), finish8(&hist, 4096));
    for _ in 0..2 {
    measure("PROD total", || { black_box(entropy::shannon_entropy(black_box(&r))); });
    measure("banks+merge+finish8(usize)", || { let d = black_box(&r); let mut c = [[0u16; 256]; 4]; banks(d, &mut c);
        let mut h = [0usize; 256]; for i in 0..256 { h[i] += (c[0][i] + c[1][i] + c[2][i] + c[3][i]) as usize; } black_box(finish8(&h, d.len())); });
    measure("banks+fused4", || { let d = black_box(&r); let mut c = [[0u16; 256]; 4]; banks(d, &mut c); black_box(unsafe { fused4(&c, d.len()) }); });
    measure("banks+fused8", || { let d = black_box(&r); let mut c = [[0u16; 256]; 4]; banks(d, &mut c); black_box(unsafe { fused8(&c, d.len()) }); });
    measure("banks+fused_portable", || { let d = black_box(&r); let mut c = [[0u16; 256]; 4]; banks(d, &mut c); black_box(fused_portable(&c, d.len())); });
    measure("banks only", || { let d = black_box(&r); let mut c = [[0u16; 256]; 4]; banks(d, &mut c); black_box(&c); });
    }
}
