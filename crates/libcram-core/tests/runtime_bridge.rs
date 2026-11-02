use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    device_adapter::DeviceReading,
    hal::{page_size, CompressionProvenance, MoveReport, Node, Numa, PageStatus, Topology},
    runtime::{Collector, Runtime, RuntimeConfig},
    runtime_bridge::{
        Bridge, BridgeConfig, DeviceSource, Publisher, RawSource, WriteCheck, WriteSource,
        MIN_WRITE_FREE_CHECKS,
    },
    telemetry::{access::AccessKind, sampling::AccessEvent},
    Error,
};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};
// Both coordinates derive from one Instant, eliminating clock-calibration
// uncertainty from queue/history/lifetime fixtures. Production uses real clocks.
fn fixture_clock() -> io::Result<(std::time::Instant, u64)> {
    use std::{sync::OnceLock, time::Instant};
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    let now = Instant::now();
    Ok((
        now,
        1_000_000_000_000 + u64::try_from(now.duration_since(origin).as_nanos()).unwrap(),
    ))
}
fn test_bridge<R: RawSource, D: DeviceSource>(
    raw: R,
    device: D,
    config: BridgeConfig,
) -> Result<(Bridge<R, D>, Publisher), Error> {
    Bridge::with_clock(raw, device, config, fixture_clock)
}
#[derive(Default)]
struct Backend(Mutex<Vec<u32>>);
impl Numa for Backend {
    unsafe fn bind(&self, _: usize, _: usize, _: u32) -> Result<(), Error> {
        Ok(())
    }
    unsafe fn move_pages(&self, _: usize, len: usize, node: u32) -> Result<MoveReport, Error> {
        self.0.lock().unwrap().push(node);
        Ok(MoveReport {
            pages: vec![PageStatus::Node(node); len / page_size()],
            not_moved: 0,
        })
    }
}
fn allocator() -> (CramAllocator, Arc<Backend>) {
    let b = Arc::new(Backend::default());
    (
        CramAllocator::with_backend(
            Topology::from_nodes(vec![
                Node {
                    id: 0,
                    is_cxl: false,
                    compression: None,
                },
                Node {
                    id: 1,
                    is_cxl: true,
                    compression: Some(CompressionProvenance::OperatorConfigured),
                },
            ])
            .unwrap(),
            b.clone(),
        )
        .unwrap(),
        b,
    )
}
#[test]
fn production_clock_empty_batch_and_clock_failure_are_distinct() {
    let (mut production, _) = Bridge::new(Raw::default(), Device, BridgeConfig::default()).unwrap();
    let update = production.device_update().unwrap().unwrap();
    assert_eq!((update.used_bytes, update.capacity_bytes), (0, 100));
    assert!(production.poll().unwrap().is_none());
    let (mut failed, _) =
        Bridge::with_clock(Raw::default(), Device, BridgeConfig::default(), || {
            Err(io::Error::other("owned clock failure"))
        })
        .unwrap();
    assert!(failed.device_update().is_err());
    assert!(failed.poll().unwrap().is_none());
}

