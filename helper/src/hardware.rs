//! Passive CPU and memory observation from two fixed files on a retained
//! genuine procfs. No pool, service, device, process list or settings I/O.
use crate::pinned::{Entity, Mount, passive_proc_child, passive_proc_root};
use rustix::fs::{Mode, OFlags, PROC_SUPER_MAGIC, ResolveFlags, fstatfs, openat2};
use std::{
    ffi::OsStr,
    fmt::{self, Display},
    fs::File,
    io::{self, BufRead, BufReader, Read},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CPU_LINE_LIMIT: usize = 1024;
const MEMORY_LIMIT: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Unknown {
    Unavailable,
    Denied,
    Unsupported,
    Invalid,
    Changed,
    CounterDecrease,
    NoInterval,
    Io,
}

impl Unknown {
    fn io(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::WouldBlock => Self::Unavailable,
            io::ErrorKind::PermissionDenied => Self::Denied,
            io::ErrorKind::Unsupported => Self::Unsupported,
            io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => Self::Invalid,
            _ => Self::Io,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Denied => "permission-denied",
            Self::Unsupported => "unsupported",
            Self::Invalid => "invalid-or-incomplete",
            Self::Changed => "kernel-source-changed",
            Self::CounterDecrease => "counter-decreased",
            Self::NoInterval => "no-comparable-interval",
            Self::Io => "io",
        }
    }
}

type Field<T> = Result<T, Unknown>;

#[derive(Clone, Copy)]
enum Metric {
    Cpu,
    Memory,
}
impl Metric {
    fn name(self) -> &'static OsStr {
        OsStr::new(match self {
            Self::Cpu => "stat",
            Self::Memory => "meminfo",
        })
    }
}

struct Source<'a> {
    root: &'a File,
    mount: u64,
    metric: Metric,
    pin: File,
    entity: Entity,
}
impl<'a> Source<'a> {
    fn pin(root: &'a File, mount: u64, metric: Metric) -> Field<Self> {
        let pin = passive_proc_child(root, metric.name(), mount, false).map_err(Unknown::io)?;
        let entity = identity(&pin).map_err(Unknown::io)?;
        Ok(Self { root, mount, metric, pin, entity })
    }

