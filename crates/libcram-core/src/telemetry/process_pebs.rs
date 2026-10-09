//! Bounded own-process PEBS lifecycle. No automatic PMU event selection.
use std::io;

pub const MAX_ATTACHMENTS: usize = 32;
pub const MAX_EVENTS: usize = 8192;
pub const MAX_OBJECT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    tid: u32,
    starttime: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    identity: Identity,
    cpu: u32,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn plan(threads: &[Identity], cpus: &[u32], cap: usize) -> io::Result<Vec<Key>> {
    if cap == 0
        || cap > MAX_ATTACHMENTS
        || threads.is_empty()
        || cpus.is_empty()
        || threads.len() > cap / cpus.len()
    {
        return Err(invalid(
            "thread/CPU attachment capacity exceeded or empty inventory",
        ));
    }
    let mut keys = Vec::with_capacity(threads.len() * cpus.len());
    for (i, identity) in threads.iter().enumerate() {
        if identity.tid == 0
            || identity.tid > i32::MAX as u32
            || threads[..i].iter().any(|t| t.tid == identity.tid)
        {
            return Err(invalid("invalid or duplicate thread identity"));
        }
        for (j, &cpu) in cpus.iter().enumerate() {
            if cpu > i32::MAX as u32 || cpus[..j].contains(&cpu) {
                return Err(invalid("invalid or duplicate CPU"));
            }
            keys.push(Key {
                identity: *identity,
                cpu,
            });
        }
    }
    keys.sort_unstable();
    Ok(keys)
}
fn parse_identity(bytes: &[u8], tid: u32) -> io::Result<Identity> {
    if bytes.len() > 4096 {
        return Err(invalid("oversized task stat"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("invalid task stat"))?;
    let (pid, _) = text
        .split_once(" (")
        .ok_or_else(|| invalid("missing task name"))?;
    if pid.parse::<u32>().ok() != Some(tid) {
        return Err(invalid("task identity mismatch"));
    }
    let (_, fields) = text
        .rsplit_once(") ")
        .ok_or_else(|| invalid("missing stat fields"))?;
    let starttime = fields
        .split_ascii_whitespace()
        .nth(19)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("missing starttime"))?;
    Ok(Identity { tid, starttime })
}
fn next_ring(cursor: &mut usize, count: usize) -> usize {
    let index = *cursor % count;
    *cursor = (index + 1) % count;
    index
}

#[cfg(all(target_os = "linux", feature = "ebpf"))]
pub use live::{ProcessPebsCollector, ProcessPebsStatus};
#[cfg(all(target_os = "linux", feature = "ebpf"))]
mod live {
    use super::*;
    use crate::telemetry::{
        pebs::PebsSampler,
        sampling::{AccessEvent, ExplicitPebsConfig},
    };
    use std::io::Read;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ProcessPebsStatus {
        NotPolled,
        NoSamples,
        Sampling,
        MissingCapability,
        AttachFailure,
        RefreshFailure,
        CapacityExceeded,
        DecodeFailure,
        Loss,
    }
    /// Own-process only. Call poll regularly; no workers are spawned.
    /// Errors clear all attachments and discard this poll's entire result.
    /// A later poll retries discovery/attachment, never replays partial output.
    pub struct ProcessPebsCollector {
        object: Vec<u8>,
        cpus: Vec<u32>,
        configs: Vec<ExplicitPebsConfig>,
        max_attachments: usize,
        max_events: usize,
        attachments: Vec<(Key, PebsSampler)>,
        cursor: usize,
        owner: u32,
        status: ProcessPebsStatus,
    }
    fn read_identity(tid: u32) -> io::Result<Identity> {
        let mut data = [0u8; 4097];
        let mut file = std::fs::File::open(format!("/proc/self/task/{tid}/stat"))?.take(4097);
        let mut n = 0;
        loop {
            let got = file.read(&mut data[n..])?;
            if got == 0 {
                break;
            }
            n += got;
        }
        parse_identity(&data[..n], tid)
    }
    fn scan(limit: usize) -> io::Result<Vec<Identity>> {
        let mut threads = Vec::with_capacity(limit);
        for entry in std::fs::read_dir("/proc/self/task")? {
            if threads.len() == limit {
                return Err(invalid("thread capacity exceeded"));
            }
            let entry = entry?;
            let tid = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|tid| *tid > 0 && *tid <= i32::MAX as u32)
                .ok_or_else(|| invalid("invalid task directory"))?;
            threads.push(read_identity(tid)?);
        }
        Ok(threads)
    }
    impl ProcessPebsCollector {
        /// Validate bounds before internal allocation or attachment. Object must be
        /// the trusted libcram eBPF artifact, not arbitrary untrusted BPF code.
        /// CPUs and raw configurations are explicit, caller-audited choices.
        pub fn new(
            object: Vec<u8>,
            cpus: Vec<u32>,
            configs: Vec<ExplicitPebsConfig>,
            max_attachments: usize,
            max_events: usize,
        ) -> io::Result<Self> {
            if object.is_empty()
                || object.len() > MAX_OBJECT_BYTES
                || cpus.is_empty()
                || max_attachments == 0
                || max_attachments > MAX_ATTACHMENTS
                || cpus.len() > max_attachments
                || max_events == 0
                || max_events > MAX_EVENTS
                || configs.is_empty()
                || configs.len() > 2
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid bounded PEBS configuration",
                ));
            }
            for (i, cpu) in cpus.iter().enumerate() {
                if *cpu > i32::MAX as u32 || cpus[..i].contains(cpu) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid/duplicate CPU",
                    ));
                }
            }
            for c in &configs {
                c.validate()?;
            }
            if configs.len() == 2 && configs[0].kind == configs[1].kind {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "duplicate operation",
                ));
            }
            Ok(Self {
                object,
                cpus,
                configs,
                max_attachments,
                max_events,
                attachments: Vec::new(),
                cursor: 0,
                owner: std::process::id(),
                status: ProcessPebsStatus::NotPolled,
            })
        }
        pub fn status(&self) -> ProcessPebsStatus {
            self.status
        }
        pub fn attached_samplers(&self) -> usize {
            self.attachments.len()
        }
        fn fail<T>(&mut self, status: ProcessPebsStatus, error: io::Error) -> io::Result<T> {
            self.attachments.clear();
            self.cursor = 0;
            self.status = status;
            Err(error)
        }
        pub fn poll(&mut self) -> io::Result<Vec<AccessEvent>> {
            if self.owner != std::process::id() {
                return self.fail(
                    ProcessPebsStatus::RefreshFailure,
                    invalid("collector cannot cross fork"),
                );
            }
            let threads = match scan(self.max_attachments / self.cpus.len()) {
                Ok(t) => t,
                Err(e) => {
                    return self.fail(
                        if e.kind() == io::ErrorKind::InvalidData {
                            ProcessPebsStatus::CapacityExceeded
                        } else {
                            ProcessPebsStatus::RefreshFailure
                        },
                        e,
                    )
                }
            };
            let wanted = match plan(&threads, &self.cpus, self.max_attachments) {
                Ok(p) => p,
                Err(e) => return self.fail(ProcessPebsStatus::CapacityExceeded, e),
            };
            self.attachments.retain(|(key, _)| wanted.contains(key));
            for key in &wanted {
                if self.attachments.iter().any(|(k, _)| k == key) {
                    continue;
                }
                let sampler = match PebsSampler::attach(
                    &self.object,
                    key.identity.tid,
                    key.cpu,
                    &self.configs,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        return self.fail(
                            if matches!(
                                e.kind(),
                                io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
                            ) {
                                ProcessPebsStatus::MissingCapability
                            } else {
                                ProcessPebsStatus::AttachFailure
                            },
                            e,
                        )
                    }
                };
                // Recheck membership and generation after perf_event_open. On race,
                // the local sampler and all previously created samplers are dropped.
                match read_identity(key.identity.tid) {
                    Ok(now) if now == key.identity => (),
                    _ => {
                        return self.fail(
                            ProcessPebsStatus::RefreshFailure,
                            invalid("thread changed during attachment"),
                        )
                    }
                }
                self.attachments.push((*key, sampler));
            }
            self.attachments.sort_unstable_by_key(|(key, _)| *key);
            let mut result = Vec::with_capacity(self.max_events);
            let mut empty = 0;
            let count = self.attachments.len();
            // One record per turn, at most max_events attempts. Empty rings also
            // consume a turn; this bounds work independently of producer activity.
            for _ in 0..self.max_events {
                let index = next_ring(&mut self.cursor, count);
                let report = self.attachments[index]
                    .1
                    .poll_events(1, |event| result.push(event));
                if report.rejected != 0 {
                    return self.fail(
                        ProcessPebsStatus::DecodeFailure,
                        invalid("invalid PEBS event"),
                    );
                }
                if report.accepted == 0 {
                    empty += 1;
                } else {
                    empty = 0;
                }
                if empty == count {
                    break;
                }
            }
            for index in 0..count {
                match self.attachments[index].1.dropped_samples() {
                    Ok(0) => (),
                    Ok(_) => return self.fail(ProcessPebsStatus::Loss, invalid("PEBS ring loss")),
                    Err(e) => return self.fail(ProcessPebsStatus::Loss, e),
                }
            }
            if result.iter().any(|e| e.tgid != self.owner) {
                return self.fail(ProcessPebsStatus::DecodeFailure, invalid("foreign TGID"));
            }
            self.status = if result.is_empty() {
                ProcessPebsStatus::NoSamples
            } else {
                ProcessPebsStatus::Sampling
            };
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_reuse_exit_and_capacity() {
        let old = Identity {
            tid: 7,
            starttime: 1,
        };
        let reused = Identity {
            tid: 7,
            starttime: 2,
        };
        let fresh = Identity {
            tid: 9,
            starttime: 3,
        };
        let first = plan(&[old], &[0, 2], 4).expect("first");
        let next = plan(&[reused, fresh], &[0, 2], 4).expect("next");
        assert_eq!(first.len(), 2);
        assert_eq!(next.len(), 4);
        assert!(!next.iter().any(|key| first.contains(key)));
        assert!(plan(&[reused, fresh], &[0, 2], 3).is_err());
        assert!(plan(&[], &[0], 4).is_err());
        assert!(plan(&[old, old], &[0], 4).is_err());
    }
    #[test]
    fn bounded_stat_parser_handles_parentheses() {
        let stat = format!("7 (worker ) name) S {} 123 0", "0 ".repeat(18));
        assert_eq!(
            parse_identity(stat.as_bytes(), 7).expect("stat"),
            Identity {
                tid: 7,
                starttime: 123
            }
        );
        assert!(parse_identity(stat.as_bytes(), 8).is_err());
        assert!(parse_identity(b"7 (broken) S", 7).is_err());
        assert!(parse_identity(&[b'0'; 4097], 7).is_err());
    }
    #[test]
    fn budget_rotates_even_when_smaller_than_ring_count() {
        let mut cursor = 0;
        let mut visits = Vec::new();
        for _ in 0..6 {
            visits.push(next_ring(&mut cursor, 3));
        }
        assert_eq!(visits, [0, 1, 2, 0, 1, 2]);
    }
}
