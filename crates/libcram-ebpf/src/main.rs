#![no_std]
#![no_main]
#![deny(clippy::unwrap_used)]
use aya_ebpf::{
    EbpfContext,
    bindings::bpf_perf_event_data,
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns},
    macros::map,
    maps::{PerCpuArray, RingBuf},
    programs::PerfEventContext,
};
mod wire;
#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 4] = *b"GPL\0";
#[map]
static EVENTS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);
#[map]
static DROPPED: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);
// Entry points are written out instead of `#[perf_event]`: that macro (aya-ebpf
// 0.1) discards the function's result and always returns 0, which on Linux
// >= 6.10 skips the kernel's PERF_EVENT_IOC_REFRESH sample limit.
#[unsafe(no_mangle)]
#[unsafe(link_section = "perf_event")]
pub fn sample_load(ctx: *mut core::ffi::c_void) -> u32 {
    sample(PerfEventContext::new(ctx.cast()), 1)
}
#[unsafe(no_mangle)]
#[unsafe(link_section = "perf_event")]
pub fn sample_store(ctx: *mut core::ffi::c_void) -> u32 {
    sample(PerfEventContext::new(ctx.cast()), 2)
}
fn sample(ctx: PerfEventContext, operation: u64) -> u32 {
    // SAFETY: perf_event programs receive bpf_perf_event_data, whose addr field
    // is verifier-approved. It is a sampled address, never dereferenced here.
    let address = unsafe { (*(ctx.as_ptr() as *const bpf_perf_event_data)).addr };
    // SAFETY: these BPF helpers take no pointer arguments.
    let time = unsafe { bpf_ktime_get_ns() };
    let pid = bpf_get_current_pid_tgid() >> 32;
    if let Some(record) = wire::event(time, address, pid, operation) {
        if EVENTS.output(&record, 0).is_err() {
            lost();
        }
    } else {
        lost();
    }
    // Nonzero keeps the kernel's overflow accounting: since Linux 6.10 a zero
    // return skips the PERF_EVENT_IOC_REFRESH sample limit (event_limit) that
    // bounds sampling bursts. The default output that follows has no perf ring
    // to write to (libcram never mmaps one), so it records nothing.
    1
}
fn lost() {
    if let Some(counter) = DROPPED.get_ptr_mut(0) {
        // SAFETY: this per-CPU map slot is valid for the current invocation.
        // Diagnostic only: snapshots/possible nested callbacks are not transactional.
        unsafe {
            *counter = (*counter).saturating_add(1);
        }
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
