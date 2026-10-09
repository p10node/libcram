use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    device_adapter::DeviceReading,
    hal::{page_size, CompressionProvenance, MoveReport, Node, Numa, PageStatus, Topology},
    runtime::{Collector, Runtime, RuntimeConfig},
    runtime_bridge::{monotonic_ns, Bridge, BridgeConfig, DeviceSource, RawSource},
    telemetry::{access::AccessKind, sampling::AccessEvent},
    Error,
};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};
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
fn rolling_window_expires_and_token_ttl_does_not_refresh_on_receipt() {
    use std::time::Duration;
    let (a, _) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let address = owned.as_slice().as_ptr() as usize;
    let raw = Raw::default();
    let (mut bridge, p) = Bridge::new(raw.clone(), Device, BridgeConfig::default()).unwrap();
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
    let (mut bridge, p) = Bridge::new(
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
        time_ns: monotonic_ns().unwrap(),
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
    let (bridge, publisher) = Bridge::new(raw.clone(), Device, BridgeConfig::default()).unwrap();
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
    let (mut bridge, p) = Bridge::new(raw.clone(), Device, BridgeConfig::default()).unwrap();
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
    let (mut bridge, p) = Bridge::new(
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
    let (mut bridge, p) = Bridge::new(
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
        let (mut bridge, p) = Bridge::new(raw.clone(), Device, config).unwrap();
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
    let (mut bridge, p) = Bridge::new(raw.clone(), Device, BridgeConfig::default()).unwrap();
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
    let (mut bridge, _p) = Bridge::new(Raw::default(), reports, BridgeConfig::default()).unwrap();
    let first = bridge.device_update().unwrap().unwrap();
    assert_eq!((first.used_bytes, first.capacity_bytes), (17, 123));
    assert_eq!(first.poison_pages, vec![11, 22]);
    let second = bridge.device_update().unwrap().unwrap();
    assert_eq!((second.used_bytes, second.capacity_bytes), (19, 127));
    assert_eq!(second.poison_pages, vec![33]);
    assert!(bridge.device_update().is_err());
    assert!(bridge.poll().unwrap().is_none());
}
