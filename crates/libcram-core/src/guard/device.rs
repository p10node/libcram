//! Device pressure policy: actual device occupancy and explicit hardware poison reports.
#![deny(clippy::unwrap_used)]

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    Normal = 0,
    High = 1,
    Critical = 2,
    Emergency = 3,
}

/// Watermarks use physical device occupancy, not this process's allocation sum.
/// Zero capacity is unknown and must be handled conservatively by the caller.
pub fn capacity_level(used: u64, capacity: u64) -> Option<PressureLevel> {
    if capacity == 0 {
        return None;
    }
    let percent_numerator = u128::from(used) * 100;
    let capacity = u128::from(capacity);
    Some(if percent_numerator < capacity * 70 {
        PressureLevel::Normal
    } else if percent_numerator < capacity * 85 {
        PressureLevel::High
    } else if percent_numerator <= capacity * 95 {
        PressureLevel::Critical
    } else {
        PressureLevel::Emergency
    })
}

/// Bounded unique-page storm detector. A trip is latched until the guard is replaced
/// after explicit device requalification; elapsed time alone does not restore safety.
/// It consumes reports from a hardware adapter; it does not install a SIGBUS handler.
pub struct PoisonStormGuard {
    window_ms: u64,
    threshold: usize,
    history: std::collections::VecDeque<(u64, u64)>,
    last_timestamp: u64,
    tripped: bool,
}
impl PoisonStormGuard {
    pub fn new(window_ms: u64, threshold: usize) -> Option<Self> {
        if window_ms == 0 || threshold == 0 || threshold > 4096 {
            return None;
        }
        Some(Self {
            window_ms,
            threshold,
            history: std::collections::VecDeque::new(),
            last_timestamp: 0,
            tripped: false,
        })
    }
    /// Returns None for time regression without mutating state. Page IDs are hardware
    /// frame identities, not process virtual addresses. Duplicate reports do not count twice.
    pub fn record(&mut self, timestamp_ms: u64, page_id: u64) -> Option<bool> {
        if timestamp_ms < self.last_timestamp {
            return None;
        }
        self.last_timestamp = timestamp_ms;
        if self.tripped {
            return Some(true);
        }
        while self
            .history
            .front()
            .is_some_and(|&(time, _)| timestamp_ms - time >= self.window_ms)
        {
            self.history.pop_front();
        }
        if !self.history.iter().any(|&(_, page)| page == page_id) {
            self.history.push_back((timestamp_ms, page_id));
        }
        if self.history.len() >= self.threshold {
            self.tripped = true;
            self.history.clear();
        }
        Some(self.tripped)
    }
    pub fn tripped(&self) -> bool {
        self.tripped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_watermark_edges_and_invalid_capacity() {
        assert_eq!(capacity_level(0, 0), None);
        for (used, expected) in [
            (0, PressureLevel::Normal),
            (69, PressureLevel::Normal),
            (70, PressureLevel::High),
            (84, PressureLevel::High),
            (85, PressureLevel::Critical),
            (95, PressureLevel::Critical),
            (96, PressureLevel::Emergency),
            (101, PressureLevel::Emergency),
        ] {
            assert_eq!(capacity_level(used, 100), Some(expected));
        }
        assert_eq!(
            capacity_level(u64::MAX, u64::MAX),
            Some(PressureLevel::Emergency)
        );
    }
    #[test]
    fn storm_counts_unique_pages_expires_and_latches() {
        assert!(PoisonStormGuard::new(0, 3).is_none());
        assert!(PoisonStormGuard::new(100, 0).is_none());
        assert!(PoisonStormGuard::new(100, 4097).is_none());
        let mut guard = PoisonStormGuard::new(100, 3).expect("valid config");
        assert_eq!(guard.record(0, 10), Some(false));
        assert_eq!(guard.record(1, 10), Some(false));
        assert_eq!(guard.record(2, 20), Some(false));
        assert_eq!(guard.record(101, 30), Some(false));
        assert_eq!(guard.record(100, 40), None);
        assert_eq!(guard.record(102, 40), Some(false));
        assert_eq!(guard.record(103, 50), Some(true));
        assert!(guard.tripped());
        assert_eq!(guard.record(500, 60), Some(true));
    }
}
