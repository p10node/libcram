use super::*;
use crate::hal::{Node, SystemNuma, Topology};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn would_block(result: Result<(), Error>) -> bool {
    matches!(result, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock)
}

fn allocator() -> anyhow::Result<Arc<CramAllocator>> {
    Ok(Arc::new(CramAllocator::with_backend(
        Topology::from_nodes(vec![Node {
            id: 0,
            is_cxl: false,
            compression: None,
        }])?,
        Arc::new(SystemNuma),
    )?))
}

fn block_callback(a: &Arc<CramAllocator>) -> anyhow::Result<mpsc::Sender<()>> {
    let (entered, entering) = mpsc::channel();
    let (release, releasing) = mpsc::channel();
    let releasing = Mutex::new(releasing);
    a.register_pressure_callback(Some(Arc::new(move |_| {
        entered.send(()).expect("signal callback entry");
        releasing
            .lock()
            .expect("release lock")
            .recv()
            .expect("release callback");
    })))?;
    a.inner.pressure.notify(7)?;
    entering.recv_timeout(Duration::from_secs(2))?;
    Ok(release)
}

#[test]
fn replacement_barrier_waits_for_old_callback_and_rejects_competitors() -> anyhow::Result<()> {
    let a = allocator()?;
    let release = block_callback(&a)?;
    let b_calls = Arc::new(AtomicUsize::new(0));
    let calls = b_calls.clone();
    a.register_pressure_callback(Some(Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
    })))?;
    let other = a.clone();
    let (done, completion) = mpsc::channel();
    let worker = thread::spawn(move || {
        done.send(other.unregister_pressure_callback_and_wait())
            .expect("barrier completion");
    });
    // Observe the actual state transition, not a sleep or thread-start signal.
    // The deadline is only a watchdog for broken implementations.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut observed = false;
    while Instant::now() < deadline {
        if a.inner.pressure.callback.0.lock().expect("slot").quiescing {
            observed = true;
            break;
        }
        if completion.try_recv().is_ok() {
            break;
        }
        thread::yield_now();
    }
    let pending = matches!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
    let registration_rejected = would_block(a.register_pressure_callback(None))
        && would_block(a.register_pressure_callback(Some(Arc::new(|_| {}))));
    // Only issue the competing wait after proving the original owns quiescence.
    let (second_done, second_completion) = mpsc::channel();
    let contender = a.clone();
    let second_worker = thread::spawn(move || {
        let result = contender.unregister_pressure_callback_and_wait();
        let _ = second_done.send(result);
    });
    let second_wait_rejected = observed
        && matches!(
            second_completion.recv_timeout(Duration::from_secs(2)),
            Ok(Err(Error::Io(error))) if error.kind() == std::io::ErrorKind::WouldBlock
        );
    release.send(())?;
    worker.join().expect("barrier thread");
    second_worker.join().expect("competing barrier thread");
    assert!(
        observed && pending,
        "barrier must track selected A after replacement by B"
    );
    assert!(registration_rejected && second_wait_rejected);
    completion.recv_timeout(Duration::from_secs(2))??;
    assert_eq!(b_calls.load(Ordering::SeqCst), 0);
    a.register_pressure_callback(None)?;
    Ok(())
}

#[test]
fn queued_old_generation_cannot_call_replacement() -> anyhow::Result<()> {
    let a = allocator()?;
    let release = block_callback(&a)?;
    // Worker is inside A, so this old-generation message is genuinely queued.
    a.inner.pressure.notify(11)?;
    let (delivered, deliveries) = mpsc::channel();
    a.register_pressure_callback(Some(Arc::new(move |level| {
        delivered.send(level).expect("delivery");
    })))?;
    release.send(())?;
    let generation = a.inner.pressure.callback.0.lock().expect("slot").generation;
    // The owned FIFO send waits for the old entry to be consumed. It avoids a
    // best-effort dropped notification and provides a positive drain witness.
    a.inner.pressure.tx.send((generation, 33))?;
    assert_eq!(deliveries.recv_timeout(Duration::from_secs(2))?, 33);
    a.unregister_pressure_callback_and_wait()?;
    assert!(deliveries.try_recv().is_err());
    Ok(())
}

#[test]
fn callback_destructor_reentry_and_panic_do_not_kill_worker() -> anyhow::Result<()> {
    struct Reenter {
        allocator: Weak<CramAllocator>,
        result: mpsc::Sender<bool>,
    }
    impl Drop for Reenter {
        fn drop(&mut self) {
            let a = self.allocator.upgrade().expect("fixture still alive");
            let self_wait_rejected = would_block(a.unregister_pressure_callback_and_wait());
            let registration_allowed = a.register_pressure_callback(None).is_ok();
            self.result
                .send(self_wait_rejected && registration_allowed)
                .expect("destructor result");
            panic!("deliberate callback capture destructor panic");
        }
    }
    let a = allocator()?;
    let (result, results) = mpsc::channel();
    let capture = Reenter {
        allocator: Arc::downgrade(&a),
        result,
    };
    let weak = Arc::downgrade(&a);
    a.register_pressure_callback(Some(Arc::new(move |_| {
        let _keep_capture = &capture;
        weak.upgrade()
            .expect("alive")
            .register_pressure_callback(None)
            .expect("remove slot reference");
    })))?;
    a.inner.pressure.notify(19)?;
    assert!(results.recv_timeout(Duration::from_secs(2))?);
    a.unregister_pressure_callback_and_wait()?;
    let (delivered, deliveries) = mpsc::channel();
    a.register_pressure_callback(Some(Arc::new(move |level| {
        delivered.send(level).expect("worker survived");
    })))?;
    a.inner.pressure.notify(37)?;
    assert_eq!(deliveries.recv_timeout(Duration::from_secs(2))?, 37);
    a.unregister_pressure_callback_and_wait()?;
    Ok(())
}
