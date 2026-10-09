use crate::Error;

pub fn page_size() -> usize {
    // SAFETY: sysconf takes no pointers and has no ownership effects.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as usize
    } else {
        4096
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageStatus {
    Node(u32),
    Error(i32),
}
impl PageStatus {
    pub fn from_raw(status: i32) -> Self {
        if status < 0 {
            Self::Error(status.saturating_neg())
        } else {
            Self::Node(status as u32)
        }
    }
}
#[derive(Clone, Debug)]
pub struct MoveReport {
    pub pages: Vec<PageStatus>,
    pub not_moved: usize,
}
impl MoveReport {
    pub fn all_on(&self, node: u32) -> bool {
        self.not_moved == 0
            && !self.pages.is_empty()
            && self.pages.iter().all(|p| *p == PageStatus::Node(node))
    }
}
/// Implementations must not retain addresses after returning.
pub trait Numa: Send + Sync {
    /// # Safety
    /// The caller must own a live, page-aligned mapping of at least `len` bytes.
    unsafe fn bind(&self, address: usize, len: usize, node: u32) -> Result<(), Error>;
    /// # Safety
    /// The caller must keep this page-aligned mapping live until return. Migration
    /// changes physical placement, not virtual addresses or the bytes themselves.
    unsafe fn move_pages(&self, address: usize, len: usize, node: u32)
        -> Result<MoveReport, Error>;
}
#[derive(Debug)]
pub struct SystemNuma;
fn validate(address: usize, len: usize, node: u32) -> Result<(), Error> {
    let page = page_size();
    if address == 0
        || !address.is_multiple_of(page)
        || len == 0
        || !len.is_multiple_of(page)
        || address.checked_add(len).is_none()
        || node > 65535
    {
        Err(Error::InvalidLayout)
    } else {
        Ok(())
    }
}
#[cfg(any(target_os = "linux", test))]
fn bind_mask(node: u32) -> (Vec<libc::c_ulong>, libc::c_ulong) {
    let bits = std::mem::size_of::<libc::c_ulong>() * 8;
    let mut mask = vec![0 as libc::c_ulong; node as usize / bits + 1];
    mask[node as usize / bits] |= (1 as libc::c_ulong) << (node as usize % bits);
    // Linux get_nodes() subtracts one before copying the bitmap. Include the
    // requested bit after that subtraction, including at machine-word edges.
    (mask, node as libc::c_ulong + 2)
}

impl Numa for SystemNuma {
    unsafe fn bind(&self, address: usize, len: usize, node: u32) -> Result<(), Error> {
        validate(address, len, node)?;
        #[cfg(target_os = "linux")]
        {
            let (mask, maxnode) = bind_mask(node);
            // MPOL_BIND=2, flags=0: policy only, before first touch.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_mbind,
                    address as *mut libc::c_void,
                    len,
                    2,
                    mask.as_ptr(),
                    maxnode,
                    0,
                )
            };
            if rc < 0 {
                Err(std::io::Error::last_os_error().into())
            } else {
                Ok(())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::Unsupported)
        }
    }
    unsafe fn move_pages(
        &self,
        address: usize,
        len: usize,
        node: u32,
    ) -> Result<MoveReport, Error> {
        validate(address, len, node)?;
        #[cfg(target_os = "linux")]
        {
            let count = len / page_size();
            let pages: Vec<_> = (0..count)
                .map(|i| (address + i * page_size()) as *mut libc::c_void)
                .collect();
            let nodes = vec![node as libc::c_int; count];
            let mut status = vec![-libc::EIO; count];
            // pid=0, MPOL_MF_MOVE=2, never MOVE_ALL (which requires extra privilege).
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_move_pages,
                    0,
                    count as libc::c_ulong,
                    pages.as_ptr(),
                    nodes.as_ptr(),
                    status.as_mut_ptr(),
                    2,
                )
            };
            if rc < 0 {
                Err(std::io::Error::last_os_error().into())
            } else {
                Ok(MoveReport {
                    pages: status.into_iter().map(PageStatus::from_raw).collect(),
                    not_moved: rc as usize,
                })
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::Unsupported)
        }
    }
}

#[cfg(test)]
mod mask_tests {
    use super::bind_mask;
    #[test]
    fn kernel_maxnode_minus_one_still_includes_requested_bit() {
        let bits = std::mem::size_of::<libc::c_ulong>() * 8;
        for node in [0, 1, 63, 64, 65, 127, 128, 65535] {
            let (mask, maxnode) = bind_mask(node);
            // Linux get_nodes() decrements maxnode before copying the bitmap.
            let copied_bits = maxnode - 1;
            assert!(
                (node as libc::c_ulong) < copied_bits,
                "node {node} excluded"
            );
            assert!((copied_bits as usize) <= mask.len() * bits);
            assert_eq!(mask.iter().map(|v| v.count_ones()).sum::<u32>(), 1);
            assert_ne!(
                mask[node as usize / bits] & (1 << (node as usize % bits)),
                0
            );
        }
    }
}
