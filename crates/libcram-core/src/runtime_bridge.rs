//! Bounded raw-access/device bridge. Counts are samples, not operation totals.
use crate::{
    device_adapter::DeviceReading,
    runtime::{Collector, DeviceUpdate, Observation, ObservationToken},
    telemetry::{access::AccessKind, sampling::AccessEvent},
    Error,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
    time::{Duration, Instant},
};
const WINDOW: Duration = Duration::from_millis(100);
/// Write evidence older than this is discarded.
pub const WRITE_EVIDENCE_WINDOW: Duration = Duration::from_secs(10);
/// Write-free checks required, within the window, before reads may count as
/// read-dominant under write tracking.
pub const MIN_WRITE_FREE_CHECKS: usize = 3;
const MAX_CHECKS_PER_ALLOCATION: usize = 16;
/// One sampled page, checked for writes at the end of a sampling cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteCheck {
    /// The id passed to [`WriteSource::track`].
    pub id: u64,
    pub page: usize,
    pub written: bool,
}
/// Unbiased write evidence for tracked allocations (see
/// `telemetry::write_sampler::WriteSampler`). Implementations must bound their
/// work per cycle and report failure as an error.
pub trait WriteSource: Send + 'static {
    /// Start tracking writes to `range` under `id`.
    fn track(&mut self, id: u64, range: std::ops::Range<usize>) -> io::Result<()>;
    /// Stop tracking `id`; must not fail.
    fn untrack(&mut self, id: u64);
    /// Pages checked since the previous call.
    fn cycle(&mut self) -> io::Result<Vec<WriteCheck>>;
}
/// Implementations must bound allocation/work and report loss as an error.
pub trait RawSource: Send + 'static {
    fn read_events(&mut self) -> io::Result<Vec<AccessEvent>>;
}
/// Success must represent a new trusted physical snapshot, never cached health.
/// Fixture implementations are software policy evidence, not hardware evidence.
pub trait DeviceSource: Send + 'static {
    fn read_device(&mut self) -> io::Result<DeviceReading>;
}
#[cfg(all(target_os = "linux", feature = "ebpf"))]
impl RawSource for crate::telemetry::process_pebs::ProcessPebsCollector {
    fn read_events(&mut self) -> io::Result<Vec<AccessEvent>> {
        self.poll()
    }
}
#[cfg(unix)]
impl DeviceSource for crate::device_adapter::DirectoryDeviceAdapter {
    fn read_device(&mut self) -> io::Result<DeviceReading> {
        crate::device_adapter::DeviceAdapter::read(self)
    }
}
/// Read the raw source's CLOCK_MONOTONIC domain (not wall clock).
pub fn monotonic_ns() -> io::Result<u64> {
    #[cfg(unix)]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: valid writable timespec; clock_gettime retains no pointer.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return Err(io::Error::last_os_error());
        }
        u64::try_from(ts.tv_sec)
            .ok()
            .and_then(|s| s.checked_mul(1_000_000_000))
            .and_then(|s| s.checked_add(ts.tv_nsec as u64))
            .ok_or_else(|| io::Error::other("monotonic clock overflow"))
    }
    #[cfg(not(unix))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "CLOCK_MONOTONIC unavailable",
        ))
    }
}
#[derive(Clone, Copy)]
pub struct BridgeConfig {
    pub max_registrations: usize,
    pub input_capacity: usize,
    pub max_events: usize,
    pub max_history: usize,
    pub max_output: usize,
    pub token_ttl: Duration,
}
impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            max_registrations: 128,
            input_capacity: 128,
            max_events: 8192,
            max_history: 8192,
            max_output: 32,
            token_ttl: Duration::from_secs(1),
        }
    }
}
/// Nonblocking metadata publication. Full queue irreversibly latches loss;
/// construct a new bridge to resume. Successful enqueue is not registration ACK.
#[derive(Clone)]
pub struct Publisher {
    tx: SyncSender<ObservationToken>,
    lost: Arc<AtomicBool>,
}
impl Publisher {
    pub fn register(&self, token: ObservationToken) -> Result<(), Error> {
        if self.lost.load(Ordering::Acquire) {
            return Err(Error::InvalidSample);
        }
        self.tx.try_send(token).map_err(|_| {
            self.lost.store(true, Ordering::Release);
            Error::InvalidSample
        })
    }
}
struct Registered {
    id: u64,
    token: ObservationToken,
}
/// Write evidence of one allocation, kept across token refreshes.
struct WriteTrack {
    key: u64,
    token: ObservationToken,
    checks: VecDeque<(Instant, bool)>,
}
impl WriteTrack {
    fn write_free(&self, now: Instant) -> bool {
        let recent = self
            .checks
            .iter()
            .filter(|(t, _)| now.duration_since(*t) < WRITE_EVIDENCE_WINDOW);
        recent.clone().count() >= MIN_WRITE_FREE_CHECKS && recent.clone().all(|(_, w)| !w)
    }
}
struct Sample {
    id: u64,
    page: usize,
    time: Instant,
    kind: AccessKind,
}
pub struct Bridge<R: RawSource, D: DeviceSource> {
    raw: R,
    device: D,
    config: BridgeConfig,
    rx: Receiver<ObservationToken>,
    lost: Arc<AtomicBool>,
    registrations: Vec<Registered>,
    next_id: u64,
    history: Vec<Sample>,
    pending: VecDeque<Observation>,
    poison: BTreeSet<u64>,
    tgid: u32,
    clock: fn() -> io::Result<(Instant, u64)>,
    writes: Option<Box<dyn WriteSource>>,
    tracks: Vec<WriteTrack>,
    next_track: u64,
}
impl<R: RawSource, D: DeviceSource> Bridge<R, D> {
    /// Take write evidence from `source` instead of raw write samples.
    ///
    /// Raw `Write` events are then ignored. An observed page reports
    /// `writes == 0` only when its allocation has at least
    /// [`MIN_WRITE_FREE_CHECKS`] write-free checks and no written check within
    /// [`WRITE_EVIDENCE_WINDOW`]; otherwise `writes == max(reads, 1)`, which the
    /// migration policy never treats as read-dominant. Missing evidence
    /// therefore fails closed. Call once, before the first `device_update`.
    pub fn track_writes(&mut self, source: impl WriteSource) -> Result<(), Error> {
        if self.writes.is_some() || !self.registrations.is_empty() {
            return Err(Error::InvalidSample);
        }
        self.writes = Some(Box::new(source));
        Ok(())
    }
    /// Keep one write track per live registered allocation, refresh its token,
    /// and record the checks of the current sampling cycle.
    fn sync_writes(&mut self, now: Instant) -> Result<(), Error> {
        let Some(source) = self.writes.as_mut() else {
            return Ok(());
        };
        for r in &self.registrations {
            if let Some(t) = self
                .tracks
                .iter_mut()
                .find(|t| t.token.same_allocation(&r.token))
            {
                if r.token.created_at() > t.token.created_at() {
                    t.token = r.token.clone();
                }
                continue;
            }
            let Some(range) = r.token.range() else {
                continue;
            };
            if self.tracks.len() == self.config.max_registrations {
                return Err(Error::InvalidSample);
            }
            self.next_track = self.next_track.checked_add(1).ok_or(Error::InvalidSample)?;
            source.track(self.next_track, range)?;
            self.tracks.push(WriteTrack {
                key: self.next_track,
                token: r.token.clone(),
                checks: VecDeque::new(),
            });
        }
        self.tracks.retain(|t| {
            let live = t.token.range().is_some()
                && now.duration_since(t.token.created_at()) <= WRITE_EVIDENCE_WINDOW;
            if !live {
                source.untrack(t.key);
            }
            live
        });
        for check in source.cycle()? {
            if let Some(t) = self.tracks.iter_mut().find(|t| t.key == check.id) {
                t.checks.push_back((now, check.written));
                while t.checks.len() > MAX_CHECKS_PER_ALLOCATION
                    || t.checks
                        .front()
                        .is_some_and(|(at, _)| now.duration_since(*at) >= WRITE_EVIDENCE_WINDOW)
                {
                    t.checks.pop_front();
                }
            }
        }
        Ok(())
    }
    pub fn new(raw: R, device: D, config: BridgeConfig) -> Result<(Self, Publisher), Error> {
        Self::with_clock(raw, device, config, || {
            // Earlier Instant conservatively accounts for calibration latency.
            let now = Instant::now();
            Ok((now, monotonic_ns()?))
        })
    }
    /// Construct with a trusted calibration function for a custom raw source.
    /// The pair is (Instant, source-domain nanoseconds) for the same instant,
    /// or an earlier Instant for conservative conversion. The source clock must
    /// progress at the same rate; incorrect calibration can invalidate freshness.
    /// This seam also allows fixture clocks without relaxing production TTLs.
    pub fn with_clock(
        raw: R,
        device: D,
        config: BridgeConfig,
        clock: fn() -> io::Result<(Instant, u64)>,
    ) -> Result<(Self, Publisher), Error> {
        if !(1..=4096).contains(&config.max_registrations)
            || !(1..=4096).contains(&config.input_capacity)
            || !(1..=8192).contains(&config.max_events)
            || !(1..=65536).contains(&config.max_history)
            || !(1..=128).contains(&config.max_output)
            || config.token_ttl.is_zero()
        {
            return Err(Error::InvalidSample);
        }
        let (tx, rx) = sync_channel(config.input_capacity);
        let lost = Arc::new(AtomicBool::new(false));
        let publisher = Publisher {
            tx,
            lost: lost.clone(),
        };
        Ok((
            Self {
                raw,
                device,
                config,
                rx,
                lost,
                registrations: Vec::new(),
                next_id: 0,
                history: Vec::new(),
                pending: VecDeque::new(),
                poison: BTreeSet::new(),
                tgid: std::process::id(),
                clock,
                writes: None,
                tracks: Vec::new(),
                next_track: 0,
            },
            publisher,
        ))
    }
    fn fail(&mut self) {
        self.pending.clear();
        self.history.clear();
    }
    fn prepare(&mut self) -> Result<DeviceUpdate, Error> {
        if self.tgid != std::process::id()
            || self.lost.load(Ordering::Acquire)
            || !self.pending.is_empty()
        {
            return Err(Error::InvalidSample);
        }
        let now = Instant::now();
        self.registrations.retain(|r| {
            r.token.range().is_some()
                && now.duration_since(r.token.created_at()) <= self.config.token_ttl
        });
        for _ in 0..self.config.input_capacity {
            let Ok(token) = self.rx.try_recv() else { break };
            if token.range().is_none()
                || now
                    .checked_duration_since(token.created_at())
                    .is_some_and(|a| a > self.config.token_ttl)
            {
                continue;
            }
            if let Some(i) = self
                .registrations
                .iter()
                .position(|r| r.token.same_allocation(&token))
            {
                // Do not let out-of-order metadata publication roll back the token generation.
                if self.registrations[i].token.created_at() >= token.created_at() {
                    continue;
                }
                self.registrations.remove(i);
            }
            if self.registrations.len() == self.config.max_registrations {
                self.lost.store(true, Ordering::Release);
                return Err(Error::InvalidSample);
            }
            self.next_id = self.next_id.checked_add(1).ok_or(Error::InvalidSample)?;
            self.registrations.push(Registered {
                id: self.next_id,
                token,
            });
        }
        let reading = self.device.read_device()?;
        if reading.capacity_bytes == 0
            || reading.used_bytes > reading.capacity_bytes
            || reading.poison_pages.len() > 64
        {
            return Err(Error::InvalidSample);
        }
        self.sync_writes(now)?;
        let events = self.raw.read_events()?;
        if events.len() > self.config.max_events {
            return Err(Error::InvalidSample);
        }
        // Preserve original event age; never receipt-stamp an old event.
        let (now, raw_now) = (self.clock)()?;
        self.history.retain(|e| {
            now.duration_since(e.time) < WINDOW && self.registrations.iter().any(|r| r.id == e.id)
        });
        for e in events {
            if e.tgid != self.tgid
                || e.time_ns > raw_now
                || (self.writes.is_some() && e.kind == AccessKind::Write)
            {
                continue;
            }
            let age = Duration::from_nanos(raw_now - e.time_ns);
            if age >= WINDOW {
                continue;
            }
            let Some(time) = now.checked_sub(age) else {
                continue;
            };
            let Ok(address) = usize::try_from(e.address) else {
                continue;
            };
            let Some(r) = self.registrations.iter().find(|r| {
                r.token
                    .range()
                    .is_some_and(|range| range.contains(&address))
            }) else {
                continue;
            };
            if time < r.token.created_at()
                || now.duration_since(r.token.created_at()) > self.config.token_ttl
            {
                continue;
            }
            if self.history.len() == self.config.max_history {
                return Err(Error::InvalidSample);
            }
            self.history.push(Sample {
                id: r.id,
                page: address / crate::hal::page_size() * crate::hal::page_size(),
                time,
                kind: e.kind,
            });
        }
        let mut output: BTreeMap<(u64, usize), Observation> = BTreeMap::new();
        for e in &self.history {
            let Some(r) = self.registrations.iter().find(|r| r.id == e.id) else {
                continue;
            };
            let key = (e.id, e.page);
            if !output.contains_key(&key) && output.len() == self.config.max_output {
                return Err(Error::InvalidSample);
            }
            let o = output.entry(key).or_insert_with(|| Observation {
                token: r.token.clone(),
                address: e.page,
                reads: 0,
                writes: 0,
                observed_at: e.time,
            });
            match e.kind {
                AccessKind::Read => o.reads += 1,
                AccessKind::Write => o.writes += 1,
            };
            o.observed_at = o.observed_at.max(e.time);
        }
        if self.writes.is_some() {
            for o in output.values_mut() {
                let write_free = self
                    .tracks
                    .iter()
                    .find(|t| t.token.same_allocation(&o.token))
                    .is_some_and(|t| t.write_free(now));
                o.writes = if write_free { 0 } else { o.reads.max(1) };
            }
        }
        if self.lost.load(Ordering::Acquire) {
            return Err(Error::InvalidSample);
        }
        self.pending = output.into_values().collect();
        let poison: BTreeSet<_> = reading.poison_pages.into_iter().collect();
        let poison_pages = poison.difference(&self.poison).copied().collect();
        self.poison = poison;
        Ok(DeviceUpdate {
            used_bytes: reading.used_bytes,
            capacity_bytes: reading.capacity_bytes,
            poison_pages,
        })
    }
}
impl<R: RawSource, D: DeviceSource> Collector for Bridge<R, D> {
    fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
        let result = self.prepare();
        if result.is_err() {
            self.fail();
        }
        result.map(Some)
    }
    fn poll(&mut self) -> Result<Option<Observation>, Error> {
        if self.lost.load(Ordering::Acquire) {
            self.fail();
            return Err(Error::InvalidSample);
        }
        Ok(self.pending.pop_front())
    }
}
