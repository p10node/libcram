//! Local native-endian ring ABI, shared by both perf-event entrypoints.
pub fn event(time: u64, address: u64, tgid: u64, operation: u64) -> Option<[u64; 4]> {
    if address == 0 || tgid > u32::MAX as u64 || (operation != 1 && operation != 2) {
        None
    } else {
        Some([time, address, tgid, operation])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_address_and_explicit_operation_only() {
        assert_eq!(event(10, 0x1234, 7, 1), Some([10, 0x1234, 7, 1]));
        assert_eq!(event(10, 0x1234, 7, 2), Some([10, 0x1234, 7, 2]));
        assert_eq!(event(10, 0, 7, 1), None);
        assert_eq!(event(10, 0x1234, 7, 0), None);
    }
}
