//! Owned workload only. Raw events are audited for Intel Broadwell-X (family
//! 6, model 79). End-to-end read/write evidence through the production bridge:
//! governed PEBS retired loads (`ProcessPebsCollector`, loads only) for reads,
//! and sampled userfaultfd write-protection (`WriteSampler`) for writes.
//!
//! Four owned allocations get the access mix of
//! `docs/evidence/2026-11-02/source-split` (per step):
//! - A: 16 KiB, 8 loads (read-mostly, L1)
//! - B: 64 MiB pointer chase, 1 load (read-only, DRAM)
//! - C: 16 KiB, 1 load + 2 stores (write-heavy, L1)
//! - D: 64 MiB chase, 1 load + 1 store after it (write-heavy, DRAM)
//!
//! With PEBS store samples, C was classified read-dominant in every run. Here
//! the bridge's write evidence is checked per observation: after one second,
//! A and B must be reported write-free (`writes == 0`), C and D never. How
//! often `reads / 4 > writes` holds is reported too; it also needs at least
//! four read samples on one page within a 100 ms window, which sparse PEBS
//! samples over 64 MiB rarely give.
//!
//! usage: write_policy_acceptance ELF WORKLOAD_CPU COLLECTOR_CPU SECONDS
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    use libcram_core::{
        allocator::{AccessHint, CramAllocator},
        device_adapter::DeviceReading,
        runtime::Collector,
        runtime_bridge::{Bridge, BridgeConfig, DeviceSource},
        telemetry::{
            process_pebs::ProcessPebsCollector, sampling::broadwell_x, write_sampler::WriteSampler,
        },
    };
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    struct Device;
    impl DeviceSource for Device {
        fn read_device(&mut self) -> std::io::Result<DeviceReading> {
            Ok(DeviceReading {
                used_bytes: 0,
                capacity_bytes: 1 << 30,
                poison_pages: vec![],
            })
        }
    }
    fn pin(cpu: usize) -> anyhow::Result<()> {
        ensure!(cpu < libc::CPU_SETSIZE as usize, "invalid CPU");
        // SAFETY: initialized local cpu_set_t; affects the calling thread only.
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
        Ok(())
    }
    /// Single random cycle over the cache lines of `bytes` (Sattolo).
    fn chase(bytes: &mut [u8], seed: u64) {
        let base = bytes.as_mut_ptr() as *mut u64;
        let lines = bytes.len() / 64;
        let mut perm: Vec<u32> = (0..lines as u32).collect();
        let mut x = seed;
        for i in (1..lines).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            perm.swap(i, (x % i as u64) as usize);
        }
        for i in 0..lines {
            // SAFETY: both slots are line starts inside `bytes`.
            unsafe {
                *base.add(perm[i] as usize * 8) =
                    base.add(perm[(i + 1) % lines] as usize * 8) as u64;
            }
        }
    }
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "usage: write_policy_acceptance ELF WORKLOAD_CPU COLLECTOR_CPU SECONDS"
    );
    ensure!(
        broadwell_x::detected(),
        "raw events audited only for Broadwell-X"
    );
    let object = std::fs::read(&args[1]).context("ELF")?;
    let (workload_cpu, collector_cpu): (usize, usize) = (args[2].parse()?, args[3].parse()?);
    let seconds = Duration::from_secs(args[4].parse()?);
    pin(collector_cpu)?;
    let allocator = CramAllocator::new()?;
    let page = libcram_core::hal::page_size();
    let sizes = [16 << 10, 64 << 20, 16 << 10, 64 << 20];
    let mut regions = Vec::new();
    for size in sizes {
        let mut owned = allocator.allocate_owned(size, page, AccessHint::Auto)?;
        owned.as_mut_slice().fill(0x5a);
        regions.push(owned);
    }
    chase(regions[1].as_mut_slice(), 88172645463325252);
    chase(regions[3].as_mut_slice(), 0x9e37_79b9_7f4a_7c15);
    let ranges: Vec<std::ops::Range<usize>> = regions
        .iter()
        .map(|r| {
            let start = r.as_slice().as_ptr() as usize;
            start..start + r.as_slice().len()
        })
        .collect();
    let bases: Vec<usize> = regions
        .iter_mut()
        .map(|r| r.as_mut_slice().as_mut_ptr() as usize)
        .collect();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || -> anyhow::Result<u64> {
        pin(workload_cpu)?;
        let (a, c) = (bases[0] as *const u64, bases[2] as *mut u64);
        let (mut b, mut d) = (bases[1] as *const u64, bases[3] as *mut u64);
        let (mut sum, mut steps) = (0u64, 0u64);
        while !worker_stop.load(Ordering::Relaxed) {
            for _ in 0..(1 << 14) {
                // SAFETY: indices stay inside A and C (2048 u64 each); b and d
                // follow pointer cycles inside B and D, and d[1] is in d's line.
                unsafe {
                    for k in 0..8 {
                        sum = sum.wrapping_add(std::ptr::read_volatile(
                            a.add((steps as usize * 8 + k) & 2047),
                        ));
                    }
                    b = std::ptr::read_volatile(b) as *const u64;
                    d = std::ptr::read_volatile(d) as *mut u64;
                    std::ptr::write_volatile(d.add(1), sum);
                    sum = sum.wrapping_add(std::ptr::read_volatile(c.add(steps as usize & 2047)));
                    std::ptr::write_volatile(c.add((steps as usize + 1) & 2047), sum);
                    std::ptr::write_volatile(c.add((steps as usize + 2) & 2047), sum);
                }
                steps += 1;
            }
        }
        std::hint::black_box((sum, b));
        Ok(steps)
    });
    // Run the bridge in a closure so the worker is always stopped before the
    // owned regions it touches are dropped, whatever the outcome.
    let run = || -> anyhow::Result<[[u64; 4]; 4]> {
        let collector = ProcessPebsCollector::new(
            object,
            vec![workload_cpu as u32],
            broadwell_x::load_configs(200003).to_vec(),
            8,
            8192,
        )?;
        // The 64 MiB regions spread samples over many pages per 100 ms window.
        let config = BridgeConfig {
            max_output: 128,
            ..BridgeConfig::default()
        };
        let (mut bridge, publisher) = Bridge::new(collector, Device, config)?;
        bridge.track_writes(WriteSampler::new(
            WriteSampler::DEFAULT_PAGES_PER_CYCLE,
            WriteSampler::DEFAULT_MIN_INTERVAL,
        )?)?;
        // [observations, write-free observations, read-dominant observations,
        //  last write evidence after the first second (0 none, 1 written, 2 free)]
        let mut seen = [[0u64; 4]; 4];
        let begin = Instant::now();
        let mut published: Option<Instant> = None;
        while begin.elapsed() < seconds {
            if published.is_none_or(|p| p.elapsed() >= Duration::from_millis(400)) {
                for r in &regions {
                    publisher.register(r.observation_token(0.0)?)?;
                }
                published = Some(Instant::now());
            }
            bridge.device_update()?;
            while let Some(o) = bridge.poll()? {
                if let Some(i) = ranges.iter().position(|r| r.contains(&o.address)) {
                    let free = o.writes == 0;
                    seen[i][0] += 1;
                    seen[i][1] += u64::from(free);
                    seen[i][2] += u64::from(o.reads > 0 && o.reads / 4 > o.writes);
                    // Evidence after the first second, once checks have built up.
                    if begin.elapsed() > Duration::from_secs(1) {
                        seen[i][3] = 1 + u64::from(free);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(seen)
    };
    let result = run();
    stop.store(true, Ordering::Relaxed);
    let steps = worker
        .join()
        .map_err(|_| anyhow::anyhow!("worker panicked"))??;
    let seen = result?;
    println!("steps={steps} seconds={}", seconds.as_secs());
    let expected = [true, true, false, false];
    let mut pass = true;
    for (i, name) in ["A", "B", "C", "D"].iter().enumerate() {
        let [observations, free, dominant, last] = seen[i];
        let ok = if expected[i] {
            last == 2
        } else {
            free == 0 && dominant == 0
        };
        pass &= ok;
        println!(
            "REGION {name} observations={observations} write_free_observations={free} read_dominant_observations={dominant} last_write_free={} expected={} {}",
            match last { 0 => "none", 1 => "no", _ => "yes" },
            if expected[i] { "yes" } else { "never" },
            if ok { "OK" } else { "WRONG" }
        );
    }
    ensure!(
        pass,
        "write evidence did not separate read-dominant from write-heavy allocations"
    );
    println!("WRITE_POLICY_PASS");
    Ok(())
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf")))]
fn main() {
    eprintln!("requires Linux x86_64 and --features ebpf");
    std::process::exit(2);
}
