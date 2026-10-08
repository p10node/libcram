use cram::*;
#[test]
fn aligned_allocation_round_trip() {
    assert_eq!(cram_init(), CRAM_OK);
    let p = cram_alloc(127, 64, 1);
    assert!(!p.is_null());
    assert_eq!(p as usize % 64, 0);
    unsafe {
        p.cast::<u8>().write_bytes(0xa5, 127);
        assert_eq!(cram_free(p), CRAM_OK);
    }
}

#[test]
fn telemetry_and_invalid_arguments() {
    unsafe {
        assert_eq!(
            cram_get_telemetry(std::ptr::null_mut()),
            CRAM_INVALID_ARGUMENT
        );
        assert_eq!(cram_free(std::ptr::null_mut()), CRAM_OK);
        let mut snapshot = std::mem::MaybeUninit::<cram_telemetry>::uninit();
        assert_eq!(cram_get_telemetry(snapshot.as_mut_ptr()), CRAM_OK);
        assert!(snapshot.assume_init().psi_some_avg10.is_finite());
        assert_eq!(
            cram_register_pressure_callback(None, std::ptr::null_mut()),
            CRAM_OK
        );
    }
    for (size, alignment, hint) in [
        (0, 8, 0),
        (8, 0, 0),
        (8, 3, 0),
        (8, 8, 3),
        (usize::MAX, 8, 0),
    ] {
        assert!(cram_alloc(size, alignment, hint).is_null());
        assert_eq!(cram_last_error(), CRAM_INVALID_ARGUMENT);
    }
}
#[test]
fn parallel_allocations_and_thread_local_errors() {
    assert!(cram_alloc(0, 8, 0).is_null());
    let threads: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                assert_eq!(cram_init(), CRAM_OK);
                for _ in 0..100 {
                    let p = cram_alloc(1024, 128, 2);
                    assert!(!p.is_null());
                    unsafe {
                        p.cast::<u8>().write_bytes(23, 1024);
                        assert_eq!(cram_free(p), CRAM_OK);
                    }
                }
                assert_eq!(cram_last_error(), CRAM_OK);
            })
        })
        .collect();
    for t in threads {
        t.join().expect("worker panic");
    }
    assert_eq!(cram_last_error(), CRAM_INVALID_ARGUMENT);
}
