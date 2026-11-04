//! Optional read-only SMART observations. Never a storage-admission input.
use super::{
    DeviceKind, HardwareMapping, HardwareObservation, ObservationError, RootMount,
    dependency_count, nvme_controller, read_attribute, read_device, read_root_mount,
};
use platform_io::disk_smart::{self, AtaData, AtaHealth, AtaPowerMode, CommandTimeout};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};
use std::{
    os::fd::AsFd,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use storage_contract::disk_details::{
    DiskHealth, DiskHealthAssessment, DiskHealthUnavailable as Reason,
};
use storage_engine::{DiskObservationAccess, DiskObservationDirectory};

static QUERY_ACTIVE: AtomicBool = AtomicBool::new(false);
const OUTPUT_LIMIT: usize = 64 * 1024;
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

struct QuerySlot;
impl Drop for QuerySlot {
    fn drop(&mut self) {
        QUERY_ACTIVE.store(false, Ordering::Release);
    }
}

pub(crate) fn observe(
    root: &Path,
    directory: &DiskObservationDirectory,
    hardware: &HardwareObservation,
) -> DiskHealth {
    let reason = match hardware.mapping {
        HardwareMapping::Multiple => Some(Reason::MultipleDevices),
        HardwareMapping::Unknown => Some(Reason::MappingUnknown),
        HardwareMapping::Single => None,
    };
    if let Some(reason) = reason {
        return unavailable(reason);
    }
    if QUERY_ACTIVE
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return unavailable(Reason::Busy);
    }
    let slot = QuerySlot;
    let directory = match directory.try_clone() {
        Ok(directory) => directory,
        Err(error) => return unavailable(directory_reason(error)),
    };
    let root = root.to_owned();
    let mounted_path = hardware.device_path.clone();
    let (send, receive) = mpsc::sync_channel(1);
    // At most one dedicated worker, including kill/reap. A kernel-stuck ioctl
    // retains the slot rather than consuming arbitrarily many HTTP/blocking jobs.
    let worker = std::thread::Builder::new()
        .name("disk-smart".into())
        .spawn(move || {
            let _slot = slot;
            // This guard, not the HTTP receiver, owns every query, device file,
            // child and kill/reap tail. A late worker cannot outlive disk drain.
            let result = directory
                .observe(|access| Ok(query(&root, access, mounted_path.as_deref())))
                .map_err(directory_reason)
                .and_then(std::convert::identity);
            if send.send(result).is_err() {
                tracing::debug!("SMART observation completed after caller deadline");
            }
        });
    if let Err(error) = worker {
        return unavailable(io_reason("start SMART worker", error));
    }
    match receive.recv_timeout(QUERY_TIMEOUT + Duration::from_secs(1)) {
        Ok(Ok(health)) => health,
        Ok(Err(reason)) => unavailable(reason),
        Err(mpsc::RecvTimeoutError::Timeout) => unavailable(Reason::TimedOut),
        Err(mpsc::RecvTimeoutError::Disconnected) => unavailable(Reason::QueryFailed),
    }
}

pub(crate) fn unavailable(reason: Reason) -> DiskHealth {
    DiskHealth::Unavailable { reason }
}

struct DeviceBinding {
    mount: RootMount,
    sysfs_path: PathBuf,
    sysfs: File,
    mounted_sysfs_path: PathBuf,
    mounted_sysfs: File,
    device: File,
    device_path: String,
    protocol: Protocol,
    number: (u32, u32),
}

#[derive(Clone, Copy)]
enum Protocol {
    Nvme,
    Sat,
}

