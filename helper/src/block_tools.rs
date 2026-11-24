//! Explicit, bounded observation of Linux block-layer objects. No totals across
//! disks, partitions or stacked devices. Passive inventory is separate from
//! the one explicit controller health observation; no disk write actions.
mod aliases;
mod health;
mod kernel;
pub(crate) use health::HealthQuery;

use crate::maintenance::HostMounts;
use crate::pinned::{Entity, require_no_automount};
use rustix::fs::{Mode, OFlags, makedev, open, openat};
use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_DEVICES: usize = 256;
const MAX_MOUNTS: usize = 32;
pub(crate) const SECTOR_BYTES: u128 = 512; // Linux block statistics and sysfs size, even on 4Kn
type Field<T> = Result<T, Reason>;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Reason {
    NotReported,
    Denied,
    Unsupported,
    Invalid,
    Changed,
    Limit,
    NoSample,
    Reset,
    Io,
}
impl Reason {
    fn io_kind(kind: io::ErrorKind) -> Self {
        match kind {
            io::ErrorKind::PermissionDenied => Self::Denied,
            io::ErrorKind::NotFound => Self::NotReported,
            io::ErrorKind::Unsupported => Self::Unsupported,
            io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => Self::Invalid,
            _ => Self::Io,
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotReported => "not-reported",
            Self::Denied => "permission-denied",
            Self::Unsupported => "unsupported",
            Self::Invalid => "invalid-source",
            Self::Changed => "changed",
            Self::Limit => "limit",
            Self::NoSample => "not-requested",
            Self::Reset => "counter-reset",
            Self::Io => "io",
        }
    }
}

fn invalid(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}
fn unsupported(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why)
}
// Only these terminal source names may cross from / into a kernel mount.
// Their callers still independently qualify sysfs or optional devtmpfs.
enum BlockRoot {
    Sysfs,
    DeviceAliases,
}
impl BlockRoot {
    const PIN: OFlags = OFlags::PATH.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

    fn pin(self) -> io::Result<File> {
        let parent = File::from(open(c"/", Self::PIN, Mode::empty())?);
        // A child of an indirect autofs root can itself be a lookup trigger.
        Self::no_automount(&parent)?;
        self.pin_at(&parent)
    }

    fn pin_at(self, parent: &File) -> io::Result<File> {
        let name = match self {
            Self::Sysfs => c"sys",
            Self::DeviceAliases => c"dev",
        };
        // O_DIRECTORY (or a trailing slash) would express a traversal intent
        // even with O_PATH. Pin only the terminal name, then check its type.
        // This does not require an already-warmed path cache.
        let fd = File::from(openat(parent, name, Self::PIN, Mode::empty())?);
        Self::no_automount(&fd)?;
        if !fd.metadata()?.is_dir() {
            return Err(invalid("expected a fixed block observation directory"));
        }
        Ok(fd)
    }

    fn no_automount(fd: &File) -> io::Result<()> {
        require_no_automount(fd).map_err(|err| {
            if err.kind() == io::ErrorKind::InvalidData {
                unsupported("fixed block observation source is an automount, not an already mounted directory")
            } else {
                err
            }
        })
    }
}

fn number(b: &[u8]) -> Option<u64> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse().ok()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Dev(pub(crate) u32, pub(crate) u32);
impl Dev {
    fn parse(raw: &[u8]) -> Option<Self> {
        let colon = raw.iter().position(|v| *v == b':')?;
        let d = Self(
            u32::try_from(number(&raw[..colon])?).ok()?,
            u32::try_from(number(&raw[colon + 1..])?).ok()?,
        );
        (d.0 != 0 && d.to_string().as_bytes() == raw).then_some(d)
    }
    fn rdev(self) -> u64 {
        makedev(self.0, self.1)
    }
}
impl fmt::Display for Dev {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.0, self.1)
    }
}

