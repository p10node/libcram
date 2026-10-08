use libcram_core::{
    allocator::CramAllocator,
    hal::{Node, SystemNuma, Topology},
    telemetry::{PsiLine, PsiSample},
};
use std::{
    sync::{mpsc, Arc},
    time::Duration,
};
#[test]
fn callback_is_async_reentrant_replaceable_and_panic_isolated() {
    let a = Arc::new(
        CramAllocator::with_backend(
            Topology::from_nodes(vec![Node {
                id: 0,
                is_cxl: false,
                compression: None,
            }])
            .expect("topology"),
            Arc::new(SystemNuma),
        )
        .expect("allocator"),
    );
    let (tx, rx) = mpsc::channel();
    let weak = Arc::downgrade(&a);
    let caller = std::thread::current().id();
    a.register_pressure_callback(Some(Arc::new(move |level| {
        let allocator = weak.upgrade().expect("alive");
        let _ = allocator.telemetry();
        tx.send((level, std::thread::current().id())).expect("send");
    })))
    .expect("register");
    a.submit_pressure(PsiSample {
        some: PsiLine {
            avg10: 15.0,
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("sample");
    let (level, thread) = rx.recv_timeout(Duration::from_secs(2)).expect("callback");
    assert_eq!(level, 2);
    assert_ne!(thread, caller);
    assert_eq!(a.telemetry().pressure_level, 2);
    let (started, received) = mpsc::channel();
    a.register_pressure_callback(Some(Arc::new(move |_| {
        started.send(()).expect("entered");
        panic!("intentional callback panic");
    })))
    .expect("register");
    a.submit_pressure(PsiSample::default()).expect("sample");
    received
        .recv_timeout(Duration::from_secs(2))
        .expect("panic callback entered");
    let (next, received) = mpsc::channel();
    a.register_pressure_callback(Some(Arc::new(move |level| {
        next.send(level).expect("next");
    })))
    .expect("replace");
    a.submit_pressure(PsiSample::default()).expect("sample");
    assert_eq!(
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("worker survived panic"),
        0
    );
    a.register_pressure_callback(None).expect("unregister");
    a.submit_pressure(PsiSample::default()).expect("sample");
    assert_eq!(a.telemetry().psi_some_avg10, 0.0);
}