    fn reader(&self) -> Field<File> {
        // The only readable activation is a fixed single component, already
        // pinned as a regular file from this proc mount. No overmount, link,
        // proc magic link, non-proc filesystem or arbitrary input is allowed.
        let f = File::from(
            openat2(
                self.root,
                self.metric.name(),
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .map_err(io::Error::from)
            .map_err(Unknown::io)?,
        );
        if !f.metadata().map_err(Unknown::io)?.is_file()
            || fstatfs(&f).map_err(io::Error::from).map_err(Unknown::io)?.f_type != PROC_SUPER_MAGIC
            || Mount::on(&f).map_err(Unknown::io)?.is_none_or(|m| m.id != self.mount)
            || identity(&f).map_err(Unknown::io)? != self.entity
        {
            return Err(Unknown::Changed);
        }
        Ok(f)
    }

    fn require_named(&self) -> Field<()> {
        let now = passive_proc_child(self.root, self.metric.name(), self.mount, false)
            .map_err(Unknown::io)?;
        if identity(&now).map_err(Unknown::io)? != self.entity
            || identity(&self.pin).map_err(Unknown::io)? != self.entity
        {
            return Err(Unknown::Changed);
        }
        Ok(())
    }
}

fn identity(f: &File) -> io::Result<Entity> {
    Entity::of(&f.metadata()?)
}

fn words(b: &[u8]) -> impl Iterator<Item = &[u8]> {
    b.split(u8::is_ascii_whitespace).filter(|s| !s.is_empty())
}

fn number(b: Option<&[u8]>) -> Field<u64> {
    let b = b.filter(|v| !v.is_empty() && v.iter().all(u8::is_ascii_digit)).ok_or(Unknown::Invalid)?;
    std::str::from_utf8(b).ok().and_then(|s| s.parse().ok()).ok_or(Unknown::Invalid)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ticks([u64; 8]);
impl Ticks {
    fn decode(line: &[u8]) -> Field<Self> {
        if line.len() > CPU_LINE_LIMIT || !line.ends_with(b"\n") || line[..line.len()-1].contains(&b'\n') {
            return Err(Unknown::Invalid);
        }
        let mut fields = words(line);
        if fields.next() != Some(b"cpu") {
            return Err(Unknown::Invalid); // never substitute an individual core
        }
        let mut times = [0; 8];
        for v in &mut times {
            *v = number(fields.next())?;
        }
        // The current Linux aggregate line has two additional guest fields.
        // Their ticks are already included in user and nice. Do not sum twice.
        for _ in 0..2 {
            number(fields.next())?;
        }
        if fields.next().is_some() {
            return Err(Unknown::Invalid);
        }
        Ok(Self(times))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Percent(u16);
impl Percent {
    fn of(numerator: u128, total: u128) -> Self {
        // In this caller: 0 <= numerator <= total; sum of at most 8 u64
        // deltas, so hundredths-of-percent rounding cannot overflow u128.
        Self(((numerator * 10_000 + total / 2) / total) as u16)
    }
}
impl Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cpu {
    pub(crate) interval_ns: u128,
    pub(crate) active: Percent,
    pub(crate) iowait: Percent,
    pub(crate) steal: Percent,
}
impl Cpu {
    fn between(first: Ticks, last: Ticks, elapsed: Duration) -> Field<Self> {
        let mut delta = [0; 8];
        for ((slot, before), after) in delta.iter_mut().zip(first.0).zip(last.0) {
            *slot = u128::from(after.checked_sub(before).ok_or(Unknown::CounterDecrease)?);
        }
        let total = delta.iter().sum();
        if elapsed.is_zero() || total == 0 {
            return Err(Unknown::NoInterval);
        }
        let [user, nice, system, _, iowait, irq, softirq, steal] = delta;
        Ok(Self {
            interval_ns: elapsed.as_nanos(),
            active: Percent::of(user + nice + system + irq + softirq, total),
            iowait: Percent::of(iowait, total),
            steal: Percent::of(steal, total),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Memory {
    pub(crate) total_bytes: u128,
    pub(crate) available_bytes: u128,
}
impl Memory {
    fn decode(bytes: &[u8]) -> Field<Self> {
        if bytes.len() > MEMORY_LIMIT || !bytes.ends_with(b"\n") {
            return Err(Unknown::Invalid);
        }
        let (mut total, mut available) = (None, None);
        for line in bytes.split(|b| *b == b'\n') {
            let mut fields = words(line);
            let slot = match fields.next() {
                Some(b"MemTotal:") => &mut total,
                Some(b"MemAvailable:") => &mut available,
                _ => continue, // unrelated counters are not exposed or interpreted
            };
            let value = number(fields.next())?;
            if fields.next() != Some(b"kB") || fields.next().is_some() || slot.replace(value).is_some() {
                return Err(Unknown::Invalid);
            }
        }
        match (total, available) {
            (Some(t), Some(a)) if t != 0 && a <= t => Ok(Self {
                total_bytes: u128::from(t) * 1024,
                available_bytes: u128::from(a) * 1024,
            }),
            (Some(_), Some(_)) => Err(Unknown::Invalid),
            _ => Err(Unknown::Unavailable), // never substitute MemFree or zero
        }
    }
}

fn cpu_read(source: &Source<'_>) -> Field<(Instant, Ticks)> {
    let mut bytes = Vec::new();
    BufReader::new(source.reader()?.take(CPU_LINE_LIMIT as u64))
        .read_until(b'\n', &mut bytes)
        .map_err(Unknown::io)?;
    let sampled = Instant::now();
    source.require_named()?;
    Ok((sampled, Ticks::decode(&bytes)?))
}

fn cpu(root: &File, mount: u64) -> Field<Cpu> {
    let source = Source::pin(root, mount, Metric::Cpu)?;
    let (start, before) = cpu_read(&source)?;
    thread::sleep(Duration::from_secs(1));
    let (end, after) = cpu_read(&source)?;
    Cpu::between(before, after, end.checked_duration_since(start).ok_or(Unknown::NoInterval)?)
}

fn memory(root: &File, mount: u64) -> Field<Memory> {
    let source = Source::pin(root, mount, Metric::Memory)?;
    let mut bytes = Vec::new();
    source.reader()?.take((MEMORY_LIMIT + 1) as u64).read_to_end(&mut bytes).map_err(Unknown::io)?;
    source.require_named()?;
    Memory::decode(&bytes)
}

pub(crate) struct Report {
    pub(crate) started_ms: Option<u128>,
    pub(crate) ended_ms: Option<u128>,
    pub(crate) cpu: Field<Cpu>,
    pub(crate) memory: Field<Memory>,
}
impl Report {
    pub(crate) fn complete(&self) -> bool {
        self.cpu.is_ok() && self.memory.is_ok()
    }
}

fn unix_ms() -> Option<u128> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|t| t.as_millis())
}

pub(crate) fn observe() -> Report {
    let started_ms = unix_ms();
    let pair = (|| {
        let (root, mount) = passive_proc_root().map_err(Unknown::io)?;
        let (cpu, memory) = (cpu(&root, mount), memory(&root, mount));
        let (now, current_mount) = passive_proc_root().map_err(Unknown::io)?;
        if current_mount != mount || identity(&root).map_err(Unknown::io)? != identity(&now).map_err(Unknown::io)? {
            return Err(Unknown::Changed);
        }
        Ok((cpu, memory))
    })();
    let (cpu, memory) = pair.unwrap_or_else(|why| (Err(why), Err(why)));
    let ended_ms = unix_ms().filter(|end| started_ms.is_some_and(|start| start <= *end));
    Report { started_ms, ended_ms, cpu, memory }
}

impl Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("source=linux-procfs scope=kernel-aggregate not=zxs-process-or-cgroup started_unix_ms=")?;
        let stamp = |x: Option<u128>| x.map_or_else(|| "unknown".into(), |v| v.to_string());
        writeln!(f, "{} ended_unix_ms={}", stamp(self.started_ms), stamp(self.ended_ms))?;
        match self.cpu {
            Ok(c) => writeln!(f, "cpu_interval_ns={} cpu_active_pct={} cpu_iowait_pct={} cpu_steal_pct={}",
                c.interval_ns, c.active, c.iowait, c.steal)?,
            Err(why) => writeln!(f, "cpu=unknown({})", why.label())?,
        }
        match self.memory {
            Ok(m) => write!(f, "ram_total_bytes={} ram_available_estimate_bytes={}", m.total_bytes, m.available_bytes),
            Err(why) => write!(f, "ram=unknown({})", why.label()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_cpu_uses_a_single_aggregate_and_does_not_double_count_guest_time() {
        let before = Ticks::decode(b"cpu  20 4 9 80 30 2 3 7 6 1\n").unwrap();
        let after = Ticks::decode(b"cpu  38 8 21 122 38 4 5 19 14 2\n").unwrap();
        let c = Cpu::between(before, after, Duration::from_millis(1042)).unwrap();
        assert_eq!(c.interval_ns, 1_042_000_000);
        assert_eq!((c.active.to_string(), c.iowait.to_string(), c.steal.to_string()),
                   ("38.00".into(), "8.00".into(), "12.00".into()));
    }

    #[test]
    fn hardware_cpu_discontinuity_or_bad_shape_is_never_zero_utilization() {
        let first = Ticks([10; 8]);
        let mut decreased = first;
        decreased.0[4] -= 1;
        let window = Duration::from_secs(1);
        assert_eq!(Cpu::between(first, decreased, window), Err(Unknown::CounterDecrease));
        assert_eq!(Cpu::between(first, first, window), Err(Unknown::NoInterval));
        assert_eq!(Cpu::between(first, Ticks([100; 8]), Duration::ZERO), Err(Unknown::NoInterval));
        let idle = Cpu::between(Ticks([0; 8]), Ticks([0, 0, 0, 5, 0, 0, 0, 0]), window).unwrap();
        assert_eq!(idle.active.to_string(), "0.00");
        for bad in [
            &b"cpu0 1 2 3 4 5 6 7 8 0 0\n"[..], b"cpu 1 2 3 4 5 6 7 8\n",
            b"cpu 1 2 3 4 5 6 7 8 0 0", b"cpu -1 2 3 4 5 6 7 8 0 0\n",
            b"cpu 1 2 3 4 5 6 7 8 0 0 99\n",
        ] {
            assert_eq!(Ticks::decode(bad), Err(Unknown::Invalid));
        }
    }

    #[test]
    fn hardware_memory_requires_both_exact_fields_and_preserves_measured_zero() {
        let m = Memory::decode(b"MemTotal: 4096 kB\nMemFree: 75 kB\nMemAvailable: 2048 kB\n").unwrap();
        assert_eq!((m.total_bytes, m.available_bytes), (4_194_304, 2_097_152));
        assert_eq!(Memory::decode(b"MemTotal: 4096 kB\nMemAvailable: 0 kB\n").unwrap().available_bytes, 0);
        assert_eq!(Memory::decode(b"MemTotal: 4096 kB\nMemFree: 1024 kB\n"), Err(Unknown::Unavailable));
        for bad in [
            &b"MemTotal: 1 kB\nMemAvailable: 2 kB\n"[..], b"MemTotal: 0 kB\nMemAvailable: 0 kB\n",
            b"MemTotal: 9 kB\nMemAvailable: 1 kB\nMemAvailable: 1 kB\n",
            b"MemTotal: 3 MB\nMemAvailable: 2 kB\n", b"MemTotal: 3 kB\nMemAvailable: 1 kB",
        ] {
            assert_eq!(Memory::decode(bad), Err(Unknown::Invalid));
        }
        assert_eq!(Memory::decode(&vec![b'\n'; MEMORY_LIMIT + 1]), Err(Unknown::Invalid));
    }

    #[test]
    fn hardware_reports_partial_results_without_exposing_kernel_text() {
        let report = Report {
            started_ms: Some(10), ended_ms: Some(12),
            cpu: Err(Unknown::CounterDecrease),
            memory: Ok(Memory { total_bytes: 8192, available_bytes: 4096 }),
        };
        let shown = report.to_string();
        assert!(!report.complete());
        assert!(shown.contains("cpu=unknown(counter-decreased)"));
        assert!(shown.contains("ram_total_bytes=8192 ram_available_estimate_bytes=4096"));
        assert!(!shown.contains("MemTotal:"));
    }
}