fn query(
    root: &Path,
    directory: &DiskObservationAccess<'_>,
    mounted_path: Option<&str>,
) -> Result<DiskHealth, Reason> {
    let binding = bind(root, directory, mounted_path)?;
    let deadline = Instant::now() + QUERY_TIMEOUT;
    binding.revalidate(root, directory)?;
    let report = match binding.protocol {
        Protocol::Nvme => {
            let health = capture_nvme(&binding.device, directory, deadline)?;
            command_timeout(directory, deadline)?;
            nvme_record(health, observation_time()?, binding.device_path.clone())
        }
        Protocol::Sat => {
            let snapshot = capture_ata(&binding.device, directory, deadline)?;
            command_timeout(directory, deadline)?;
            let observed_at = observation_time()?;
            // The only subprocess is an offline decoder. No target path is in
            // its argv or replay text; all held device/sysfs FDs are CLOEXEC.
            // Official smartctl selects parsed_ata_device for literal "-";
            // missing/malformed replay commands fail without any device fallback.
            let mut command = replay_command(snapshot.input()?);
            let (status, bytes) = run_until(&mut command, deadline, directory)?;
            parse(&bytes, status, observed_at, binding.device_path.clone())?
        }
    };
    binding.revalidate(root, directory)?;
    Ok(report)
}

fn capture_nvme(
    device: &File,
    directory: &DiskObservationAccess<'_>,
    deadline: Instant,
) -> Result<disk_smart::NvmeHealth, Reason> {
    command_timeout(directory, deadline)?;
    let device = disk_smart::BlockDevice::bind(device.as_fd()).map_err(native_reason)?;
    let namespace = disk_smart::nvme_namespace(device).map_err(native_reason)?;
    let controller = disk_smart::nvme_controller(device, command_timeout(directory, deadline)?)
        .map_err(native_reason)?;
    disk_smart::nvme_health(
        device,
        controller.health_namespace(namespace),
        command_timeout(directory, deadline)?,
    )
    .map_err(native_reason)
}

fn observation_time() -> Result<u64, Reason> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            tracing::debug!(?error, "SMART observation clock precedes Unix epoch");
            Reason::QueryFailed
        })?;
    u64::try_from(now.as_millis()).map_err(|_| Reason::QueryFailed)
}

fn nvme_record(
    health: disk_smart::NvmeHealth,
    observed_at_unix_ms: u64,
    device_path: String,
) -> DiskHealth {
    DiskHealth::Smart {
        observed_at_unix_ms,
        device_path,
        assessment: if health.critical_warning == 0 {
            DiskHealthAssessment::Passed
        } else {
            DiskHealthAssessment::Failed
        },
        temperature_celsius: health.temperature_celsius,
        power_on_hours: Some(health.power_on_hours),
    }
}

fn command_timeout(
    directory: &DiskObservationAccess<'_>,
    deadline: Instant,
) -> Result<CommandTimeout, Reason> {
    directory.check_running().map_err(directory_reason)?;
    CommandTimeout::remaining(deadline.saturating_duration_since(Instant::now()))
        .map_err(native_reason)
}

struct AtaSnapshot {
    identify: [u8; 512],
    measurements: Option<AtaMeasurements>,
}

struct AtaMeasurements {
    attributes: [u8; 512],
    thresholds: [u8; 512],
    health: AtaHealth,
}

fn capture_ata(
    device: &File,
    directory: &DiskObservationAccess<'_>,
    deadline: Instant,
) -> Result<AtaSnapshot, Reason> {
    command_timeout(directory, deadline)?;
    let device = disk_smart::BlockDevice::bind(device.as_fd()).map_err(native_reason)?;
    let power = disk_smart::ata_power_mode(device, command_timeout(directory, deadline)?)
        .map_err(native_reason)?;
    if power == AtaPowerMode::LowPower {
        return Err(Reason::LowPower);
    }
    let identify = disk_smart::ata_data(
        device,
        AtaData::Identify,
        command_timeout(directory, deadline)?,
    )
    .map_err(native_reason)?;
    // Let the installed decoder interpret unsupported/disabled/ambiguous SMART
    // capability. Do not enable SMART or send attribute commands in those states.
    let measurements = if ata_smart_readable(&identify) {
        let attributes = disk_smart::ata_data(
            device,
            AtaData::Attributes,
            command_timeout(directory, deadline)?,
        )
        .map_err(native_reason)?;
        let thresholds = disk_smart::ata_data(
            device,
            AtaData::Thresholds,
            command_timeout(directory, deadline)?,
        )
        .map_err(native_reason)?;
        let health = disk_smart::ata_health(device, command_timeout(directory, deadline)?)
            .map_err(native_reason)?;
        Some(AtaMeasurements {
            attributes,
            thresholds,
            health,
        })
    } else {
        None
    };
    Ok(AtaSnapshot {
        identify,
        measurements,
    })
}

