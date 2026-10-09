use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    hal::{page_size, CompressionProvenance, MoveReport, Node, Numa, PageStatus, Topology},
    runtime::{Collector, DeviceUpdate, Observation, Runtime, RuntimeConfig},
    Error,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
#[derive(Default)]
struct Backend(Mutex<Vec<u32>>);
impl Numa for Backend {
    unsafe fn bind(&self, _: usize, _: usize, _: u32) -> Result<(), Error> {
        Ok(())
    }
    unsafe fn move_pages(&self, _: usize, len: usize, node: u32) -> Result<MoveReport, Error> {
        self.0.lock().expect("moves").push(node);
        Ok(MoveReport {
            pages: vec![PageStatus::Node(node); len / page_size()],
            not_moved: 0,
        })
    }
}
struct Source(VecDeque<Observation>);
impl Collector for Source {
    fn poll(&mut self) -> Result<Option<Observation>, Error> {
        Ok(self.0.pop_front())
    }
    fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
        Ok(Some(DeviceUpdate {
            used_bytes: 0,
            capacity_bytes: 100,
            poison_pages: vec![],
        }))
    }
}
fn allocator() -> (CramAllocator, Arc<Backend>) {
    let backend = Arc::new(Backend::default());
    let topology = Topology::from_nodes(vec![
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
    .expect("topology");
    (
        CramAllocator::with_backend(topology, backend.clone()).expect("allocator"),
        backend,
    )
}
#[test]
fn invalid_budgets_are_rejected_before_start() {
    for config in [
        RuntimeConfig {
            max_observations: 0,
            ..Default::default()
        },
        RuntimeConfig {
            max_observations: 129,
            ..Default::default()
        },
        RuntimeConfig {
            max_pages: 0,
            ..Default::default()
        },
        RuntimeConfig {
            max_pages: 4097,
            ..Default::default()
        },
        RuntimeConfig {
            sample_ttl: std::time::Duration::ZERO,
            ..Default::default()
        },
        RuntimeConfig {
            interval: std::time::Duration::ZERO,
            ..Default::default()
        },
    ] {
        let (allocator, _) = allocator();
        assert!(Runtime::new(allocator, Source(VecDeque::new()), config).is_err());
    }
}
#[test]
fn status_counts_stale_identity_and_source_failures() {
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .expect("owned");
    let dead = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .expect("dead");
    let dead_token = dead.observation_token(0.0).expect("dead token");
    drop(dead);
    let address = owned.as_slice().as_ptr() as usize;
    let token = owned.observation_token(0.0).expect("token");
    let source = Source(VecDeque::from([
        Observation {
            token: token.clone(),
            address,
            reads: 100,
            writes: 0,
            observed_at: Instant::now() - std::time::Duration::from_secs(3),
        },
        Observation {
            token: dead_token,
            address,
            reads: 100,
            writes: 0,
            observed_at: Instant::now(),
        },
        Observation {
            token,
            address: address + page_size(),
            reads: 100,
            writes: 0,
            observed_at: Instant::now(),
        },
    ]));
    let mut runtime = Runtime::new(allocator, source, RuntimeConfig::default()).expect("runtime");
    runtime.tick().expect("tick");
    assert!(backend.0.lock().expect("moves").is_empty());
    assert_eq!(runtime.status().dropped, 3);
    assert_eq!(runtime.status().ticks, 1);
}
#[test]
fn failing_batch_cannot_partially_promote() {
    struct BatchFailure(Option<Observation>);
    impl Collector for BatchFailure {
        fn poll(&mut self) -> Result<Option<Observation>, Error> {
            self.0.take().map(Some).ok_or(Error::Unsupported)
        }
        fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
            Ok(Some(DeviceUpdate {
                used_bytes: 0,
                capacity_bytes: 100,
                poison_pages: vec![],
            }))
        }
    }
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .expect("owned");
    let source = BatchFailure(Some(Observation {
        token: owned.observation_token(0.0).expect("token"),
        address: owned.as_slice().as_ptr() as usize,
        reads: 100,
        writes: 0,
        observed_at: Instant::now(),
    }));
    let mut runtime = Runtime::new(allocator, source, RuntimeConfig::default()).expect("runtime");
    assert!(runtime.tick().is_err());
    assert!(backend.0.lock().expect("moves").is_empty());
}
#[test]
fn worker_runs_pipeline_and_stop_wakes_long_interval() {
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .expect("owned");
    let source = Source(VecDeque::from([Observation {
        token: owned.observation_token(0.0).expect("token"),
        address: owned.as_slice().as_ptr() as usize,
        reads: 100,
        writes: 0,
        observed_at: Instant::now(),
    }]));
    let runtime = Runtime::new(
        allocator,
        source,
        RuntimeConfig {
            interval: std::time::Duration::from_secs(60),
            ..Default::default()
        },
    )
    .expect("runtime");
    let mut worker = runtime.start().expect("start");
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    while backend.0.lock().expect("moves").is_empty() && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let before = Instant::now();
    worker.stop().expect("join");
    assert!(before.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(*backend.0.lock().expect("moves"), vec![1]);
}
#[test]
fn managed_constructor_joins_legacy_monitor() {
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .expect("owned");
    allocator.submit_device_usage(0, 100).expect("device");
    owned
        .record_sample(libcram_core::migration::AccessSample {
            reads: 100,
            writes: 0,
            entropy: 0.0,
        })
        .expect("legacy sample");
    allocator
        .start_monitoring(std::time::Duration::from_millis(50))
        .expect("legacy monitor");
    let _runtime = Runtime::new(allocator, Source(VecDeque::new()), RuntimeConfig::default())
        .expect("runtime");
    std::thread::sleep(std::time::Duration::from_millis(180));
    assert!(
        backend.0.lock().expect("moves").is_empty(),
        "legacy migration worker still alive"
    );
}
#[test]
fn observation_moves_only_its_payload_page() {
    let (allocator, _) = allocator();
    let owned = allocator
        .allocate_owned(page_size() * 3, page_size(), AccessHint::WriteHeavy)
        .expect("owned");
    let source = Source(VecDeque::from([Observation {
        token: owned.observation_token(0.0).expect("token"),
        address: owned.as_slice().as_ptr() as usize + page_size() + 7,
        reads: 100,
        writes: 0,
        observed_at: Instant::now(),
    }]));
    let mut runtime = Runtime::new(allocator, source, RuntimeConfig::default()).expect("runtime");
    runtime.tick().expect("tick");
    assert_eq!(runtime.telemetry().cram_bytes, page_size() as u64);
}
#[test]
fn source_failure_revokes_cached_device_health() {
    struct Fails;
    impl Collector for Fails {
        fn poll(&mut self) -> Result<Option<Observation>, Error> {
            Err(Error::Unsupported)
        }
        fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
            Ok(None)
        }
    }
    let (allocator, _) = allocator();
    allocator.submit_device_usage(0, 100).expect("healthy");
    let mut runtime = Runtime::new(allocator, Fails, RuntimeConfig::default()).expect("runtime");
    assert!(runtime.tick().is_err());
    assert!(
        runtime.telemetry().pressure_level >= 2,
        "source failure must revoke cached health"
    );
}
#[test]
fn collector_promotes_then_write_heavy_reverses_real_policy() {
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .expect("owned");
    let token = owned
        .observation_token(0.0)
        .expect("synchronized entropy metadata");
    let address = owned.as_slice().as_ptr() as usize;
    let source = Source(VecDeque::from([
        Observation {
            token: token.clone(),
            address,
            reads: 100,
            writes: 0,
            observed_at: Instant::now(),
        },
        Observation {
            token,
            address,
            reads: 0,
            writes: 100,
            observed_at: Instant::now(),
        },
    ]));
    let mut runtime = Runtime::new(
        allocator,
        source,
        RuntimeConfig {
            max_observations: 1,
            ..Default::default()
        },
    )
    .expect("runtime");
    runtime.tick().expect("read cycle");
    assert_eq!(runtime.telemetry().cram_bytes, page_size() as u64);
    runtime.tick().expect("write cycle");
    assert_eq!(runtime.telemetry().cram_bytes, 0);
    assert_eq!(*backend.0.lock().expect("moves"), vec![1, 0]);
    assert_eq!(runtime.status().accepted, 2);
    assert_eq!(runtime.status().pages_attempted, 2);
    assert_eq!(runtime.status().pages_moved, 2);
}

#[test]
fn event_before_token_creation_is_rejected() {
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let observed_at = Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(1));
    let source = Source(VecDeque::from([Observation {
        token: owned.observation_token(0.0).unwrap(),
        address: owned.as_slice().as_ptr() as usize,
        reads: 100,
        writes: 0,
        observed_at,
    }]));
    let mut runtime = Runtime::new(allocator, source, RuntimeConfig::default()).unwrap();
    runtime.tick().unwrap();
    assert_eq!(runtime.status().dropped, 1);
    assert!(backend.0.lock().unwrap().is_empty());
}

