//! x86_64 diagnostic: which 4-bank u16 counting loop is cheapest on the s5 VM?
//! Build: rustc --edition 2021 -C opt-level=3 -C codegen-units=1 x86_probe.rs
//! Run:   taskset -c 0 ./x86_probe
//! Each variant: zero banks, count 4096 bytes, merge into [usize; 256] (as in
//! production banked_hist, minus the period scout and finish). Exactness is
//! checked against a scalar histogram before any timing. A `w >>= 16`
//! extraction variant was dropped: LLVM emits code identical to `prod`.
#![cfg(target_arch = "x86_64")]
use std::arch::asm;
use std::hint::black_box;
use std::time::Instant;

#[repr(C, align(64))]
struct Banks([[u16; 256]; 4]);
type Count = fn(&[u8], &mut Banks);

fn prod(data: &[u8], b: &mut Banks) {
    let [h0, h1, h2, h3] = &mut b.0;
    for bytes in data.as_chunks::<8>().0 {
        let word = u64::from_ne_bytes(*bytes);
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

fn bytes(data: &[u8], b: &mut Banks) {
    let [h0, h1, h2, h3] = &mut b.0;
    for c in data.as_chunks::<8>().0 {
        h0[c[0] as usize] += 1;
        h1[c[1] as usize] += 1;
        h2[c[2] as usize] += 1;
        h3[c[3] as usize] += 1;
        h0[c[4] as usize] += 1;
        h1[c[5] as usize] += 1;
        h2[c[6] as usize] += 1;
        h3[c[7] as usize] += 1;
    }
}

// The asm variants extract bytes with al/ah + `shr rax, 16` and increment
// bank k&3 (byte offsets 0/512/1024/1536 in the contiguous Banks).

/// Indexed `inc word ptr [base + idx*2 + off]` (what LLVM emits for prod).
#[inline(always)]
fn asm_idx(data: &[u8], b: &mut Banks) {
    let base = b.0.as_mut_ptr().cast::<u8>();
    for c in data.as_chunks::<8>().0 {
        // SAFETY: reads 8 bytes of `c`; writes stay inside the 2048-byte Banks.
        unsafe {
            asm!(
                "mov rax, qword ptr [{src}]",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah", "shr rax, 16",
                "inc word ptr [{base} + rcx*2]",
                "inc word ptr [{base} + rdx*2 + 512]",
                "inc word ptr [{base} + rsi*2 + 1024]",
                "inc word ptr [{base} + rdi*2 + 1536]",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah",
                "inc word ptr [{base} + rcx*2]",
                "inc word ptr [{base} + rdx*2 + 512]",
                "inc word ptr [{base} + rsi*2 + 1024]",
                "inc word ptr [{base} + rdi*2 + 1536]",
                src = in(reg) c.as_ptr(), base = in(reg) base,
                out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
                options(nostack),
            );
        }
    }
}

/// One 8-byte word, al/ah + `shr rax, 16` extraction, one indexed `inc` per byte
/// through four bank base registers (no 32-bit displacement in the encoding).
macro_rules! word4b {
    ($src:expr, $b:expr) => {
        asm!(
            "mov rax, qword ptr [{src}]",
            "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
            "movzx esi, al", "movzx edi, ah", "shr rax, 16",
            "inc word ptr [{b0} + rcx*2]",
            "inc word ptr [{b1} + rdx*2]",
            "inc word ptr [{b2} + rsi*2]",
            "inc word ptr [{b3} + rdi*2]",
            "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
            "movzx esi, al", "movzx edi, ah",
            "inc word ptr [{b0} + rcx*2]",
            "inc word ptr [{b1} + rdx*2]",
            "inc word ptr [{b2} + rsi*2]",
            "inc word ptr [{b3} + rdi*2]",
            src = in(reg) $src, b0 = in(reg) $b[0], b1 = in(reg) $b[1],
            b2 = in(reg) $b[2], b3 = in(reg) $b[3],
            out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
            options(nostack),
        )
    };
}

fn bank_ptrs(b: &mut Banks) -> [*mut u16; 4] {
    let base = b.0.as_mut_ptr().cast::<u16>();
    // SAFETY: each bank is 256 u16 inside the same allocation.
    unsafe { [base, base.add(256), base.add(512), base.add(768)] }
}

fn asm_4b(data: &[u8], b: &mut Banks) {
    let p = bank_ptrs(b);
    for c in data.as_chunks::<8>().0 {
        // SAFETY: as asm_idx.
        unsafe { word4b!(c.as_ptr(), p) };
    }
}

fn asm_4b_x4(data: &[u8], b: &mut Banks) {
    let p = bank_ptrs(b);
    for c in data.as_chunks::<32>().0 {
        let s = c.as_ptr();
        // SAFETY: as asm_idx; offsets 0..24 stay inside the 32-byte chunk.
        unsafe {
            word4b!(s, p);
            word4b!(s.add(8), p);
            word4b!(s.add(16), p);
            word4b!(s.add(24), p);
        }
    }
}

fn asm_idx_x4(data: &[u8], b: &mut Banks) {
    for c in data.as_chunks::<32>().0 {
        asm_idx(c, b);
    }
}

/// `lea p, [base + idx*2]` then non-indexed `inc word ptr [p + off]`
/// (store address can use port 7 on Haswell/Broadwell).
fn asm_lea(data: &[u8], b: &mut Banks) {
    let base = b.0.as_mut_ptr().cast::<u8>();
    for c in data.as_chunks::<8>().0 {
        // SAFETY: as asm_idx.
        unsafe {
            asm!(
                "mov rax, qword ptr [{src}]",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah", "shr rax, 16",
                "lea {p}, [{base} + rcx*2]", "inc word ptr [{p}]",
                "lea {q}, [{base} + rdx*2]", "inc word ptr [{q} + 512]",
                "lea {p}, [{base} + rsi*2]", "inc word ptr [{p} + 1024]",
                "lea {q}, [{base} + rdi*2]", "inc word ptr [{q} + 1536]",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah",
                "lea {p}, [{base} + rcx*2]", "inc word ptr [{p}]",
                "lea {q}, [{base} + rdx*2]", "inc word ptr [{q} + 512]",
                "lea {p}, [{base} + rsi*2]", "inc word ptr [{p} + 1024]",
                "lea {q}, [{base} + rdi*2]", "inc word ptr [{q} + 1536]",
                src = in(reg) c.as_ptr(), base = in(reg) base,
                p = out(reg) _, q = out(reg) _,
                out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
                options(nostack),
            );
        }
    }
}

/// Split RMW: indexed `movzx` load, register `inc`, indexed `mov` store.
fn asm_split(data: &[u8], b: &mut Banks) {
    let base = b.0.as_mut_ptr().cast::<u8>();
    for c in data.as_chunks::<8>().0 {
        // SAFETY: as asm_idx.
        unsafe {
            asm!(
                "mov rax, qword ptr [{src}]",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah", "shr rax, 16",
                "movzx {t:e}, word ptr [{base} + rcx*2]", "inc {t:e}", "mov word ptr [{base} + rcx*2], {t:x}",
                "movzx {u:e}, word ptr [{base} + rdx*2 + 512]", "inc {u:e}", "mov word ptr [{base} + rdx*2 + 512], {u:x}",
                "movzx {t:e}, word ptr [{base} + rsi*2 + 1024]", "inc {t:e}", "mov word ptr [{base} + rsi*2 + 1024], {t:x}",
                "movzx {u:e}, word ptr [{base} + rdi*2 + 1536]", "inc {u:e}", "mov word ptr [{base} + rdi*2 + 1536], {u:x}",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah",
                "movzx {t:e}, word ptr [{base} + rcx*2]", "inc {t:e}", "mov word ptr [{base} + rcx*2], {t:x}",
                "movzx {u:e}, word ptr [{base} + rdx*2 + 512]", "inc {u:e}", "mov word ptr [{base} + rdx*2 + 512], {u:x}",
                "movzx {t:e}, word ptr [{base} + rsi*2 + 1024]", "inc {t:e}", "mov word ptr [{base} + rsi*2 + 1024], {t:x}",
                "movzx {u:e}, word ptr [{base} + rdi*2 + 1536]", "inc {u:e}", "mov word ptr [{base} + rdi*2 + 1536], {u:x}",
                src = in(reg) c.as_ptr(), base = in(reg) base,
                t = out(reg) _, u = out(reg) _,
                out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
                options(nostack),
            );
        }
    }
}

/// Split RMW with non-indexed load/store through a `lea` pointer.
fn asm_lea_split(data: &[u8], b: &mut Banks) {
    let base = b.0.as_mut_ptr().cast::<u8>();
    for c in data.as_chunks::<8>().0 {
        // SAFETY: as asm_idx.
        unsafe {
            asm!(
                "mov rax, qword ptr [{src}]",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah", "shr rax, 16",
                "lea {p}, [{base} + rcx*2]", "movzx {t:e}, word ptr [{p}]", "inc {t:e}", "mov word ptr [{p}], {t:x}",
                "lea {q}, [{base} + rdx*2]", "movzx {u:e}, word ptr [{q} + 512]", "inc {u:e}", "mov word ptr [{q} + 512], {u:x}",
                "lea {p}, [{base} + rsi*2]", "movzx {t:e}, word ptr [{p} + 1024]", "inc {t:e}", "mov word ptr [{p} + 1024], {t:x}",
                "lea {q}, [{base} + rdi*2]", "movzx {u:e}, word ptr [{q} + 1536]", "inc {u:e}", "mov word ptr [{q} + 1536], {u:x}",
                "movzx ecx, al", "movzx edx, ah", "shr rax, 16",
                "movzx esi, al", "movzx edi, ah",
                "lea {p}, [{base} + rcx*2]", "movzx {t:e}, word ptr [{p}]", "inc {t:e}", "mov word ptr [{p}], {t:x}",
                "lea {q}, [{base} + rdx*2]", "movzx {u:e}, word ptr [{q} + 512]", "inc {u:e}", "mov word ptr [{q} + 512], {u:x}",
                "lea {p}, [{base} + rsi*2]", "movzx {t:e}, word ptr [{p} + 1024]", "inc {t:e}", "mov word ptr [{p} + 1024], {t:x}",
                "lea {q}, [{base} + rdi*2]", "movzx {u:e}, word ptr [{q} + 1536]", "inc {u:e}", "mov word ptr [{q} + 1536], {u:x}",
                src = in(reg) c.as_ptr(), base = in(reg) base,
                p = out(reg) _, q = out(reg) _, t = out(reg) _, u = out(reg) _,
                out("rax") _, out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
                options(nostack),
            );
        }
    }
}

/// Byte loads (no shift extraction), `lea` + non-indexed `inc`.
fn asm_bytes_lea(data: &[u8], b: &mut Banks) {
    let base = b.0.as_mut_ptr().cast::<u8>();
    for c in data.as_chunks::<8>().0 {
        // SAFETY: as asm_idx.
        unsafe {
            asm!(
                "movzx ecx, byte ptr [{src}]", "movzx edx, byte ptr [{src} + 1]",
                "movzx esi, byte ptr [{src} + 2]", "movzx edi, byte ptr [{src} + 3]",
                "lea {p}, [{base} + rcx*2]", "inc word ptr [{p}]",
                "lea {q}, [{base} + rdx*2]", "inc word ptr [{q} + 512]",
                "lea {p}, [{base} + rsi*2]", "inc word ptr [{p} + 1024]",
                "lea {q}, [{base} + rdi*2]", "inc word ptr [{q} + 1536]",
                "movzx ecx, byte ptr [{src} + 4]", "movzx edx, byte ptr [{src} + 5]",
                "movzx esi, byte ptr [{src} + 6]", "movzx edi, byte ptr [{src} + 7]",
                "lea {p}, [{base} + rcx*2]", "inc word ptr [{p}]",
                "lea {q}, [{base} + rdx*2]", "inc word ptr [{q} + 512]",
                "lea {p}, [{base} + rsi*2]", "inc word ptr [{p} + 1024]",
                "lea {q}, [{base} + rdi*2]", "inc word ptr [{q} + 1536]",
                src = in(reg) c.as_ptr(), base = in(reg) base,
                p = out(reg) _, q = out(reg) _,
                out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
                options(nostack),
            );
        }
    }
}

/// Same-shape floor as benches/entropy.rs: one scattered u16 store per byte.
fn floor(data: &[u8], b: &mut Banks) {
    for bytes in data.as_chunks::<8>().0 {
        let word = u64::from_ne_bytes(*bytes);
        for lane in 0..8 {
            b.0[lane & 3][((word >> (8 * lane)) & 255) as usize] = lane as u16;
        }
    }
}

fn run(count: Count, data: &[u8], hist: &mut [usize; 256]) {
    let mut banks = Banks([[0u16; 256]; 4]);
    count(data, &mut banks);
    let [h0, h1, h2, h3] = &banks.0;
    for i in 0..256 {
        hist[i] += (h0[i] + h1[i] + h2[i] + h3[i]) as usize;
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    const N: usize = 4096;
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let random: Vec<u8> = (0..N).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x >> 56) as u8 }).collect();
    let seq: Vec<u8> = (0..N).map(|i| i as u8).collect();
    let text: Vec<u8> = (0..N).map(|i| b"the quick brown fox jumps over the lazy dog "[(i * 7) % 44]).collect();
    let zero = vec![0u8; N];

    let variants: &[(&str, Count)] = &[
        ("prod", prod), ("bytes", bytes), ("asm_idx", asm_idx), ("asm_idx_x4", asm_idx_x4),
        ("asm_4b", asm_4b), ("asm_4b_x4", asm_4b_x4),
        ("asm_lea", asm_lea), ("asm_split", asm_split), ("asm_lea_split", asm_lea_split),
        ("asm_bytes_lea", asm_bytes_lea),
    ];
    for data in [&random, &seq, &text, &zero] {
        let mut want = [0usize; 256];
        data.iter().for_each(|&b| want[b as usize] += 1);
        for (name, f) in variants {
            let mut got = [0usize; 256];
            run(*f, data, &mut got);
            assert_eq!(got, want, "{name}");
        }
    }
    println!("correctness ok");

    let all: Vec<(&str, Count, bool)> = variants.iter().map(|&(n, f)| (n, f, true))
        .chain([("floor(store only)", floor as Count, false)]).collect();
    for (label, data) in [("random", &random), ("seq", &seq)] {
        const ROUNDS: usize = 101;
        const ITERS: usize = 400;
        let mut samples = vec![Vec::with_capacity(ROUNDS); all.len()];
        let mut floor_banks = Banks([[0u16; 256]; 4]);
        for _ in 0..ROUNDS {
            for (k, (_, f, merged)) in all.iter().enumerate() {
                let t = Instant::now();
                for _ in 0..ITERS {
                    if *merged {
                        let mut hist = [0usize; 256];
                        run(*f, black_box(data), &mut hist);
                        black_box(&hist);
                    } else {
                        f(black_box(data), &mut floor_banks);
                        black_box(&mut floor_banks);
                    }
                }
                samples[k].push(t.elapsed().as_nanos() as f64 / ITERS as f64);
            }
        }
        let base = median(&mut samples[0].clone());
        for (k, (name, _, _)) in all.iter().enumerate() {
            let min = samples[k].iter().cloned().fold(f64::INFINITY, f64::min);
            let med = median(&mut samples[k]);
            println!("{label}/{name:<20} median {med:8.1} ns  min {min:8.1}  vs prod {:+6.1}%", 100.0 * (med / base - 1.0));
        }
    }
}
