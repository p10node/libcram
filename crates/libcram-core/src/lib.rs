#![deny(clippy::unwrap_used)]
pub mod allocator;
pub mod entropy;
pub mod guard;
pub mod hal;
pub mod migration;
pub mod telemetry;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid topology")]
    InvalidTopology,
    #[error("invalid allocation layout")]
    InvalidLayout,
    #[error("allocation failed")]
    OutOfMemory,
    #[error("pointer is not owned by this allocator")]
    UnknownAllocation,
    #[error("allocation poison guard corrupted")]
    GuardCorrupted,
    #[error("internal lock poisoned")]
    Poisoned,
    #[error("unsupported operation on this platform")]
    Unsupported,
    #[error("invalid sample")]
    InvalidSample,
    #[error("system error: {0}")]
    Io(#[from] std::io::Error),
}
