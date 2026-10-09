use libcram_core::{
    allocator::CramAllocator,
    hal::{Node, SystemNuma, Topology},
    telemetry::PsiSample,
};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
fn allocator() -> Arc<CramAllocator> {
    Arc::new(
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
    )
}
#[test]
fn quiescent_unregister_waits_for_selected_callback_and_discards_future_delivery() {
    let a = allocator();
    let (entered, entering) = mpsc::channel();
    let (release, releasing) = mpsc::channel();
    let releasing = Mutex::new(releasing);
    a.register_pressure_callback(Some(Arc::new(move |_| {
        entered.send(()).expect("entered");
        releasing.lock().expect("lock").recv().expect("release");
    })))
    .expect("register");
    a.submit_pressure(PsiSample::default()).expect("submit");
    entering
        .recv_timeout(Duration::from_secs(2))
        .expect("callback running");
    let other = a.clone();
    let (done, completion) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        done.send(other.unregister_pressure_callback_and_wait())
            .expect("done")
    });
    let premature = completion.recv_timeout(Duration::from_millis(100));
    release.send(()).expect("release callback");
    assert!(
        premature.is_err(),
        "unregister returned while callback still owns userdata"
    );
    completion
        .recv_timeout(Duration::from_secs(2))
        .expect("joined callback")
        .expect("unregister");
    worker.join().expect("worker");
    a.submit_pressure(PsiSample::default())
        .expect("submit after unregister");
    assert!(entering.recv_timeout(Duration::from_millis(50)).is_err());
}
#[test]
fn callback_cannot_wait_for_itself_and_new_generation_still_works() {
    let a = allocator();
    let weak = Arc::downgrade(&a);
    let (tx, rx) = mpsc::channel();
    a.register_pressure_callback(Some(Arc::new(move |_| {
        let a = weak.upgrade().expect("alive");
        tx.send(a.unregister_pressure_callback_and_wait().is_err())
            .expect("result");
    })))
    .expect("register");
    a.submit_pressure(PsiSample::default()).expect("submit");
    assert!(rx
        .recv_timeout(Duration::from_secs(2))
        .expect("no self-deadlock"));
    a.unregister_pressure_callback_and_wait()
        .expect("external unregister");
    let (tx, rx) = mpsc::channel();
    a.register_pressure_callback(Some(Arc::new(move |level| {
        tx.send(level).expect("new generation");
    })))
    .expect("register new");
    a.submit_pressure(PsiSample::default()).expect("submit");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2))
            .expect("new callback"),
        0
    );
    a.unregister_pressure_callback_and_wait()
        .expect("final unregister");
}
