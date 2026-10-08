use libcram_core::hal::{page_size, Numa, PageStatus, SystemNuma};
#[test]
fn validates_page_ranges_and_reports_platform_capability() {
    let n = SystemNuma;
    let page = page_size();
    assert!(unsafe { n.bind(1, page, 0) }.is_err());
    assert!(unsafe { n.move_pages(1, page, 0) }.is_err());
    assert_eq!(
        PageStatus::from_raw(-libc::EACCES),
        PageStatus::Error(libc::EACCES)
    );
    assert_eq!(PageStatus::from_raw(3), PageStatus::Node(3));
    #[cfg(not(target_os = "linux"))]
    assert!(matches!(
        unsafe { n.bind(page, page, 0) },
        Err(libcram_core::Error::Unsupported)
    ));
}
#[cfg(target_os = "linux")]
#[test]
fn real_linux_syscalls_return_results_or_kernel_errors() {
    let page = page_size();
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(p, libc::MAP_FAILED);
    unsafe { *(p as *mut u8) = 42 };
    let n = SystemNuma;
    let bind = unsafe { n.bind(p as usize, page, 0) };
    eprintln!("real mbind: {bind:?}");
    let moved = unsafe { n.move_pages(p as usize, page, 0) };
    eprintln!("real move_pages: {moved:?}");
    match moved {
        Ok(r) => assert_eq!(r.pages.len(), 1),
        Err(libcram_core::Error::Io(e)) => assert!(e.raw_os_error().is_some()),
        other => panic!("unexpected: {other:?}"),
    }
    unsafe { libc::munmap(p, page) };
}
