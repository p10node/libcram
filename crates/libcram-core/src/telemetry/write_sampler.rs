//! Sampled write detection for owned allocations, without the PMU.
//!
//! Each cycle write-protects one random page per tracked allocation (up to a
//! budget) with userfaultfd in asynchronous write-protect mode
//! (`UFFD_FEATURE_WP_ASYNC`, Linux >= 6.7): a write to a protected page is
//! resolved by the kernel itself, without a signal or a userfault message, and
//! clears the page's uffd-wp bit. At the next cycle the bit is read from
//! `/proc/self/pagemap` (bit 57); a page that lost it was written.
//!
//! This replaces PEBS store samples as write evidence. On Broadwell-X, PEBS
//! attributes stores to the first store retiring after a stall, so a
//! write-heavy cache-resident page can look write-free
//! (`docs/evidence/2026-11-02/source-split`). Page protection has no such bias
//! (`docs/evidence/2026-11-02/write-track`). Its cost is set by the sample
//! budget (≈3.6 µs per sampled page on s5), not by the write rate.
//!
//! The fd uses `UFFD_USER_MODE_ONLY`, so `vm.unprivileged_userfaultfd=0` does
//! not block it. Container seccomp profiles may still deny the syscall.
use crate::runtime_bridge::{WriteCheck, WriteSource};
use std::{
    collections::BTreeMap,
    fs::File,
    io,
    ops::Range,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::FileExt,
    },
    time::{Duration, Instant},
};

const UFFD_API: u64 = 0xaa;
const UFFD_USER_MODE_ONLY: libc::c_int = 1;
const UFFD_FEATURE_WP_UNPOPULATED: u64 = 1 << 13;
const UFFD_FEATURE_WP_ASYNC: u64 = 1 << 15;
const UFFDIO_API: libc::c_ulong = 0xc018_aa3f;
const UFFDIO_REGISTER: libc::c_ulong = 0xc020_aa00;
const UFFDIO_UNREGISTER: libc::c_ulong = 0x8010_aa01;
const UFFDIO_WRITEPROTECT: libc::c_ulong = 0xc018_aa06;
const UFFDIO_REGISTER_MODE_WP: u64 = 1 << 1;
const UFFDIO_WRITEPROTECT_MODE_WP: u64 = 1 << 0;
/// `/proc/<pid>/pagemap`: the PTE is uffd write-protected.
const PM_UFFD_WP: u64 = 1 << 57;

#[repr(C)]
struct UffdioApi {
    api: u64,
    features: u64,
    ioctls: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct UffdioRange {
    start: u64,
    len: u64,
}
#[repr(C)]
struct UffdioRegister {
    range: UffdioRange,
    mode: u64,
    ioctls: u64,
}
#[repr(C)]
struct UffdioWriteprotect {
    range: UffdioRange,
    mode: u64,
}

/// Write evidence for owned allocations from sampled asynchronous
/// write-protection.
pub struct WriteSampler {
    uffd: OwnedFd,
    pagemap: File,
    page: usize,
    /// Full pages of each tracked allocation.
    tracked: BTreeMap<u64, Range<usize>>,
    /// Pages protected by the last cycle.
    pending: Vec<(u64, usize)>,
    pages_per_cycle: usize,
    min_interval: Duration,
    last_cycle: Option<Instant>,
    cursor: u64,
    rng: u64,
}

impl WriteSampler {
    /// Pages protected per cycle: 0.070–0.079% of a core measured for 16 pages
    /// per 100 ms (`docs/evidence/2026-11-02/write-track`, standalone probe).
    pub const DEFAULT_PAGES_PER_CYCLE: usize = 16;
    /// Shortest time between cycles; also the shortest write-detection window.
    pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(100);

