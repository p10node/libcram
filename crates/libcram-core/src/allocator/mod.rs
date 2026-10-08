mod pressure;
use crate::{
    guard::PoisonGuard,
    hal::{page_size, Numa, SystemNuma, Topology},
    Error,
};
pub use pressure::PressureCallback;
use std::{
    collections::BTreeMap,
    ptr::NonNull,
    sync::{Arc, Mutex},
};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessHint {
    Auto = 0,
    ReadHeavy = 1,
    WriteHeavy = 2,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TelemetrySnapshot {
    pub allocated_bytes: u64,
    /// Payload bytes covered by an accepted CRAM allocation policy or a
    /// successful per-page migration result. Not a measurement of physical
    /// compression ratio, committed resident pages, or hardware capacity.
    pub cram_bytes: u64,
    pub allocation_count: u64,
    pub pressure_level: u32,
    pub psi_some_avg10: f64,
    pub psi_full_avg10: f64,
}
pub(crate) struct Mapping {
    base: usize,
    len: usize,
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.base as *mut libc::c_void, self.len) };
    }
}
pub(crate) struct Allocation {
    pub(crate) address: usize,
    pub(crate) size: usize,
    pub(crate) state: Mutex<crate::migration::State>,
    pub(crate) _mapping: Mapping,
    guard: PoisonGuard,
    pub(crate) cram_bytes: std::sync::atomic::AtomicU64,
}
pub(crate) struct Inner {
    pub(crate) allocations: Mutex<BTreeMap<usize, Arc<Allocation>>>,
    pub(crate) topology: Topology,
    pub(crate) numa: Arc<dyn Numa>,
    pub(crate) cursor: Mutex<usize>,
    pub(crate) last_migration: Mutex<Option<crate::migration::MigrationReport>>,
    pub(crate) pressure: pressure::Pressure,
    pub(crate) device: Mutex<crate::guard::device::DeviceHealth>,
    pub(crate) started: std::time::Instant,
}
/// Raw allocations live until deallocated or this allocator is dropped. The caller
/// must stop all pointer accesses before either event. Use owned allocations for
/// allocations that must outlive the allocator handle.
pub struct CramAllocator {
    pub(crate) inner: Arc<Inner>,
    monitor: Mutex<Option<pressure::Monitor>>,
}
impl CramAllocator {
    pub fn new() -> Result<Self, Error> {
        let allocator = Self::with_backend(Topology::discover()?, Arc::new(SystemNuma))?;
        if cfg!(target_os = "linux") {
            allocator.start_monitoring(std::time::Duration::from_millis(250))?;
        }
        Ok(allocator)
    }
    pub fn with_backend(topology: Topology, numa: Arc<dyn Numa>) -> Result<Self, Error> {
        let topology = Topology::from_nodes(topology.nodes)?;
        Ok(Self {
            inner: Arc::new(Inner {
                allocations: Mutex::new(BTreeMap::new()),
                topology,
                numa,
                cursor: Mutex::new(0),
                last_migration: Mutex::new(None),
                pressure: pressure::Pressure::new()?,
                device: Mutex::new(crate::guard::device::DeviceHealth::default()),
                started: std::time::Instant::now(),
            }),
            monitor: Mutex::new(None),
        })
    }
    pub fn allocate(
        &self,
        size: usize,
        align: usize,
        hint: AccessHint,
    ) -> Result<NonNull<u8>, Error> {
        let layout =
            std::alloc::Layout::from_size_align(size, align).map_err(|_| Error::InvalidLayout)?;
        if size == 0 {
            return Err(Error::InvalidLayout);
        }
        let page = page_size();
        let align = layout.align().max(page);
        let mapped_len = size
            .checked_add(PoisonGuard::LEN)
            .and_then(|n| n.checked_add(page - 1))
            .map(|n| n / page * page)
            .ok_or(Error::InvalidLayout)?;
        let len = mapped_len
            .checked_add(align)
            .filter(|n| *n <= isize::MAX as usize)
            .ok_or(Error::InvalidLayout)?;
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(Error::OutOfMemory);
        }
        let mapping = Mapping {
            base: base as usize,
            len,
        };
        let address = (mapping.base + align - 1) & !(align - 1);
        let target = if hint == AccessHint::ReadHeavy && self.inner.pressure_level() < 2 {
            self.inner.topology.cram_node()
        } else {
            self.inner.topology.dram_node()
        };
        let mut cram = false;
        if let Some(node) = target {
            if unsafe { self.inner.numa.bind(address, mapped_len, node) }.is_ok() {
                cram = Some(node) == self.inner.topology.cram_node();
            } else if let Some(dram) = self.inner.topology.dram_node() {
                let _ = unsafe { self.inner.numa.bind(address, mapped_len, dram) };
            }
        }
        let guard = unsafe { PoisonGuard::initialize((address + size) as *mut u8) };
        let record = Arc::new(Allocation {
            address,
            size,
            state: Mutex::new(crate::migration::State {
                sample: None,
                nodes: vec![if cram { target } else { None }; mapped_len / page],
                offset: 0,
            }),
            _mapping: mapping,
            guard,
            cram_bytes: std::sync::atomic::AtomicU64::new(if cram { size as u64 } else { 0 }),
        });
        self.inner
            .allocations
            .lock()
            .map_err(|_| Error::Poisoned)?
            .insert(address, record);
        NonNull::new(address as *mut u8).ok_or(Error::OutOfMemory)
    }
    /// # Safety
    /// All accesses through the pointer and any aliases must have stopped. The pointer
    /// must identify the current allocation, not a stale pointer to a reused address.
    /// A corrupted canary is reported after reclaiming the allocation.
    pub unsafe fn deallocate(&self, ptr: NonNull<u8>) -> Result<(), Error> {
        let record = self
            .inner
            .allocations
            .lock()
            .map_err(|_| Error::Poisoned)?
            .remove(&(ptr.as_ptr() as usize))
            .ok_or(Error::UnknownAllocation)?;
        if unsafe { record.guard.check() } {
            Ok(())
        } else {
            Err(Error::GuardCorrupted)
        }
    }
    pub fn allocate_owned(
        &self,
        size: usize,
        align: usize,
        hint: AccessHint,
    ) -> Result<OwnedAllocation, Error> {
        let ptr = self.allocate(size, align, hint)?;
        let record = self
            .inner
            .allocations
            .lock()
            .map_err(|_| Error::Poisoned)?
            .get(&(ptr.as_ptr() as usize))
            .cloned()
            .ok_or(Error::UnknownAllocation)?;
        Ok(OwnedAllocation {
            record,
            inner: self.inner.clone(),
        })
    }
    pub fn telemetry(&self) -> TelemetrySnapshot {
        let records = self
            .inner
            .allocations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (sample, level) = *self
            .inner
            .pressure
            .sample
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        TelemetrySnapshot {
            allocated_bytes: records.values().map(|r| r.size as u64).sum(),
            cram_bytes: records
                .values()
                .map(|r| r.cram_bytes.load(std::sync::atomic::Ordering::Relaxed))
                .sum(),
            allocation_count: records.len() as u64,
            pressure_level: level.max(self.inner.pressure_level()),
            psi_some_avg10: sample.some.avg10,
            psi_full_avg10: sample.full.avg10,
        }
    }
}