#[test]
fn dynamic_owned_allocation_live_status_and_joined_source_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct LiveSource(std::sync::mpsc::Receiver<Observation>, Arc<AtomicUsize>);
    impl Drop for LiveSource {
        fn drop(&mut self) {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Collector for LiveSource {
        fn poll(&mut self) -> Result<Option<Observation>, Error> {
            Ok(self.0.try_recv().ok())
        }
        fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
            Ok(Some(DeviceUpdate {
                used_bytes: 0,
                capacity_bytes: 100,
                poison_pages: vec![],
            }))
        }
    }
    for explicit in [true, false] {
        let (allocator, backend) = allocator();
        let drops = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        let mut worker = Runtime::new(
            allocator,
            LiveSource(rx, drops.clone()),
            RuntimeConfig {
                interval: std::time::Duration::from_millis(10),
                ..Default::default()
            },
        )
        .unwrap()
        .start()
        .unwrap();
        let mut owned = worker
            .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
            .unwrap();
        owned.as_mut_slice().fill(9);
        tx.try_send(Observation {
            token: owned.observation_token(0.0).unwrap(),
            address: owned.as_slice().as_ptr() as usize,
            reads: 100,
            writes: 0,
            observed_at: Instant::now(),
        })
        .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while worker.status().accepted == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(worker.status().accepted, 1);
        assert_eq!(worker.telemetry().allocation_count, 1);
        assert_eq!(worker.telemetry().cram_bytes, page_size() as u64);
        assert_eq!(*backend.0.lock().unwrap(), vec![1]);
        if explicit {
            worker.stop().unwrap();
            worker.stop().unwrap();
            assert!(worker.status().stopped);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
        drop(worker);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(owned.as_slice()[0], 9);
    }
}

#[test]
fn oversized_poison_batch_is_rejected_before_health_or_poll() {
    struct Oversized;
    impl Collector for Oversized {
        fn poll(&mut self) -> Result<Option<Observation>, Error> {
            panic!("must reject before poll")
        }
        fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
            Ok(Some(DeviceUpdate {
                used_bytes: 0,
                capacity_bytes: 100,
                poison_pages: vec![0; 65],
            }))
        }
    }
    let (allocator, backend) = allocator();
    allocator.submit_device_usage(0, 100).unwrap();
    let mut runtime = Runtime::new(allocator, Oversized, RuntimeConfig::default()).unwrap();
    assert!(matches!(runtime.tick(), Err(Error::InvalidSample)));
    assert!(runtime.telemetry().pressure_level >= 2);
    assert!(backend.0.lock().unwrap().is_empty());
    assert_eq!(runtime.status().errors, 1);
}