fn ata_smart_readable(identify: &[u8; 512]) -> bool {
    let word = |index: usize| u16::from_le_bytes([identify[index * 2], identify[index * 2 + 1]]);
    // ATA IDENTIFY words 83 and 87 carry the validity bits. Unknown support is
    // accepted when SMART is explicitly enabled, as in smartctl; unknown enable
    // state is rejected by its default (non-permissive) mandatory-command policy.
    (word(83) & 0xc000 != 0x4000 || word(82) & 1 != 0)
        && word(87) & 0xc000 == 0x4000
        && word(85) & 1 != 0
}

impl AtaSnapshot {
    fn replay(&self) -> String {
        let mut text = String::with_capacity(8 * 1024);
        replay_data(&mut text, "IDENTIFY DEVICE", &self.identify);
        if let Some(data) = &self.measurements {
            replay_data(&mut text, "SMART READ ATTRIBUTE VALUES", &data.attributes);
            replay_data(
                &mut text,
                "SMART READ ATTRIBUTE THRESHOLDS",
                &data.thresholds,
            );
            let result = match data.health {
                AtaHealth::Passed => "0",
                AtaHealth::Failed => "1",
                AtaHealth::RegistersUnavailable => "-1 errno=38 [ATA return registers unavailable]",
            };
            text.push_str("REPORT-IOCTL: Device=offline Command=SMART STATUS CHECK\n");
            text.push_str("REPORT-IOCTL: Device=offline Command=SMART STATUS CHECK returned ");
            text.push_str(result);
            text.push('\n');
        }
        // Three fixed 512-byte sector dumps and four fixed command records.
        // This is an internal format bound, not a truncating external read.
        assert!(
            text.len() < 8 * 1024,
            "invariant: fixed ATA replay fits 8 KiB"
        );
        text
    }

