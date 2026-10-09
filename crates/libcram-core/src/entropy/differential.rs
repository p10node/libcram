use super::*;
#[test]
fn random_unaligned_all_available_kernels() {
    let mut seed = 0x87162dff_u64;
    let mut data = vec![0u8; 17000];
    for byte in &mut data {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *byte = seed as u8;
    }
    let mut kernels: Vec<(&str, Kernel)> = vec![("scalar", scalar_hist)];
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("neon") {
        kernels.push(("neon", neon));
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            kernels.push(("avx2", avx2));
        }
        if std::is_x86_feature_detected!("avx512bw") {
            kernels.push(("avx512", avx512));
        }
    }
    for (name, kernel) in kernels {
        eprintln!("exercising {name}");
        for offset in 0..64 {
            for len in [
                0, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 255, 256, 4095, 4096, 4097, 16384,
            ] {
                let bytes = &data[offset..offset + len];
                let mut expected = [0usize; 256];
                scalar_hist(bytes, &mut expected);
                let mut actual = [0usize; 256];
                // SAFETY: each listed kernel was checked for runtime availability above.
                unsafe {
                    kernel(bytes, &mut actual);
                }
                assert_eq!(actual, expected, "{name}: offset={offset}, len={len}");
                assert_eq!(shannon_entropy(bytes), scalar_entropy(bytes));
                let oracle: f64 = if len == 0 {
                    0.0
                } else {
                    expected
                        .iter()
                        .filter(|&&c| c > 0)
                        .map(|&c| {
                            let p = c as f64 / len as f64;
                            -p * p.log2()
                        })
                        .sum()
                };
                assert!((scalar_entropy(bytes) - oracle).abs() < 1e-12);
            }
        }
    }
}