#[test]
fn managed_ownership_rejects_legacy_metadata_even_after_stop() {
    let (allocator, _) = allocator();
    let owned = allocator
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let sample = libcram_core::migration::AccessSample {
        reads: 100,
        writes: 0,
        entropy: 0.0,
    };
    owned.record_sample(sample).unwrap();
    let runtime =
        Runtime::new(allocator, Source(VecDeque::new()), RuntimeConfig::default()).unwrap();
    assert!(matches!(
        owned.record_sample(sample),
        Err(Error::Unsupported)
    ));
    let mut worker = runtime.start().unwrap();
    worker.stop().unwrap();
    assert!(matches!(
        owned.record_sample(sample),
        Err(Error::Unsupported)
    ));
    assert!(owned.observation_token(0.0).is_ok());
}

#[test]
fn page_quota_counts_discarded_observations_and_observation_quota_keeps_queue() {
    let (allocator, backend) = allocator();
    let owned = allocator
        .allocate_owned(page_size() * 4, page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let token = owned.observation_token(0.0).unwrap();
    let source = Source(
        (0..4)
            .map(|i| Observation {
                token: token.clone(),
                address: owned.as_slice().as_ptr() as usize + i * page_size(),
                reads: 100,
                writes: 0,
                observed_at: Instant::now(),
            })
            .collect(),
    );
    let mut runtime = Runtime::new(
        allocator,
        source,
        RuntimeConfig {
            max_pages: 1,
            max_observations: 2,
            ..Default::default()
        },
    )
    .unwrap();
    runtime.tick().unwrap();
    assert_eq!(runtime.status().pages_attempted, 1);
    assert_eq!(runtime.status().accepted, 1);
    assert_eq!(runtime.status().dropped, 1);
    runtime.tick().unwrap();
    assert_eq!(runtime.status().pages_attempted, 2);
    assert_eq!(runtime.status().accepted, 2);
    assert_eq!(runtime.status().dropped, 2);
    assert_eq!(backend.0.lock().unwrap().len(), 2);
}

#[test]
fn expired_token_fresh_event_future_event_and_foreign_identity_are_dropped() {
    use std::time::Duration;
    assert_eq!(
        RuntimeConfig::default().interval,
        Duration::from_millis(100)
    );
    let (a, backend) = allocator();
    let owned = a
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    assert!(owned.observation_token(f64::NAN).is_err());
    let stale = owned.observation_token(0.0).unwrap();
    std::thread::sleep(Duration::from_millis(25));
    let (foreign, _) = allocator();
    let foreign_owned = foreign
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    let source = Source(VecDeque::from([
        Observation {
            token: stale,
            address: owned.as_slice().as_ptr() as usize,
            reads: 100,
            writes: 0,
            observed_at: Instant::now(),
        },
        Observation {
            token: owned.observation_token(0.0).unwrap(),
            address: owned.as_slice().as_ptr() as usize,
            reads: 100,
            writes: 0,
            observed_at: Instant::now() + Duration::from_secs(1),
        },
        Observation {
            token: foreign_owned.observation_token(0.0).unwrap(),
            address: foreign_owned.as_slice().as_ptr() as usize,
            reads: 100,
            writes: 0,
            observed_at: Instant::now(),
        },
    ]));
    let mut runtime = Runtime::new(
        a,
        source,
        RuntimeConfig {
            sample_ttl: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .unwrap();
    runtime.tick().unwrap();
    assert_eq!(runtime.status().dropped, 3);
    assert_eq!(runtime.status().accepted, 0);
    assert!(backend.0.lock().unwrap().is_empty());
    let later = runtime
        .allocate_owned(page_size(), page_size(), AccessHint::WriteHeavy)
        .unwrap();
    assert_eq!(later.as_slice().len(), page_size());
}

#[test]
fn stopped_runtime_revokes_health_for_future_allocations() {
    let (a, _) = allocator();
    let mut rt = Runtime::new(a, Source(VecDeque::new()), RuntimeConfig::default()).unwrap();
    rt.tick().unwrap();
    let mut worker = rt.start().unwrap();
    worker.stop().unwrap();
    assert!(worker.telemetry().pressure_level >= 2);
    let _owned = worker
        .allocate_owned(page_size(), page_size(), AccessHint::ReadHeavy)
        .unwrap();
    assert_eq!(worker.telemetry().cram_bytes, 0);
}
#[test]
fn pressure_without_observations_evacuates_with_page_quota() {
    struct Pressure;
    impl Collector for Pressure {
        fn poll(&mut self) -> Result<Option<Observation>, Error> {
            Ok(None)
        }
        fn device_update(&mut self) -> Result<Option<DeviceUpdate>, Error> {
            Ok(Some(DeviceUpdate {
                used_bytes: 99,
                capacity_bytes: 100,
                poison_pages: vec![],
            }))
        }
    }
    let (a, b) = allocator();
    a.submit_device_usage(0, 100).unwrap();
    let _owned = a
        .allocate_owned(page_size() * 3, page_size(), AccessHint::ReadHeavy)
        .unwrap();
    let mut rt = Runtime::new(
        a,
        Pressure,
        RuntimeConfig {
            max_pages: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rt.telemetry().cram_bytes, (page_size() * 3) as u64);
    rt.tick().unwrap();
    assert_eq!(rt.telemetry().cram_bytes, (page_size() * 2) as u64);
    rt.tick().unwrap();
    rt.tick().unwrap();
    assert_eq!(rt.telemetry().cram_bytes, 0);
    assert_eq!(*b.0.lock().unwrap(), vec![0, 0, 0]);
}
