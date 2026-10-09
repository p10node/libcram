/// Return a verified short period, or zero when the bounded scout declines.
/// The first byte recurrence is only a candidate: all remaining bytes, including
/// a partial final repetition, must equal their period-shifted counterparts.
/// A missed period merely selects the normal histogram; no input state is cached.
#[inline]
pub(super) fn period(data: &[u8]) -> usize {
    if data.len() < 128 {
        return 0;
    }
    let Some(index) = data[1..65].iter().position(|byte| *byte == data[0]) else {
        return 0;
    };
    let period = index + 1;
    if data[period..] == data[..data.len() - period] {
        period
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::period;

    #[test]
    fn short_period_requires_complete_byte_equality() {
        for width in 1..=64 {
            let tile: Vec<u8> = (0..width).map(|i| (i * 37 + 19) as u8).collect();
            let data: Vec<u8> = (0..131200).map(|i| tile[i % width]).collect();
            for offset in 0..64 {
                for len in [
                    0, 63, 64, 65, 127, 128, 129, 255, 256, 4095, 4096, 4097, 32769, 65537, 131073,
                ] {
                    assert_eq!(
                        period(&data[offset..offset + len]),
                        if len < 128 { 0 } else { width },
                        "width={width},offset={offset},len={len}"
                    );
                }
            }
        }
        let mut bytes: Vec<u8> = (0..4097).map(|i| ((i % 33) * 37 + 19) as u8).collect();
        for pos in 0..bytes.len() {
            bytes[pos] ^= 0x81;
            assert_eq!(period(&bytes), 0, "mutation={pos}");
            bytes[pos] ^= 0x81;
        }
        // The first recurrence can be a false candidate. Declining a later real
        // period is intentional; the ordinary histogram remains the fallback.
        let ambiguous: Vec<u8> = [1, 2, 1, 3].into_iter().cycle().take(4096).collect();
        assert_eq!(period(&ambiguous), 0);
        let long: Vec<u8> = (0..256).cycle().take(4096).map(|i| i as u8).collect();
        assert_eq!(period(&long), 0);
    }
}
