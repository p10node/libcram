use libcram_core::allocator::{AccessHint, CramAllocator};
use std::sync::{Arc, Barrier};
#[test]
fn sixty_four_threads_allocate_write_verify_and_free() {
    let a = Arc::new(CramAllocator::new().expect("allocator"));
    let barrier = Arc::new(Barrier::new(64));
    std::thread::scope(|scope| {
        for thread in 0..64 {
            let a = a.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                for round in 0..128 {
                    let size = 1 + (round * 37 + thread) % 16384;
                    let p = a
                        .allocate(size, 1 << (round % 13), AccessHint::Auto)
                        .expect("allocate");
                    unsafe {
                        std::ptr::write_bytes(p.as_ptr(), thread as u8, size);
                        assert!(std::slice::from_raw_parts(p.as_ptr(), size)
                            .iter()
                            .all(|v| *v == thread as u8));
                        a.deallocate(p).expect("deallocate");
                    }
                }
            });
        }
    });
    assert_eq!(a.telemetry().allocation_count, 0);
    assert_eq!(a.telemetry().allocated_bytes, 0);
}
