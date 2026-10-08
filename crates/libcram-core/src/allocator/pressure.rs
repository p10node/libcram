use super::{CramAllocator, Inner};
use crate::{telemetry::PsiSample, Error};
use std::{
    sync::{
        mpsc::{self, SyncSender},
        Arc, Mutex, Weak,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub type PressureCallback = Arc<dyn Fn(u32) + Send + Sync>;
type CallbackSlot = Arc<Mutex<(u64, Option<PressureCallback>)>>;
pub(crate) struct Pressure {
    pub sample: Mutex<(PsiSample, u32)>,
    callback: CallbackSlot,
    tx: SyncSender<(u64, u32)>,
}
impl Pressure {
    pub fn new() -> Result<Self, Error> {
        let callback: CallbackSlot = Arc::new(Mutex::new((0, None)));
        let callbacks = callback.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("cram-callback".into())
            .spawn(move || {
                while let Ok((generation, level)) = rx.recv() {
                    let cb = {
                        let slot = callbacks.lock().unwrap_or_else(|e| e.into_inner());
                        if slot.0 == generation {
                            slot.1.clone()
                        } else {
                            None
                        }
                    };
                    if let Some(cb) = cb {
                        let _ =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(level)));
                    }
                }
            })?;
        Ok(Self {
            sample: Mutex::new((PsiSample::default(), 0)),
            callback,
            tx,
        })
    }
    pub fn submit(&self, sample: PsiSample) -> Result<(), Error> {
        for value in [
            sample.some.avg10,
            sample.some.avg60,
            sample.some.avg300,
            sample.full.avg10,
            sample.full.avg60,
            sample.full.avg300,
        ] {
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err(Error::InvalidSample);
            }
        }
        let level = if sample.full.avg10 >= 5.0 || sample.some.avg10 >= 10.0 {
            2
        } else if sample.some.avg10 >= 1.0 || sample.full.avg10 > 0.0 {
            1
        } else {
            0
        };
        *self.sample.lock().map_err(|_| Error::Poisoned)? = (sample, level);
        Ok(())
    }
    pub(crate) fn notify(&self, level: u32) -> Result<(), Error> {
        let generation = self.callback.lock().map_err(|_| Error::Poisoned)?.0;
        // Bounded, best-effort notifications: slow callbacks cannot block allocation or monitoring.
        let _ = self.tx.try_send((generation, level));
        Ok(())
    }
}
pub(crate) struct Monitor {
    stop: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
}
impl Monitor {
    pub fn start(inner: Weak<Inner>, interval: Duration) -> Result<Self, Error> {
        if interval < Duration::from_millis(10) {
            return Err(Error::InvalidSample);
        }
        let (stop, rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("cram-monitor".into())
            .spawn(move || {
                while matches!(
                    rx.recv_timeout(interval),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let Some(inner) = inner.upgrade() else { break };
                    if let Ok(sample) = crate::telemetry::read_memory_pressure() {
                        let _ = inner.pressure.submit(sample);
                    }
                    let _ = inner.pressure.notify(inner.pressure_level());
                    let report = match crate::migration::step(&inner, 32, 256) {
                        Ok(report) => report,
                        Err(error) => crate::migration::MigrationReport {
                            syscall_errors: vec![error.to_string()],
                            ..Default::default()
                        },
                    };
                    if report.pages_attempted > 0 || !report.syscall_errors.is_empty() {
                        *inner
                            .last_migration
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(report);
                    }
                }
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
impl Inner {
    pub(crate) fn pressure_level(&self) -> u32 {
        let psi = self
            .pressure
            .sample
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .1;
        let device = if self.topology.cram_node().is_some() {
            self.device
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .level(std::time::Instant::now()) as u32
        } else {
            0
        };
        psi.max(device)
    }
}
impl CramAllocator {
    /// Supply trusted physical device occupancy. Reports expire after one second.
    /// Unknown/zero capacity blocks CRAM; process allocation bytes are not a valid substitute.
    pub fn submit_device_usage(&self, used: u64, capacity: u64) -> Result<(), Error> {
        let valid = self
            .inner
            .device
            .lock()
            .map_err(|_| Error::Poisoned)?
            .observe_usage(used, capacity, std::time::Instant::now())
            .is_some();
        self.inner.pressure.notify(self.inner.pressure_level())?;
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidSample)
        }
    }
    /// Ingest a hardware adapter's physical page identity. Three distinct reports
    /// within one second latch Emergency for this allocator's lifetime.
    pub fn report_poison(&self, page_id: u64) -> Result<(), Error> {
        let elapsed = self
            .inner
            .started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        self.inner
            .device
            .lock()
            .map_err(|_| Error::Poisoned)?
            .report_poison(elapsed, page_id)
            .ok_or(Error::InvalidSample)?;
        self.inner.pressure.notify(self.inner.pressure_level())
    }

    /// Replaces the callback. Queued old-generation notifications are discarded;
    /// an already selected/in-flight callback may finish AFTER this call or Drop.
    /// Not a quiescence barrier. Captured state must remain valid through invocation;
    /// use Weak allocator references to avoid reference cycles. Callback panics are
    /// contained. Queue capacity is one and notifications can be dropped under load.
    pub fn register_pressure_callback(
        &self,
        callback: Option<PressureCallback>,
    ) -> Result<(), Error> {
        let mut slot = self
            .inner
            .pressure
            .callback
            .lock()
            .map_err(|_| Error::Poisoned)?;
        slot.0 = slot.0.wrapping_add(1);
        slot.1 = callback;
        Ok(())
    }
    pub fn submit_pressure(&self, sample: PsiSample) -> Result<(), Error> {
        self.inner.pressure.submit(sample)?;
        self.inner.pressure.notify(self.inner.pressure_level())
    }
    /// Starts at most one bounded monitor. Custom/mock backends are opt-in; new()
    /// enables this automatically on Linux. Drop joins monitoring, not callbacks.
    pub fn start_monitoring(&self, interval: Duration) -> Result<(), Error> {
        let mut monitor = self.monitor.lock().map_err(|_| Error::Poisoned)?;
        if monitor.is_none() {
            *monitor = Some(Monitor::start(Arc::downgrade(&self.inner), interval)?);
        }
        Ok(())
    }
}
