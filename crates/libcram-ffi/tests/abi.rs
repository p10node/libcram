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

#[test]
fn canary_error_distinguishes_consumed_native_ownership() {
    let p = cram_alloc(64, 64, CRAM_AUTO);
    assert!(!p.is_null());
    // SAFETY: this fixture owns the allocation, including its mapped trailing canary.
    unsafe {
        p.cast::<u8>().add(64).write(0);
    }
    // Status 5 explicitly means the corrupted allocation was consumed.
    assert_eq!(unsafe { cram_free(p) }, 5);
}

#[test]
fn metadata_admission_rejects_invalid_samples_and_accepts_owned_updates() {
    assert_eq!(cram_submit_device_usage(0, 0), CRAM_INVALID_ARGUMENT);
    assert_eq!(cram_submit_device_usage(20, 100), CRAM_OK);
    let p = cram_alloc(64, 64, CRAM_AUTO);
    assert!(!p.is_null());
    assert_eq!(
        cram_record_access_sample(p, 10, 0, f64::NAN),
        CRAM_INVALID_ARGUMENT
    );
    assert_eq!(cram_record_access_sample(p, 10, 0, 2.0), CRAM_OK);
    assert_eq!(
        cram_record_access_sample(std::ptr::null_mut(), 10, 0, 2.0),
        CRAM_INVALID_ARGUMENT
    );
    // SAFETY: this fixture exclusively owns the live allocation.
    assert_eq!(unsafe { cram_free(p) }, CRAM_OK);
}
