use std::hint::black_box;
use std::time::Instant;
fn measure(name: &str, mut f: impl FnMut()) {
    for _ in 0..20000 { f(); }
    let mut s = Vec::new();
    for _ in 0..9 { let t = Instant::now(); for _ in 0..50000 { f(); } s.push(t.elapsed().as_nanos() as f64 / 50000.0); }
    s.sort_by(f64::total_cmp);
    println!("{name:<26} median {:8.2} ns  min {:8.2}", s[4], s[0]);
}
#[inline(always)]
fn word(b: &[u8; 8]) -> u64 { let mut w = u64::from_ne_bytes(*b); unsafe { std::arch::asm!("/* {w} */", w = inout(reg) w, options(pure, nomem, nostack, preserves_flags)); } w }
fn stores_only(d: &[u8], t: &mut [[u16; 256]; 4]) { for b in d.as_chunks::<8>().0 { let w = word(b); for k in 0..8 { t[k & 3][((w >> (8 * k)) & 255) as usize] = k as u16; } } }
fn loads_only(d: &[u8], t: &[[u16; 256]; 4]) -> u64 { let mut s = [0u64; 4]; for b in d.as_chunks::<8>().0 { let w = word(b); for k in 0..8 { s[k & 3] += t[k & 3][((w >> (8 * k)) & 255) as usize] as u64; } } s.iter().sum() }
fn rmw(d: &[u8], t: &mut [[u16; 256]; 4]) { for b in d.as_chunks::<8>().0 { let w = word(b); for k in 0..8 { t[k & 3][((w >> (8 * k)) & 255) as usize] += 1; } } }
fn stream(d: &[u8]) -> u64 { let mut s = [0u64; 4]; for b in d.as_chunks::<8>().0 { let w = word(b); for k in 0..8 { s[k & 3] += (w >> (8 * k)) & 255; } } s.iter().sum() }
fn main() {
    let mut st = 0x9e3779b97f4a7c15_u64;
    let r: Vec<u8> = (0..4096).map(|_| { st ^= st << 13; st ^= st >> 7; st ^= st << 17; st as u8 }).collect();
    let mut t = [[0u16; 256]; 4];
    measure("extract+add (no table)", || { black_box(stream(black_box(&r))); });
    measure("scatter loads only", || { black_box(loads_only(black_box(&r), black_box(&t))); });
    measure("scatter stores only", || { stores_only(black_box(&r), &mut t); black_box(&t); });
    measure("scatter RMW (+1)", || { rmw(black_box(&r), &mut t); black_box(&t); });
}
