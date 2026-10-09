use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    hal::{page_size, CompressionProvenance, MoveReport, Node, Numa, PageStatus, Topology},
    telemetry::{PsiLine, PsiSample},
    Error,
};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Backend(Mutex<Vec<u32>>);
impl Numa for Backend {
    unsafe fn bind(&self, _: usize, _: usize, node: u32) -> Result<(), Error> {
        self.0.lock().expect("backend").push(node);
        Ok(())
    }
    unsafe fn move_pages(&self, _: usize, len: usize, node: u32) -> Result<MoveReport, Error> {
        self.0.lock().expect("backend").push(node);
        Ok(MoveReport {
            pages: vec![PageStatus::Node(node); len / page_size()],
            not_moved: 0,
        })
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
fn missing_usage_pressure_and_poison_gate_real_allocation_policy() {
    let (a, backend) = allocator();
    let unknown = a
        .allocate_owned(64, 64, AccessHint::ReadHeavy)
        .expect("fallback");
    assert_eq!(
        *backend.0.lock().expect("backend").last().expect("binding"),
        0
    );
    assert_eq!(a.telemetry().cram_bytes, 0);
    a.submit_device_usage(20, 100).expect("healthy");
    let healthy = a
        .allocate_owned(64, 64, AccessHint::ReadHeavy)
        .expect("cram");
    assert_eq!(a.telemetry().cram_bytes, 64);
    a.submit_device_usage(85, 100).expect("critical");
    assert_eq!(a.telemetry().pressure_level, 2);
    let fallback = a
        .allocate_owned(64, 64, AccessHint::ReadHeavy)
        .expect("fallback");
    assert_eq!(
        *backend.0.lock().expect("backend").last().expect("binding"),
        0
    );
    a.migrate_once(10, 32)
        .expect("pressure demotion without access sample");
    assert_eq!(a.telemetry().cram_bytes, 0);
    a.submit_device_usage(0, 100).expect("recovered");
    a.submit_pressure(PsiSample {
        full: PsiLine {
            avg10: 8.0,
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("psi");
    let psi = a
        .allocate_owned(64, 64, AccessHint::ReadHeavy)
        .expect("psi fallback");
    assert_eq!(
        *backend.0.lock().expect("backend").last().expect("binding"),
        0
    );
    a.submit_pressure(PsiSample::default())
        .expect("psi recovered");
    for page in [11, 22, 33] {
        a.report_poison(page).expect("hardware event");
    }
    a.submit_device_usage(0, 100)
        .expect("usage cannot clear poison");
    assert_eq!(a.telemetry().pressure_level, 3);
    let poisoned = a
        .allocate_owned(64, 64, AccessHint::ReadHeavy)
        .expect("poison fallback");
    assert_eq!(
        *backend.0.lock().expect("backend").last().expect("binding"),
        0
    );
    drop((unknown, healthy, fallback, psi, poisoned));
}

#[test]
fn auto_initial_placement_is_size_based_and_pressure_gated() {
    let (a, _) = allocator();
    a.submit_device_usage(0, 100).expect("fixture occupancy");
    let small = a.allocate_owned(1024, 64, AccessHint::Auto).expect("small");
    assert_eq!(a.telemetry().cram_bytes, 0);
    let large = a
        .allocate_owned(65536, 64, AccessHint::Auto)
        .expect("large");
    assert_eq!(a.telemetry().cram_bytes, 65536);
    a.submit_device_usage(96, 100).expect("emergency");
    let fallback = a
        .allocate_owned(65536, 64, AccessHint::Auto)
        .expect("fallback");
    assert_eq!(a.telemetry().cram_bytes, 65536);
    drop((small, large, fallback));
}
