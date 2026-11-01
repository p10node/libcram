//! Owned workload only. Raw events below are audited for Intel Broadwell-X,
//! family 6 model 79, not a portable CPU event selector. No system-wide attach.
//!
//! Workloads (all buffers initialised before attach):
//! - `l1`: 16 KiB read region, 16 KiB write region, 1 load + 1 store per step.
//! - `mixed`: 8 L1 loads + 1 dependent load chasing a random cycle through a
//!   64 MiB region (L2/L3/DRAM) + 1 store per step.
//! - `baseline`: `l1` without sampling.
//!
//! The load count each workload retires is known exactly, so the PMU count
//! of an all-load event can be checked for coverage, independent of ALL_LOADS.
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{bail, ensure, Context};
    use libcram_core::telemetry::{
        access::{AccessKind, AccessWindow},
        pebs::PebsSampler,
        sampling::{broadwell_x, ExplicitPebsConfig},
    };
    use std::time::{Duration, Instant};
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        (4..=6).contains(&args.len()),
        "usage: pebs_acceptance ELF CPU PERIOD [retired-loads|all-loads|l1-hit|l1-miss] [l1|mixed|baseline] (Broadwell-X only)"
    );
    let cpu: usize = args[2].parse()?;
    let period: u64 = args[3].parse()?;
    let load_kind = args.get(4).map(String::as_str).unwrap_or("retired-loads");
    let (load_config, all_loads) = match load_kind {
        "retired-loads" => (broadwell_x::RETIRED_LOADS, true),
        "all-loads" => (0x81d0, true),
        "l1-hit" => (0x01d1, false),
        "l1-miss" => (0x08d1, false),
        _ => bail!("unsupported load event"),
    };
    let mode = args.get(5).map(String::as_str).unwrap_or("l1");
    let (mixed, baseline) = match mode {
        "l1" => (false, false),
        "mixed" => (true, false),
        "baseline" => (false, true),
        _ => bail!("workload must be l1, mixed or baseline"),
    };
    println!(
        "load_event={load_kind} config={load_config:#x} workload={mode}; cache-specific events do NOT represent all loads"
    );
    ensure!(cpu < libc::CPU_SETSIZE as usize, "invalid CPU");
    ensure!(
        broadwell_x::detected(),
        "raw events audited only for GenuineIntel family 6 model 79"
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
        fn contains(&self, page: u64) -> bool {
            page >= self.0 as u64 && page < self.0 as u64 + self.1 as u64
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
    const CHASE_BYTES: usize = 64 << 20;
    let chase = Mapping::new(if mixed { CHASE_BYTES } else { 4096 })?;
    if mixed {
        // Single random cycle over cache lines (Sattolo), one pointer per line.
        let lines = CHASE_BYTES / 64;
        let mut perm: Vec<u32> = (0..lines as u32).collect();
        let mut x = 88172645463325252u64;
        for i in (1..lines).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            perm.swap(i, (x % i as u64) as usize);
        }
        for i in 0..lines {
            let next = perm[(i + 1) % lines] as usize * 8;
            // SAFETY: both indices address u64 slots inside the owned mapping.
            unsafe {
                *chase.0.add(perm[i] as usize * 8) = chase.0.add(next) as u64;
            }
        }
    }
    let object = std::fs::read(&args[1]).context("ELF")?;
    // Store period is companion_period(period): equal periods make PEBS fmt2
    // drop records whenever both counters overflow together.
    let [_, store] = broadwell_x::configs(period);
    let configs = [
        ExplicitPebsConfig {
            pmu_type: broadwell_x::PMU_TYPE,
            config: load_config,
            config1: 0,
            period,
            kind: AccessKind::Read,
        },
        store,
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
    // Each round is a non-inlined function taking and returning values only,
    // so no local is address-taken and every step retires exactly the loads
    // and stores written here (no stack spills or reloads).
    const L1_STEPS: u64 = 128 * 2048;
    const MIXED_STEPS: u64 = 4096;
    /// SAFETY: r and w point to disjoint live mappings of 2048 u64 each.
    #[inline(never)]
    unsafe fn l1_round(r: *const u64, w: *mut u64, mut sum: u64) -> u64 {
        for _ in 0..128 {
            for i in 0..2048 {
                sum = sum.wrapping_add(std::ptr::read_volatile(r.add(i)));
                std::ptr::write_volatile(w.add(i), sum);
            }
        }
        sum
    }
    /// SAFETY: as l1_round; cur points into a cycle of pointers to itself.
    #[inline(never)]
    unsafe fn mixed_round(
        r: *const u64,
        w: *mut u64,
        mut cur: *const u64,
        mut sum: u64,
    ) -> (*const u64, u64) {
        for j in 0..MIXED_STEPS as usize {
            for k in 0..8 {
                sum = sum.wrapping_add(std::ptr::read_volatile(r.add((j * 8 + k) & 2047)));
            }
            cur = std::ptr::read_volatile(cur) as *const u64;
            std::ptr::write_volatile(w.add(j & 2047), sum);
        }
        (cur, sum)
    }
    let mut cur = chase.0 as *const u64;
    let begin = Instant::now();
    while begin.elapsed() < Duration::from_secs(3) {
        // SAFETY: disjoint owned mappings sized as required; chase holds a
        // pointer cycle in mixed mode.
        unsafe {
            if mixed {
                (cur, sum) = mixed_round(read.0, write.0, cur, sum);
            } else {
                sum = l1_round(read.0, write.0, sum);
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
    std::hint::black_box(cur);
    println!("measurement_wall_ns={wall_ns} measurement_rounds={rounds} baseline={baseline}");
    let Some(mut sampler) = sampler else {
        println!("BASELINE_ONLY; no sampling and no attribution acceptance");
        return Ok(());
    };
    let counts = sampler.event_counts()?;
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
    let (mut l1_reads, mut chase_reads, mut other_reads) = (0, 0, 0);
    let (mut rw, mut wr, mut ww, mut other_writes) = (0, 0, 0, 0);
    for (pid, page, c) in window.snapshot(now)? {
        if pid != std::process::id() {
            continue;
        }
        if read.contains(page) {
            l1_reads += c.reads;
            rw += c.writes;
        } else if mixed && chase.contains(page) {
            chase_reads += c.reads;
            rw += c.writes;
        } else if write.contains(page) {
            wr += c.reads;
            ww += c.writes;
        } else {
            other_reads += c.reads;
            other_writes += c.writes;
        }
    }
    let rr = l1_reads + chase_reads;
    let in_range = rr as f64 / (rr + other_reads).max(1) as f64;
    let (load_steps, loads_per_step) = if mixed {
        (rounds * MIXED_STEPS, 9)
    } else {
        (rounds * L1_STEPS, 1)
    };
    let expected_loads = load_steps * loads_per_step;
    let load_coverage = counts[0] as f64 / expected_loads as f64;
    let store_coverage = counts[1] as f64 / load_steps as f64;
    let dropped = sampler.dropped_samples()?;
    println!("periods={:?}", sampler.periods());
    println!("raw_pmu_counts={counts:?} expected_loads={expected_loads} expected_stores={load_steps} load_coverage={load_coverage:.5} store_coverage={store_coverage:.5}");
    println!("cpu={cpu} period={period} events={} rounds={rounds} checksum={sum} accepted={accepted} rejected={rejected} dropped={dropped} l1_reads={l1_reads} chase_reads={chase_reads} other_reads={other_reads} read_in_range={in_range:.4} read_region_writes={rw} write_region_reads={wr} write_region_writes={ww} other_writes={other_writes}", sampler.attached_events());
    ensure!(
        rr >= 10 && ww >= 10,
        "insufficient independently attributed load/store samples"
    );
    ensure!(
        rw == 0 && wr == 0,
        "cross-attributed accesses on disjoint read-only/write-only workload regions"
    );
    ensure!(dropped == 0 && rejected == 0, "sample loss/rejection");
    if mixed {
        // PEBS records the first qualifying load to retire after the counter
        // arms, which after a long stall is usually the stalled (DRAM) load.
        // L1 loads are therefore under-sampled here; this is reported, not
        // gated. The gate is that non-L1 loads carry valid addresses.
        let sampled_l1_share = l1_reads as f64 / rr.max(1) as f64;
        println!(
            "sampled_l1_share={sampled_l1_share:.4} retired_l1_share={:.4}",
            8.0 / 9.0
        );
        ensure!(chase_reads >= 10, "non-L1 loads must be attributed");
    }
    if all_loads {
        ensure!(
            in_range >= 0.95,
            "load samples outside the workload's read regions"
        );
        ensure!(
            load_coverage >= 0.99,
            "load event counted fewer loads than the workload retired"
        );
    }
    ensure!(store_coverage >= 0.99, "store event undercounted");
    println!("PEBS_ATTRIBUTION_PASS; overhead is NOT measured by this probe");
    Ok(())
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf")))]
fn main() {
    eprintln!("requires Linux x86_64 and --features ebpf");
    std::process::exit(2);
}
