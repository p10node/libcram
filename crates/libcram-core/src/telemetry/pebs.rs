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
use aya::{
    maps::{MapData, RingBuf},
    programs::PerfEvent,
    Ebpf,
};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
fn failure(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
/// Keeps the perf FDs and BPF object alive; dropping detaches sampling.
/// No worker thread is spawned. Caller chooses a bounded polling cadence.
pub struct PebsSampler {
    events: Vec<OwnedFd>,
    ring: RingBuf<MapData>,
    _bpf: Ebpf,
}
#[derive(Default, Debug, Clone, Copy)]
pub struct PollReport {
    pub accepted: usize,
    pub rejected: usize,
}
impl PebsSampler {
    /// Attach to one Linux TID on one CPU (not all process threads).
    /// Call separately for additional TIDs/CPUs; records are keyed by TGID.
    /// `configs` must be CPU-model-audited load-only/store-only precise events.
    /// An incorrect raw event can mislabel samples; this is an explicit expert
    /// configuration, not automatic PEBS discovery or DATA_SRC decoding.
    /// The perf-event BPF context exposes addr, not PERF_SAMPLE_DATA_SRC.
    /// Requires Intel x86_64, Linux >= 5.8, BPF/perf privileges and PEBS PMU.
    pub fn attach(
        object: &[u8],
        pid: u32,
        cpu: u32,
        configs: &[ExplicitPebsConfig],
    ) -> io::Result<Self> {
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
        }
        #[cfg(not(target_arch = "x86_64"))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "PEBS requires Intel x86_64; ARM SPE is not implemented",
        ));
        #[cfg(target_arch = "x86_64")]
        {
            let mut bpf = Ebpf::load(object).map_err(failure)?;
            let ring = RingBuf::try_from(
                bpf.take_map("EVENTS")
                    .ok_or_else(|| failure("missing EVENTS map"))?,
            )
            .map_err(failure)?;
            let mut events = Vec::new();
            for &config in configs {
                let name = match config.kind {
                    AccessKind::Read => "sample_load",
                    AccessKind::Write => "sample_store",
                };
                let program: &mut PerfEvent = bpf
                    .program_mut(name)
                    .ok_or_else(|| failure("missing perf-event program"))?
                    .try_into()
                    .map_err(failure)?;
                program.load().map_err(failure)?;
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
                let prog_fd = program.fd().map_err(failure)?.as_fd().as_raw_fd();
                // SAFETY: owned and prog_fd remain valid through ioctl; SET_BPF takes fd by value.
                if unsafe { libc::ioctl(owned.as_raw_fd(), 0x40042408u64, prog_fd) } < 0 {
                    return Err(io::Error::last_os_error());
                }
                events.push(owned);
            }
            // Enable only after every attachment succeeds, avoiding partial startup.
            for fd in &events {
                // SAFETY: PERF_EVENT_IOC_ENABLE has a scalar argument and a live perf fd.
                if unsafe { libc::ioctl(fd.as_raw_fd(), 0x2400u64, 0) } < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(Self {
                events,
                ring,
                _bpf: bpf,
            })
        }
    }
    /// Consume at most budget records. Invalid/expired records are counted,
    /// never inferred. Ring overflow is observable via dropped_samples().
    pub fn poll(&mut self, window: &mut AccessWindow, budget: usize) -> PollReport {
        let (accepted, rejected) = super::sampling::drain_events(
            budget,
            || {
                self.ring
                    .next()
                    .map(|item| super::sampling::decode_event(&item))
            },
            |e| window.record(e.time_ns, e.address, e.tgid, e.kind),
        );
        PollReport { accepted, rejected }
    }
    /// Decode at most `budget` records without choosing an allocation/window.
    /// Malformed records consume budget and are rejected, never delivered.
    pub fn poll_events(
        &mut self,
        budget: usize,
        mut callback: impl FnMut(super::sampling::AccessEvent),
    ) -> PollReport {
        let (accepted, rejected) = super::sampling::drain_events(
            budget,
            || {
                self.ring
                    .next()
                    .map(|item| super::sampling::decode_event(&item))
            },
            |e| {
                callback(e);
                Ok(())
            },
        );
        PollReport { accepted, rejected }
    }
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
    /// Best-effort cumulative per-CPU diagnostic; not an atomic snapshot.
    /// Does not include PMU scheduling loss or kernel recursion suppression.
    pub fn dropped_samples(&self) -> io::Result<u64> {
        let map: aya::maps::PerCpuArray<_, u64> = aya::maps::PerCpuArray::try_from(
            self._bpf
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
}
