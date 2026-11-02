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
/// True when two PEBS events may sample together: periods differ and share no
/// common factor. PEBS format 2 (Haswell/Broadwell) drops both records when
/// the two counters overflow within a few hundred events of each other. With
/// equal or commensurate periods a loop retiring loads and stores at a fixed
/// ratio keeps the overflows aligned and loses nearly every sample (measured:
/// 0 of 5,368 per event, `docs/evidence/2026-11-01/governor`).
pub fn periods_independent(a: u64, b: u64) -> bool {
    a != b && gcd(a, b) == 1
}
/// Smallest odd period `>= b` that is independent of `a` (see
/// [`periods_independent`]). Stays below 2^63 for `a`, `b` below 2^62.
pub fn independent_period(a: u64, b: u64) -> u64 {
    let mut p = b | 1;
    while !periods_independent(a, p) {
        p += 2;
    }
    p
}
/// Period for the second event paired with `period`: about `period × 65/64`,
/// odd and independent of `period`. The two counters' overflow phases then
/// drift apart by ~period/64 events per sample, so after any common start
/// (enable, PERF_EVENT_IOC_PERIOD) they leave the collision window within one
/// sample once period/64 exceeds it (~1,000 events measured on s5), and
/// re-enter it for under one sample in 64. Sample weights of the two kinds
/// differ by only ~1.6%, so read/write sample ratios stay comparable.
pub fn companion_period(period: u64) -> u64 {
    let period = period.min(1 << 62);
    independent_period(period, period + (period / 64).max(2))
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
#[cfg(any(test, all(target_os = "linux", feature = "ebpf")))]
pub(crate) fn validate_pair(configs: &[ExplicitPebsConfig]) -> io::Result<()> {
    match configs {
        [a, b] if !periods_independent(a.period, b.period) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "two PEBS events need distinct coprime periods (PEBS fmt2 drops both records when the counters overflow together); see companion_period",
        )),
        _ => Ok(()),
    }
}
/// Raw events audited on Intel Broadwell-X (family 6, model 79) only. This is
/// not a portable selector: call [`broadwell_x::detected`] before using it.
/// Evidence: `docs/evidence/2026-11-01`.
pub mod broadwell_x {
    use super::{companion_period, AccessKind, ExplicitPebsConfig};
    /// `PERF_TYPE_RAW`.
    pub const PMU_TYPE: u32 = 4;
    /// MEM_LOAD_UOPS_RETIRED, umask L1_HIT|L2_HIT|L3_HIT|L3_MISS|HIT_LFB.
    /// Replaces MEM_UOPS_RETIRED.ALL_LOADS (`0x81d0`), whose PEBS data address
    /// on this part holds microcode-issued load addresses. The retired count
    /// matched ALL_LOADS within 0.5% (L1 to DRAM). Loads with an unknown data
    /// source (UC/IO) are not counted. Kernel-inclusive DLA is not audited;
    /// `PebsSampler` always excludes the kernel.
    pub const RETIRED_LOADS: u64 = 0x67d1;
    /// MEM_UOPS_RETIRED.ALL_STORES. User-only DLA verified; kernel-inclusive
    /// DLA is wrong on this part.
    pub const ALL_STORES: u64 = 0x82d0;
    /// Retired user loads every `load_period` and stores every
    /// [`companion_period`]`(load_period)`, in that order. A load sample stands
    /// for `load_period` loads, a store sample for the larger store period.
    pub fn configs(load_period: u64) -> [ExplicitPebsConfig; 2] {
        let store_period = companion_period(load_period);
        [
            ExplicitPebsConfig {
                pmu_type: PMU_TYPE,
                config: RETIRED_LOADS,
                config1: 0,
                period: load_period,
                kind: AccessKind::Read,
            },
            ExplicitPebsConfig {
                pmu_type: PMU_TYPE,
                config: ALL_STORES,
                config1: 0,
                period: store_period,
                kind: AccessKind::Write,
            },
        ]
    }
    /// Retired user loads only, for pipelines that take write evidence from
    /// `telemetry::write_sampler::WriteSampler` (`Bridge::track_writes`)
    /// instead of PEBS stores, whose attribution is biased on this part
    /// (`docs/evidence/2026-11-02/source-split`).
    pub fn load_configs(load_period: u64) -> [ExplicitPebsConfig; 1] {
        let [load, _] = configs(load_period);
        [load]
    }
    /// True only on a GenuineIntel family 6 model 79 CPU.
    pub fn detected() -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: CPUID leaves 0 and 1 exist on every x86_64 CPU. Rust 1.89
            // declares this intrinsic unsafe; newer toolchains make it safe.
            #[allow(unused_unsafe)]
            let (vendor, signature) = unsafe {
                (
                    std::arch::x86_64::__cpuid(0),
                    std::arch::x86_64::__cpuid(1).eax,
                )
            };
            let intel =
                vendor.ebx == 0x756e6547 && vendor.edx == 0x49656e69 && vendor.ecx == 0x6c65746e;
            let family = (signature >> 8) & 0xf;
            let model = ((signature >> 4) & 0xf) | ((signature >> 12) & 0xf0);
            intel && family == 6 && model == 79
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    }
}
/// Samples per second per monitored CPU that keep the measured Broadwell-X
/// cost below 1% of one core in the worst steady state: the governor lets the
/// rate reach this cap before raising periods, so 1,200 × 4.99 µs/sample
/// (upper 95% CI) + 0.33% governed collector = 0.93%
/// (`docs/evidence/2026-11-01/governor`).
pub const DEFAULT_MAX_SAMPLES_PER_CPU_SEC: u32 = 1200;
/// Largest period the governor will program (≈1.1e12 events per sample).
pub const MAX_GOVERNED_PERIOD: u64 = 1 << 40;
/// Caps the projected PEBS sample rate on every monitored CPU by multiplying
/// all configured (base) periods by one shared scale >= 1. One shared scale
/// keeps samples equally weighted across threads, CPUs and operations at any
/// moment; consumers comparing counts across scale changes must weight by
/// [`RateGovernor::period`].
///
/// Raises at once when the projected rate exceeds the cap, leaving 25%
/// headroom. Lowers only when the projection is below 40% of the cap, by at
/// most 2x per update. Between updates a workload burst can exceed the cap for
/// one interval; the kernel's `perf_event_max_sample_rate` remains the hard
/// backstop.
#[derive(Debug, Clone)]
pub struct RateGovernor {
    cap: f64,
    scale: f64,
}
impl RateGovernor {
    const HEADROOM: f64 = 1.25;
    const MAX_SCALE: f64 = MAX_GOVERNED_PERIOD as f64;
    pub fn new(max_samples_per_cpu_sec: u32) -> io::Result<Self> {
        if max_samples_per_cpu_sec == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sample-rate cap must be positive",
            ));
        }
        Ok(Self {
            cap: max_samples_per_cpu_sec as f64,
            scale: 1.0,
        })
    }
    pub fn max_samples_per_cpu_sec(&self) -> u32 {
        self.cap as u32
    }
    pub fn scale(&self) -> f64 {
        self.scale
    }
    /// Period to program for a configured base period: exactly `base` at scale
    /// 1, otherwise odd, never below `base` and at most
    /// `max(base, MAX_GOVERNED_PERIOD)`.
    pub fn period(&self, base: u64) -> u64 {
        if self.scale <= 1.0 {
            return base;
        }
        let scaled = (base as f64 * self.scale).min(MAX_GOVERNED_PERIOD as f64) as u64 | 1;
        scaled.max(base)
    }
    /// `base_samples` holds, per monitored CPU, the events counted during
    /// `elapsed_ns` divided by each event's base period, i.e. the samples that
    /// CPU would have produced at scale 1. Returns true when the scale changed
    /// and periods must be reprogrammed.
    pub fn update(&mut self, elapsed_ns: u64, base_samples: impl IntoIterator<Item = f64>) -> bool {
        if elapsed_ns == 0 {
            return false;
        }
        let peak = base_samples
            .into_iter()
            .filter(|s| s.is_finite() && *s > 0.0)
            .fold(0.0, f64::max);
        let need = peak / (elapsed_ns as f64 / 1e9) / self.cap;
        let next = if need > self.scale {
            need * Self::HEADROOM
        } else if need * Self::HEADROOM * 2.0 < self.scale {
            (need * Self::HEADROOM).max(self.scale / 2.0)
        } else {
            return false;
        };
        let next = next.clamp(1.0, Self::MAX_SCALE);
        if next == self.scale {
            return false;
        }
        self.scale = next;
        true
    }
}
/// Decode the local native-endian eBPF ABI: time_ns, address, tgid, operation.
/// Zero/unknown address or operation is never attributed to a memory page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessEvent {
    pub time_ns: u64,
    pub address: u64,
    pub tgid: u32,
    pub kind: AccessKind,
}
pub fn decode_event(bytes: &[u8]) -> io::Result<AccessEvent> {
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
    Ok(AccessEvent {
        time_ns: fields[0],
        address: fields[1],
        tgid: fields[2] as u32,
        kind,
    })
}
/// Compatibility ingestion path, including AccessWindow admission checks.
pub fn ingest_event(bytes: &[u8], window: &mut AccessWindow) -> io::Result<()> {
    let e = decode_event(bytes)?;
    window.record(e.time_ns, e.address, e.tgid, e.kind)
}
#[cfg(any(test, all(target_os = "linux", feature = "ebpf")))]
pub(crate) fn drain_events(
    budget: usize,
    mut next: impl FnMut() -> Option<io::Result<AccessEvent>>,
    mut accept: impl FnMut(AccessEvent) -> io::Result<()>,
) -> (usize, usize) {
    let (mut accepted, mut rejected) = (0, 0);
    for _ in 0..budget {
        let Some(event) = next() else { break };
        if event.and_then(&mut accept).is_ok() {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }
    (accepted, rejected)
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
    fn typed_decoder_matches_ingestion_and_rejects_malformed() {
        for fields in [
            [10u64, 0x1234, 7, 1],
            [u64::MAX, u64::MAX, u32::MAX as u64, 2],
        ] {
            let bytes: Vec<_> = fields.into_iter().flat_map(u64::to_ne_bytes).collect();
            let event = decode_event(&bytes).expect("typed event");
            assert_eq!(
                (event.time_ns, event.address, event.tgid),
                (fields[0], fields[1], fields[2] as u32)
            );
            let mut direct = AccessWindow::new(100, 8, 4096).expect("window");
            let mut legacy = AccessWindow::new(100, 8, 4096).expect("window");
            direct
                .record(event.time_ns, event.address, event.tgid, event.kind)
                .expect("record");
            ingest_event(&bytes, &mut legacy).expect("ingest");
            assert_eq!(
                direct
                    .counts(event.time_ns, event.address, event.tgid)
                    .expect("counts"),
                legacy
                    .counts(event.time_ns, event.address, event.tgid)
                    .expect("counts")
            );
            for len in 0..32 {
                assert!(decode_event(&bytes[..len]).is_err());
            }
            let mut extended = bytes.clone();
            extended.push(0);
            assert!(decode_event(&extended).is_err());
        }
        for fields in [
            [1u64, 0, 7, 1],
            [1, 1, u32::MAX as u64 + 1, 1],
            [1, 1, 7, 0],
            [1, 1, 7, 3],
        ] {
            let bytes: Vec<_> = fields.into_iter().flat_map(u64::to_ne_bytes).collect();
            assert!(decode_event(&bytes).is_err());
        }
    }
    #[test]
    fn bounded_drain_counts_decode_and_window_rejections() {
        let good = AccessEvent {
            time_ns: 1000,
            address: 4096,
            tgid: 7,
            kind: AccessKind::Read,
        };
        let old = AccessEvent { time_ns: 1, ..good };
        let mut input = [
            Ok(good),
            Ok(old),
            Err(io::Error::other("malformed")),
            Ok(good),
        ]
        .into_iter();
        let mut w = AccessWindow::new(10, 4, 4096).expect("window");
        let counts = drain_events(
            3,
            || input.next(),
            |e| w.record(e.time_ns, e.address, e.tgid, e.kind),
        );
        assert_eq!(counts, (1, 2));
        assert!(
            input.next().is_some(),
            "budget must not consume fourth record"
        );
        let mut calls = 0;
        assert_eq!(
            drain_events(
                0,
                || {
                    calls += 1;
                    Some(Ok(good))
                },
                |_| Ok(())
            ),
            (0, 0)
        );
        assert_eq!(calls, 0);
    }
    #[test]
    fn broadwell_x_profile_is_retired_loads_and_stores() {
        let [load, store] = broadwell_x::configs(200003);
        assert_eq!((load.pmu_type, load.config, load.config1), (4, 0x67d1, 0));
        assert_eq!(
            (store.pmu_type, store.config, store.config1),
            (4, 0x82d0, 0)
        );
        assert_eq!(
            (load.kind, store.kind),
            (AccessKind::Read, AccessKind::Write)
        );
        assert_eq!(
            (load.period, store.period),
            (200003, companion_period(200003))
        );
        validate_pair(&[load, store]).expect("independent pair");
        assert_ne!(load.config, 0x81d0, "ALL_LOADS DLA is wrong on Broadwell-X");
        load.validate().expect("load");
        store.validate().expect("store");
        #[cfg(not(target_arch = "x86_64"))]
        assert!(!broadwell_x::detected());
    }
    #[test]
    fn paired_periods_never_align() {
        assert!(!periods_independent(200003, 200003));
        assert!(!periods_independent(200003, 400006));
        assert!(!periods_independent(9, 15));
        assert!(periods_independent(200003, 323609));
        for p in [
            1u64,
            2,
            3,
            20011,
            200003,
            2000003,
            3602591,
            1 << 40,
            (1 << 62) - 1,
        ] {
            let c = companion_period(p);
            assert!(periods_independent(p, c), "{p} {c}");
            assert_eq!(c % 2, 1);
            if p > 6400 && p < 1 << 61 {
                let ratio = c as f64 / p as f64;
                assert!((ratio - 65.0 / 64.0).abs() < 0.001, "{p} {c}");
            }
        }
        assert_eq!(independent_period(15, 9), 11);
        assert_eq!(independent_period(7, 7), 9);
        let same = [broadwell_x::configs(200003)[0]; 2];
        assert!(validate_pair(&same).is_err());
        assert!(validate_pair(&same[..1]).is_ok());
    }
    #[test]
    fn governor_caps_peak_cpu_with_hysteresis() {
        const SEC: u64 = 1_000_000_000;
        assert!(RateGovernor::new(0).is_err());
        let mut g = RateGovernor::new(1500).expect("governor");
        assert_eq!(g.period(200003), 200003);
        // Under the cap, or no elapsed time / invalid input: unchanged.
        assert!(!g.update(SEC, [1400.0, 10.0]));
        assert!(!g.update(0, [1e9]));
        assert!(!g.update(SEC, [f64::NAN, f64::INFINITY, -5.0]));
        assert_eq!(g.scale(), 1.0);
        // The busiest CPU decides; raise at once with 25% headroom.
        assert!(g.update(SEC / 10, [10.0, 2242.0]));
        assert!((g.scale() - 22420.0 / 1500.0 * 1.25).abs() < 1e-9);
        let p = g.period(200003);
        assert!(p % 2 == 1 && p > 200003);
        // At the new scale the same workload projects to cap / 1.25.
        let projected = 22420.0 / g.scale();
        assert!((projected - 1200.0).abs() < 1e-6);
        // Inside the hysteresis band (40%..100% of cap): unchanged.
        assert!(!g.update(SEC, [22420.0]));
        assert!(!g.update(SEC, [0.41 * 1500.0 * g.scale()]));
        // Far below the cap: lower by at most 2x per update, never below 1.
        let before = g.scale();
        assert!(g.update(SEC, [1.0]));
        assert!((g.scale() - before / 2.0).abs() < 1e-9);
        while g.update(SEC, []) {}
        assert_eq!(g.scale(), 1.0);
        assert_eq!(g.period(200003), 200003);
        // Periods are clamped, but never below the configured base.
        assert!(g.update(1, [1e300]));
        assert_eq!(g.period(3), MAX_GOVERNED_PERIOD | 1);
        assert_eq!(g.period(u64::MAX >> 1), u64::MAX >> 1);
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
