//! Device health policies and allocation canaries are distinct safety mechanisms.
pub mod device;
#[derive(Debug)]
pub struct PoisonGuard {
    address: usize,
}
impl PoisonGuard {
    pub const LEN: usize = 32;
    /// # Safety
    /// `pointer` must be writable for LEN bytes. Keep it alive until the final check.
    pub unsafe fn initialize(pointer: *mut u8) -> Self {
        for i in 0..Self::LEN {
            unsafe { pointer.add(i).write(Self::pattern(i)) };
        }
        Self {
            address: pointer as usize,
        }
    }
    fn pattern(i: usize) -> u8 {
        (i as u8).wrapping_mul(37) ^ 0xa5
    }
    /// # Safety
    /// The original storage must remain live and no writer may concurrently touch it.
    pub unsafe fn check(&self) -> bool {
        (0..Self::LEN)
            .all(|i| unsafe { (self.address as *const u8).add(i).read() == Self::pattern(i) })
    }
}
