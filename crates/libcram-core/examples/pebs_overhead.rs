//! Owned-workload CPU cost of the PEBS + BPF collector (Broadwell-X raw events).
//!
//! Interleaved ABBA blocks of identical fixed work, with and without the real
//! `PebsSampler` + BPF ELF attached to this process's workload thread only.
//! Several sample periods are interleaved so per-sample cost can be fitted by
//! regression instead of inferred from a single noisy difference.
//!
//! Measured per block:
//! - workload thread CPU time (`CLOCK_THREAD_CPUTIME_ID`). With
//!   `CONFIG_IRQ_TIME_ACCOUNTING` unset, PMI/NMI, PEBS-drain and BPF time is
//!   charged to the interrupted task; time stolen by other tasks is not.
//! - wall time and involuntary context switches (interference flag).
//! - collector thread CPU time from attach-complete to final ring drain.
//!
//! With `cap=N` the collector thread runs the production
//! `ProcessPebsCollector` (thread discovery, PMU count reads, rate governor
//! capped at N samples/s per CPU, period reprogramming) instead of a bare
//! `PebsSampler`; PERIOD is then the base period the governor scales.
//!
//! No system-wide attach. Prints raw BLOCK lines; analysis is done offline.
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
fn main() -> anyhow::Result<()> {
    use anyhow::{bail, ensure, Context};
    use libcram_core::telemetry::{
        access::{AccessKind, AccessWindow},
        pebs::PebsSampler,
        process_pebs::ProcessPebsCollector,
        sampling::{broadwell_x, companion_period, ExplicitPebsConfig},
    };
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

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
    fn clock(id: libc::clockid_t) -> u64 {
        // SAFETY: clock_gettime writes exactly one timespec.
        unsafe {
            let mut t: libc::timespec = std::mem::zeroed();
            libc::clock_gettime(id, &mut t);
            t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
        }
    }
    fn switches() -> (i64, i64) {
        // SAFETY: getrusage writes one rusage for the calling thread.
        unsafe {
            let mut r: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_THREAD, &mut r);
            (r.ru_nvcsw, r.ru_nivcsw)
        }
    }

    let a: Vec<String> = std::env::args().collect();
    ensure!(
        (9..=11).contains(&a.len()),
        "usage: pebs_overhead ELF WORK_CPU COLLECTOR_CPU retired-loads|all-loads|l1-hit PERIOD[,PERIOD..] PAIRS ROUNDS POLL_MS [jitter] [cap=N]"
    );
    let object = std::fs::read(&a[1]).context("ELF")?;
    let work_cpu: usize = a[2].parse()?;
    let coll_cpu: usize = a[3].parse()?;
    let load_config: u64 = match a[4].as_str() {
        "retired-loads" => broadwell_x::RETIRED_LOADS,
        "all-loads" => 0x81d0,
        "l1-hit" => 0x01d1,
        _ => bail!("unsupported load event"),
    };
    let periods: Vec<u64> = a[5].split(',').map(str::parse).collect::<Result<_, _>>()?;
    let pairs: usize = a[6].parse()?;
    let rounds: u64 = a[7].parse()?;
    let poll = Duration::from_millis(a[8].parse()?);
    let (mut jitter, mut cap) = (false, None);
    for flag in &a[9..] {
        match flag.split_once('=') {
            None if flag == "jitter" => jitter = true,
            Some(("cap", n)) => cap = Some(n.parse::<u32>()?),
            _ => bail!("optional flags are jitter and cap=N"),
        }
    }
    ensure!(
        work_cpu != coll_cpu,
        "workload and collector CPUs must differ"
    );
    ensure!(
        broadwell_x::detected(),
        "raw events audited only for GenuineIntel family 6 model 79"
    );
    pin(work_cpu)?;
    // SAFETY: gettid takes no arguments.
    let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u32;

    struct Mapping(*mut u64, usize);
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: this object owns the entire still-live mapping.
            unsafe { libc::munmap(self.0.cast(), self.1) };
        }
    }
    let map = |n: usize| -> anyhow::Result<Mapping> {
        // SAFETY: new private anonymous mapping, no fixed address.
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
        ensure!(p != libc::MAP_FAILED, "mmap");
        // SAFETY: fresh writable mapping of n bytes.
        unsafe { std::ptr::write_bytes(p.cast::<u8>(), 0x5a, n) };
        Ok(Mapping(p.cast(), n))
    };
    let read = map(16384)?;
    let write = map(16384)?;
    // One volatile load + one volatile store per iteration. Pointers are copied
    // into locals so the compiler cannot reload them through possible aliases.
    // In jitter mode every iteration also reads the TSC; any gap >= GAP_TSC is
    // time the thread lost (PEBS assist, NMI/PMI, BPF, timer tick, ...).
    const BUCKETS: [u64; 8] = [64, 250, 1000, 2000, 4000, 8000, 16000, 64000]; // ns
    struct Run {
        sum: u64,
        tsc: u64,
        gaps: [u64; 8],
        gap_tsc: [u64; 8],
    }
    let (r, w) = (read.0, write.0);
    let work = move |n: u64, gap_tsc: &[u64; 8]| -> Run {
        let mut run = Run {
            sum: 0,
            tsc: 0,
            gaps: [0; 8],
            gap_tsc: [0; 8],
        };
        // SAFETY: RDTSC has no memory side effects.
        let t0 = unsafe { core::arch::x86_64::_rdtsc() };
        let mut prev = t0;
        for _ in 0..n {
            for _ in 0..128 {
                for i in 0..2048 {
                    // SAFETY: disjoint owned mappings, in-bounds index.
                    unsafe {
                        run.sum = run.sum.wrapping_add(std::ptr::read_volatile(r.add(i)));
                        std::ptr::write_volatile(w.add(i), run.sum);
                    }
                    if jitter {
                        // SAFETY: as above.
                        let now = unsafe { core::arch::x86_64::_rdtsc() };
                        let d = now.wrapping_sub(prev);
                        prev = now;
                        if d >= gap_tsc[0] {
                            let mut b = 0;
                            while b + 1 < 8 && d >= gap_tsc[b + 1] {
                                b += 1;
                            }
                            run.gaps[b] += 1;
                            run.gap_tsc[b] += d;
                        }
                    }
                }
            }
        }
        // SAFETY: as above.
        run.tsc = unsafe { core::arch::x86_64::_rdtsc() } - t0;
        run
    };
    // Calibrate TSC ticks per ns against CLOCK_MONOTONIC (constant_tsc host).
    let tsc_per_ns = {
        // SAFETY: RDTSC has no memory side effects.
        let (c0, t0) = (
            unsafe { core::arch::x86_64::_rdtsc() },
            clock(libc::CLOCK_MONOTONIC),
        );
        std::thread::sleep(Duration::from_millis(200));
        let (c1, t1) = (
            unsafe { core::arch::x86_64::_rdtsc() },
            clock(libc::CLOCK_MONOTONIC),
        );
        (c1 - c0) as f64 / (t1 - t0) as f64
    };
    let gap_tsc: [u64; 8] = BUCKETS.map(|ns| (ns as f64 * tsc_per_ns) as u64);
    enum Cmd {
        Attach(u64),
        Detach,
        Stop,
    }
    #[derive(Default)]
    struct Stats {
        cpu_ns: u64,
        accepted: usize,
        rejected: usize,
        dropped: u64,
        polls: u64,
        counts: Vec<u64>,
        scale: f64,
        periods: Vec<u64>,
        attach_cpu_ns: u64,
        settled_accepted: usize,
        settled_ns: u64,
    }
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
    let (rep_tx, rep_rx) = mpsc::channel::<Result<Option<Stats>, String>>();
    let collector = std::thread::spawn(move || -> anyhow::Result<()> {
        pin(coll_cpu)?;
        let mut window = AccessWindow::new(100_000_000, 65536, 4096)?;
        loop {
            let period = match cmd_rx.recv()? {
                Cmd::Stop => return Ok(()),
                Cmd::Detach => bail!("detach without attach"),
                Cmd::Attach(p) => p,
            };
            let configs = [
                ExplicitPebsConfig {
                    pmu_type: broadwell_x::PMU_TYPE,
                    config: load_config,
                    config1: 0,
                    period,
                    kind: AccessKind::Read,
                },
                ExplicitPebsConfig {
                    pmu_type: broadwell_x::PMU_TYPE,
                    config: broadwell_x::ALL_STORES,
                    config1: 0,
                    // Equal periods make PEBS fmt2 drop lockstep records.
                    period: companion_period(period),
                    kind: AccessKind::Write,
                },
            ];
            if let Some(cap) = cap {
                let c0 = clock(libc::CLOCK_THREAD_CPUTIME_ID);
                let mut st = Stats::default();
                // Construction and first poll (thread scan + attach) are charged.
                let attached = ProcessPebsCollector::new(
                    object.clone(),
                    vec![work_cpu as u32],
                    configs.to_vec(),
                    8,
                    8192,
                )
                .and_then(|c| c.with_rate_limit(Some(cap)))
                .and_then(|mut c| c.poll().map(|e| (c, e.len())));
                let mut c = match attached {
                    Ok((c, n)) => {
                        st.accepted += n;
                        st.polls += 1;
                        c
                    }
                    Err(e) => {
                        rep_tx.send(Err(e.to_string()))?;
                        continue;
                    }
                };
                st.attach_cpu_ns = clock(libc::CLOCK_THREAD_CPUTIME_ID) - c0;
                rep_tx.send(Ok(None))?;
                // Steady state starts at the first governor update (second poll).
                let mut settled_at = None;
                loop {
                    match cmd_rx.recv_timeout(poll) {
                        Err(RecvTimeoutError::Timeout) => {
                            let n = c.poll()?.len();
                            st.accepted += n;
                            st.polls += 1;
                            match settled_at {
                                None => settled_at = Some(clock(libc::CLOCK_MONOTONIC)),
                                Some(_) => st.settled_accepted += n,
                            }
                        }
                        Ok(Cmd::Detach) => break,
                        Ok(_) => bail!("unexpected command"),
                        Err(RecvTimeoutError::Disconnected) => bail!("controller gone"),
                    }
                }
                let end = clock(libc::CLOCK_MONOTONIC);
                st.settled_ns = settled_at.map_or(0, |t| end - t);
                loop {
                    let n = c.poll()?.len();
                    st.accepted += n;
                    st.polls += 1;
                    if settled_at.is_some() {
                        st.settled_accepted += n;
                    }
                    if n == 0 {
                        break;
                    }
                }
                st.scale = c.period_scale();
                st.periods = c.periods();
                drop(c);
                st.cpu_ns = clock(libc::CLOCK_THREAD_CPUTIME_ID) - c0;
                rep_tx.send(Ok(Some(st)))?;
                continue;
            }
            let mut s = match PebsSampler::attach(&object, tid, work_cpu as u32, &configs) {
                Ok(s) => s,
                Err(e) => {
                    rep_tx.send(Err(e.to_string()))?;
                    continue;
                }
            };
            let mut st = Stats::default();
            let c0 = clock(libc::CLOCK_THREAD_CPUTIME_ID);
            rep_tx.send(Ok(None))?;
            loop {
                match cmd_rx.recv_timeout(poll) {
                    Err(RecvTimeoutError::Timeout) => {
                        let r = s.poll(&mut window, 8192);
                        st.accepted += r.accepted;
                        st.rejected += r.rejected;
                        st.polls += 1;
                    }
                    Ok(Cmd::Detach) => break,
                    Ok(_) => bail!("unexpected command"),
                    Err(RecvTimeoutError::Disconnected) => bail!("controller gone"),
                }
            }
            loop {
                let r = s.poll(&mut window, 8192);
                st.accepted += r.accepted;
                st.rejected += r.rejected;
                st.polls += 1;
                if r.accepted + r.rejected == 0 {
                    break;
                }
            }
            st.cpu_ns = clock(libc::CLOCK_THREAD_CPUTIME_ID) - c0;
            st.counts = s.event_counts()?;
            st.dropped = s.dropped_samples()?;
            st.scale = 1.0;
            st.periods = s.periods().to_vec();
            drop(s);
            rep_tx.send(Ok(Some(st)))?;
        }
    });

    let mut seq = 0u64;
    let mut block = |pair: usize, period: u64, sampled: bool| -> anyhow::Result<()> {
        if sampled {
            cmd_tx.send(Cmd::Attach(period))?;
            match rep_rx.recv()? {
                Ok(None) => {}
                Ok(Some(_)) => bail!("protocol"),
                Err(e) => bail!("attach failed: {e}"),
            }
        }
        let (v0, i0) = switches();
        let w0 = clock(libc::CLOCK_MONOTONIC);
        let c0 = clock(libc::CLOCK_THREAD_CPUTIME_ID);
        let run = work(rounds, &gap_tsc);
        let c1 = clock(libc::CLOCK_THREAD_CPUTIME_ID);
        let w1 = clock(libc::CLOCK_MONOTONIC);
        let (v1, i1) = switches();
        let st = if sampled {
            cmd_tx.send(Cmd::Detach)?;
            match rep_rx.recv()? {
                Ok(Some(s)) => s,
                Ok(None) => bail!("protocol"),
                Err(e) => bail!("collector: {e}"),
            }
        } else {
            Stats::default()
        };
        println!(
            "BLOCK seq={seq} pair={pair} period={period} kind={} wall_ns={} cpu_ns={} tsc={} nvcsw={} nivcsw={} coll_cpu_ns={} attach_cpu_ns={} settled_accepted={} settled_ns={} accepted={} rejected={} dropped={} polls={} counts={:?} scale={:.3} periods={:?} gaps={:?} gap_tsc={:?} checksum={}",
            if sampled { "sampled" } else { "base" },
            w1 - w0,
            c1 - c0,
            run.tsc,
            v1 - v0,
            i1 - i0,
            st.cpu_ns,
            st.attach_cpu_ns,
            st.settled_accepted,
            st.settled_ns,
            st.accepted,
            st.rejected,
            st.dropped,
            st.polls,
            st.counts,
            st.scale,
            st.periods,
            run.gaps,
            run.gap_tsc,
            run.sum
        );
        seq += 1;
        Ok(())
    };
    println!(
        "CONFIG work_cpu={work_cpu} collector_cpu={coll_cpu} load=0x{load_config:x} store=0x82d0 load_periods={periods:?} store_period=companion pairs={pairs} rounds={rounds} poll_ms={} jitter={jitter} cap={cap:?} tsc_per_ns={tsc_per_ns:.6} bucket_ns={BUCKETS:?}",
        poll.as_millis()
    );
    block(usize::MAX, periods[0], false)?; // warm-up, discard
    block(usize::MAX, periods[0], true)?;
    for pair in 0..pairs {
        for &p in &periods {
            let order = if pair % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            };
            for sampled in order {
                block(pair, p, sampled)?;
            }
        }
    }
    cmd_tx.send(Cmd::Stop)?;
    collector
        .join()
        .map_err(|_| anyhow::anyhow!("collector panicked"))??;
    println!("DONE; raw blocks only, acceptance is decided offline");
    Ok(())
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf")))]
fn main() {
    eprintln!("requires Linux x86_64 and --features ebpf");
    std::process::exit(2);
}
