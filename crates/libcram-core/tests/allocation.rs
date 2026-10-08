use libcram_core::{
    allocator::{AccessHint, CramAllocator},
    Error,
};
#[test]
fn ownership_alignment_accounting_and_poison() {
    let a = CramAllocator::new().expect("allocator");
    let b = CramAllocator::new().expect("allocator");
    assert!(a.allocate(0, 8, AccessHint::Auto).is_err());
    assert!(a.allocate(8, 3, AccessHint::Auto).is_err());
    assert!(a.allocate(usize::MAX, 8, AccessHint::Auto).is_err());
    for align in [1, 8, 4096, 65536, 1048576] {
        let p = a.allocate(19, align, AccessHint::ReadHeavy).expect("alloc");
        assert_eq!(p.as_ptr() as usize % align, 0);
        unsafe { std::ptr::write_bytes(p.as_ptr(), 7, 19) };
        assert!(matches!(
            unsafe { b.deallocate(p) },
            Err(Error::UnknownAllocation)
        ));
        assert_eq!(a.telemetry().allocated_bytes, 19);
        unsafe { a.deallocate(p) }.expect("free");
        assert!(matches!(
            unsafe { a.deallocate(p) },
            Err(Error::UnknownAllocation)
        ));
    }
    let p = a.allocate(16, 8, AccessHint::Auto).expect("alloc");
    unsafe { p.as_ptr().add(16).write(0) };
    assert!(matches!(
        unsafe { a.deallocate(p) },
        Err(Error::GuardCorrupted)
    ));
    assert_eq!(a.telemetry().allocation_count, 0);
}
