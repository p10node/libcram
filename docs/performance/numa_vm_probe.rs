//! Linux-only owned-mapping probe; NUMA migration is not CRAM acceptance.
use anyhow::{ensure, Result};
use libcram_core::hal::{page_size, Numa, SystemNuma};

struct Mapping {
    address: usize,
    len: usize,
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this object exclusively owns the mmap until drop.
        unsafe {
            libc::munmap(self.address as *mut libc::c_void, self.len);
        }
    }
}
fn query(mapping: &Mapping, expected: i32) -> Result<()> {
    let count = mapping.len / page_size();
    let pages: Vec<_> = (0..count)
        .map(|i| (mapping.address + i * page_size()) as *mut libc::c_void)
        .collect();
    let mut status = vec![-999; count];
    // SAFETY: all addresses belong to this live mapping; arrays contain count entries.
    // A null nodes pointer queries placement without migrating anything.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_move_pages,
            0,
            count,
            pages.as_ptr(),
            std::ptr::null::<i32>(),
            status.as_mut_ptr(),
            0,
        )
    };
    ensure!(rc == 0, "query failed: {}", std::io::Error::last_os_error());
    ensure!(
        status.iter().all(|&n| n == expected),
        "unexpected placement: {status:?}"
    );
    println!("queried {count} owned pages: all on node {expected}");
    Ok(())
}
fn verify(mapping: &Mapping, salt: usize) -> Result<()> {
    // SAFETY: the mapping is live, initialized and exclusively owned by this process.
    let bytes = unsafe { std::slice::from_raw_parts(mapping.address as *const u8, mapping.len) };
    ensure!(
        bytes
            .iter()
            .enumerate()
            .all(|(i, &b)| b == (i.wrapping_mul(37) ^ (i >> 8) ^ salt) as u8),
        "payload changed"
    );
    Ok(())
}
fn main() -> Result<()> {
    let len = page_size() * 16;
    // SAFETY: anonymous private mapping, no supplied address and no file descriptor.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    ensure!(address != libc::MAP_FAILED, "mmap failed");
    let mapping = Mapping {
        address: address as usize,
        len,
    };
    // SAFETY: the live, page-aligned mapping is exclusively owned and not yet faulted.
    unsafe {
        SystemNuma.bind(mapping.address, mapping.len, 0)?;
    }
    // SAFETY: uniquely borrowed writable mapping, initialized across its full length.
    let bytes = unsafe { std::slice::from_raw_parts_mut(address as *mut u8, len) };
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (i.wrapping_mul(37) ^ (i >> 8) ^ 91) as u8;
    }
    query(&mapping, 0)?;
    for node in [1, 0] {
        // SAFETY: all pages belong to this live private mapping; no concurrent access.
        let report = unsafe { SystemNuma.move_pages(mapping.address, mapping.len, node)? };
        ensure!(report.all_on(node), "incomplete migration: {report:?}");
        query(&mapping, node as i32)?;
        verify(&mapping, 91)?;
    }
    println!("PASS: real SystemNuma bind, migration 0->1->0, placement queries and byte preservation; CRAM NOT tested");
    Ok(())
}
