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
/// Minimum spacing of governor updates (PMU count reads, period changes).
pub const RATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// Samples one monitored CPU may take per governor interval: twice the
/// governor's per-second cap times [`RATE_INTERVAL`]. The kernel enforces it
/// through each event's PERF_EVENT_IOC_REFRESH limit. Events start with
/// [`INITIAL_GRANT`]; every interval, events that used their limit share what
/// the CPU's other events do not hold. A burst shorter than an interval can
/// therefore take at most this many samples (the governor alone reacts only
/// after an interval). At twice the cap, a steady governed workload (which
/// stays below the cap) does not reach it, and a truncated count still shows
/// the governor a rate at least twice its cap, so periods rise at once.
pub const INITIAL_GRANT: u32 = 2;
pub fn sample_budget(governor: &crate::telemetry::sampling::RateGovernor) -> u32 {
    ((2.0 * f64::from(governor.max_samples_per_cpu_sec()) * RATE_INTERVAL.as_secs_f64()) as u32)
        .max(1)
}
/// Add the events counted since `previous`, expressed as samples at the
/// configured base periods, to `cpu`'s total, then store `counts` as the next
/// baseline. A count below its baseline is treated as a fresh counter.
fn add_base_samples(
    totals: &mut Vec<(u32, f64)>,
    cpu: u32,
    counts: &[u64],
    previous: &mut [u64],
    base: &[u64],
) {
    let mut sum = 0.0;
    for ((count, prev), period) in counts.iter().zip(previous.iter_mut()).zip(base) {
        let delta = count.checked_sub(*prev).unwrap_or(*count);
        sum += delta as f64 / (*period).max(1) as f64;
        *prev = *count;
    }
    match totals.iter_mut().find(|(c, _)| *c == cpu) {
        Some((_, total)) => *total += sum,
        None => totals.push((cpu, sum)),
    }
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
        pebs::{PebsEvents, PebsProgram},
        sampling::{
            independent_period, validate_pair, AccessEvent, ExplicitPebsConfig, RateGovernor,
            DEFAULT_MAX_SAMPLES_PER_CPU_SEC,
        },
    };
    use std::{collections::BTreeMap, io::Read, time::Instant};

    /// Scaled periods, with the second kept independent of the first so the
    /// two PEBS counters never overflow in lockstep.
    fn governed_periods(
        governor: Option<&RateGovernor>,
        configs: &[ExplicitPebsConfig],
    ) -> Vec<u64> {
        let mut periods: Vec<u64> = configs
            .iter()
            .map(|c| governor.map_or(c.period, |g| g.period(c.period)))
            .collect();
        if let [a, b] = periods[..] {
            periods[1] = independent_period(a, b);
        }
        periods
    }
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
    ///
    /// A [`RateGovernor`] capping samples per monitored CPU is on by default
    /// ([`DEFAULT_MAX_SAMPLES_PER_CPU_SEC`]); see [`Self::with_rate_limit`].
    /// It runs inside `poll`, at most once per [`RATE_INTERVAL`], so its
    /// reaction time is the larger of that and the caller's poll cadence.
    ///
    /// One [`PebsProgram`] (BPF object and ring) is loaded per selected CPU at
    /// its first attachment and shared by every thread attached on that CPU,
    /// so a new thread costs perf_event_open, not a BPF load.
    pub struct ProcessPebsCollector {
        object: Vec<u8>,
        cpus: Vec<u32>,
        configs: Vec<ExplicitPebsConfig>,
        max_attachments: usize,
        max_events: usize,
        attachments: Vec<Attachment>,
        /// Loaded programs keyed by CPU, sorted by CPU.
        programs: Vec<(u32, PebsProgram)>,
        cursor: usize,
        owner: u32,
        status: ProcessPebsStatus,
        governor: Option<RateGovernor>,
        rate_checked: Option<Instant>,
    }
    struct Attachment {
        key: Key,
        events: PebsEvents,
        /// PMU counts at the last governor update, per configured event.
        counts: [u64; 2],
    }
    fn attach_status(e: &io::Error) -> ProcessPebsStatus {
        if matches!(
            e.kind(),
            io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
        ) {
            ProcessPebsStatus::MissingCapability
        } else {
            ProcessPebsStatus::AttachFailure
        }
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
            validate_pair(&configs)?;
            Ok(Self {
                object,
                cpus,
                configs,
                max_attachments,
                max_events,
                attachments: Vec::new(),
                programs: Vec::new(),
                cursor: 0,
                owner: std::process::id(),
                status: ProcessPebsStatus::NotPolled,
                governor: Some(RateGovernor::new(DEFAULT_MAX_SAMPLES_PER_CPU_SEC)?),
                rate_checked: None,
            })
        }
        /// Cap samples per second on each monitored CPU (`None` disables the
        /// governor and keeps the configured periods). Must be called before
        /// the first poll.
        pub fn with_rate_limit(mut self, max_samples_per_cpu_sec: Option<u32>) -> io::Result<Self> {
            if !self.attachments.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "rate limit must be set before attachment",
                ));
            }
            self.governor = max_samples_per_cpu_sec.map(RateGovernor::new).transpose()?;
            Ok(self)
        }
        pub fn status(&self) -> ProcessPebsStatus {
            self.status
        }
        /// Attached (thread, CPU) pairs.
        pub fn attached_samplers(&self) -> usize {
            self.attachments.len()
        }
        /// Loaded BPF programs, at most one per selected CPU.
        pub fn loaded_programs(&self) -> usize {
            self.programs.len()
        }
        /// Shared multiplier currently applied to every configured period
        /// (1.0 when the governor is off or idle).
        pub fn period_scale(&self) -> f64 {
            self.governor.as_ref().map_or(1.0, RateGovernor::scale)
        }
        /// Period currently programmed for each configuration, in order. Each
        /// returned sample stands for about this many events of its kind.
        pub fn periods(&self) -> Vec<u64> {
            governed_periods(self.governor.as_ref(), &self.configs)
        }
        /// Drops every attachment and program: rings and their queued records
        /// are discarded, so a later poll never replays pre-failure output.
        fn fail<T>(&mut self, status: ProcessPebsStatus, error: io::Error) -> io::Result<T> {
            self.attachments.clear();
            self.programs.clear();
            self.cursor = 0;
            self.rate_checked = None;
            self.status = status;
            Err(error)
        }
        /// Read PMU counts, update the governor and reprogram every sampler
        /// when the shared scale changes. Rate-limited by RATE_INTERVAL.
        fn govern(&mut self) -> io::Result<()> {
            let Some(governor) = self.governor.as_mut() else {
                return Ok(());
            };
            let now = Instant::now();
            let Some(last) = self.rate_checked else {
                self.rate_checked = Some(now);
                return Ok(());
            };
            let elapsed = now.saturating_duration_since(last);
            if elapsed < RATE_INTERVAL {
                return Ok(());
            }
            self.rate_checked = Some(now);
            let n = self.configs.len();
            let mut base = [1u64; 2];
            for (b, c) in base.iter_mut().zip(&self.configs) {
                *b = c.period;
            }
            let mut totals = Vec::with_capacity(self.cpus.len());
            let budget = u64::from(sample_budget(governor));
            // Per CPU: (attachment, event, remaining limit, samples this interval).
            let mut limits: BTreeMap<u32, Vec<(usize, usize, u64, u64)>> = BTreeMap::new();
            for (index, a) in self.attachments.iter_mut().enumerate() {
                let counts = a.events.event_counts()?;
                for (event, (remaining, used)) in a.events.account(&counts)?.into_iter().enumerate()
                {
                    limits
                        .entry(a.key.cpu)
                        .or_default()
                        .push((index, event, remaining, used));
                }
                add_base_samples(
                    &mut totals,
                    a.key.cpu,
                    &counts,
                    &mut a.counts[..n],
                    &base[..n],
                );
            }
            for events in limits.values() {
                // Events that ran out share what the CPU's other events do not
                // hold; every other event is topped back up by what it used,
                // so a steady event never runs dry (a spent limit stops its
                // counter and would hide the rate from the governor). Grants
                // never take the CPU's outstanding limits above the budget
                // (plus one per spent event).
                let mut free = budget.saturating_sub(events.iter().map(|e| e.2).sum());
                for &(index, event, remaining, used) in events {
                    let top = used.min(free);
                    if remaining > 1 && top > 0 {
                        self.attachments[index]
                            .events
                            .grant(event, u32::try_from(top).unwrap_or(u32::MAX))?;
                        free -= top;
                    }
                }
                let spent: Vec<_> = events.iter().filter(|e| e.2 <= 1).collect();
                if !spent.is_empty() {
                    let share = u32::try_from(free / spent.len() as u64)
                        .unwrap_or(u32::MAX)
                        .max(1);
                    for &&(index, event, _, _) in &spent {
                        self.attachments[index].events.grant(event, share)?;
                    }
                }
            }
            let elapsed_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
            if governor.update(elapsed_ns, totals.iter().map(|(_, s)| *s)) {
                let periods = governed_periods(Some(governor), &self.configs);
                for a in &mut self.attachments {
                    a.events.set_periods(&periods)?;
                }
            }
            Ok(())
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
            self.attachments.retain(|a| wanted.contains(&a.key));
            // New samplers start at the governed periods, keeping one shared scale.
            let configs: Vec<ExplicitPebsConfig> = self
                .configs
                .iter()
                .zip(self.periods())
                .map(|(c, period)| ExplicitPebsConfig { period, ..*c })
                .collect();
            for key in &wanted {
                if self.attachments.iter().any(|a| a.key == *key) {
                    continue;
                }
                let program = match self.programs.iter().position(|(c, _)| *c == key.cpu) {
                    Some(index) => index,
                    None => match PebsProgram::load(&self.object) {
                        Ok(program) => {
                            self.programs.push((key.cpu, program));
                            self.programs.len() - 1
                        }
                        Err(e) => return self.fail(attach_status(&e), e),
                    },
                };
                let mut events =
                    match self.programs[program]
                        .1
                        .attach(key.identity.tid, key.cpu, &configs)
                    {
                        Ok(events) => events,
                        Err(e) => return self.fail(attach_status(&e), e),
                    };
                if self.governor.is_some() {
                    if let Err(e) = events.limit_samples(INITIAL_GRANT) {
                        return self.fail(ProcessPebsStatus::AttachFailure, e);
                    }
                }
                // Recheck membership and generation after perf_event_open. On race,
                // the local events and all previous attachments are dropped.
                match read_identity(key.identity.tid) {
                    Ok(now) if now == key.identity => (),
                    _ => {
                        return self.fail(
                            ProcessPebsStatus::RefreshFailure,
                            invalid("thread changed during attachment"),
                        )
                    }
                }
                self.attachments.push(Attachment {
                    key: *key,
                    events,
                    counts: [0; 2],
                });
            }
            self.attachments.sort_unstable_by_key(|a| a.key);
            self.programs.sort_unstable_by_key(|(cpu, _)| *cpu);
            let mut result = Vec::with_capacity(self.max_events);
            let mut empty = 0;
            let count = self.programs.len();
            // One record per CPU ring per turn, at most max_events attempts. Empty
            // rings also consume a turn; this bounds work independently of
            // producer activity and keeps a busy CPU from starving the others.
            let attempts = if count == 0 { 0 } else { self.max_events };
            for _ in 0..attempts {
                let index = next_ring(&mut self.cursor, count);
                let report = self.programs[index]
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
                match self.programs[index].1.dropped_samples() {
                    Ok(0) => (),
                    Ok(_) => return self.fail(ProcessPebsStatus::Loss, invalid("PEBS ring loss")),
                    Err(e) => return self.fail(ProcessPebsStatus::Loss, e),
                }
            }
            if result.iter().any(|e| e.tgid != self.owner) {
                return self.fail(ProcessPebsStatus::DecodeFailure, invalid("foreign TGID"));
            }
            if let Err(e) = self.govern() {
                return self.fail(ProcessPebsStatus::RefreshFailure, e);
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
    fn sample_budget_is_twice_the_per_cpu_cap_over_one_interval() {
        use crate::telemetry::sampling::{RateGovernor, DEFAULT_MAX_SAMPLES_PER_CPU_SEC};
        let default = RateGovernor::new(DEFAULT_MAX_SAMPLES_PER_CPU_SEC).expect("cap");
        assert_eq!(sample_budget(&default), 240);
        assert_eq!(sample_budget(&RateGovernor::new(1).expect("cap")), 1);
        assert_eq!(sample_budget(&RateGovernor::new(10).expect("cap")), 2);
    }
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
    fn base_samples_sum_per_cpu_and_advance_baseline() {
        let mut totals = Vec::new();
        let mut a = [100u64, 10];
        add_base_samples(&mut totals, 30, &[300, 50], &mut a, &[100, 20]);
        assert_eq!(a, [300, 50]);
        assert_eq!(totals, [(30, 2.0 + 2.0)]);
        // Another thread on the same CPU adds; another CPU is separate.
        let mut b = [0u64, 0];
        add_base_samples(&mut totals, 30, &[1000, 0], &mut b, &[100, 20]);
        add_base_samples(&mut totals, 12, &[50], &mut [0], &[100]);
        assert_eq!(totals, [(30, 14.0), (12, 0.5)]);
        // No new events: zero; a reset counter restarts from zero.
        let mut t2 = Vec::new();
        add_base_samples(&mut t2, 30, &[300, 50], &mut a, &[100, 20]);
        add_base_samples(&mut t2, 30, &[40, 50], &mut a, &[100, 20]);
        assert_eq!(t2, [(30, 0.4)]);
        assert_eq!(a, [40, 50]);
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
