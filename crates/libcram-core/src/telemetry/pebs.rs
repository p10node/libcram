//! Optional Linux loader for explicitly audited Intel PEBS load/store events.
use super::sampling::ExplicitPebsConfig;
use std::io;
#[repr(C)]
#[derive(Default)]
struct PerfAttr {
    type_: u32,
    size: u32,
    config: u64,
    period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup: u32,
    bp_type: u32,
    config1: u64,
    config2: u64,
}
fn attributes(c: ExplicitPebsConfig) -> io::Result<PerfAttr> {
    c.validate()?;
    Ok(PerfAttr {
        type_: c.pmu_type,
        size: std::mem::size_of::<PerfAttr>() as u32,
        config: c.config,
        config1: c.config1,
        period: c.period,
        sample_type: 1 << 3,
        flags: 1 | (1 << 5) | (1 << 6) | (2 << 15),
        // Disable large-PEBS auto-reload batching. Its context-switch drain
        // uses perf_event_output directly, bypassing our BPF overflow handler.
        // Linux intel_pmu_hw_config checks wakeup_events && !watermark here.
        wakeup: 1,
        ..PerfAttr::default()
    })
}
use super::access::{AccessKind, AccessWindow};
use super::sampling::{decode_event, drain_events, AccessEvent};
use aya::{
    maps::{MapData, PerCpuArray, RingBuf},
    programs::PerfEvent,
    Ebpf, EbpfLoader,
};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
/// `_IOW('$', 4, __u64)`.
const PERF_EVENT_IOC_PERIOD: u64 = 0x40082404;
/// `_IOW('$', 8, __u32)`.
const PERF_EVENT_IOC_SET_BPF: u64 = 0x40042408;
/// `_IO('$', 0)`.
const PERF_EVENT_IOC_ENABLE: u64 = 0x2400;
fn failure(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn program_name(kind: AccessKind) -> &'static str {
    match kind {
        AccessKind::Read => "sample_load",
        AccessKind::Write => "sample_store",
    }
}
fn validate_target(pid: u32, cpu: u32, configs: &[ExplicitPebsConfig]) -> io::Result<()> {
    if pid == 0
        || pid > i32::MAX as u32
        || cpu > i32::MAX as u32
        || configs.is_empty()
        || configs.len() > 2
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "positive target pid, valid cpu, one or two PMU configurations required",
        ));
    }
    for c in configs {
        c.validate()?;
    }
    if configs.len() == 2 && configs[0].kind == configs[1].kind {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "duplicate operation configuration",
        ));
    }
    super::sampling::validate_pair(configs)
}
#[derive(Default, Debug, Clone, Copy)]
pub struct PollReport {
    pub accepted: usize,
    pub rejected: usize,
}
/// One loaded libcram BPF object: both perf-event programs, the EVENTS ring
/// and the DROPPED map. Loading parses kernel BTF and runs the verifier
/// (≈43 ms of CPU on s5); attaching more perf events to a loaded program
/// costs only perf_event_open and two ioctls.
///
/// Use one program per CPU. Every perf event attached to it writes to the same
/// ring, and the kernel BPF ring takes a trylock in NMI context, so programs
/// shared by concurrently sampling CPUs can lose reservations.
pub struct PebsProgram {
    ring: RingBuf<MapData>,
    bpf: Ebpf,
}
impl PebsProgram {
    /// Load the trusted libcram eBPF object. Requires Intel x86_64,
    /// Linux >= 5.8 and BPF privileges.
    pub fn load(object: &[u8]) -> io::Result<Self> {
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = object;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "PEBS requires Intel x86_64; ARM SPE is not implemented",
            ))
        }
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: CPUID leaf zero is available on every x86_64 CPU. Rust 1.89
            // declares this intrinsic unsafe; newer toolchains make it safe.
            #[allow(unused_unsafe)]
            let vendor = unsafe { std::arch::x86_64::__cpuid(0) };
            if vendor.ebx != 0x756e6547 || vendor.edx != 0x49656e69 || vendor.ecx != 0x6c65746e {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "explicit PEBS loader requires GenuineIntel",
                ));
            }
            // Ebpf::load parses /sys/kernel/btf/vmlinux twice (EbpfLoader::new
            // and an explicit .btf); the loader alone yields the same object
            // with one parse (~40 ms CPU on the 6 MB s5 BTF).
            let mut bpf = EbpfLoader::new().load(object).map_err(failure)?;
            let ring = RingBuf::try_from(
                bpf.take_map("EVENTS")
                    .ok_or_else(|| failure("missing EVENTS map"))?,
            )
            .map_err(failure)?;
            for kind in [AccessKind::Read, AccessKind::Write] {
                let program: &mut PerfEvent = bpf
                    .program_mut(program_name(kind))
                    .ok_or_else(|| failure("missing perf-event program"))?
                    .try_into()
                    .map_err(failure)?;
                program.load().map_err(failure)?;
            }
            Ok(Self { ring, bpf })
        }
    }
    /// Attach to one Linux TID on one CPU (not all process threads): one
    /// precise perf event per configuration, each running this program.
    /// `configs` must be CPU-model-audited load-only/store-only precise events.
    /// An incorrect raw event can mislabel samples; this is an explicit expert
    /// configuration, not automatic PEBS discovery or DATA_SRC decoding.
    /// The perf-event BPF context exposes addr, not PERF_SAMPLE_DATA_SRC.
    /// Nothing is enabled unless every event opens and attaches.
    pub fn attach(
        &self,
        pid: u32,
        cpu: u32,
        configs: &[ExplicitPebsConfig],
    ) -> io::Result<PebsEvents> {
        validate_target(pid, cpu, configs)?;
        let mut events = Vec::with_capacity(configs.len());
        for &config in configs {
            let program: &PerfEvent = self
                .bpf
                .program(program_name(config.kind))
                .ok_or_else(|| failure("missing perf-event program"))?
                .try_into()
                .map_err(failure)?;
            let prog_fd = program.fd().map_err(failure)?.as_fd().as_raw_fd();
            let attr = attributes(config)?;
            // SAFETY: attr is the initialized Linux PERF_ATTR_SIZE_VER1 ABI; the
            // kernel reads exactly size bytes. This call transfers no Rust ownership.
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_perf_event_open,
                    &attr as *const PerfAttr,
                    pid as i32,
                    cpu as i32,
                    -1i32,
                    8u64,
                )
            };
            if fd < 0 {
                let source = io::Error::last_os_error();
                return Err(io::Error::new(
                    source.kind(),
                    format!(
                        "PEBS perf_event_open failed (unsupported precise event, invalid CPU/event, perf_event_paranoid or missing CAP_PERFMON): {source}"
                    ),
                ));
            }
            // SAFETY: successful perf_event_open returned a newly owned descriptor.
            let owned = unsafe { OwnedFd::from_raw_fd(fd as i32) };
            // SAFETY: owned and prog_fd remain valid through ioctl; SET_BPF takes
            // the program fd by value and the kernel keeps its own reference.
            if unsafe { libc::ioctl(owned.as_raw_fd(), PERF_EVENT_IOC_SET_BPF, prog_fd) } < 0 {
                return Err(io::Error::last_os_error());
            }
            events.push(owned);
        }
        // Enable only after every attachment succeeds, avoiding partial startup.
        for fd in &events {
            // SAFETY: PERF_EVENT_IOC_ENABLE has a scalar argument and a live perf fd.
            if unsafe { libc::ioctl(fd.as_raw_fd(), PERF_EVENT_IOC_ENABLE, 0) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(PebsEvents {
            events,
            periods: configs.iter().map(|c| c.period).collect(),
        })
    }
    /// Consume at most budget records from every attachment of this program.
    /// Invalid/expired records are counted, never inferred. Ring overflow is
    /// observable via dropped_samples().
    pub fn poll(&mut self, window: &mut AccessWindow, budget: usize) -> PollReport {
        let (accepted, rejected) = drain_events(
            budget,
            || self.ring.next().map(|item| decode_event(&item)),
            |e| window.record(e.time_ns, e.address, e.tgid, e.kind),
        );
        PollReport { accepted, rejected }
    }
    /// Decode at most `budget` records without choosing an allocation/window.
    /// Malformed records consume budget and are rejected, never delivered.
    pub fn poll_events(
        &mut self,
        budget: usize,
        mut callback: impl FnMut(AccessEvent),
    ) -> PollReport {
        let (accepted, rejected) = drain_events(
            budget,
            || self.ring.next().map(|item| decode_event(&item)),
            |e| {
                callback(e);
                Ok(())
            },
        );
        PollReport { accepted, rejected }
    }
    /// Best-effort cumulative per-CPU diagnostic over every attachment of this
    /// program; not an atomic snapshot. Does not include PMU scheduling loss,
    /// kernel recursion suppression or PEBS records the kernel drops before the
    /// BPF handler.
    pub fn dropped_samples(&self) -> io::Result<u64> {
        let map: PerCpuArray<_, u64> = PerCpuArray::try_from(
            self.bpf
                .map("DROPPED")
                .ok_or_else(|| failure("missing DROPPED map"))?,
        )
        .map_err(failure)?;
        Ok(map
            .get(&0, 0)
            .map_err(failure)?
            .iter()
            .fold(0u64, |n, v| n.saturating_add(*v)))
    }
}
/// The perf events of one (TID, CPU) attachment. Dropping closes them, which
/// detaches them from the program; the program and its ring stay loaded.
pub struct PebsEvents {
    events: Vec<OwnedFd>,
    periods: Vec<u64>,
}
impl PebsEvents {
    /// Raw PMU counts per configured event, useful to distinguish an idle or
    /// unscheduled counter from a userspace ring-consumption failure.
    pub fn event_counts(&self) -> io::Result<Vec<u64>> {
        self.events
            .iter()
            .map(|fd| {
                let mut count = 0u64;
                // SAFETY: fd is owned; PERF_FORMAT=0 returns one writable u64.
                let n = unsafe { libc::read(fd.as_raw_fd(), (&mut count as *mut u64).cast(), 8) };
                if n == 8 {
                    Ok(count)
                } else if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "PMU count unavailable",
                    ))
                }
            })
            .collect()
    }
    pub fn attached_events(&self) -> usize {
        self.events.len()
    }
    /// Current sample period of each configured event, in `attach` order.
    pub fn periods(&self) -> &[u64] {
        &self.periods
    }
    /// Reprogram each configured event's sample period in place, in `attach`
    /// order, with PERF_EVENT_IOC_PERIOD. The kernel restarts each counter's
    /// countdown; counts are unaffected. On error, earlier events may already
    /// use their new period and `periods()` reports what was applied.
    pub fn set_periods(&mut self, periods: &[u64]) -> io::Result<()> {
        if periods.len() != self.events.len()
            || periods.iter().any(|p| *p == 0 || *p >> 63 != 0)
            || matches!(periods, [a, b] if !super::sampling::periods_independent(*a, *b))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "one nonzero period below 2^63 per configured event, pairwise distinct and coprime, required",
            ));
        }
        for ((fd, current), period) in self.events.iter().zip(&mut self.periods).zip(periods) {
            if current == period {
                continue;
            }
            // SAFETY: fd is a live owned perf fd; PERF_EVENT_IOC_PERIOD reads
            // exactly one u64 through the pointer and retains nothing.
            if unsafe { libc::ioctl(fd.as_raw_fd(), PERF_EVENT_IOC_PERIOD, period as *const u64) }
                < 0
            {
                return Err(io::Error::last_os_error());
            }
            *current = *period;
        }
        Ok(())
    }
}
/// One program with one attachment: the single-target API. Keeps the perf
/// FDs and BPF object alive; dropping detaches sampling. No worker thread is
/// spawned. Caller chooses a bounded polling cadence. To attach several TIDs
/// on one CPU cheaply, use [`PebsProgram`] directly.
pub struct PebsSampler {
    events: PebsEvents,
    program: PebsProgram,
}
impl PebsSampler {
    /// Validate, load a fresh program and attach it to `pid` on `cpu`; see
    /// [`PebsProgram::attach`]. Records are keyed by TGID.
    pub fn attach(
        object: &[u8],
        pid: u32,
        cpu: u32,
        configs: &[ExplicitPebsConfig],
    ) -> io::Result<Self> {
        validate_target(pid, cpu, configs)?;
        let program = PebsProgram::load(object)?;
        let events = program.attach(pid, cpu, configs)?;
        Ok(Self { events, program })
    }
    pub fn poll(&mut self, window: &mut AccessWindow, budget: usize) -> PollReport {
        self.program.poll(window, budget)
    }
    pub fn poll_events(&mut self, budget: usize, callback: impl FnMut(AccessEvent)) -> PollReport {
        self.program.poll_events(budget, callback)
    }
    pub fn event_counts(&self) -> io::Result<Vec<u64>> {
        self.events.event_counts()
    }
    pub fn attached_events(&self) -> usize {
        self.events.attached_events()
    }
    pub fn periods(&self) -> &[u64] {
        self.events.periods()
    }
    pub fn set_periods(&mut self, periods: &[u64]) -> io::Result<()> {
        self.events.set_periods(periods)
    }
    pub fn dropped_samples(&self) -> io::Result<u64> {
        self.program.dropped_samples()
    }
}
#[cfg(test)]
mod tests {
    use super::super::access::AccessKind;
    use super::*;
    #[test]
    fn precise_address_sampling_required() {
        let a = attributes(ExplicitPebsConfig {
            pmu_type: 4,
            config: 0x81d0,
            config1: 0,
            period: 10000,
            kind: AccessKind::Read,
        })
        .expect("attr");
        assert_eq!(a.type_, 4);
        assert_eq!(a.size, 72);
        assert_eq!(a.sample_type, 1 << 3);
        // Large-PEBS context-switch drains bypass the BPF overflow handler.
        assert_eq!(a.wakeup, 1);
        assert_eq!((a.flags >> 14) & 1, 0); // event-count wakeups, not watermark
        assert_eq!((a.flags >> 15) & 3, 2);
        assert_ne!(a.flags & (1 << 5), 0);
        assert_ne!(a.flags & 1, 0);
    }
    #[test]
    fn ioctl_encodings() {
        assert_eq!(PERF_EVENT_IOC_ENABLE, 0x24 << 8);
        // _IOW(type='$', nr=8, size=sizeof(u32))
        assert_eq!(
            PERF_EVENT_IOC_SET_BPF,
            (1 << 30) | (4 << 16) | (0x24 << 8) | 8
        );
        // _IOW(type='$', nr=4, size=sizeof(u64))
        assert_eq!(
            PERF_EVENT_IOC_PERIOD,
            (1 << 30) | (8 << 16) | (0x24 << 8) | 4
        );
    }
}
