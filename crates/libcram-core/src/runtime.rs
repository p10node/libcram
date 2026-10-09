//! Explicit opt-in managed collection. No payload aliases are ever sampled.
use crate::{
    allocator::{AccessHint, Allocation, CramAllocator, OwnedAllocation, TelemetrySnapshot},
    migration::AccessSample,
    Error,
};
use std::{
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct ObservationToken {
    record: Weak<Allocation>,
    entropy: f64,
    created: Instant,
}
impl ObservationToken {
    pub(crate) fn new(record: &Arc<Allocation>, entropy: f64) -> Result<Self, Error> {
        AccessSample {
            reads: 0,
            writes: 0,
            entropy,
        }
        .validate()?;
        Ok(Self {
            record: Arc::downgrade(record),
            entropy,
            created: Instant::now(),
        })
    }
}
pub struct Observation {
    pub token: ObservationToken,
    pub address: usize,
    pub reads: u64,
    pub writes: u64,
    pub observed_at: Instant,
}
pub struct DeviceUpdate {
    pub used_bytes: u64,
    pub capacity_bytes: u64,
    pub poison_pages: Vec<u64>,
}
/// Calls must be nonblocking and bounded. Implementations own their bounded queues.
pub trait Collector: Send + 'static {
    fn poll(&mut self) -> Result<Option<Observation>, Error>;
    fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error>;
}
pub struct RuntimeConfig {
    pub max_observations: usize,
    pub max_pages: usize,
    pub sample_ttl: Duration,
    pub interval: Duration,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_observations: 32,
            max_pages: 256,
            sample_ttl: Duration::from_secs(1),
            interval: Duration::from_millis(100),
        }
    }
}
pub struct ManagedRuntime {
    allocator: Arc<CramAllocator>,
    status: Arc<Mutex<RuntimeStatus>>,
    stop: std::sync::mpsc::Sender<()>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl ManagedRuntime {
    pub fn allocate_owned(
        &self,
        size: usize,
        align: usize,
        hint: AccessHint,
    ) -> Result<OwnedAllocation, Error> {
        self.allocator.allocate_owned(size, align, hint)
    }
    pub fn status(&self) -> RuntimeStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn telemetry(&self) -> TelemetrySnapshot {
        self.allocator.telemetry()
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        let _ = self.stop.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| Error::Poisoned)?;
        }
        Ok(())
    }
}
impl Drop for ManagedRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
#[derive(Clone, Debug, Default)]
pub struct RuntimeStatus {
    pub ticks: u64,
    pub accepted: u64,
    pub dropped: u64,
    pub errors: u64,
    pub degraded_ticks: u64,
    pub degraded: bool,
    pub stopped: bool,
    pub pages_attempted: u64,
    pub pages_moved: u64,
}
pub struct Runtime<C: Collector> {
    allocator: Arc<CramAllocator>,
    source: C,
    shared_status: Arc<Mutex<RuntimeStatus>>,
    config: RuntimeConfig,
    status: RuntimeStatus,
}
impl<C: Collector> Drop for Runtime<C> {
    fn drop(&mut self) {
        self.status.stopped = true;
        *self.shared_status.lock().unwrap_or_else(|e| e.into_inner()) = self.status.clone();
    }
}
impl<C: Collector> Runtime<C> {
    pub fn new(
        mut allocator: CramAllocator,
        source: C,
        config: RuntimeConfig,
    ) -> Result<Self, Error> {
        if !(1..=128).contains(&config.max_observations)
            || !(1..=4096).contains(&config.max_pages)
            || config.sample_ttl.is_zero()
            || config.interval < Duration::from_millis(10)
        {
            return Err(Error::InvalidSample);
        }
        allocator.stop_monitoring()?;
        // Permanently transfer metadata admission to tokens. Existing owned handles
        // may outlive the worker, but cannot reintroduce undated legacy samples.
        allocator
            .inner
            .managed
            .store(true, std::sync::atomic::Ordering::Release);
        for record in allocator
            .inner
            .allocations
            .lock()
            .map_err(|_| Error::Poisoned)?
            .values()
        {
            record.state.lock().map_err(|_| Error::Poisoned)?.sample = None;
        }
        Ok(Self {
            allocator: Arc::new(allocator),
            shared_status: Arc::new(Mutex::new(RuntimeStatus::default())),
            source,
            config,
            status: RuntimeStatus::default(),
        })
    }
    pub fn start(self) -> Result<ManagedRuntime, Error> {
        let allocator = self.allocator.clone();
        let status = self.shared_status.clone();
        let (stop, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("cram-runtime".into())
            .spawn(move || {
                let mut runtime = self;
                loop {
                    if rx.try_recv().is_ok() {
                        break;
                    }
                    let _ = runtime.tick();
                    if !matches!(
                        rx.recv_timeout(runtime.config.interval),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
            })?;
        Ok(ManagedRuntime {
            allocator,
            status,
            stop,
            handle: Some(handle),
        })
    }
    pub fn allocate_owned(
        &self,
        size: usize,
        align: usize,
        hint: AccessHint,
    ) -> Result<OwnedAllocation, Error> {
        self.allocator.allocate_owned(size, align, hint)
    }
    pub fn telemetry(&self) -> TelemetrySnapshot {
        self.allocator.telemetry()
    }
    pub fn status(&self) -> RuntimeStatus {
        self.status.clone()
    }
    pub fn tick(&mut self) -> Result<(), Error> {
        self.status.ticks = self.status.ticks.saturating_add(1);
        let mut rejected_update = false;
        let result = self.tick_inner(&mut rejected_update);
        if result.is_err() {
            self.status.errors = self.status.errors.saturating_add(1);
            if !rejected_update {
                let _ = self.allocator.submit_device_usage(0, 0);
            }
        }
        self.status.degraded = result.is_err() || self.allocator.inner.pressure_level() >= 2;
        if self.status.degraded {
            self.status.degraded_ticks = self.status.degraded_ticks.saturating_add(1);
        }
        *self.shared_status.lock().unwrap_or_else(|e| e.into_inner()) = self.status.clone();
        result
    }
    fn tick_inner(&mut self, rejected_update: &mut bool) -> Result<(), Error> {
        if let Some(update) = self.source.device_update()? {
            if update.poison_pages.len() > 64 {
                *rejected_update = true;
                return Err(Error::InvalidSample);
            }
            self.allocator
                .submit_device_usage(update.used_bytes, update.capacity_bytes)?;
            for page in update.poison_pages {
                self.allocator.report_poison(page)?;
            }
        }
        let mut batch = Vec::with_capacity(self.config.max_observations);
        for _ in 0..self.config.max_observations {
            let Some(observation) = self.source.poll()? else {
                break;
            };
            batch.push(observation);
        }
        let mut remaining = self.config.max_pages;
        for observation in batch {
            if remaining == 0 {
                self.status.dropped = self.status.dropped.saturating_add(1);
                continue;
            }
            let now = Instant::now();
            if now
                .checked_duration_since(observation.observed_at)
                .is_none_or(|age| age > self.config.sample_ttl)
                || observation.observed_at < observation.token.created
                || now.duration_since(observation.token.created) > self.config.sample_ttl
            {
                self.status.dropped = self.status.dropped.saturating_add(1);
                continue;
            }
            let Some(record) = observation.token.record.upgrade() else {
                self.status.dropped = self.status.dropped.saturating_add(1);
                continue;
            };
            let registry = self
                .allocator
                .inner
                .allocations
                .lock()
                .map_err(|_| Error::Poisoned)?;
            if !registry
                .get(&record.address)
                .is_some_and(|r| Arc::ptr_eq(r, &record))
                || observation.address < record.address
                || observation.address - record.address >= record.size
            {
                self.status.dropped = self.status.dropped.saturating_add(1);
                continue;
            }
            {
                let mut state = record.state.lock().map_err(|_| Error::Poisoned)?;
                state.sample = Some(AccessSample {
                    reads: observation.reads,
                    writes: observation.writes,
                    entropy: observation.token.entropy,
                });
                state.offset = (observation.address - record.address) / crate::hal::page_size();
            }
            let report = crate::migration::step_records(
                &self.allocator.inner,
                vec![record.clone()],
                remaining.min(1),
                &mut 0,
            );
            record.state.lock().map_err(|_| Error::Poisoned)?.sample = None;
            let report = report?;
            self.status.accepted = self.status.accepted.saturating_add(1);
            self.status.pages_attempted = self
                .status
                .pages_attempted
                .saturating_add(report.pages_attempted as u64);
            self.status.pages_moved = self
                .status
                .pages_moved
                .saturating_add(report.pages_moved as u64);
            remaining = remaining.saturating_sub(report.pages_attempted);
        }
        Ok(())
    }
}