    fn input(&self) -> Result<File, Reason> {
        // memfd is anonymous RAM-backed input, not a file on the observed disk.
        // Supplying a completed input avoids a pipe writer or a second worker.
        let fd = rustix::fs::memfd_create(c"zxs-smart-replay", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(|error| io_reason("create offline SMART input", error.into()))?;
        let mut file = File::from(fd);
        file.write_all(self.replay().as_bytes())
            .map_err(|error| io_reason("write offline SMART input", error))?;
        file.rewind()
            .map_err(|error| io_reason("rewind offline SMART input", error))?;
        Ok(file)
    }
}

fn replay_data(text: &mut String, name: &'static str, data: &[u8; 512]) {
    use std::fmt::Write;
    writeln!(text, "REPORT-IOCTL: Device=offline Command={name}")
        .expect("invariant: String formatting cannot fail");
    writeln!(
        text,
        "REPORT-IOCTL: Device=offline Command={name} returned 0"
    )
    .expect("invariant: String formatting cannot fail");
    writeln!(text, "===== [{name}] DATA START (BASE-16) =====")
        .expect("invariant: String formatting cannot fail");
    for (index, row) in data.as_chunks::<16>().0.iter().enumerate() {
        write!(text, "{:03}-{:03}:", index * 16, index * 16 + 15)
            .expect("invariant: String formatting cannot fail");
        for byte in row {
            write!(text, " {byte:02x}").expect("invariant: String formatting cannot fail");
        }
        text.push('\n');
    }
    writeln!(text, "===== [{name}] DATA END =====")
        .expect("invariant: String formatting cannot fail");
}

fn replay_command(input: File) -> Command {
    let mut command = Command::new("/usr/sbin/smartctl");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .args(["--json", "--health", "--attributes", "-"])
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn native_reason(error: disk_smart::Error) -> Reason {
    tracing::debug!(?error, "native SMART observation unavailable");
    match error {
        disk_smart::Error::Io { source, .. }
            if source.kind() == io::ErrorKind::PermissionDenied =>
        {
            Reason::PermissionDenied
        }
        disk_smart::Error::Io { source, .. } if source.kind() == io::ErrorKind::TimedOut => {
            Reason::TimedOut
        }
        disk_smart::Error::TimedOut => Reason::TimedOut,
        disk_smart::Error::ScsiStatus { host, driver, .. } if host == 3 || driver & 0x0f == 6 => {
            Reason::TimedOut
        }
        disk_smart::Error::InvalidResponse { .. } => Reason::InvalidOutput,
        _ => Reason::QueryFailed,
    }
}

fn bind(
    root: &Path,
    directory: &DiskObservationAccess<'_>,
    mounted_path: Option<&str>,
) -> Result<DeviceBinding, Reason> {
    let mount = read_root_mount(root, directory).map_err(observation_reason)?;
    let link = PathBuf::from(format!("/sys/dev/block/{}:{}", mount.major, mount.minor));
    let block =
        fs::canonicalize(&link).map_err(|error| io_reason("resolve SMART device", error))?;
    if !block.starts_with("/sys/devices") || block.starts_with("/sys/devices/virtual") {
        return Err(Reason::UnsupportedDevice);
    }
    let mounted = read_device(&block).map_err(observation_reason)?;
    if (mounted.major, mounted.minor) != (mount.major, mount.minor)
        || mounted_path != Some(format!("/dev/{}", mounted.name).as_str())
    {
        return Err(Reason::DeviceChanged);
    }
    directory.check_running().map_err(directory_reason)?;
    let mounted_sysfs =
        File::open(&block).map_err(|error| io_reason("hold mounted sysfs identity", error))?;
    let mounted_sysfs_path = block.clone();
    let disk = match mounted.kind {
        DeviceKind::Disk => block,
        DeviceKind::Partition => block
            .parent()
            .expect("invariant: sysfs block has parent")
            .to_owned(),
    };
    let device = read_device(&disk).map_err(observation_reason)?;
    if device.kind != DeviceKind::Disk {
        return Err(Reason::UnsupportedDevice);
    }
    match dependency_count(&disk).map_err(observation_reason)? {
        0 => {}
        1 => return Err(Reason::UnsupportedDevice),
        _ => return Err(Reason::MultipleDevices),
    }
    let protocol = if nvme_controller(&disk, Path::new("/sys"))
        .map_err(observation_reason)?
        .is_some()
    {
        Protocol::Nvme
    } else if read_attribute(&disk.join("device/vendor"))
        .map_err(observation_reason)?
        .as_deref()
        == Some("ATA")
    {
        Protocol::Sat
    } else {
        return Err(Reason::UnsupportedDevice);
    };
    directory.check_running().map_err(directory_reason)?;
    let sysfs = File::open(&disk).map_err(|error| io_reason("hold SMART sysfs identity", error))?;
    let device_path = format!("/dev/{}", device.name);
    directory.check_running().map_err(directory_reason)?;
    let fd = rustix::fs::open(
        &device_path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| io_reason("open SMART device read-only", error.into()))?;
    let device_file = File::from(fd);
    let metadata = device_file
        .metadata()
        .map_err(|error| io_reason("stat held SMART device", error))?;
    let number = (device.major, device.minor);
    if !metadata.file_type().is_block_device()
        || (
            rustix::fs::major(metadata.rdev()),
            rustix::fs::minor(metadata.rdev()),
        ) != number
    {
        return Err(Reason::DeviceChanged);
    }
    Ok(DeviceBinding {
        mount,
        sysfs_path: disk,
        sysfs,
        mounted_sysfs_path,
        mounted_sysfs,
        device: device_file,
        device_path,
        protocol,
        number,
    })
}

impl DeviceBinding {
    fn revalidate(&self, root: &Path, directory: &DiskObservationAccess<'_>) -> Result<(), Reason> {
        let mount = read_root_mount(root, directory).map_err(observation_reason)?;
        same_held_inode(&self.device, Path::new(&self.device_path))?;
        self.revalidate_mount(mount, Path::new("/sys"))
    }

    fn revalidate_mount(&self, mount: RootMount, sysfs: &Path) -> Result<(), Reason> {
        if mount != self.mount {
            return Err(Reason::DeviceChanged);
        }
        same_held_inode(&self.sysfs, &self.sysfs_path)?;
        same_held_inode(&self.mounted_sysfs, &self.mounted_sysfs_path)?;
        let link = sysfs.join(format!("dev/block/{}:{}", mount.major, mount.minor));
        let mounted = fs::canonicalize(link)
            .map_err(|error| io_reason("revalidate mounted device", error))?;
        if mounted != self.mounted_sysfs_path {
            return Err(Reason::DeviceChanged);
        }
        let mounted_device = read_device(&mounted).map_err(observation_reason)?;
        if (mounted_device.major, mounted_device.minor) != (mount.major, mount.minor) {
            return Err(Reason::DeviceChanged);
        }
        let device = read_device(&self.sysfs_path).map_err(observation_reason)?;
        if (device.major, device.minor) != self.number
            || dependency_count(&self.sysfs_path).map_err(observation_reason)? != 0
        {
            return Err(Reason::DeviceChanged);
        }
        Ok(())
    }
}

fn same_held_inode(held: &File, path: &Path) -> Result<(), Reason> {
    // Holding the inode detects removal/recreation, including dev_t reuse.
    let held = held
        .metadata()
        .map_err(|error| io_reason("stat held device identity", error))?;
    let current =
        fs::metadata(path).map_err(|error| io_reason("revalidate device identity", error))?;
    if (held.dev(), held.ino()) != (current.dev(), current.ino()) || held.nlink() == 0 {
        return Err(Reason::DeviceChanged);
    }
    Ok(())
}

/// Child cleanup stays inside the directory's admitted guard, including panic
/// unwinding. A failed wait cannot silently Drop the child and certify disk drain.
struct QueryChild<'a, 'directory> {
    process: Option<std::process::Child>,
    directory: &'a DiskObservationAccess<'directory>,
}

impl Drop for QueryChild<'_, '_> {
    fn drop(&mut self) {
        let Some(mut process) = self.process.take() else {
            return;
        };
        if let Err(error) = process.kill() {
            tracing::debug!(?error, "stop SMART child");
        }
        if let Err(error) = process.wait() {
            self.directory
                .retain_unresolved_process_cleanup(process, error);
        }
    }
}

fn run_until(
    command: &mut Command,
    deadline: Instant,
    directory: &DiskObservationAccess<'_>,
) -> Result<(i32, Vec<u8>), Reason> {
    command_timeout(directory, deadline)?;
    let process = command.spawn().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => Reason::ToolMissing,
        _ => io_reason("execute smartctl", error),
    })?;
    let mut child = QueryChild {
        process: Some(process),
        directory,
    };
    let result = collect_output(
        child
            .process
            .as_mut()
            .expect("invariant: new SMART child is owned"),
        deadline,
        directory,
    );
    if result.is_ok() {
        // collect_output returns success only after try_wait reports the actual
        // exit and stdout is drained. Error/unwind cleanup belongs to Drop.
        child.process = None;
    }
    result
}

