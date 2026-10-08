use libcram_core::guard::PoisonGuard;
#[test]
fn catches_each_poison_byte_without_reading_payload() {
    let mut bytes = [0u8; 64];
    let guard = unsafe { PoisonGuard::initialize(bytes.as_mut_ptr()) };
    assert!(unsafe { guard.check() });
    // Do not keep an iter_mut reference live while the guard reads through its pointer.
    #[allow(clippy::needless_range_loop)]
    for i in 0..PoisonGuard::LEN {
        bytes[i] ^= 1;
        assert!(!unsafe { guard.check() });
        bytes[i] ^= 1;
    }
    assert!(unsafe { guard.check() });
}
