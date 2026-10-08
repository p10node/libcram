//! Migration uses caller-submitted metadata only. It never samples live payload
//! bytes: doing so through an alias while a caller writes would be undefined behavior.
use crate::{
    allocator::{CramAllocator, Inner},
    hal::{page_size, PageStatus},
    Error,
};
use std::sync::atomic::Ordering;
#[derive(Clone, Copy, Debug)]
pub struct AccessSample {
    pub reads: u64,
    pub writes: u64,
    pub entropy: f64,
}
impl AccessSample {
    pub fn validate(&self) -> Result<(), Error> {
        if !self.entropy.is_finite() || !(0.0..=8.0).contains(&self.entropy) {
            Err(Error::InvalidSample)
        } else {
            Ok(())
        }
    }
    fn compressed(&self) -> bool {
        self.entropy < 4.5 && self.reads > 0 && self.reads / 4 > self.writes
    }
}
pub(crate) struct State {
    pub sample: Option<AccessSample>,
    pub nodes: Vec<Option<u32>>,
    pub offset: usize,
}
#[derive(Clone, Debug, Default)]
pub struct MigrationReport {
    pub pages_attempted: usize,
    pub pages_moved: usize,
    pub page_errors: Vec<(usize, i32)>,
    pub syscall_errors: Vec<String>,
}
impl CramAllocator {
    /// Last nonempty background attempt, including bounded per-page failures.
    /// Addresses are diagnostic values only and do not extend allocation lifetime.
    pub fn last_background_migration(&self) -> Result<Option<MigrationReport>, Error> {
        Ok(self
            .inner
            .last_migration
            .lock()
            .map_err(|_| Error::Poisoned)?
            .clone())
    }
    /// A bounded synchronous step, also used by the background monitor. Errors are
    /// retained per page; a partially migrated allocation is never marked all-CRAM.
    pub fn migrate_once(
        &self,
        max_allocations: usize,
        max_pages: usize,
    ) -> Result<MigrationReport, Error> {
        step(&self.inner, max_allocations, max_pages)
    }
    /// Submit an independently synchronized sample; this method does not read `ptr`.
    pub fn record_sample(
        &self,
        ptr: std::ptr::NonNull<u8>,
        sample: AccessSample,
    ) -> Result<(), Error> {
        sample.validate()?;
        let records = self.inner.allocations.lock().map_err(|_| Error::Poisoned)?;
        let r = records
            .get(&(ptr.as_ptr() as usize))
            .ok_or(Error::UnknownAllocation)?;
        let mut state = r.state.lock().map_err(|_| Error::Poisoned)?;
        state.sample = Some(sample);
        state.offset = 0;
        Ok(())
    }
}
pub(crate) fn step(
    inner: &Inner,
    max_allocations: usize,
    max_pages: usize,
) -> Result<MigrationReport, Error> {
    let mut report = MigrationReport::default();
    let mut cursor = inner.cursor.lock().map_err(|_| Error::Poisoned)?;
    let records = {
        let registry = inner.allocations.lock().map_err(|_| Error::Poisoned)?;
        use std::ops::Bound::{Excluded, Unbounded};
        registry
            .range((Excluded(*cursor), Unbounded))
            .chain(registry.range(..=*cursor))
            .take(max_allocations.min(128))
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>()
    };
    let page = page_size();
    let mut remaining = max_pages.min(4096);
    for record in records {
        if remaining == 0 {
            break;
        }
        *cursor = record.address;
        let mut state = record.state.lock().map_err(|_| Error::Poisoned)?;
        let pressured = inner.pressure_level() >= 2;
        if state.sample.is_none() && !pressured {
            continue;
        }
        let target = if !pressured && state.sample.is_some_and(|s| s.compressed()) {
            inner.topology.cram_node()
        } else {
            inner.topology.dram_node()
        };
        let Some(target) = target else { continue };
        // Start at zero on a policy reversal, so previously promoted pages are reclaimed first.
        let start = state.offset;
        let count = remaining.min(state.nodes.len() - start);
        state.offset = (start + count) % state.nodes.len();
        remaining -= count;
        if state.nodes[start..start + count]
            .iter()
            .all(|n| *n == Some(target))
        {
            continue;
        }
        let cram = inner.topology.cram_node();
        let mut bytes = record.cram_bytes.load(Ordering::Relaxed);
        let address = record.address + start * page;
        report.pages_attempted += count;
        let result = unsafe { inner.numa.move_pages(address, count * page, target) };
        match result {
            Ok(result) if result.pages.len() == count => {
                for (i, status) in result.pages.into_iter().enumerate() {
                    match status {
                        PageStatus::Node(node) => {
                            let payload =
                                record.size.saturating_sub((start + i) * page).min(page) as u64;
                            if cram.is_some() && state.nodes[start + i] == cram {
                                bytes -= payload;
                            }
                            state.nodes[start + i] = Some(node);
                            if Some(node) == cram {
                                bytes += payload;
                            }
                            if node == target {
                                report.pages_moved += 1;
                            }
                        }
                        PageStatus::Error(errno) => {
                            report.page_errors.push((address + i * page, errno))
                        }
                    }
                }
            }
            Ok(_) => report
                .syscall_errors
                .push("backend returned incorrect page count".into()),
            Err(e) => report.syscall_errors.push(e.to_string()),
        }
        record.cram_bytes.store(bytes, Ordering::Relaxed);
    }
    Ok(report)
}
