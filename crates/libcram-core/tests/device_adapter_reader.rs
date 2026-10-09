#![cfg(unix)]
use libcram_core::device_adapter::{DeviceAdapter, DirectoryDeviceAdapter};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
#[test]
fn requires_positive_provenance_on_every_read() {
    let f = Fixture::new();
    let mut reader = DirectoryDeviceAdapter::new(&f.0).expect("reader");
    for marker in [
        "0",
        "",
        "yes",
        "1 junk",
        "11",
        "1\n0",
        "+1",
        "11111111111111111111111111111111",
    ] {
        fs::write(f.0.join("cram/compression_enabled"), marker).expect("marker");
        f.publish(8, now() - 10, 123, 456);
        assert!(reader.read().is_err(), "accepted marker {marker:?}");
    }
    fs::remove_file(f.0.join("cram/compression_enabled")).expect("remove marker");
    assert!(reader.read().is_err());
    fs::write(f.0.join("cram/compression_enabled"), "1\n").expect("restore marker");
    assert_eq!(reader.read().expect("positive provenance").used_bytes, 123);
}

#[test]
fn rejects_symlink_report_without_following_it() {
    let f = Fixture::new();
    f.publish(1, now() - 10, 321, 654);
    fs::rename(f.0.join("cram/libcram-report-v1"), f.0.join("cram/other")).expect("move");
    std::os::unix::fs::symlink("other", f.0.join("cram/libcram-report-v1")).expect("owned symlink");
    assert!(DirectoryDeviceAdapter::new(&f.0)
        .expect("reader")
        .read()
        .is_err());
}

#[test]
fn pins_configured_directory_and_bounds_actual_reads() {
    let f = Fixture::new();
    f.publish(1, now() - 10, 321, 654);
    let mut reader = DirectoryDeviceAdapter::new(&f.0).expect("reader");
    fs::rename(f.0.join("cram"), f.0.join("old-cram")).expect("move directory");
    fs::create_dir(f.0.join("cram")).expect("replacement");
    fs::write(f.0.join("cram/compression_enabled"), "1").expect("marker");
    f.publish(2, now() - 5, 111, 222);
    assert_eq!(reader.read().expect("pinned original").used_bytes, 321);
    let mut replacement = DirectoryDeviceAdapter::new(&f.0).expect("replacement reader");
    f.raw(&vec![b' '; 16385]);
    assert!(replacement.read().is_err());
    f.publish(3, now() - 2000, 111, 222);
    assert!(replacement.read().is_err());
    f.publish(4, now() + 10000, 111, 222);
    assert!(replacement.read().is_err());
    fs::remove_file(f.0.join("cram/libcram-report-v1")).expect("remove report");
    assert_eq!(
        replacement.read().expect_err("missing report").kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn rejects_untrusted_write_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.publish(1, now() - 10, 1, 2);
    let mut reader = DirectoryDeviceAdapter::new(&f.0).expect("reader");
    for name in ["libcram-report-v1", "compression_enabled"] {
        let path = f.0.join("cram").join(name);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).expect("permissions");
        assert!(
            reader.read().is_err(),
            "writable by untrusted users: {name}"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore");
    }
    fs::set_permissions(f.0.join("cram"), fs::Permissions::from_mode(0o777))
        .expect("directory permissions");
    assert!(reader.read().is_err(), "directory trust must be rechecked");
    assert!(DirectoryDeviceAdapter::new(&f.0).is_err());
    fs::set_permissions(f.0.join("cram"), fs::Permissions::from_mode(0o700)).expect("restore");
    assert_eq!(reader.read().expect("restored trust").used_bytes, 1);
}

#[test]
fn rejects_nonregular_files_and_oversized_poison_batch() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let mut reader = DirectoryDeviceAdapter::new(&f.0).expect("reader");
    let path = f.0.join("cram/libcram-report-v1");
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("c path");
    // SAFETY: owned fixture path is a valid NUL-terminated string.
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    assert!(reader.read().is_err(), "FIFO must not block");
    fs::remove_file(&path).expect("remove FIFO");
    fs::create_dir(&path).expect("directory report");
    assert!(reader.read().is_err());
    fs::remove_dir(&path).expect("remove directory");
    f.raw(
        format!(
            "CRAM-PHYSICAL-V1 1 {} 1 2 257 {}",
            now() - 10,
            vec!["1"; 257].join(" ")
        )
        .as_bytes(),
    );
    assert!(reader.read().is_err());
    f.publish(2, now() - 5, 123, 456);
    assert_eq!(
        reader.read().expect("valid bounded report").capacity_bytes,
        456
    );
}