fn collect_output(
    child: &mut std::process::Child,
    deadline: Instant,
    directory: &DiskObservationAccess<'_>,
) -> Result<(i32, Vec<u8>), Reason> {
    let mut stdout = child
        .stdout
        .take()
        .expect("invariant: SMART stdout is piped");
    rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
        .map_err(|error| io_reason("set SMART pipe nonblocking", error.into()))?;
    let mut bytes = Vec::new();
    let mut status = None;
    loop {
        directory.check_running().map_err(directory_reason)?;
        if Instant::now() >= deadline {
            return Err(Reason::TimedOut);
        }
        let mut chunk = [0; 4096];
        match stdout.read(&mut chunk) {
            Ok(0) if status.is_some() => {
                return Ok((status.expect("invariant: status checked"), bytes));
            }
            Ok(count) => {
                if bytes.len() + count > OUTPUT_LIMIT {
                    return Err(Reason::InvalidOutput);
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(io_reason("read SMART output", error)),
        }
        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|error| io_reason("wait SMART child", error))?
                .map(|status| status.code().ok_or(Reason::QueryFailed))
                .transpose()?;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(data_serialization::Deserialize)]
struct SmartOutput {
    smartctl: SmartExit,
    smart_status: Option<SmartStatus>,
    temperature: Option<Temperature>,
    power_on_time: Option<PowerOnTime>,
    power_mode: Option<PowerMode>,
}
#[derive(data_serialization::Deserialize)]
struct SmartExit {
    exit_status: i32,
}
#[derive(data_serialization::Deserialize)]
struct SmartStatus {
    passed: bool,
}
#[derive(data_serialization::Deserialize)]
struct Temperature {
    current: i32,
}
#[derive(data_serialization::Deserialize)]
struct PowerOnTime {
    hours: u64,
}
#[derive(data_serialization::Deserialize)]
struct PowerMode {
    name: String,
}

fn parse(
    bytes: &[u8],
    status: i32,
    observed_at_unix_ms: u64,
    device_path: String,
) -> Result<DiskHealth, Reason> {
    let value: SmartOutput = json_codec::from_slice(bytes).map_err(|error| {
        tracing::debug!(?error, "invalid SMART JSON");
        Reason::InvalidOutput
    })?;
    if value.smartctl.exit_status != status {
        return Err(Reason::InvalidOutput);
    }
    if status == 3 {
        return Err(match value.power_mode.as_ref().map(|v| v.name.as_str()) {
            Some("SLEEP" | "STANDBY" | "STANDBY_Y") => Reason::LowPower,
            _ => {
                tracing::debug!(status, "SMART query failed without a low-power report");
                Reason::QueryFailed
            }
        });
    }
    if !(0..=255).contains(&status) || status & 7 != 0 {
        tracing::debug!(status, "SMART query failed before a complete device report");
        return Err(Reason::QueryFailed);
    }
    let passed = value.smart_status.ok_or(Reason::IncompleteOutput)?.passed;
    if passed && status & 8 != 0 {
        return Err(Reason::InvalidOutput);
    }
    Ok(DiskHealth::Smart {
        observed_at_unix_ms,
        device_path,
        assessment: if !passed || status & 16 != 0 {
            DiskHealthAssessment::Failed
        } else if status & (32 | 64 | 128) != 0 {
            DiskHealthAssessment::Warning
        } else {
            DiskHealthAssessment::Passed
        },
        temperature_celsius: value.temperature.map(|v| v.current),
        power_on_hours: value.power_on_time.map(|v| v.hours),
    })
}

fn observation_reason(error: ObservationError) -> Reason {
    if !matches!(&error, ObservationError::Directory { .. }) {
        tracing::debug!(?error, "SMART device mapping unavailable");
    }
    match error {
        ObservationError::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied => {
            Reason::PermissionDenied
        }
        ObservationError::MountMissing(_) => Reason::DeviceChanged,
        ObservationError::Directory { source, .. } => directory_reason(*source),
        ObservationError::Io { .. } | ObservationError::Invalid { .. } => Reason::MappingUnknown,
    }
}
fn directory_reason(error: storage_engine::Error) -> Reason {
    tracing::debug!(?error, "SMART registered directory observation unavailable");
    match error {
        storage_engine::Error::Filesystem(platform_io::fs_safety::Error::Io(source))
            if source.kind() == io::ErrorKind::PermissionDenied =>
        {
            Reason::PermissionDenied
        }
        error => match error.code() {
            storage_contract::ErrorCode::DiskUnavailable
            | storage_contract::ErrorCode::DiskIdentityMismatch => Reason::DeviceChanged,
            _ => Reason::QueryFailed,
        },
    }
}

fn io_reason(operation: &'static str, error: io::Error) -> Reason {
    tracing::debug!(operation, ?error, "SMART observation unavailable");
    if error.kind() == io::ErrorKind::PermissionDenied {
        Reason::PermissionDenied
    } else {
        Reason::QueryFailed
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/disk_health.rs"]
mod tests;
