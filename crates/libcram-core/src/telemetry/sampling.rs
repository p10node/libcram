//! Explicit PMU configuration: no portable Intel event number exists.
use super::access::{AccessKind, AccessWindow};
use std::io;
#[derive(Debug, Clone, Copy)]
pub struct ExplicitPebsConfig {
    pub pmu_type: u32,
    pub config: u64,
    pub config1: u64,
    pub period: u64,
    pub kind: AccessKind,
}
impl ExplicitPebsConfig {
    /// Caller must obtain the raw event/umask/config1 from this CPU model's
    /// PMU documentation and assert it samples *only* the declared operation.
    /// This validates shape, not vendor event semantics. Software/tracepoint
    /// PMUs are rejected; raw/hybrid PMU types still require explicit auditing.
    pub fn validate(&self) -> io::Result<()> {
        if self.pmu_type < 4 || self.config == 0 || self.period == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "explicit raw PEBS memory event and nonzero period required",
            ));
        }
        Ok(())
    }
}
/// Decode the local native-endian eBPF ABI: time_ns, address, tgid, operation.
/// Zero/unknown address or operation is never attributed to a memory page.
pub fn ingest_event(bytes: &[u8], window: &mut AccessWindow) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid PEBS ring event");
    if bytes.len() != 32 {
        return Err(invalid());
    }
    let mut fields = [0u64; 4];
    for (dest, src) in fields.iter_mut().zip(bytes.as_chunks::<8>().0) {
        *dest = u64::from_ne_bytes(*src);
    }
    if fields[1] == 0 || fields[2] > u32::MAX as u64 {
        return Err(invalid());
    }
    let kind = match fields[3] {
        1 => AccessKind::Read,
        2 => AccessKind::Write,
        _ => return Err(invalid()),
    };
    window.record(fields[0], fields[1], fields[2] as u32, kind)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_order_and_query_between_bounded_drains_do_not_change_retained_samples() {
        let events = [[100u64, 4096, 7, 1], [120, 8192, 8, 2], [110, 4097, 7, 1]];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut w = AccessWindow::new(30, 2, 4096).expect("window");
            let mut watermark = 0;
            for index in order {
                let raw: Vec<u8> = events[index]
                    .into_iter()
                    .flat_map(u64::to_ne_bytes)
                    .collect();
                ingest_event(&raw, &mut w).expect("valid delayed event");
                watermark = watermark.max(events[index][0]);
                w.counts(watermark, 4096, 7).expect("query between drains");
            }
            assert_eq!(w.retained_samples(), 2);
            assert_eq!(w.capacity_evictions, 1);
            assert_eq!(w.counts(120, 4096, 7).expect("reads").reads, 1);
            assert_eq!(w.counts(120, 8192, 8).expect("writes").writes, 1);
            assert_eq!(w.counts(140, 4096, 7).expect("expired read").reads, 0);
            assert_eq!(w.counts(140, 8192, 8).expect("live write").writes, 1);
        }
    }
    #[test]
    fn sample_decode_and_config_rejection() {
        let mut w = AccessWindow::new(100, 8, 4096).expect("window");
        let raw: Vec<u8> = [10u64, 0x1234, 7, 2]
            .into_iter()
            .flat_map(u64::to_ne_bytes)
            .collect();
        ingest_event(&raw, &mut w).expect("event");
        assert_eq!(w.counts(10, 0x1000, 7).expect("counts").writes, 1);
        assert!(ingest_event(&raw[..31], &mut w).is_err());
        let bad: Vec<u8> = [10u64, 0x1234, 7, 0]
            .into_iter()
            .flat_map(u64::to_ne_bytes)
            .collect();
        assert!(ingest_event(&bad, &mut w).is_err());
        assert!(ExplicitPebsConfig {
            pmu_type: 1,
            config: 1,
            config1: 0,
            period: 1000,
            kind: AccessKind::Read
        }
        .validate()
        .is_err());
        assert!(ExplicitPebsConfig {
            pmu_type: 4,
            config: 1,
            config1: 0,
            period: 0,
            kind: AccessKind::Read
        }
        .validate()
        .is_err());
    }
}