    /// Open a userfaultfd with asynchronous write-protection. Fails with
    /// `Unsupported` when the kernel lacks it (Linux < 6.7, or no
    /// `CONFIG_PTE_MARKER_UFFD_WP`).
    pub fn new(pages_per_cycle: usize, min_interval: Duration) -> io::Result<Self> {
        if !(1..=4096).contains(&pages_per_cycle) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pages per cycle must be in 1..=4096",
            ));
        }
        // SAFETY: userfaultfd takes only integer flags and returns a new fd.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_userfaultfd,
                libc::O_CLOEXEC | libc::O_NONBLOCK | UFFD_USER_MODE_ONLY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful userfaultfd returned a newly owned descriptor.
        let uffd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        let wanted = UFFD_FEATURE_WP_ASYNC | UFFD_FEATURE_WP_UNPOPULATED;
        let mut api = UffdioApi {
            api: UFFD_API,
            features: wanted,
            ioctls: 0,
        };
        // SAFETY: live fd; the kernel reads and writes exactly one uffdio_api.
        if unsafe { libc::ioctl(uffd.as_raw_fd(), UFFDIO_API, &mut api) } < 0
            || api.features & wanted != wanted
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "userfaultfd asynchronous write-protect unavailable (Linux >= 6.7 with CONFIG_PTE_MARKER_UFFD_WP)",
            ));
        }
        Ok(Self {
            uffd,
            pagemap: File::open("/proc/self/pagemap")?,
            page: crate::hal::page_size(),
            tracked: BTreeMap::new(),
            pending: Vec::new(),
            pages_per_cycle,
            min_interval,
            last_cycle: None,
            cursor: 0,
            rng: 0x9e37_79b9_7f4a_7c15 ^ u64::from(std::process::id()),
        })
    }

    fn protect(&self, page: usize, on: bool) -> io::Result<()> {
        let mut wp = UffdioWriteprotect {
            range: UffdioRange {
                start: page as u64,
                len: self.page as u64,
            },
            mode: if on { UFFDIO_WRITEPROTECT_MODE_WP } else { 0 },
        };
        // SAFETY: live fd; the kernel reads exactly one uffdio_writeprotect.
        if unsafe { libc::ioctl(self.uffd.as_raw_fd(), UFFDIO_WRITEPROTECT, &mut wp) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn still_protected(&self, page: usize) -> io::Result<bool> {
        let mut entry = [0u8; 8];
        self.pagemap
            .read_exact_at(&mut entry, (page / self.page * 8) as u64)?;
        Ok(u64::from_ne_bytes(entry) & PM_UFFD_WP != 0)
    }

    fn next_random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
}

impl WriteSource for WriteSampler {
    /// Register the full pages of `range` for write-protection. A range
    /// without a full page is tracked but never sampled, so it never
    /// produces write-free evidence.
    fn track(&mut self, id: u64, range: Range<usize>) -> io::Result<()> {
        let start = range.start.div_ceil(self.page) * self.page;
        let end = range.end / self.page * self.page;
        if start < end {
            let mut register = UffdioRegister {
                range: UffdioRange {
                    start: start as u64,
                    len: (end - start) as u64,
                },
                mode: UFFDIO_REGISTER_MODE_WP,
                ioctls: 0,
            };
            // SAFETY: live fd; the kernel reads and writes one uffdio_register.
            if unsafe { libc::ioctl(self.uffd.as_raw_fd(), UFFDIO_REGISTER, &mut register) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        self.tracked.insert(id, start..end.max(start));
        Ok(())
    }

    /// Stop tracking. Best effort: the allocation may already be unmapped,
    /// which removes its registration anyway.
    fn untrack(&mut self, id: u64) {
        let pending = std::mem::take(&mut self.pending);
        for (owner, page) in pending {
            if owner == id {
                let _ = self.protect(page, false);
            } else {
                self.pending.push((owner, page));
            }
        }
        if let Some(range) = self.tracked.remove(&id).filter(|r| !r.is_empty()) {
            let mut unregister = UffdioRange {
                start: range.start as u64,
                len: (range.end - range.start) as u64,
            };
            // SAFETY: live fd; the kernel reads exactly one uffdio_range.
            unsafe { libc::ioctl(self.uffd.as_raw_fd(), UFFDIO_UNREGISTER, &mut unregister) };
        }
    }

    /// Report every page protected by the previous cycle, then protect one
    /// random page in each of the next `pages_per_cycle` tracked allocations
    /// (round robin). Does nothing until `min_interval` has passed.
    fn cycle(&mut self) -> io::Result<Vec<WriteCheck>> {
        let now = Instant::now();
        if self
            .last_cycle
            .is_some_and(|last| now.duration_since(last) < self.min_interval)
        {
            return Ok(Vec::new());
        }
        self.last_cycle = Some(now);
        let mut checks = Vec::with_capacity(self.pending.len());
        for (id, page) in std::mem::take(&mut self.pending) {
            let written = !self.still_protected(page)?;
            if !written {
                self.protect(page, false)?;
            }
            checks.push(WriteCheck { id, page, written });
        }
        let ids: Vec<u64> = self
            .tracked
            .range(self.cursor + 1..)
            .chain(self.tracked.range(..=self.cursor))
            .filter(|(_, range)| !range.is_empty())
            .map(|(id, _)| *id)
            .take(self.pages_per_cycle)
            .collect();
        for id in ids {
            let range = self.tracked[&id].clone();
            let pages = ((range.end - range.start) / self.page) as u64;
            let page = range.start + (self.next_random() % pages) as usize * self.page;
            self.protect(page, true)?;
            self.pending.push((id, page));
            self.cursor = id;
        }
        Ok(checks)
    }
}

impl Drop for WriteSampler {
    fn drop(&mut self) {
        for (_, page) in std::mem::take(&mut self.pending) {
            let _ = self.protect(page, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pages(*mut u8, usize);
    impl Pages {
        fn new(n: usize) -> Self {
            let len = n * crate::hal::page_size();
            // SAFETY: new private anonymous mapping; nothing else refers to it.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert_ne!(p, libc::MAP_FAILED);
            // SAFETY: fresh writable mapping of len bytes.
            unsafe { std::ptr::write_bytes(p.cast::<u8>(), 1, len) };
            Self(p.cast(), len)
        }
        fn page(&self, i: usize) -> usize {
            self.0 as usize + i * crate::hal::page_size()
        }
        fn write(&self, i: usize) {
            // SAFETY: i addresses a page inside the owned mapping.
            unsafe { std::ptr::write_volatile((self.page(i) + 8) as *mut u64, 7) };
        }
    }
    impl Drop for Pages {
        fn drop(&mut self) {
            // SAFETY: this object owns the mapping.
            unsafe { libc::munmap(self.0.cast(), self.1) };
        }
    }

    /// Environments without the feature (old kernels, seccomp) are reported,
    /// not silently passed: the test prints why it could not run.
    fn sampler() -> Option<WriteSampler> {
        match WriteSampler::new(8, Duration::ZERO) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("SKIP userfaultfd write-protect unavailable here: {e}");
                None
            }
        }
    }

    #[test]
    fn written_pages_are_reported_and_untouched_pages_are_not() {
        let Some(mut s) = sampler() else { return };
        let page = crate::hal::page_size();
        let (a, b) = (Pages::new(1), Pages::new(1));
        s.track(1, a.page(0)..a.page(0) + page)
            .expect("write sampler");
        s.track(2, b.page(0)..b.page(0) + page)
            .expect("write sampler");
        assert!(s.cycle().expect("write sampler").is_empty());
        for round in 0..4 {
            if round % 2 == 0 {
                a.write(0);
            } else {
                b.write(0);
            }
            let mut checks = s.cycle().expect("write sampler");
            checks.sort_by_key(|c| c.id);
            assert_eq!(
                checks,
                vec![
                    WriteCheck {
                        id: 1,
                        page: a.page(0),
                        written: round % 2 == 0
                    },
                    WriteCheck {
                        id: 2,
                        page: b.page(0),
                        written: round % 2 == 1
                    },
                ],
                "round {round}"
            );
        }
        // Reads never count as writes.
        // SAFETY: reads a page inside the owned mapping.
        let _ = unsafe { std::ptr::read_volatile(a.page(0) as *const u64) };
        assert!(s.cycle().expect("write sampler").iter().all(|c| !c.written));
    }

    #[test]
    fn partial_pages_are_never_sampled_and_untrack_stops_sampling() {
        let Some(mut s) = sampler() else { return };
        let pages = Pages::new(3);
        // No full page inside: tracked, never sampled.
        s.track(1, pages.page(0) + 8..pages.page(1) - 8)
            .expect("write sampler");
        s.track(2, pages.page(1)..pages.page(3))
            .expect("write sampler");
        s.cycle().expect("write sampler");
        let checks = s.cycle().expect("write sampler");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, 2);
        assert!((pages.page(1)..pages.page(3)).contains(&checks[0].page));
        s.untrack(2);
        assert!(s.cycle().expect("write sampler").is_empty());
        assert!(s.cycle().expect("write sampler").is_empty());
    }

    #[test]
    fn budget_and_interval_bound_each_cycle() {
        let Some(_) = sampler() else { return };
        let mut s = WriteSampler::new(2, Duration::from_secs(3600)).expect("write sampler");
        let pages = Pages::new(4);
        for i in 0..4 {
            let start = pages.page(i);
            s.track(i as u64 + 1, start..start + crate::hal::page_size())
                .expect("write sampler");
        }
        assert!(s.cycle().expect("write sampler").is_empty());
        assert_eq!(s.pending.len(), 2);
        // Within min_interval nothing is checked or protected.
        assert!(s.cycle().expect("write sampler").is_empty());
        assert_eq!(s.pending.len(), 2);
        assert!(WriteSampler::new(0, Duration::ZERO).is_err());
    }
}
