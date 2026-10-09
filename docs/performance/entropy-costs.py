"""Disposable native AArch64 component profiler, not the Criterion acceptance gate.

Run from any directory using Python 3. Requires rustc in PATH. All generated Rust
and executables live in a temporary directory and are removed afterward.
"""
from pathlib import Path
import hashlib
import platform
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "crates/libcram-core/src/entropy/mod.rs"
WRAPPERS = r'''
pub fn histogram_cost(data: &[u8]) {
    assert!(std::arch::is_aarch64_feature_detected!("neon"));
    let mut hist = [0; 256];
    // SAFETY: runtime detection verifies NEON; the kernel bounds all loads.
    unsafe { neon(data, &mut hist); }
    std::hint::black_box(&hist);
}
pub fn finish_cost(hist: &[usize; 256], n: usize) -> f64 { finish(hist, n) }
'''
PROBE = r'''
#[path="mod.rs"] mod entropy;
use std::{hint::black_box, time::Instant};
fn measure(name: &str, mut f: impl FnMut()) {
    for _ in 0..10000 { f(); }
    let mut samples = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        for _ in 0..100000 { f(); }
        samples.push(start.elapsed().as_nanos() as f64 / 100000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("{name}: median {:.2} ns", samples[4]);
}
fn main() {
    let mut state = 0x9e3779b97f4a7c15_u64;
    let random: Vec<u8> = (0..4096).map(|_| {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        state as u8
    }).collect();
    let ascii: Vec<u8> = b"libcram memory tier orchestration ".iter()
        .copied().cycle().take(4096).collect();
    for (name, data) in [("zero", vec![0; 4096]), ("ascii", ascii), ("random", random)] {
        assert_eq!(entropy::shannon_entropy(&data), entropy::scalar_entropy(&data));
        let mut hist = [0; 256];
        for byte in &data { hist[*byte as usize] += 1; }
        measure(&format!("{name}/histogram"), || entropy::histogram_cost(black_box(&data)));
        measure(&format!("{name}/finish"), || {
            black_box(entropy::finish_cost(black_box(&hist), black_box(data.len())));
        });
        measure(&format!("{name}/total"), || {
            black_box(entropy::shannon_entropy(black_box(&data)));
        });
    }
}
'''


def main():
    if platform.machine().lower() not in {"aarch64", "arm64"}:
        raise SystemExit("This diagnostic profiles the AArch64 NEON kernel only.")
    source = SOURCE.read_bytes()
    print("source_sha256:", hashlib.sha256(source).hexdigest(), flush=True)
    subprocess.run(["rustc", "--version"], check=True)
    print("Diagnostic medians; not Criterion upper confidence bounds.", flush=True)
    with tempfile.TemporaryDirectory(prefix="hermes-verify-entropy-") as directory:
        work = Path(directory)
        (work / "mod.rs").write_text(source.decode() + WRAPPERS)
        (work / "probe.rs").write_text(PROBE)
        binary = work / "probe"
        subprocess.run(["rustc", "--edition=2021", "-O", str(work / "probe.rs"),
                        "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)
    print("Temporary sources and executable removed.", flush=True)


if __name__ == "__main__":
    main()
