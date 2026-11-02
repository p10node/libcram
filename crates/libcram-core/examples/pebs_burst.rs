//! Owned workload only. Raw events are audited for Intel Broadwell-X (family
//! 6, model 79). Burst bound of the production `ProcessPebsCollector`.
//!
//! The workload thread alternates BURST_MS of L1 load/store streaming with
//! IDLE_MS of sleep. During idle the 100 ms rate governor relaxes its period
//! scale, so every burst starts near the base period: the case the governor
//! alone cannot bound. The collector thread polls every 5 ms. Reports the
//! samples of each burst and the largest number of samples whose event
//! timestamps fall in any 100 ms window, against the per-CPU budget.
//!
//! usage: pebs_burst ELF WORKLOAD_CPU COLLECTOR_CPU BASE_PERIOD BURST_MS IDLE_MS SECONDS
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{ensure, Context};
    use libcram_core::telemetry::{
        process_pebs::ProcessPebsCollector,
        sampling::{broadwell_x, DEFAULT_MAX_SAMPLES_PER_CPU_SEC},
    };
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
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
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 8,
        "usage: pebs_burst ELF WORKLOAD_CPU COLLECTOR_CPU BASE_PERIOD BURST_MS IDLE_MS SECONDS"
    );
    ensure!(
        broadwell_x::detected(),
        "raw events audited only for Broadwell-X"
    );
    let object = std::fs::read(&args[1]).context("ELF")?;
    let (workload_cpu, collector_cpu): (usize, usize) = (args[2].parse()?, args[3].parse()?);
    let period: u64 = args[4].parse()?;
    let burst = Duration::from_millis(args[5].parse()?);
    let idle = Duration::from_millis(args[6].parse()?);
    let seconds = Duration::from_secs(args[7].parse()?);
    pin(collector_cpu)?;
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || -> anyhow::Result<u64> {
        pin(workload_cpu)?;
        let (read, mut write) = (vec![1u64; 2048], vec![0u64; 2048]);
        let mut sum = 0u64;
        let mut bursts = 0;
        while !worker_stop.load(Ordering::Relaxed) {
            let start = Instant::now();
            while start.elapsed() < burst {
                for i in 0..2048 {
                    // SAFETY: indices are inside both owned vectors.
                    unsafe {
                        sum = sum.wrapping_add(std::ptr::read_volatile(read.as_ptr().add(i)));
                        std::ptr::write_volatile(write.as_mut_ptr().add(i), sum);
                    }
                }
            }
            bursts += 1;
            std::thread::sleep(idle);
        }
        std::hint::black_box(sum);
        Ok(bursts)
    });
    let mut collector = ProcessPebsCollector::new(
        object,
        vec![workload_cpu as u32],
        broadwell_x::configs(period).to_vec(),
        8,
        8192,
    )?;
    let mut times = Vec::new();
    let begin = Instant::now();
    while begin.elapsed() < seconds {
        times.extend(collector.poll()?.into_iter().map(|e| e.time_ns));
        std::thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::Relaxed);
    let bursts = worker
        .join()
        .map_err(|_| anyhow::anyhow!("worker panicked"))??;
    for _ in 0..4 {
        times.extend(collector.poll()?.into_iter().map(|e| e.time_ns));
        std::thread::sleep(Duration::from_millis(5));
    }
    times.sort_unstable();
    // Largest number of samples in any 100 ms window of event time.
    let mut worst = 0;
    let mut first = 0;
    for (last, t) in times.iter().enumerate() {
        while *t - times[first] >= 100_000_000 {
            first += 1;
        }
        worst = worst.max(last + 1 - first);
    }
    let budget = 2 * DEFAULT_MAX_SAMPLES_PER_CPU_SEC / 10;
    let elapsed = begin.elapsed().as_secs_f64();
    println!(
        "base_period={period} burst_ms={} idle_ms={} bursts={bursts} samples={} mean_per_s={:.0} worst_100ms_window={worst} per_cpu_budget_per_100ms={budget} final_scale={:.2} status={:?}",
        burst.as_millis(),
        idle.as_millis(),
        times.len(),
        times.len() as f64 / elapsed,
        collector.period_scale(),
        collector.status()
    );
    Ok(())
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf")))]
fn main() {
    eprintln!("requires Linux x86_64 and --features ebpf");
    std::process::exit(2);
}
