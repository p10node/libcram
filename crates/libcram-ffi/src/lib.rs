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
pub const CRAM_AUTO: u32 = 0;
pub const CRAM_READ_HEAVY: u32 = 1;
pub const CRAM_WRITE_HEAVY: u32 = 2;
static ALLOCATOR: OnceLock<Result<CramAllocator, ()>> = OnceLock::new();
thread_local! { static LAST_ERROR: Cell<i32> = const { Cell::new(CRAM_OK) }; }
fn allocator() -> Result<&'static CramAllocator, i32> {
    ALLOCATOR
        .get_or_init(|| CramAllocator::new().map_err(|_| ()))
        .as_ref()
        .map_err(|_| CRAM_ERROR)
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
/// The allocation belongs to the process-wide allocator until cram_free succeeds.
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
            .map_err(|_| CRAM_ERROR)
    })
    .unwrap_or(std::ptr::null_mut())
}
/// Free an allocation; null is a successful no-op.
/// # Safety
/// All accesses to ptr must have stopped, including on other threads. A valid live
/// allocation may be freed only once. Foreign addresses are rejected by the core
/// registry, but address reuse means stale/double-free pointers are not safe.
#[no_mangle]
pub unsafe extern "C" fn cram_free(ptr: *mut c_void) -> i32 {
    boundary(|| match NonNull::new(ptr.cast::<u8>()) {
        None => Ok(()),
        Some(p) => unsafe { allocator()?.deallocate(p).map_err(|_| CRAM_ERROR) },
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
        std::sync::Arc::new(move |level| unsafe { cb(level, address as *mut c_void) })
            as std::sync::Arc<dyn Fn(u32) + Send + Sync>
    })
}
pub type CramPressureCallback = Option<unsafe extern "C" fn(u32, *mut c_void)>;
/// Replace the process-wide callback; NULL unregisters future deliveries.
/// This is NOT a quiescent unregister: an already scheduled callback may still run.
/// # Safety
/// callback must be thread-safe and must not throw, unwind, or longjmp. user and
/// the callback code must remain valid until process exit, even after replacement
/// or unregister. No ownership is transferred. Invocation is asynchronous without
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
            .map_err(|_| CRAM_ERROR)
    })
    .err()
    .unwrap_or(CRAM_OK)
}
#[cfg(test)]
mod tests {
    use super::*;
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
