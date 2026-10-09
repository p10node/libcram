//! Owned workload only. Raw events below are audited for Intel Broadwell-X,
//! family 6 model 79, not a portable CPU event selector. No system-wide attach.
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    use libcram_core::telemetry::{
        access::{AccessKind, AccessWindow},
        pebs::PebsSampler,
        sampling::ExplicitPebsConfig,
    };
    use std::time::{Duration, Instant};
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        (4..=6).contains(&args.len()),
        "usage: pebs_acceptance ELF CPU PERIOD [all-loads|l1-hit|l1-miss] [baseline] (Broadwell-X only)"
    );
    let cpu: usize = args[2].parse()?;
    let period: u64 = args[3].parse()?;
    let baseline = args.get(5).is_some_and(|s| s == "baseline");
    ensure!(
        args.len() < 6 || baseline,
        "sixth argument must be baseline"
    );
    let load_kind = args.get(4).map(String::as_str).unwrap_or("all-loads");
    let load_config = match load_kind {
        "all-loads" => 0x81d0,
        "l1-hit" => 0x01d1,
        "l1-miss" => 0x08d1,
        _ => anyhow::bail!("unsupported load event"),
    };
    println!("load_event={load_kind}; cache-specific modes do NOT represent all loads");
    ensure!(cpu < libc::CPU_SETSIZE as usize, "invalid CPU");
    // SAFETY: CPUID is available on x86_64 (unsafe in the MSRV).
    #[allow(unused_unsafe)]
    let cpuid = unsafe { std::arch::x86_64::__cpuid(1) };
    let family = (cpuid.eax >> 8) & 15;
    let model = ((cpuid.eax >> 4) & 15) | ((cpuid.eax >> 12) & 0xf0);
    ensure!(
        family == 6 && model == 79,
        "raw events audited only for family 6 model 79"
    );
    // SAFETY: initialized local cpu_set_t, valid CPU, and current-thread affinity only.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        ensure!(
            libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set) == 0,
            "affinity: {}",
            std::io::Error::last_os_error()
        );
    }
    struct Mapping(*mut u64, usize);
    impl Mapping {
        fn new(n: usize) -> anyhow::Result<Self> {
            // SAFETY: new private mapping, no fixed address or existing memory modified.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    n,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            ensure!(
                p != libc::MAP_FAILED,
                "mmap: {}",
                std::io::Error::last_os_error()
            );
            // SAFETY: fresh writable mapping of n bytes, exclusively owned here.
            unsafe { std::ptr::write_bytes(p, 0x5a, n) };
            Ok(Self(p.cast(), n))
        }
    }
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: this object owns the entire still-live mapping.
            unsafe {
                libc::munmap(self.0.cast(), self.1);
            }
        }
    }
    let read = Mapping::new(16384)?;
    let write = Mapping::new(16384)?;
    let object = std::fs::read(&args[1]).context("ELF")?;
    let configs = [
        ExplicitPebsConfig {
            pmu_type: 4,
            config: load_config,
            config1: 0,
            period,
            kind: AccessKind::Read,
        },
        ExplicitPebsConfig {
            pmu_type: 4,
            config: 0x82d0,
            config1: 0,
            period,
            kind: AccessKind::Write,
        },
    ];
    // SAFETY: gettid takes no pointer arguments.
    let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
    let mut sampler = if baseline {
        None
    } else {
        Some(PebsSampler::attach(&object, tid, cpu as u32, &configs)?)
    };
    // Acceptance retains this bounded three-second experiment, not a production window.
    let mut window = AccessWindow::new(10_000_000_000, 65536, 4096)?;
    let mut accepted = 0;
    let mut rejected = 0;
    let mut rounds = 0u64;
    let mut sum = 0u64;
    let begin = Instant::now();
    while begin.elapsed() < Duration::from_secs(3) {
        for _ in 0..128 {
            for i in 0..2048 {
                // SAFETY: non-overlapping owned mappings, in-bounds u64 index, no aliases.
                unsafe {
                    sum = sum.wrapping_add(std::ptr::read_volatile(read.0.add(i)));
                    std::ptr::write_volatile(write.0.add(i), sum);
                }
            }
        }
        rounds += 1;
        let report = sampler
            .as_mut()
            .map(|s| s.poll(&mut window, 8192))
            .unwrap_or_default();
        accepted += report.accepted;
        rejected += report.rejected;
    }
    let wall_ns = begin.elapsed().as_nanos();
    println!("measurement_wall_ns={wall_ns} measurement_rounds={rounds} baseline={baseline}");
    let Some(mut sampler) = sampler else {
        println!("BASELINE_ONLY; no sampling and no attribution acceptance");
        return Ok(());
    };
    let report = sampler.poll(&mut window, 8192);
    accepted += report.accepted;
    rejected += report.rejected;
    // SAFETY: clock_gettime writes exactly one initialized timespec.
    let now = unsafe {
        let mut t: libc::timespec = std::mem::zeroed();
        ensure!(
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) == 0,
            "clock"
        );
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    };
    let mut rr = 0;
    let mut rw = 0;
    let mut wr = 0;
    let mut ww = 0;
    for page in 0..4 {
        let c = window.counts(now, read.0 as u64 + page * 4096, std::process::id())?;
        rr += c.reads;
        rw += c.writes;
        let c = window.counts(now, write.0 as u64 + page * 4096, std::process::id())?;
        wr += c.reads;
        ww += c.writes;
    }
    println!(
        "regions read={:#x} write={:#x}; retained_pages={:?}",
        read.0 as usize,
        write.0 as usize,
        window
            .snapshot(now)?
            .into_iter()
            .take(16)
            .collect::<Vec<_>>()
    );
    println!("raw_pmu_counts={:?}", sampler.event_counts()?);
    let dropped = sampler.dropped_samples()?;
    println!("cpu={cpu} period={period} events={} rounds={rounds} checksum={sum} accepted={accepted} rejected={rejected} dropped={dropped} read_region_reads={rr} read_region_writes={rw} write_region_reads={wr} write_region_writes={ww}",sampler.attached_events());
    ensure!(
        rr >= 10 && ww >= 10,
        "insufficient independently attributed load/store samples"
    );
    ensure!(
        rw == 0 && wr == 0,
        "cross-attributed accesses on disjoint read-only/write-only workload regions"
    );
    ensure!(dropped == 0 && rejected == 0, "sample loss/rejection");
    println!("PEBS_ATTRIBUTION_PASS; overhead is NOT measured by this probe");
    Ok(())
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf")))]
fn main() {
    eprintln!("requires Linux x86_64 and --features ebpf");
    std::process::exit(2);
}