#[derive(Debug)]
pub(crate) struct Query {
    device: Option<Dev>,
    rates: bool,
}
impl Query {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        use std::os::unix::ffi::OsStrExt;
        let (mut device, mut rates) = (None, false);
        while let Some(arg) = args.next() {
            if arg == "--rates" && !rates {
                rates = true;
            } else if arg == "--device" && device.is_none() {
                let raw = args.next().ok_or("expected MAJOR:MINOR")?;
                device = Some(
                    Dev::parse(raw.as_bytes()).ok_or("expected a canonical numeric MAJOR:MINOR")?,
                );
            } else {
                return Err("expected only --device MAJOR:MINOR and/or --rates, once each");
            }
        }
        Ok(Self { device, rates })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Disk,
    Partition {
        number: u32,
        start: u64,
        parent: Dev,
        parent_object: Entity,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) dev: Dev,
    object: Entity,
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) sectors: Field<u64>,
    pub(crate) diskseq: Field<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counts {
    reads: u64,
    read_sectors: u64,
    writes: u64,
    write_sectors: u64,
}
impl Counts {
    fn parse(bytes: &[u8]) -> Field<Self> {
        let v: Vec<_> = bytes
            .split(u8::is_ascii_whitespace)
            .filter(|v| !v.is_empty())
            .map(number)
            .collect::<Option<Vec<_>>>()
            .ok_or(Reason::Invalid)?;
        if !matches!(v.len(), 11 | 15 | 17) {
            return Err(Reason::Unsupported);
        }
        Ok(Self {
            reads: v[0],
            read_sectors: v[2],
            writes: v[4],
            write_sectors: v[6],
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rate {
    pub(crate) read_bytes: u128,
    pub(crate) write_bytes: u128,
    pub(crate) interval_ns: u128,
}
impl Rate {
    // Keep the actual byte deltas. A busy device that transferred less than
    // one byte per second on average is distinguishable from one that was idle.
    pub(crate) fn bps(self, bytes: u128) -> String {
        let average = bytes * 1_000_000_000 / self.interval_ns; // bytes <= u64::MAX * 512
        if bytes != 0 && average == 0 {
            "<1".into()
        } else {
            average.to_string()
        }
    }
}
fn rates(
    original: &Identity,
    now: &Identity,
    a: Counts,
    b: Counts,
    elapsed: Duration,
) -> Field<Rate> {
    if original != now {
        return Err(Reason::Changed);
    }
    now.diskseq?;
    now.sectors?; // name, device number and equal counters cannot prove one lifetime
    let ns = elapsed.as_nanos();
    if ns == 0 {
        return Err(Reason::Invalid);
    }
    if b.reads < a.reads || b.writes < a.writes {
        return Err(Reason::Reset);
    }
    let bytes = |x: u64, y: u64| -> Field<u128> {
        let d = y.checked_sub(x).ok_or(Reason::Reset)?;
        Ok(u128::from(d) * SECTOR_BYTES)
    };
    Ok(Rate {
        read_bytes: bytes(a.read_sectors, b.read_sectors)?,
        write_bytes: bytes(a.write_sectors, b.write_sectors)?,
        interval_ns: ns,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentSource {
    KernelAncestor,
    InferredParent,
}
impl AttachmentSource {
    fn label(self) -> &'static str {
        match self {
            Self::KernelAncestor => "observed-sysfs-ancestor",
            Self::InferredParent => "inferred-from-partition-parent",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Attachment {
    pub(crate) subsystem: &'static str,
    pub(crate) source: AttachmentSource,
    pub(crate) nvme_transport: Field<&'static str>,
}
pub(crate) struct Row {
    pub(crate) id: Identity,
    pub(crate) model: Field<String>,
    pub(crate) fs_uuid: Field<String>,
    pub(crate) partuuid: Field<String>,
    pub(crate) mount_ids: Field<Vec<u64>>,
    pub(crate) rate: Field<Rate>,
    pub(crate) attachment: Field<Attachment>,
}
pub(crate) struct Report {
    pub(crate) start_ms: Option<u128>,
    pub(crate) end_ms: Option<u128>,
    requested_rates: bool,
    pub(crate) rows: Vec<(Dev, Field<Row>)>,
}
fn utc_ms() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|v| v.as_millis())
}

impl Query {
    pub(crate) fn observe(&self) -> io::Result<Report> {
        let start_ms = utc_ms();
        let sys = kernel::Sys::open()?;
        let devices = match self.device {
            Some(d) => vec![d],
            None => sys.devices()?,
        };
        let mut samples = Vec::with_capacity(devices.len());
        for dev in devices {
            let entry: Field<_> = (|| {
                let node = sys
                    .pin(dev)
                    .map_err(|error| Reason::io_kind(error.kind()))?;
                let identity = node
                    .identity(&sys)
                    .map_err(|error| Reason::io_kind(error.kind()))?;
                let first = self.rates.then(|| node.counts());
                Ok((node, identity, first))
            })();
            samples.push((dev, entry));
        }
        // No synthetic transfers. Other processes may legitimately do no I/O.
        if self.rates {
            std::thread::sleep(Duration::from_secs(1));
        }
        let dev_root = aliases::DevRoot::pin(); // retain the original mount before taking the shared table
        let mounts = HostMounts::current().map_err(|error| Reason::io_kind(error.kind()));
        let aliases = aliases::read(dev_root, mounts.as_ref().map_err(|r| *r));
        let mut rows = Vec::with_capacity(samples.len());
        for (dev, entry) in samples {
            let row = entry.and_then(|(node, id, first)| {
                let second = self.rates.then(|| node.counts());
                let model = node.model(&sys, &id);
                let attachment = node.attachment(&sys, &id);
                let current = node
                    .identity(&sys)
                    .map_err(|error| Reason::io_kind(error.kind()))?;
                if current != id {
                    return Err(Reason::Changed);
                }
                let rate = match (first, second) {
                    (Some(Ok((t0, a))), Some(Ok((t1, b)))) => rates(
                        &id,
                        &current,
                        a,
                        b,
                        t1.checked_duration_since(t0).ok_or(Reason::Invalid)?,
                    ),
                    (Some(Err(why)), _) | (_, Some(Err(why))) => Err(why),
                    _ => Err(Reason::NoSample),
                };
                let mount_ids = mounts.as_ref().map_err(|v| *v).and_then(|m| {
                    let ids: Vec<_> = m.ids_for_device(dev.rdev()).take(MAX_MOUNTS + 1).collect();
                    if ids.len() > MAX_MOUNTS {
                        Err(Reason::Limit)
                    } else {
                        Ok(ids)
                    }
                });
                let fs_uuid = aliases.fs.get(dev);
                let partuuid = match id.kind {
                    Kind::Disk => Err(Reason::NotReported),
                    Kind::Partition { .. } => aliases.partition.get(dev),
                };
                Ok(Row {
                    id,
                    model,
                    fs_uuid,
                    partuuid,
                    mount_ids,
                    rate,
                    attachment,
                })
            });
            rows.push((dev, row));
        }
        // Do not attribute old sysfs observations to a newer visible source.
        // A replacement descriptor serves only as evidence, never as a target.
        sys.require_named()?;
        Ok(Report {
            start_ms,
            end_ms: utc_ms(),
            requested_rates: self.rates,
            rows,
        })
    }
}

fn value<T: fmt::Display>(v: &Field<T>) -> String {
    match v {
        Ok(v) => v.to_string(),
        Err(r) => format!("unknown({})", r.label()),
    }
}
impl Report {
    pub(crate) fn complete(&self) -> bool {
        self.rows.iter().all(|(_, r)| {
            r.as_ref().is_ok_and(|v| {
                v.id.sectors.is_ok()
                    && v.mount_ids.is_ok()
                    && (!self.requested_rates || v.rate.is_ok())
            })
        })
    }
}
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "source=linux-sysfs+thread-mountinfo rows={} totals=not-summed health=not-checked registration=not-checked",
            self.rows.len()
        )?;
        match (self.start_ms, self.end_ms) {
            (Some(a), Some(b)) if b >= a => write!(f, " sample_unix_ms={a}..{b}")?,
            _ => f.write_str(" sample_unix_ms=unknown")?,
        }
        for (dev, row) in &self.rows {
            write!(f, "\ndev={dev}")?;
            let row = match row {
                Ok(r) => r,
                Err(r) => {
                    write!(f, " observation=unknown({})", r.label())?;
                    continue;
                }
            };
            let id = &row.id;
            write!(f, " kernel_name={}", id.name)?;
            match id.kind {
                Kind::Disk => f.write_str(" kind=whole-block-device")?,
                Kind::Partition {
                    number,
                    start,
                    parent,
                    ..
                } => write!(
                    f,
                    " kind=partition partition_number={number} start_sector_512={start} parent={parent}"
                )?,
            }
            write!(
                f,
                " capacity_bytes={} diskseq={} model_sysfs={} fs_uuid_udev_alias={} partuuid_udev_alias={}",
                value(&id.sectors.map(|n| u128::from(n) * SECTOR_BYTES)),
                value(&id.diskseq),
                value(&row.model),
                value(&row.fs_uuid),
                value(&row.partuuid)
            )?;
            write!(
                f,
                " ancestor_subsystem_sysfs={} subsystem_evidence={} nvme_transport_sysfs={}",
                value(&row.attachment.map(|a| a.subsystem)),
                value(&row.attachment.map(|a| a.source.label())),
                value(&row.attachment.and_then(|a| a.nvme_transport))
            )?;
            f.write_str(" direct_mount_ids=")?;
            match &row.mount_ids {
                Ok(m) if m.is_empty() => f.write_str("no-match")?,
                Ok(m) => {
                    for (i, n) in m.iter().enumerate() {
                        if i > 0 {
                            f.write_str(",")?;
                        }
                        write!(f, "{n}")?;
                    }
                }
                Err(r) => write!(f, "unknown({})", r.label())?,
            }
            if self.requested_rates {
                match row.rate {
                    Ok(r) => write!(
                        f,
                        " interval_ns={} read_Bps={} write_Bps={}",
                        r.interval_ns,
                        r.bps(r.read_bytes),
                        r.bps(r.write_bytes)
                    )?,
                    Err(r) => write!(f, " rate=unknown({})", r.label())?,
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/block/observations.rs"]
mod tests;
