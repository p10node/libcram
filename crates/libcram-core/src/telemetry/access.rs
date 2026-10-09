//! Bounded window of samples, not inferred memory-operation totals.
use std::io;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessKind {
    Read,
    Write,
}
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessCounts {
    pub reads: u64,
    pub writes: u64,
}
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    time: u64,
    page: u64,
    pid: u32,
    kind: AccessKind,
}
/// Keeps samples in (now-window, now], with oldest-first capacity eviction.
/// Sources share one monotonic clock. Samples may arrive late within the current
/// window; queries/expiry watermarks cannot move backwards.
/// Counts are unweighted samples, NOT actual load/store operation counts.
pub struct AccessWindow {
    window: u64,
    capacity: usize,
    page_size: u64,
    last: u64,
    events: BinaryHeap<Reverse<Event>>,
    pages: HashMap<(u32, u64), AccessCounts>,
    pub capacity_evictions: u64,
}
impl AccessWindow {
    pub fn new(window: u64, capacity: usize, page_size: u64) -> io::Result<Self> {
        if window == 0 || capacity == 0 || !page_size.is_power_of_two() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "nonzero window/capacity and power-of-two page size required",
            ));
        }
        Ok(Self {
            window,
            capacity,
            page_size,
            last: 0,
            events: BinaryHeap::new(),
            pages: HashMap::new(),
            capacity_evictions: 0,
        })
    }
    fn remove_oldest(&mut self) {
        if let Some(Reverse(e)) = self.events.pop() {
            let key = (e.pid, e.page);
            if let Some(c) = self.pages.get_mut(&key) {
                match e.kind {
                    AccessKind::Read => c.reads -= 1,
                    AccessKind::Write => c.writes -= 1,
                }
                if c.reads == 0 && c.writes == 0 {
                    self.pages.remove(&key);
                }
            }
        }
    }
    pub fn advance(&mut self, now: u64) -> io::Result<()> {
        if now < self.last {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "out-of-order timestamp",
            ));
        }
        self.last = now;
        while self
            .events
            .peek()
            .is_some_and(|Reverse(e)| now - e.time >= self.window)
        {
            self.remove_oldest();
        }
        Ok(())
    }
    pub fn record(
        &mut self,
        time: u64,
        address: u64,
        pid: u32,
        kind: AccessKind,
    ) -> io::Result<()> {
        self.advance(time.max(self.last))?;
        if self.last - time >= self.window {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expired sample",
            ));
        }
        let page = address & !(self.page_size - 1);
        let c = self.pages.entry((pid, page)).or_default();
        match kind {
            AccessKind::Read => c.reads += 1,
            AccessKind::Write => c.writes += 1,
        }
        self.events.push(Reverse(Event {
            time,
            page,
            pid,
            kind,
        }));
        if self.events.len() > self.capacity {
            self.remove_oldest();
            self.capacity_evictions = self.capacity_evictions.saturating_add(1);
        }
        Ok(())
    }
    pub fn counts(&mut self, now: u64, address: u64, pid: u32) -> io::Result<AccessCounts> {
        self.advance(now)?;
        Ok(self
            .pages
            .get(&(pid, address & !(self.page_size - 1)))
            .copied()
            .unwrap_or_default())
    }
    /// Snapshot retained per-process page counts after advancing the expiry clock.
    /// Sorted by process/page identity; at most capacity entries are returned.
    pub fn snapshot(&mut self, now: u64) -> io::Result<Vec<(u32, u64, AccessCounts)>> {
        self.advance(now)?;
        let mut result: Vec<_> = self
            .pages
            .iter()
            .map(|(&(pid, page), &counts)| (pid, page, counts))
            .collect();
        result.sort_unstable_by_key(|&(pid, page, _)| (pid, page));
        Ok(result)
    }
    pub fn retained_samples(&self) -> usize {
        self.events.len()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_samples_from_interleaved_rings_survive_query_and_expire_in_time_order() {
        let mut w = AccessWindow::new(30, 4, 4096).expect("window");
        w.record(100, 4096, 1, AccessKind::Read).expect("ring A");
        w.record(120, 4096, 1, AccessKind::Read).expect("ring A");
        assert_eq!(w.counts(125, 4096, 1).expect("query").reads, 2);
        w.record(110, 4096, 1, AccessKind::Write)
            .expect("delayed ring B");
        assert_eq!(
            w.counts(125, 4096, 1).expect("query"),
            AccessCounts {
                reads: 2,
                writes: 1
            }
        );
        assert_eq!(
            w.counts(130, 4096, 1).expect("expiry"),
            AccessCounts {
                reads: 1,
                writes: 1
            }
        );
        assert_eq!(
            w.counts(140, 4096, 1).expect("expiry"),
            AccessCounts {
                reads: 1,
                writes: 0
            }
        );
        assert!(w.record(110, 4096, 1, AccessKind::Write).is_err());
        assert!(w.counts(139, 4096, 1).is_err());
    }
    #[test]
    fn snapshots_preserve_identity_counts_and_expiry() {
        let mut w = AccessWindow::new(10, 8, 4096).expect("window");
        w.record(1, 0x1001, 9, AccessKind::Read).expect("read");
        w.record(2, 0x3001, 7, AccessKind::Write).expect("write");
        assert_eq!(
            w.snapshot(2).expect("snapshot"),
            vec![
                (
                    7,
                    0x3000,
                    AccessCounts {
                        reads: 0,
                        writes: 1
                    }
                ),
                (
                    9,
                    0x1000,
                    AccessCounts {
                        reads: 1,
                        writes: 0
                    }
                )
            ]
        );
        assert_eq!(
            w.snapshot(11).expect("expired"),
            vec![(
                7,
                0x3000,
                AccessCounts {
                    reads: 0,
                    writes: 1
                }
            )]
        );
        assert!(w.snapshot(10).is_err());
        assert!(w.snapshot(12).expect("empty").is_empty());
    }
    #[test]
    fn aligned_window_eviction() {
        let mut w = AccessWindow::new(10, 2, 4096).expect("valid");
        w.record(1, 0x1001, 7, AccessKind::Read).expect("record");
        assert_eq!(w.counts(1, 0x1fff, 7).expect("counts").reads, 1);
        w.record(2, 0x1002, 7, AccessKind::Write).expect("record");
        assert_eq!(
            w.counts(2, 0x1000, 8).expect("counts"),
            AccessCounts::default()
        );
        w.record(3, 0x2000, 7, AccessKind::Read).expect("record");
        assert_eq!(
            w.counts(3, 0x1000, 7).expect("counts"),
            AccessCounts {
                reads: 0,
                writes: 1
            }
        );
        assert_eq!(
            w.counts(12, 0x1000, 7).expect("counts"),
            AccessCounts::default()
        );
        assert!(w.record(11, 0, 7, AccessKind::Read).is_ok());
        assert!(w.record(2, 0, 7, AccessKind::Read).is_err());
        assert!(AccessWindow::new(0, 1, 4096).is_err());
        assert!(AccessWindow::new(10, 0, 4096).is_err());
        assert!(AccessWindow::new(10, 1, 3000).is_err());
    }
}
