//! Explicit Broadwell-X L1-hot, owned-process acceptance; no universal PMU profile.
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
fn main() -> anyhow::Result<()> {
    use libcram_core::telemetry::{
        access::{AccessKind, AccessWindow},
        process_pebs::ProcessPebsCollector,
        sampling::ExplicitPebsConfig,
    };
    use std::{
        io,
        sync::mpsc,
        time::{Duration, Instant},
    };
    fn mono() -> io::Result<u64> {
        // SAFETY: zeroed timespec is valid output storage.
        let mut t: libc::timespec = unsafe { std::mem::zeroed() };
        // SAFETY: output points to a live timespec.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64)
    }
    fn fds() -> io::Result<usize> {
        Ok(std::fs::read_dir("/proc/self/fd")?.count())
    }
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(
        (3..=4).contains(&args.len()),
        "usage: managed_pebs_acceptance TRUSTED_ELF CPU [expect-denied]"
    );
    let object = std::fs::read(&args[1])?;
    let cpu: u32 = args[2].parse()?;
    anyhow::ensure!(cpu < libc::CPU_SETSIZE as u32, "invalid CPU");
    let denied = args.get(3).is_some_and(|m| m == "expect-denied");
    anyhow::ensure!(args.len() == 3 || denied, "invalid mode");
    let info = std::fs::read_to_string("/proc/cpuinfo")?;
    let field = |key: &str| {
        info.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == key).then(|| v.trim())
        })
    };
    anyhow::ensure!(
        field("cpu family") == Some("6") && field("model") == Some("79"),
        "only explicit Broadwell-X fixtures are qualified"
    );
    // SAFETY: zeroed cpu_set_t is valid before CPU_SET.
    let mut affinity: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: bounded CPU, valid mask, only this calling thread is affected.
    unsafe {
        libc::CPU_SET(cpu as usize, &mut affinity);
    }
    // SAFETY: live correctly-sized CPU mask; target 0 is this thread.
    if unsafe { libc::sched_setaffinity(0, std::mem::size_of_val(&affinity), &affinity) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let configs = vec![
        ExplicitPebsConfig {
            pmu_type: 4,
            config: 0x01d1,
            config1: 0,
            period: 200003,
            kind: AccessKind::Read,
        },
        ExplicitPebsConfig {
            pmu_type: 4,
            config: 0x82d0,
            config1: 0,
            period: 200003,
            kind: AccessKind::Write,
        },
    ];
    let before = fds()?;
    let mut source =
        ProcessPebsCollector::new(object.clone(), vec![cpu], configs.clone(), 8, 1024)?;
    if denied {
        let error = source
            .poll()
            .expect_err("precise sampling must be unavailable in this fixture");
        println!("expected_failure={error} status={:?}", source.status());
        anyhow::ensure!(
            source.attached_samplers() == 0 && fds()? == before,
            "failed attachment leaked FDs"
        );
        println!("MANAGED_PEBS_DENIAL_CLEANUP_PASS");
        return Ok(());
    }
    source.poll()?;
    anyhow::ensure!(source.attached_samplers() == 1, "expected main thread");
    let one_thread_fds = fds()?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let helper = std::thread::spawn(move || {
        let _ = ready_tx.send(());
        let _ = stop_rx.recv();
    });
    ready_rx.recv_timeout(Duration::from_secs(2))?;
    source.poll()?;
    anyhow::ensure!(source.attached_samplers() == 2, "new thread not discovered");
    stop_tx.send(())?;
    helper
        .join()
        .map_err(|_| anyhow::anyhow!("owned helper panicked"))?;
    source.poll()?;
    anyhow::ensure!(
        source.attached_samplers() == 1 && fds()? == one_thread_fds,
        "exited thread descriptors were retained"
    );
    println!("thread_attachment_history=1,2,1");
    let reads = Mapping::new()?;
    let writes = Mapping::new()?;
    let mut window = AccessWindow::new(200_000_000, 8192, 4096)?;
    let started = Instant::now();
    let mut last_poll = started;
    let (mut seen_read, mut seen_write, mut wrong_read, mut wrong_write) = (0u64, 0u64, 0u64, 0u64);
    let tgid = std::process::id();
    while started.elapsed() < Duration::from_secs(3) {
        let mut sum = 0u64;
        for i in 0..2048 {
            // SAFETY: initialized owned mappings are live, aligned and 16 KiB long.
            unsafe {
                sum = sum.wrapping_add(reads.0.cast::<u64>().add(i).read_volatile());
                writes.0.cast::<u64>().add(i).write_volatile(sum);
            }
        }
        std::hint::black_box(sum);
        if last_poll.elapsed() >= Duration::from_millis(10) {
            for event in source.poll()? {
                window.record(event.time_ns, event.address, event.tgid, event.kind)?;
            }
            for (pid, page, count) in window.snapshot(mono()?)? {
                if pid != tgid {
                    continue;
                }
                if reads.contains(page) {
                    seen_read += count.reads;
                    wrong_write += count.writes;
                }
                if writes.contains(page) {
                    seen_write += count.writes;
                    wrong_read += count.reads;
                }
            }
            window = AccessWindow::new(200_000_000, 8192, 4096)?;
            last_poll = Instant::now();
        }
    }
    println!("l1_reads={seen_read} stores={seen_write} wrong_reads={wrong_read} wrong_writes={wrong_write} status={:?}", source.status());
    anyhow::ensure!(
        seen_read >= 10 && seen_write >= 10 && wrong_read == 0 && wrong_write == 0,
        "attribution gate failed"
    );
    drop(source);
    anyhow::ensure!(fds()? == before, "collector drop leaked FDs");
    let mut partial =
        ProcessPebsCollector::new(object, vec![cpu, i32::MAX as u32], configs, 8, 1024)?;
    let error = partial
        .poll()
        .expect_err("invalid CPU must reject partial attachment");
    println!("partial_attach_error={error}");
    anyhow::ensure!(
        partial.attached_samplers() == 0 && fds()? == before,
        "partial attach leaked FDs"
    );
    println!("MANAGED_PEBS_LIFECYCLE_L1_ATTRIBUTION_PASS");
    Ok(())
}
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
struct Mapping(*mut u8);
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
impl Mapping {
    fn new() -> std::io::Result<Self> {
        // SAFETY: fresh anonymous owned mapping; checked before initializing all bytes.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: mapping grants exclusive write access to 16384 bytes.
        unsafe {
            p.cast::<u8>().write_bytes(0x5a, 16384);
        }
        Ok(Self(p.cast()))
    }
    fn contains(&self, page: u64) -> bool {
        page >= self.0 as u64 && page < self.0 as u64 + 16384
    }
}
#[cfg(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf"))]
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: mapping belongs exclusively to this guard and its accesses have ended.
        unsafe {
            libc::munmap(self.0.cast(), 16384);
        }
    }
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64", feature = "ebpf")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("requires Linux x86_64 + ebpf")
}
