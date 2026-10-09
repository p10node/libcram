//! Explicit opt-in driver bridge; not a generic Linux CRAM ABI.
use std::io;

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceReading {
    pub used_bytes: u64,
    pub capacity_bytes: u64,
    pub poison_pages: Vec<u64>,
}

pub trait DeviceAdapter: Send {
    fn read(&mut self) -> io::Result<DeviceReading>;
}

#[derive(Debug)]
pub struct DeviceReport {
    pub generation: u64,
    pub observed_unix_ms: u64,
    pub reading: DeviceReading,
}

/// A read-only, explicitly configured bridge. See docs/DEVICE_ADAPTER.md.
/// The directory and producer must be trusted; this is not hardware discovery.
#[cfg(unix)]
pub struct DirectoryDeviceAdapter {
    directory: std::fs::File,
    watermark: Option<(u64, u64)>,
    started: std::time::Instant,
    started_unix_ms: u64,
}
#[cfg(unix)]
impl DirectoryDeviceAdapter {
    /// Open and pin `<node_directory>/cram`. All ancestor directories must be
    /// trusted. Published report inodes MUST be immutable (atomic rename only).
    pub fn new(node_directory: impl AsRef<std::path::Path>) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        if !node_directory.as_ref().is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute node directory required",
            ));
        }
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(node_directory.as_ref().join("cram"))?;
        trusted_metadata(&directory.metadata()?)?;
        Ok(Self {
            directory,
            watermark: None,
            started: std::time::Instant::now(),
            started_unix_ms: unix_ms()?,
        })
    }

    fn read_file(&self, name: &std::ffi::CStr, limit: usize) -> io::Result<Vec<u8>> {
        use std::{
            io::Read,
            os::fd::{AsRawFd, FromRawFd},
        };
        // SAFETY: valid live directory fd and static NUL-terminated child name;
        // no creation flags. Successful fd transfers to exactly one File owner.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned fd above.
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        trusted_metadata(&metadata)?;
        if !metadata.is_file() {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(invalid());
        }
        Ok(bytes)
    }

    fn provenance(&self) -> io::Result<()> {
        let marker = self.read_file(c"compression_enabled", 16)?;
        if marker != b"1" && marker != b"1\n" {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(unix)]
fn trusted_metadata(metadata: &std::fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no arguments and does not mutate memory.
    let uid = unsafe { libc::geteuid() };
    if (metadata.uid() != 0 && metadata.uid() != uid) || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "untrusted CRAM bridge owner or write permissions",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn unix_ms() -> io::Result<u64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invalid())?
        .as_millis();
    u64::try_from(now).map_err(|_| invalid())
}

#[cfg(unix)]
impl DeviceAdapter for DirectoryDeviceAdapter {
    fn read(&mut self) -> io::Result<DeviceReading> {
        trusted_metadata(&self.directory.metadata()?)?;
        self.provenance()?;
        let bytes = self.read_file(c"libcram-report-v1", MAX_REPORT_BYTES)?;
        self.provenance()?;
        // A backward wall-clock adjustment cannot extend freshness indefinitely.
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).map_err(|_| invalid())?;
        let monotonic_floor = self
            .started_unix_ms
            .checked_add(elapsed)
            .ok_or_else(invalid)?;
        let report = parse_report(&bytes, unix_ms()?.max(monotonic_floor))?;
        if self.watermark.is_some_and(|(generation, stamp)| {
            report.generation <= generation || report.observed_unix_ms <= stamp
        }) {
            return Err(invalid());
        }
        self.watermark = Some((report.generation, report.observed_unix_ms));
        Ok(report.reading)
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid CRAM physical report")
}

pub const MAX_REPORT_BYTES: usize = 16_384;
pub const MAX_POISON_RECORDS: usize = 256;
pub const MAX_AGE_MS: u64 = 1_000;

/// Parse the adapter-specific physical snapshot. Does not authenticate a producer.
pub fn parse_report(bytes: &[u8], now_unix_ms: u64) -> io::Result<DeviceReport> {
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut fields = text.split_ascii_whitespace();
    if fields.next() != Some("CRAM-PHYSICAL-V1") {
        return Err(invalid());
    }
    let mut number = || {
        let field = fields.next().ok_or_else(invalid)?;
        if !field.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        field.parse::<u64>().map_err(|_| invalid())
    };
    let generation = number()?;
    let observed_unix_ms = number()?;
    let used_bytes = number()?;
    let capacity_bytes = number()?;
    let count = number()?;
    if generation == 0
        || observed_unix_ms == 0
        || capacity_bytes == 0
        || used_bytes > capacity_bytes
        || count > MAX_POISON_RECORDS as u64
        || now_unix_ms
            .checked_sub(observed_unix_ms)
            .is_none_or(|age| age > MAX_AGE_MS)
    {
        return Err(invalid());
    }
    let mut poison_pages = Vec::new();
    for _ in 0..count {
        poison_pages.push(number()?);
    }
    if fields.next().is_some() {
        return Err(invalid());
    }
    Ok(DeviceReport {
        generation,
        observed_unix_ms,
        reading: DeviceReading {
            used_bytes,
            capacity_bytes,
            poison_pages,
        },
    })
}