#[test]
fn concurrent_atomic_publications_never_mix_physical_fields() {
    let f = Fixture::new();
    let stamp = now() - 500;
    f.publish(1, stamp, 1, 10001);
    let mut reader = DirectoryDeviceAdapter::new(&f.0).expect("reader");
    assert_eq!(reader.read().expect("initial").used_bytes, 1);
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            for generation in 2..=200 {
                f.publish(
                    generation,
                    stamp + generation,
                    generation,
                    10000 + generation,
                );
            }
        });
        for _ in 0..400 {
            if let Ok(reading) = reader.read() {
                assert_eq!(reading.capacity_bytes, 10000 + reading.used_bytes);
                assert_eq!(reading.poison_pages, vec![17, 17, 91]);
            }
        }
        writer.join().expect("writer");
    });
    f.publish(201, stamp + 201, 777, 17777);
    let last = reader.read().expect("distinct final publication");
    assert_eq!((last.used_bytes, last.capacity_bytes), (777, 17777));
}

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "libcram-device-{}-{}-{}",
            std::process::id(),
            now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("owned directory");
        fs::create_dir(path.join("cram")).expect("cram");
        fs::write(path.join("cram/compression_enabled"), b"1\n").expect("provenance");
        Self(path)
    }
    fn publish(&self, generation: u64, stamp: u64, used: u64, capacity: u64) {
        self.raw(
            format!("CRAM-PHYSICAL-V1 {generation} {stamp} {used} {capacity} 3 17 17 91")
                .as_bytes(),
        );
    }
    fn raw(&self, data: &[u8]) {
        fs::write(self.0.join("cram/pending"), data).expect("stage");
        fs::rename(
            self.0.join("cram/pending"),
            self.0.join("cram/libcram-report-v1"),
        )
        .expect("atomic publication");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64
}

#[test]
fn actual_file_reads_preserve_snapshots_and_reject_replays() {
    let fixture = Fixture::new();
    let stamp = now() - 100;
    fixture.publish(11, stamp, 345, 4096);
    let mut reader = DirectoryDeviceAdapter::new(&fixture.0).expect("reader");
    let first = reader.read().expect("first snapshot");
    assert_eq!((first.used_bytes, first.capacity_bytes), (345, 4096));
    assert_eq!(first.poison_pages, vec![17, 17, 91]);
    assert!(reader.read().is_err(), "same generation is not a heartbeat");
    fixture.publish(29, stamp + 1, 789, 8192);
    let second = reader.read().expect("new snapshot");
    assert_eq!((second.used_bytes, second.capacity_bytes), (789, 8192));
    for (generation, timestamp) in [
        (28, stamp + 2),
        (29, stamp + 3),
        (30, stamp),
        (31, stamp + 1),
    ] {
        fixture.publish(generation, timestamp, 111, 2048);
        assert!(reader.read().is_err(), "regression/replay accepted");
    }
    fixture.publish(32, stamp + 4, 9999, 1);
    assert!(reader.read().is_err());
    fixture.publish(32, stamp + 4, 111, 2048);
    assert_eq!(
        reader
            .read()
            .expect("recovery without losing watermark")
            .used_bytes,
        111
    );
}