/// Unique payload owner. Migration holds Arc leases to the mapping, never payload
/// references. Mutable byte access requires an exclusive Rust borrow.
pub struct OwnedAllocation {
    record: Arc<Allocation>,
    inner: Arc<Inner>,
}
impl OwnedAllocation {
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.record.address as *const u8, self.record.size) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.record.address as *mut u8, self.record.size) }
    }
    pub fn record_sample(&self, sample: crate::migration::AccessSample) -> Result<(), Error> {
        sample.validate()?;
        let mut state = self.record.state.lock().map_err(|_| Error::Poisoned)?;
        state.sample = Some(sample);
        state.offset = 0;
        Ok(())
    }
    /// Explicitly profile at most the first 4096 bytes under a shared borrow.
    /// The caller supplies genuine access counts; the allocator never invents them.
    /// This prefix estimate need not represent the entropy of the entire allocation.
    pub fn profile(
        &self,
        reads: u64,
        writes: u64,
    ) -> Result<crate::migration::AccessSample, Error> {
        let data = self.as_slice();
        let sample = crate::migration::AccessSample {
            reads,
            writes,
            entropy: crate::entropy::shannon_entropy(&data[..data.len().min(4096)]),
        };
        self.record_sample(sample)?;
        Ok(sample)
    }
    pub fn check_guard(&self) -> Result<(), Error> {
        if unsafe { self.record.guard.check() } {
            Ok(())
        } else {
            Err(Error::GuardCorrupted)
        }
    }
}
impl Drop for OwnedAllocation {
    fn drop(&mut self) {
        self.inner
            .allocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.record.address);
    }
}
