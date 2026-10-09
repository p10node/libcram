use anyhow::{ensure, Result};
use libcram_core::allocator::{AccessHint, CramAllocator};
#[test]
fn portable_dram_allocation_preserves_distinct_owned_buffers() -> Result<()> {
    let allocator = CramAllocator::new()?;
    let mut first = allocator.allocate_owned(257, 64, AccessHint::WriteHeavy)?;
    let mut second = allocator.allocate_owned(8192, 4096, AccessHint::WriteHeavy)?;
    first.as_mut_slice().fill(19);
    second.as_mut_slice().fill(237);
    ensure!(first.as_slice().iter().all(|byte| *byte == 19));
    ensure!(second.as_slice().iter().all(|byte| *byte == 237));
    ensure!(second.as_slice().as_ptr() as usize % 4096 == 0);
    ensure!(allocator.telemetry().allocation_count == 2);
    ensure!(allocator.telemetry().cram_bytes == 0);
    drop(first);
    ensure!(allocator.telemetry().allocated_bytes == 8192);
    drop(second);
    ensure!(allocator.telemetry().allocation_count == 0);
    Ok(())
}
