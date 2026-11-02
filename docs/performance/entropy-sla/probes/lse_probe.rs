#![allow(dead_code)]
#[path = "mod.rs"]
mod entropy;
use std::hint::black_box;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering::Relaxed};
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
    println!("{name:<28} median {:8.2} ns  min {:8.2}", s[4], s[0]);
    s[4]
}

// ---- core histogram variants (hist must be additive) ----
fn banked_core(data: &[u8], hist: &mut [usize; 256]) {
    let mut c = [[0u16; 256]; 4];
    let [h0, h1, h2, h3] = &mut c;
    let (chunks, rem) = data.as_chunks::<8>();
    for b in chunks {
        let w = u64::from_ne_bytes(*b);
        let w = unsafe {
            let mut w = w;
            std::arch::asm!("/* {w} */", w = inout(reg) w, options(pure, nomem, nostack, preserves_flags));
            w
        };
        h0[(w & 255) as usize] += 1;
        h1[((w >> 8) & 255) as usize] += 1;
        h2[((w >> 16) & 255) as usize] += 1;
        h3[((w >> 24) & 255) as usize] += 1;
        h0[((w >> 32) & 255) as usize] += 1;
        h1[((w >> 40) & 255) as usize] += 1;
        h2[((w >> 48) & 255) as usize] += 1;
        h3[(w >> 56) as usize] += 1;
    }
    for i in 0..256 {
        hist[i] += (h0[i] + h1[i] + h2[i] + h3[i]) as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

#[target_feature(enable = "lse")]
fn lse16_1(data: &[u8], hist: &mut [usize; 256]) {
    let c: [AtomicU16; 256] = [const { AtomicU16::new(0) }; 256];
    let (chunks, rem) = data.as_chunks::<8>();
    for b in chunks {
        let w = u64::from_ne_bytes(*b);
        for k in 0..8 {
            c[((w >> (8 * k)) & 255) as usize].fetch_add(1, Relaxed);
        }
    }
    for i in 0..256 {
        hist[i] += c[i].load(Relaxed) as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

#[target_feature(enable = "lse")]
fn lse32_1(data: &[u8], hist: &mut [usize; 256]) {
    let c: [AtomicU32; 256] = [const { AtomicU32::new(0) }; 256];
    let (chunks, rem) = data.as_chunks::<8>();
    for b in chunks {
        let w = u64::from_ne_bytes(*b);
        for k in 0..8 {
            c[((w >> (8 * k)) & 255) as usize].fetch_add(1, Relaxed);
        }
    }
    for i in 0..256 {
        hist[i] += c[i].load(Relaxed) as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

#[target_feature(enable = "lse")]
fn lse16_2(data: &[u8], hist: &mut [usize; 256]) {
    let c: [[AtomicU16; 256]; 2] = [const { [const { AtomicU16::new(0) }; 256] }; 2];
    let (chunks, rem) = data.as_chunks::<8>();
    for b in chunks {
        let w = u64::from_ne_bytes(*b);
        for k in 0..8 {
            c[k & 1][((w >> (8 * k)) & 255) as usize].fetch_add(1, Relaxed);
        }
    }
    for i in 0..256 {
        hist[i] += (c[0][i].load(Relaxed) + c[1][i].load(Relaxed)) as usize;
    }
    for &b in rem {
        hist[b as usize] += 1;
    }
}

// mixed: half bytes via LSE, half via plain RMW (balance LSU vs frontend)
#[target_feature(enable = "lse")]
fn mixed(data: &[u8], hist: &mut [usize; 256]) {
    let a: [AtomicU16; 256] = [const { AtomicU16::new(0) }; 256];
    let mut p = [[0u16; 256]; 2];
    let (chunks, rem) = data.as_chunks::<8>();
    for b in chunks {
        let w = u64::from_ne_bytes(*b);
        let w = unsafe {
            let mut w = w;
            std::arch::asm!("/* {w} */", w = inout(reg) w, options(pure, nomem, nostack, preserves_flags));
            w
        };
        a[(w & 255) as usize].fetch_add(1, Relaxed);
        p[0][((w >> 8) & 255) as usize] += 1;
        a[((w >> 16) & 255) as usize].fetch_add(1, Relaxed);
        p[1][((w >> 24) & 255) as usize] += 1;
        a[((w >> 32) & 255) as usize].fetch_add(1, Relaxed);
        p[0][((w >> 40) & 255) as usize] += 1;
        a[((w >> 48) & 255) as usize].fetch_add(1, Relaxed);
        p[1][(w >> 56) as usize] += 1;
    }
    for i in 0..256 {
        hist[i] += (a[i].load(Relaxed) + p[0][i] + p[1][i]) as usize;
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
    let seq: Vec<u8> = (0..4096).map(|i| i as u8).collect();
    let zero = vec![0u8; 4096];
    let variants: [(&str, fn(&[u8], &mut [usize; 256])); 5] = [
        ("banked4_u16", banked_core),
        ("lse16_1", |d, h| unsafe { lse16_1(d, h) }),
        ("lse32_1", |d, h| unsafe { lse32_1(d, h) }),
        ("lse16_2", |d, h| unsafe { lse16_2(d, h) }),
        ("mixed", |d, h| unsafe { mixed(d, h) }),
    ];
    for (dn, d) in [("random", &random), ("ascii", &ascii), ("seq", &seq), ("zero", &zero)] {
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
        measure(&format!("{dn}/PROD_total"), || {
            black_box(entropy::shannon_entropy(black_box(d)));
        });
    }
}