#[test]
fn rolling_window_expires_and_token_ttl_does_not_refresh_on_receipt() {
    use std::time::Duration;
    let (a, _) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    let raw = Raw::default();
    let (mut bridge, p) = test_bridge(raw.clone(), Device, BridgeConfig::default()).unwrap();
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    raw.0
        .lock()
        .unwrap()
        .push_back(Ok(vec![event(address, AccessKind::Read)]));
    bridge.device_update().unwrap();
    assert_eq!(bridge.poll().unwrap().unwrap().reads, 1);
    raw.0
        .lock()
        .unwrap()
        .push_back(Ok(vec![event(address, AccessKind::Write)]));
    bridge.device_update().unwrap();
    let o = bridge.poll().unwrap().unwrap();
    assert_eq!((o.reads, o.writes), (1, 1));
    std::thread::sleep(Duration::from_millis(110));
    bridge.device_update().unwrap();
    assert!(bridge.poll().unwrap().is_none());
    let (mut bridge, p) = test_bridge(
        raw.clone(),
        Device,
        BridgeConfig {
            token_ttl: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .unwrap();
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    std::thread::sleep(Duration::from_millis(3));
    raw.0
        .lock()
        .unwrap()
        .push_back(Ok(vec![event(address, AccessKind::Read)]));
    bridge.device_update().unwrap();
    assert!(bridge.poll().unwrap().is_none());
}

type Batches = VecDeque<io::Result<Vec<AccessEvent>>>;
#[derive(Clone, Default)]
struct Raw(Arc<Mutex<Batches>>);
impl RawSource for Raw {
    fn read_events(&mut self) -> io::Result<Vec<AccessEvent>> {
        self.0.lock().unwrap().pop_front().unwrap_or(Ok(vec![]))
    }
}
struct Device;
impl DeviceSource for Device {
    fn read_device(&mut self) -> io::Result<DeviceReading> {
        Ok(DeviceReading {
            used_bytes: 0,
            capacity_bytes: 100,
            poison_pages: vec![],
        })
    }
}
fn event(address: usize, kind: AccessKind) -> AccessEvent {
    AccessEvent {
        address: address as u64,
        time_ns: fixture_clock().unwrap().1,
        tgid: std::process::id(),
        kind,
    }
}
#[test]
fn bridge_reverses_policy_and_source_error_revokes_health() {
    let (a, b) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let token = owned.observation_token(0.0).unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    let raw = Raw::default();
    let (bridge, publisher) = test_bridge(raw.clone(), Device, BridgeConfig::default()).unwrap();
    publisher.register(token).unwrap();
    raw.0.lock().unwrap().push_back(Ok((0..100)
        .map(|_| event(address, AccessKind::Read))
        .collect()));
    let mut runtime = Runtime::new(a, bridge, RuntimeConfig::default()).unwrap();
    runtime.tick().unwrap();
    assert_eq!(runtime.telemetry().cram_bytes, page_size() as u64);
    raw.0.lock().unwrap().push_back(Ok((0..100)
        .map(|_| event(address, AccessKind::Write))
        .collect()));
    runtime.tick().unwrap();
    assert_eq!(runtime.telemetry().cram_bytes, 0);
    assert_eq!(*b.0.lock().unwrap(), vec![1, 0]);
    raw.0
        .lock()
        .unwrap()
        .push_back(Err(io::Error::other("lost")));
    assert!(runtime.tick().is_err());
    assert!(runtime.telemetry().pressure_level >= 2);
}

#[test]
fn weak_lifetime_timestamp_and_payload_boundaries() {
    let (a, _) = allocator();
    let owned = a
        .allocate_owned(page_size() + 7, page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let start = owned.as_slice().as_ptr() as usize;
    let before = event(start, AccessKind::Read);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let token = owned.observation_token(0.0).unwrap();
    assert_eq!(token.range().unwrap(), start..start + page_size() + 7);
    let raw = Raw::default();
    let (mut bridge, p) = test_bridge(raw.clone(), Device, BridgeConfig::default()).unwrap();
    p.register(token.clone()).unwrap();
    let good = event(start, AccessKind::Read);
    let stamp = std::time::Instant::now();
    let mut foreign = event(start, AccessKind::Read);
    foreign.tgid = 0;
    let mut future = event(start, AccessKind::Read);
    future.time_ns = u64::MAX;
    let mut stale = event(start, AccessKind::Read);
    stale.time_ns -= 100_000_000;
    raw.0.lock().unwrap().push_back(Ok(vec![
        before,
        foreign,
        future,
        stale,
        event(start - 1, AccessKind::Read),
        event(start + page_size() + 7, AccessKind::Read),
        good,
        event(start + page_size() + 6, AccessKind::Write),
    ]));
    std::thread::sleep(std::time::Duration::from_millis(2));
    bridge.device_update().unwrap();
    let first = bridge.poll().unwrap().unwrap();
    assert_eq!((first.reads, first.writes), (1, 0));
    // macOS Instant and CLOCK_MONOTONIC use different clock conversions; do not
    // assert nanosecond equality across them. The 2ms receipt delay above still
    // makes receipt-stamping fail this check. Linux shares CLOCK_MONOTONIC.
    let tolerance = if cfg!(target_os = "macos") {
        std::time::Duration::from_micros(50)
    } else {
        std::time::Duration::ZERO
    };
    assert!(first.observed_at <= stamp + tolerance);
    assert!(first.observed_at >= token.created_at());
    let second = bridge.poll().unwrap().unwrap();
    assert_eq!(second.address, start + page_size());
    assert_eq!((second.reads, second.writes), (0, 1));
    assert!(bridge.poll().unwrap().is_none());
    drop(owned);
    assert!(token.range().is_none());
    assert_eq!(a.telemetry().allocation_count, 0);
    bridge.device_update().unwrap();
    assert!(bridge.poll().unwrap().is_none());
}
#[test]
fn queue_and_registration_overflow_are_sticky() {
    let (a, _) = allocator();
    let one = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let two = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let (mut bridge, p) = test_bridge(
        Raw::default(),
        Device,
        BridgeConfig {
            input_capacity: 1,
            ..Default::default()
        },
    )
    .unwrap();
    p.register(one.observation_token(0.0).unwrap()).unwrap();
    assert!(p.register(two.observation_token(0.0).unwrap()).is_err());
    assert!(bridge.device_update().is_err());
    assert!(bridge.poll().is_err());
    assert!(bridge.device_update().is_err());
    let (mut bridge, p) = test_bridge(
        Raw::default(),
        Device,
        BridgeConfig {
            max_registrations: 1,
            ..Default::default()
        },
    )
    .unwrap();
    p.register(one.observation_token(0.0).unwrap()).unwrap();
    bridge.device_update().unwrap();
    p.register(two.observation_token(0.0).unwrap()).unwrap();
    assert!(bridge.device_update().is_err());
    assert!(bridge.poll().is_err());
}
#[test]
fn event_history_and_output_overflow_clear_entire_batch() {
    let (a, _) = allocator();
    let owned = a
        .allocate_owned(page_size() * 2, page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    for config in [
        BridgeConfig {
            max_events: 1,
            ..Default::default()
        },
        BridgeConfig {
            max_history: 1,
            ..Default::default()
        },
        BridgeConfig {
            max_output: 1,
            ..Default::default()
        },
    ] {
        let raw = Raw::default();
        let (mut bridge, p) = test_bridge(raw.clone(), Device, config).unwrap();
        p.register(owned.observation_token(0.0).unwrap()).unwrap();
        raw.0
            .lock()
            .unwrap()
            .push_back(Ok(vec![event(address, AccessKind::Read)]));
        bridge.device_update().unwrap();
        assert_eq!(bridge.poll().unwrap().unwrap().reads, 1);
        raw.0.lock().unwrap().push_back(Ok(vec![
            event(address, AccessKind::Read),
            event(address + page_size(), AccessKind::Read),
        ]));
        assert!(bridge.device_update().is_err());
        assert!(bridge.poll().unwrap().is_none());
        bridge.device_update().unwrap();
        assert!(bridge.poll().unwrap().is_none());
    }
}
#[test]
fn refreshed_generation_drops_old_history_and_pre_token_events() {
    let (a, _) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    let raw = Raw::default();
    let (mut bridge, p) = test_bridge(raw.clone(), Device, BridgeConfig::default()).unwrap();
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    raw.0
        .lock()
        .unwrap()
        .push_back(Ok(vec![event(address, AccessKind::Read)]));
    bridge.device_update().unwrap();
    assert_eq!(bridge.poll().unwrap().unwrap().reads, 1);
    let old = event(address, AccessKind::Read);
    std::thread::sleep(std::time::Duration::from_millis(1));
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    raw.0
        .lock()
        .unwrap()
        .push_back(Ok(vec![old, event(address, AccessKind::Write)]));
    bridge.device_update().unwrap();
    let o = bridge.poll().unwrap().unwrap();
    assert_eq!((o.reads, o.writes), (0, 1));
    raw.0
        .lock()
        .unwrap()
        .push_back(Err(io::Error::other("loss")));
    assert!(bridge.device_update().is_err());
    assert!(bridge.poll().unwrap().is_none());
    bridge.device_update().unwrap();
    assert!(bridge.poll().unwrap().is_none());
}
#[test]
fn trusted_device_mapping_dedup_and_error_clear_pending() {
    struct Reports(VecDeque<io::Result<DeviceReading>>);
    impl DeviceSource for Reports {
        fn read_device(&mut self) -> io::Result<DeviceReading> {
            self.0.pop_front().unwrap()
        }
    }
    let reports = Reports(VecDeque::from([
        Ok(DeviceReading {
            used_bytes: 17,
            capacity_bytes: 123,
            poison_pages: vec![11, 11, 22],
        }),
        Ok(DeviceReading {
            used_bytes: 19,
            capacity_bytes: 127,
            poison_pages: vec![11, 22, 33],
        }),
        Err(io::Error::other("offline")),
    ]));
    let (mut bridge, _p) = test_bridge(Raw::default(), reports, BridgeConfig::default()).unwrap();
    let first = bridge.device_update().unwrap().unwrap();
    assert_eq!((first.used_bytes, first.capacity_bytes), (17, 123));
    assert_eq!(first.poison_pages, vec![11, 22]);
    let second = bridge.device_update().unwrap().unwrap();
    assert_eq!((second.used_bytes, second.capacity_bytes), (19, 127));
    assert_eq!(second.poison_pages, vec![33]);
    assert!(bridge.device_update().is_err());
    assert!(bridge.poll().unwrap().is_none());
}

#[derive(Default)]
struct WriteState {
    tracked: Vec<(u64, std::ops::Range<usize>)>,
    untracked: Vec<u64>,
    written: bool,
    fail: bool,
}
/// Reports one check per tracked allocation per cycle.
#[derive(Clone, Default)]
struct Writes(Arc<Mutex<WriteState>>);
impl WriteSource for Writes {
    fn track(&mut self, id: u64, range: std::ops::Range<usize>) -> io::Result<()> {
        self.0.lock().unwrap().tracked.push((id, range));
        Ok(())
    }
    fn untrack(&mut self, id: u64) {
        let mut s = self.0.lock().unwrap();
        s.tracked.retain(|(t, _)| *t != id);
        s.untracked.push(id);
    }
    fn cycle(&mut self) -> io::Result<Vec<WriteCheck>> {
        let s = self.0.lock().unwrap();
        if s.fail {
            return Err(io::Error::other("write tracking lost"));
        }
        Ok(s.tracked
            .iter()
            .map(|(id, range)| WriteCheck {
                id: *id,
                page: range.start,
                written: s.written,
            })
            .collect())
    }
}

#[test]
fn tracked_writes_fail_closed_until_write_free_and_ignore_raw_write_samples() {
    let (a, _) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::Auto)
        .unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    let raw = Raw::default();
    let writes = Writes::default();
    let (mut bridge, p) = test_bridge(raw.clone(), Device, BridgeConfig::default()).unwrap();
    bridge.track_writes(writes.clone()).unwrap();
    assert!(bridge.track_writes(writes.clone()).is_err());
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    let mut observe = |kind| {
        raw.0
            .lock()
            .unwrap()
            .push_back(Ok(vec![event(address, kind)]));
        bridge.device_update().unwrap();
        let o = bridge.poll().unwrap().unwrap();
        (o.reads, o.writes)
    };
    // Without enough write-free checks, reads never look read-dominant.
    for checks in 1..MIN_WRITE_FREE_CHECKS as u64 {
        assert_eq!(observe(AccessKind::Read), (checks, checks));
    }
    let reads = MIN_WRITE_FREE_CHECKS as u64;
    assert_eq!(observe(AccessKind::Read), (reads, 0));
    // A raw PEBS store sample is not write evidence under tracking.
    assert_eq!(observe(AccessKind::Write), (reads, 0));
    writes.0.lock().unwrap().written = true;
    assert_eq!(observe(AccessKind::Read), (reads + 1, reads + 1));
    let id = writes.0.lock().unwrap().tracked[0].0;
    drop(owned);
    bridge.device_update().unwrap();
    assert_eq!(writes.0.lock().unwrap().untracked, vec![id]);
    writes.0.lock().unwrap().fail = true;
    assert!(bridge.device_update().is_err());
    let (mut late, p) = test_bridge(Raw::default(), Device, BridgeConfig::default()).unwrap();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::Auto)
        .unwrap();
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    late.device_update().unwrap();
    assert!(late.track_writes(Writes::default()).is_err());
}

#[test]
fn runtime_promotes_only_after_write_free_evidence_and_reclaims_on_write() {
    let (a, b) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::Auto)
        .unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    let raw = Raw::default();
    let writes = Writes::default();
    let (mut bridge, p) = test_bridge(raw.clone(), Device, BridgeConfig::default()).unwrap();
    bridge.track_writes(writes.clone()).unwrap();
    p.register(owned.observation_token(0.0).unwrap()).unwrap();
    let mut runtime = Runtime::new(a, bridge, RuntimeConfig::default()).unwrap();
    let reads = |n| -> io::Result<Vec<AccessEvent>> {
        Ok((0..n).map(|_| event(address, AccessKind::Read)).collect())
    };
    for _ in 1..MIN_WRITE_FREE_CHECKS {
        raw.0.lock().unwrap().push_back(reads(100));
        runtime.tick().unwrap();
        assert_eq!(runtime.telemetry().cram_bytes, 0);
    }
    raw.0.lock().unwrap().push_back(reads(100));
    runtime.tick().unwrap();
    assert_eq!(runtime.telemetry().cram_bytes, page_size() as u64);
    writes.0.lock().unwrap().written = true;
    raw.0.lock().unwrap().push_back(reads(100));
    runtime.tick().unwrap();
    assert_eq!(runtime.telemetry().cram_bytes, 0);
    // Blocked ticks may keep the page on DRAM (node 0); CRAM (node 1) is
    // targeted exactly once, after the write-free evidence.
    let moves = b.0.lock().unwrap().clone();
    assert_eq!(moves.iter().filter(|n| **n == 1).count(), 1, "{moves:?}");
    assert_eq!(moves[moves.len() - 2..], [1, 0], "{moves:?}");
}
