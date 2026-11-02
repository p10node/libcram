//! Stable C ABI. All fallible operations report integer status without unwinding.
#![deny(clippy::unwrap_used)]
use libcram_core::allocator::{AccessHint, CramAllocator};
use std::{
    cell::Cell,
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr::NonNull,
    sync::OnceLock,
};

pub const CRAM_OK: i32 = 0;
pub const CRAM_ERROR: i32 = 1;
pub const CRAM_INVALID_ARGUMENT: i32 = 2;
pub const CRAM_PANIC: i32 = 3;
pub const CRAM_ERR_OUT_OF_MEMORY: i32 = 4;
/// The allocation has been freed; retrying this pointer is invalid.
pub const CRAM_GUARD_CORRUPTED: i32 = 5;
pub const CRAM_SUCCESS: i32 = CRAM_OK;
fn status(error: libcram_core::Error) -> i32 {
    match error {
        libcram_core::Error::OutOfMemory => CRAM_ERR_OUT_OF_MEMORY,
        libcram_core::Error::GuardCorrupted => CRAM_GUARD_CORRUPTED,
        libcram_core::Error::InvalidLayout | libcram_core::Error::InvalidSample => {
            CRAM_INVALID_ARGUMENT
        }
        _ => CRAM_ERROR,
    }
}
pub const CRAM_AUTO: u32 = 0;
pub const CRAM_READ_HEAVY: u32 = 1;
pub const CRAM_WRITE_HEAVY: u32 = 2;
static ALLOCATOR: OnceLock<Result<CramAllocator, i32>> = OnceLock::new();
thread_local! { static LAST_ERROR: Cell<i32> = const { Cell::new(CRAM_OK) }; }
fn allocator() -> Result<&'static CramAllocator, i32> {
    ALLOCATOR
        .get_or_init(|| CramAllocator::new().map_err(status))
        .as_ref()
        .map_err(|status| *status)
}
fn boundary<T>(f: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    let result = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            // A user-defined panic payload may itself panic during Drop. Leak
            // only this exceptional payload instead of risking ABI unwinding.
            std::mem::forget(payload);
            Err(CRAM_PANIC)
        }
    };
    LAST_ERROR.with(|last| last.set(result.as_ref().err().copied().unwrap_or(CRAM_OK)));
    result
}
/// Initialize the process-wide allocator. Initialization failure is sticky.
#[no_mangle]
pub extern "C" fn cram_init() -> i32 {
    boundary(|| allocator().map(|_| ()))
        .err()
        .unwrap_or(CRAM_OK)
}
/// Allocate uninitialized memory. Size must be nonzero; alignment a power of two.
/// The allocation belongs to the process-wide allocator until cram_free consumes it (including CRAM_GUARD_CORRUPTED).
#[no_mangle]
pub extern "C" fn cram_alloc(size: usize, alignment: usize, hint: u32) -> *mut c_void {
    boundary(|| {
        if size == 0 || std::alloc::Layout::from_size_align(size, alignment).is_err() {
            return Err(CRAM_INVALID_ARGUMENT);
        }
        let hint = match hint {
            0 => AccessHint::Auto,
            1 => AccessHint::ReadHeavy,
            2 => AccessHint::WriteHeavy,
            _ => return Err(CRAM_INVALID_ARGUMENT),
        };
        allocator()?
            .allocate(size, alignment, hint)
            .map(|p| p.as_ptr().cast())
            .map_err(status)
    })
    .unwrap_or(std::ptr::null_mut())
}
/// Free an allocation; null is a successful no-op.
/// CRAM_GUARD_CORRUPTED reports corruption AFTER consuming the allocation; never retry.
/// # Safety
/// All accesses to ptr must have stopped, including on other threads. A valid live
/// allocation may be freed only once. Foreign addresses are rejected by the core
/// registry, but address reuse means stale/double-free pointers are not safe.
#[no_mangle]
pub unsafe extern "C" fn cram_free(ptr: *mut c_void) -> i32 {
    boundary(|| match NonNull::new(ptr.cast::<u8>()) {
        None => Ok(()),
        // SAFETY: forwarded from the C caller contract; core validates current ownership.
        Some(p) => unsafe { allocator()?.deallocate(p).map_err(status) },
    })
    .err()
    .unwrap_or(CRAM_OK)
}
/// Most recent status on this thread. Reading does not clear it.
#[no_mangle]
pub extern "C" fn cram_last_error() -> i32 {
    LAST_ERROR.with(Cell::get)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cram_telemetry {
    pub allocated_bytes: u64,
    pub cram_bytes: u64,
    pub allocation_count: u64,
    pub pressure_level: u32,
    pub psi_some_avg10: f64,
    pub psi_full_avg10: f64,
}
/// Read a coherent core telemetry snapshot.
/// # Safety
/// output must be null or aligned, writable storage for one cram_telemetry, with
/// no concurrent access to that storage for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn cram_get_telemetry(output: *mut cram_telemetry) -> i32 {
    boundary(|| {
        if output.is_null() {
            return Err(CRAM_INVALID_ARGUMENT);
        }
        let t = allocator()?.telemetry();
        // SAFETY: caller guarantees alignment, exclusive writable storage and adequate size.
        unsafe {
            output.write(cram_telemetry {
                allocated_bytes: t.allocated_bytes,
                cram_bytes: t.cram_bytes,
                allocation_count: t.allocation_count,
                pressure_level: t.pressure_level,
                psi_some_avg10: t.psi_some_avg10,
                psi_full_avg10: t.psi_full_avg10,
            });
        }
        Ok(())
    })
    .err()
    .unwrap_or(CRAM_OK)
}
fn callback_bridge(
    callback: CramPressureCallback,
    user: *mut c_void,
) -> Option<std::sync::Arc<dyn Fn(u32) + Send + Sync>> {
    let address = user as usize;
    callback.map(|cb| {
        // SAFETY: caller keeps code/userdata live until quiescence or process exit.
        std::sync::Arc::new(move |level| unsafe { cb(level, address as *mut c_void) })
            as std::sync::Arc<dyn Fn(u32) + Send + Sync>
    })
}
pub type CramPressureCallback = Option<unsafe extern "C" fn(u32, *mut c_void)>;
/// Replace the process-wide callback; NULL unregisters future deliveries.
/// This is NOT a quiescent unregister: an already scheduled callback may still run.
/// # Safety
/// callback must be thread-safe and must not throw, unwind, or longjmp. user and
/// the callback code must remain valid until successful quiescent unregister or
/// process exit, even after replacement or nonblocking unregister. No ownership
/// is transferred. Invocation is asynchronous without
/// core locks. Do not call this with Go-managed userdata.
#[no_mangle]
pub unsafe extern "C" fn cram_register_pressure_callback(
    callback: CramPressureCallback,
    user: *mut c_void,
) -> i32 {
    boundary(|| {
        let callback = callback_bridge(callback, user);
        allocator()?
            .register_pressure_callback(callback)
            .map_err(status)
    })
    .err()
    .unwrap_or(CRAM_OK)
}
/// Quiescent unregister. On OK no previously registered callback can still run,
/// so its userdata and callback code may be released. Returns CRAM_ERROR for a
/// self-wait or concurrent quiescence/registration conflict. Never call while
/// holding an application lock needed by the callback. Serialize callback
/// registration externally when managing shared userdata lifetimes.
#[no_mangle]
pub extern "C" fn cram_unregister_pressure_callback_and_wait() -> i32 {
    boundary(|| {
        allocator()?
            .unregister_pressure_callback_and_wait()
            .map_err(status)
    })
    .err()
    .unwrap_or(CRAM_OK)
}
/// Report trusted physical usage for the selected CRAM node. Reports expire after one second.
/// Zero capacity means unknown, blocks CRAM admission and returns INVALID_ARGUMENT.
#[no_mangle]
pub extern "C" fn cram_submit_device_usage(used: u64, capacity: u64) -> i32 {
    boundary(|| {
        allocator()?
            .submit_device_usage(used, capacity)
            .map_err(status)
    })
    .err()
    .unwrap_or(CRAM_OK)
}
/// Submit synchronized access metadata. This function never reads the payload.
/// ptr must identify a current allocation; entropy is finite in [0,8].
#[no_mangle]
pub extern "C" fn cram_record_access_sample(
    ptr: *mut c_void,
    reads: u64,
    writes: u64,
    entropy: f64,
) -> i32 {
    boundary(|| {
        let pointer = NonNull::new(ptr.cast::<u8>()).ok_or(CRAM_INVALID_ARGUMENT)?;
        allocator()?
            .record_sample(
                pointer,
                libcram_core::migration::AccessSample {
                    reads,
                    writes,
                    entropy: entropy.into(),
                },
            )
            .map_err(status)
    })
    .err()
    .unwrap_or(CRAM_OK)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiescent_ffi_unregister_allows_userdata_release() {
        use std::sync::mpsc;
        unsafe extern "C" fn callback(_: u32, user: *mut c_void) {
            // SAFETY: the test keeps the Box alive through quiescent unregister.
            let tx = unsafe { &*user.cast::<mpsc::Sender<i32>>() };
            let _ = tx.send(cram_unregister_pressure_callback_and_wait());
        }
        let (tx, rx) = mpsc::channel();
        let mut user = Box::new(tx);
        // SAFETY: userdata is stable, Send/Sync, and not freed before quiescence.
        assert_eq!(
            unsafe {
                cram_register_pressure_callback(
                    Some(callback),
                    (&mut *user as *mut mpsc::Sender<i32>).cast(),
                )
            },
            CRAM_OK
        );
        allocator()
            .expect("allocator")
            .submit_pressure(Default::default())
            .expect("notify");
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .expect("callback"),
            CRAM_ERROR
        );
        assert_eq!(cram_unregister_pressure_callback_and_wait(), CRAM_OK);
        drop(user);
        assert_eq!(cram_last_error(), CRAM_OK);
        assert_eq!(cram_unregister_pressure_callback_and_wait(), CRAM_OK);
    }
    #[test]
    fn panic_is_status_not_unwind() {
        assert_eq!(
            boundary::<()>(|| panic!("injected core panic")),
            Err(CRAM_PANIC)
        );
        assert_eq!(cram_last_error(), CRAM_PANIC);
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    #[test]
    fn panic_payload_destructor_cannot_escape_boundary() {
        struct Evil;
        impl Drop for Evil {
            fn drop(&mut self) {
                panic!("payload destructor");
            }
        }
        let outer = catch_unwind(|| boundary::<()>(|| std::panic::panic_any(Evil)));
        assert!(matches!(outer, Ok(Err(CRAM_PANIC))));
    }
    #[test]
    fn callback_bridge_preserves_userdata_and_allows_reentry() {
        static VALUE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        unsafe extern "C" fn callback(level: u32, user: *mut c_void) {
            let value = unsafe { &*user.cast::<std::sync::atomic::AtomicU32>() };
            value.store(level, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(cram_init(), CRAM_OK);
        }
        let cb =
            callback_bridge(Some(callback), &VALUE as *const _ as *mut c_void).expect("callback");
        std::thread::spawn(move || cb(2)).join().expect("worker");
        assert_eq!(VALUE.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(callback_bridge(None, std::ptr::null_mut()).is_none());
    }
}
