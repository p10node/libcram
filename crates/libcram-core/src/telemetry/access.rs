//! Bounded window of samples, not inferred memory-operation totals.
use std::io;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
}
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessCounts {
    pub reads: u64,
    pub writes: u64,
}
use std::collections::{HashMap, VecDeque};
#[derive(Clone, Copy)]
struct Event {
    time: u64,
    page: u64,
    pid: u32,
    kind: AccessKind,
}
/// Keeps samples in (now-window, now], with oldest-first capacity eviction.
/// Timestamps use one monotonic clock and must arrive in nondecreasing order.
/// Counts are unweighted samples, NOT actual load/store operation counts.
pub struct AccessWindow {
    window: u64,
    capacity: usize,
    page_size: u64,
    last: u64,
    events: VecDeque<Event>,
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
            events: VecDeque::new(),
            pages: HashMap::new(),
            capacity_evictions: 0,
        })
    }
    fn remove_oldest(&mut self) {
        if let Some(e) = self.events.pop_front() {
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
            .front()
            .is_some_and(|e| now - e.time >= self.window)
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
        self.advance(time)?;
        if self.events.len() == self.capacity {
            self.remove_oldest();
            self.capacity_evictions = self.capacity_evictions.saturating_add(1);
        }
        let page = address & !(self.page_size - 1);
        let c = self.pages.entry((pid, page)).or_default();
        match kind {
            AccessKind::Read => c.reads += 1,
            AccessKind::Write => c.writes += 1,
        }
        self.events.push_back(Event {
            time,
            page,
            pid,
            kind,
        });
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
    pub fn retained_samples(&self) -> usize {
        self.events.len()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(w.record(11, 0, 7, AccessKind::Read).is_err());
        assert!(AccessWindow::new(0, 1, 4096).is_err());
        assert!(AccessWindow::new(10, 0, 4096).is_err());
        assert!(AccessWindow::new(10, 1, 3000).is_err());
    }
}
