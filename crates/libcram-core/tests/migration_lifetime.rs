use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    hal::{page_size, CompressionProvenance, MoveReport, Node, Numa, PageStatus, Topology},
    migration::AccessSample,
    Error,
};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
struct Blocking {
    entered: mpsc::Sender<usize>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl Numa for Blocking {
    unsafe fn bind(&self, _: usize, _: usize, _: u32) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    unsafe fn move_pages(
        &self,
        address: usize,
        len: usize,
        node: u32,
    ) -> Result<MoveReport, Error> {
        self.entered.send(len).expect("entered");
        self.resume
            .lock()
            .expect("lock")
            .recv_timeout(Duration::from_secs(3))
            .expect("resume");
        // The lease must keep the mapping live after the registry/owner releases it.
        assert_eq!(unsafe { (address as *const u8).read() }, 42);
        Ok(MoveReport {
            pages: (0..len / page_size())
                .map(|i| {
                    if i == 0 {
                        PageStatus::Node(node)
                    } else {
                        PageStatus::Error(libc::EACCES)
                    }
                })
                .collect(),
            not_moved: len / page_size() - 1,
        })
    }
}
fn topology() -> Topology {
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
    .expect("topology")
}
#[test]
fn background_holds_mapping_lease_after_owner_drop() {
    let (tx, rx) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let a = CramAllocator::with_backend(
        topology(),
        Arc::new(Blocking {
            entered: tx,
            resume: Mutex::new(wait),
        }),
    )
    .expect("allocator");
    a.submit_device_usage(0, 100).expect("fixture occupancy");
    let mut p = a
        .allocate_owned(page_size() * 300, page_size(), AccessHint::Auto)
        .expect("owned");
    p.as_mut_slice().fill(42);
    p.record_sample(AccessSample {
        reads: 100,
        writes: 0,
        entropy: 0.0.into(),
    })
    .expect("sample");
    a.start_monitoring(Duration::from_millis(10))
        .expect("monitor");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).expect("entered"),
        256 * page_size()
    );
    drop(p);
    assert_eq!(a.telemetry().allocation_count, 0);
    resume.send(()).expect("resume");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(report) = a.last_background_migration().expect("report") {
            assert_eq!(report.pages_attempted, 256);
            assert_eq!(report.page_errors.len(), 255);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing background report"
        );
        std::thread::yield_now();
    }
    drop(a);
}
#[test]
fn reports_partial_failures_without_marking_entire_allocation_cram() {
    let (tx, rx) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let a = Arc::new(
        CramAllocator::with_backend(
            topology(),
            Arc::new(Blocking {
                entered: tx,
                resume: Mutex::new(wait),
            }),
        )
        .expect("allocator"),
    );
    a.submit_device_usage(0, 100).expect("fixture occupancy");
    let mut p = a
        .allocate_owned(page_size() * 2, page_size(), AccessHint::Auto)
        .expect("owned");
    p.as_mut_slice().fill(42);
    p.record_sample(AccessSample {
        reads: 100,
        writes: 0,
        entropy: 0.0.into(),
    })
    .expect("sample");
    let other = a.clone();
    let task = std::thread::spawn(move || other.migrate_once(1, 2));
    rx.recv_timeout(Duration::from_secs(2)).expect("entered");
    resume.send(()).expect("resume");
    let report = task.join().expect("join").expect("report");
    assert_eq!(report.pages_moved, 1);
    assert_eq!(report.page_errors.len(), 1);
    assert_eq!(report.page_errors[0].1, libc::EACCES);
    assert_eq!(a.telemetry().cram_bytes, page_size() as u64);
}
