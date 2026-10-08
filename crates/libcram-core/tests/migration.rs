use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    hal::{page_size, CompressionProvenance, MoveReport, Node, Numa, PageStatus, Topology},
    migration::AccessSample,
    Error,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
struct Mock {
    moves: AtomicUsize,
    fail_bind: bool,
}
impl Numa for Mock {
    unsafe fn bind(&self, _: usize, _: usize, _: u32) -> Result<(), Error> {
        if self.fail_bind {
            Err(Error::Unsupported)
        } else {
            Ok(())
        }
    }
    unsafe fn move_pages(&self, _: usize, len: usize, node: u32) -> Result<MoveReport, Error> {
        self.moves.fetch_add(1, Ordering::SeqCst);
        Ok(MoveReport {
            pages: vec![PageStatus::Node(node); len / page_size()],
            not_moved: 0,
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
fn owned_sampling_moves_without_reading_mutable_caller_memory() {
    let backend = Arc::new(Mock {
        moves: AtomicUsize::new(0),
        fail_bind: false,
    });
    let a = CramAllocator::with_backend(topology(), backend.clone()).expect("allocator");
    a.submit_device_usage(0, 100).expect("fixture occupancy");
    let mut p = a
        .allocate_owned(page_size() * 2, page_size(), AccessHint::Auto)
        .expect("owned");
    p.as_mut_slice().fill(23);
    p.record_sample(AccessSample {
        reads: 100,
        writes: 0,
        entropy: 1.0,
    })
    .expect("sample");
    let report = a.migrate_once(1, 1).expect("migration");
    assert_eq!(report.pages_attempted, 1);
    assert_eq!(report.pages_moved, 1);
    assert_eq!(a.telemetry().cram_bytes, page_size() as u64);
    p.record_sample(AccessSample {
        reads: 0,
        writes: 100,
        entropy: 8.0,
    })
    .expect("sample");
    a.migrate_once(1, 8).expect("migration");
    assert_eq!(a.telemetry().cram_bytes, 0);
    assert!(p
        .record_sample(AccessSample {
            reads: 1,
            writes: 0,
            entropy: f64::NAN
        })
        .is_err());
    drop(a);
    assert!(p.as_slice().iter().all(|b| *b == 23));
    drop(p);
    assert!(backend.moves.load(Ordering::SeqCst) >= 2);
}
#[test]
fn failed_binding_is_not_reported_as_cram() {
    let a = CramAllocator::with_backend(
        topology(),
        Arc::new(Mock {
            moves: AtomicUsize::new(0),
            fail_bind: true,
        }),
    )
    .expect("allocator");
    let p = a
        .allocate(4096, 8, AccessHint::ReadHeavy)
        .expect("fallback");
    assert_eq!(a.telemetry().cram_bytes, 0);
    unsafe { a.deallocate(p) }.expect("free");
}

#[test]
fn raw_sample_reversal_reclaims_first_page_immediately() {
    let a = CramAllocator::with_backend(
        topology(),
        Arc::new(Mock {
            moves: AtomicUsize::new(0),
            fail_bind: false,
        }),
    )
    .expect("allocator");
    let p = a
        .allocate(page_size() * 4, page_size(), AccessHint::Auto)
        .expect("alloc");
    a.record_sample(
        p,
        AccessSample {
            reads: 100,
            writes: 0,
            entropy: 0.0,
        },
    )
    .expect("sample");
    a.migrate_once(1, 1).expect("promote");
    a.record_sample(
        p,
        AccessSample {
            reads: 0,
            writes: 100,
            entropy: 8.0,
        },
    )
    .expect("sample");
    a.migrate_once(1, 1).expect("demote");
    assert_eq!(a.telemetry().cram_bytes, 0);
    unsafe { a.deallocate(p) }.expect("free");
}

#[test]
fn explicit_owned_entropy_sampling_uses_a_synchronized_borrow() {
    let a = CramAllocator::with_backend(
        topology(),
        Arc::new(Mock {
            moves: AtomicUsize::new(0),
            fail_bind: false,
        }),
    )
    .expect("allocator");
    a.submit_device_usage(0, 100).expect("fixture occupancy");
    let mut p = a.allocate_owned(4096, 64, AccessHint::Auto).expect("owned");
    p.as_mut_slice().fill(9);
    assert_eq!(p.profile(100, 0).expect("sample").entropy, 0.0);
    for (i, b) in p.as_mut_slice().iter_mut().enumerate() {
        *b = i as u8;
    }
    assert_eq!(p.profile(100, 0).expect("sample").entropy, 8.0);
}
